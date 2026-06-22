mod metrics;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use dash_spv::network::PeerNetworkManager;
use dash_spv::storage::DiskStorageManager;
use dash_spv::{ClientConfig, DashSpvClient, EventHandler};
use dashcore::Network;
use key_wallet::wallet::initialization::WalletAccountCreationOptions;
use key_wallet::wallet::ManagedWalletInfo;
use key_wallet_manager::WalletManager;
use std::collections::BTreeSet;
use tokio::sync::RwLock;

use crate::metrics::BenchEventHandler;

/// Default wallet: a mnemonic with real testnet history
const DEFAULT_MNEMONIC: &str =
    "job flower agree lyrics industry note boost finger buddy dog exact fat";

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Anchor the timeline at process start so `tlog!` timestamps read as time-since-launch.
    dash_spv::timer::init();

    tracing_subscriber::fmt()
        // Logs go to stderr so they stream live and stay separate from the
        // machine-readable result summary on stdout (which run.sh captures/parses).
        // Set RUST_LOG to raise verbosity, e.g.
        //   RUST_LOG="warn,dash_spv::network=info" ./run.sh scenarios/<x>.yml
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                // Note: `dash_spv=warn` would also silence `dash_spv_bench` (prefix match), so
                // name the bench target explicitly. Timeline marks are opt-in — quiet by default;
                // enable with e.g. RUST_LOG="warn,dash_spv_bench=info,timeline=info".
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,dash_spv_bench=info")),
        )
        .init();

    // The mode picks the network. `local` runs against docker peers serving a testnet
    // snapshot; `testnet`/`mainnet` sync the real network (DNS discovery or explicit peers).
    let mode = env_or("BENCH_MODE", "local").trim().to_ascii_lowercase();
    let (network, remote) = match mode.as_str() {
        "local" => (Network::Testnet, false),
        "testnet" => (Network::Testnet, true),
        "mainnet" => (Network::Mainnet, true),
        other => {
            return Err(anyhow!(
                "BENCH_MODE must be 'local', 'testnet' or 'mainnet', got '{other}'"
            ))
        }
    };

    // BENCH_PEERS: comma-separated host:port list. Empty => DNS discovery, which is only
    // possible on testnet/mainnet — local docker peers can't be discovered, so local requires them.
    let peers: Vec<SocketAddr> = std::env::var("BENCH_PEERS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|p| p.parse::<SocketAddr>().with_context(|| format!("bad peer {p}")))
        .collect::<Result<_>>()?;

    if !remote && peers.is_empty() {
        return Err(anyhow!("BENCH_MODE=local requires BENCH_PEERS (the docker peer addresses)"));
    }

    let dns_mode = remote && peers.is_empty();

    // Optional checkpoint start height: skip genesis..start and sync a bounded range.
    let start_height: Option<u32> =
        std::env::var("BENCH_START_HEIGHT").ok().and_then(|v| v.trim().parse().ok());

    let mnemonic = env_or("BENCH_MNEMONIC", DEFAULT_MNEMONIC);
    let timeout = Duration::from_secs(1800);

    tracing::info!(
        "dash-spv-bench: mode={} ({}) on {:?}, wallet={}",
        mode,
        if dns_mode {
            "DNS discovery".to_string()
        } else {
            format!("{} peers", peers.len())
        },
        network,
        !mnemonic.is_empty(),
    );

    // Fresh storage dir => cold cache.
    let storage_dir = tempfile::tempdir().context("temp storage dir")?;
    let mut config = ClientConfig::new(network).with_storage_path(storage_dir.path().to_path_buf());
    config.enable_filters = true;
    config.enable_masternodes = false;
    config.enable_mempool_tracking = false;
    config.start_from_height = start_height;
    config.restrict_to_configured_peers = !dns_mode;
    // `max_peers` only if the scenario set it; otherwise keep the ClientConfig default.
    if let Ok(v) = std::env::var("BENCH_MAX_PEERS") {
        if let Ok(n) = v.trim().parse() {
            config.max_peers = n;
        }
    }
    for addr in &peers {
        config.add_peer(*addr);
    }

    let network_manager =
        PeerNetworkManager::new(&config).await.map_err(|e| anyhow!("network manager: {e}"))?;
    let storage_manager =
        DiskStorageManager::new(&config).await.map_err(|e| anyhow!("storage manager: {e}"))?;

    // Wallet: a single BIP44 account 0 from the mnemonic, so filter matches drive block download.
    let mut wallet_manager = WalletManager::<ManagedWalletInfo>::new(network);
    if !mnemonic.is_empty() {
        wallet_manager
            .create_wallet_from_mnemonic(&mnemonic, 0, account_creation_options())
            .map_err(|e| anyhow!("wallet from mnemonic: {e}"))?;
    }
    let wallet = Arc::new(RwLock::new(wallet_manager));

    let handler = Arc::new(BenchEventHandler::new());
    let client = DashSpvClient::new(
        config,
        network_manager,
        storage_manager,
        wallet,
        vec![handler.clone() as Arc<dyn EventHandler>],
    )
    .await
    .map_err(|e| anyhow!("client new: {e}"))?;

    // Run the client on its own task; stop as soon as the sync completes (or times out).
    let run_client = client.clone();
    let run_handle = tokio::spawn(async move {
        let _ = run_client.run().await;
    });

    let _ = tokio::time::timeout(timeout, handler.wait_done()).await;
    let m = handler.snapshot();
    dash_spv::timer::dump_profile();

    let _ = client.shutdown().await;
    run_handle.abort();
    let _ = run_handle.await;
    drop(storage_dir);

    println!("\n{m}");
    Ok(())
}

fn account_creation_options() -> WalletAccountCreationOptions {
    WalletAccountCreationOptions::SpecificAccounts(
        BTreeSet::from([0]),
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::new(),
        None,
    )
}
