use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;

use super::helpers::wait_for_sync;
use super::setup::{create_and_start_client, test_account_options, ClientHandle, TestContext};
use dash_spv::network::PeerNetworkManager;
use dash_spv::storage::DiskStorageManager;
use dash_spv::sync::SyncEvent;
use dash_spv::test_utils::{TestChain, TestEventHandler};
use dash_spv::{DashSpvClient, Network};
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use key_wallet::wallet::managed_wallet_info::ManagedWalletInfo;
use key_wallet_manager::{WalletId, WalletManager};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

fn count_txs<T: WalletInfoInterface>(manager: &WalletManager<T>, wallet_id: &WalletId) -> usize {
    let info = manager.get_wallet_info(wallet_id).expect("wallet info");
    info.accounts().all_accounts().iter().map(|a| a.transactions.len()).sum()
}

#[tokio::test]
async fn test_wallet_added_after_sync_finds_historical_txs() {
    let Some(ctx) = TestContext::new(TestChain::Minimal).await else {
        return;
    };

    let wallet: Arc<RwLock<WalletManager<ManagedWalletInfo>>> =
        Arc::new(RwLock::new(WalletManager::<ManagedWalletInfo>::new(Network::Regtest)));

    let mut client_handle = create_and_start_client(&ctx.client_config, Arc::clone(&wallet)).await;
    wait_for_sync(&mut client_handle.progress_receiver, ctx.dashd.initial_height).await;

    let wallet_id = wallet
        .write()
        .await
        .create_wallet_from_mnemonic(&ctx.dashd.wallet.mnemonic, "", 0, test_account_options())
        .expect("Failed to add wallet after sync");

    let tx_count = count_txs(wallet.read().await.deref(), &wallet_id);
    assert_eq!(
        tx_count, ctx.dashd.wallet.transaction_count,
        "Wallet should contain historical transactions after post-sync rescan"
    );

    client_handle.stop().await;
}

#[tokio::test]
async fn test_wallet_added_before_sync_finds_historical_txs() {
    let Some(ctx) = TestContext::new(TestChain::Minimal).await else {
        return;
    };

    let wallet: Arc<RwLock<WalletManager<ManagedWalletInfo>>> =
        Arc::new(RwLock::new(WalletManager::<ManagedWalletInfo>::new(Network::Regtest)));

    let network_manager = PeerNetworkManager::new(&ctx.client_config)
        .await
        .expect("Failed to create network manager");
    let storage_manager = DiskStorageManager::new(&ctx.client_config)
        .await
        .expect("Failed to create storage manager");

    let handler = Arc::new(TestEventHandler::new());
    let progress_receiver = handler.subscribe_progress();
    let sync_event_receiver = handler.subscribe_sync_events();
    let network_event_receiver = handler.subscribe_network_events();

    let client = DashSpvClient::new(
        ctx.client_config.clone(),
        network_manager,
        storage_manager,
        wallet.clone(),
        handler,
    )
    .await
    .expect("Failed to create client");

    let wallet_event_receiver = {
        let w = client.wallet().read().await;
        w.subscribe_events()
    };
    let cancel_token = CancellationToken::new();
    let run_token = cancel_token.clone();
    let run_client = client.clone();

    let wallet_id = wallet
        .write()
        .await
        .create_wallet_from_mnemonic(&ctx.dashd.wallet.mnemonic, "", 0, test_account_options())
        .expect("Failed to add wallet before sync");

    let run_handle = tokio::task::spawn(async move { run_client.run(run_token).await });

    let mut client_handle = ClientHandle {
        client,
        run_handle: Some(run_handle),
        progress_receiver,
        sync_event_receiver,
        network_event_receiver,
        wallet_event_receiver,
        cancel_token,
    };

    wait_for_sync(&mut client_handle.progress_receiver, ctx.dashd.initial_height).await;

    let tx_count = count_txs(wallet.read().await.deref(), &wallet_id);
    assert_eq!(
        tx_count, ctx.dashd.wallet.transaction_count,
        "Wallet should contain historical transactions after sync when added before sync"
    );

    client_handle.stop().await;
}

#[tokio::test]
async fn test_wallet_added_during_sync_finds_historical_txs() {
    let Some(ctx) = TestContext::new(TestChain::Minimal).await else {
        return;
    };

    let wallet: Arc<RwLock<WalletManager<ManagedWalletInfo>>> =
        Arc::new(RwLock::new(WalletManager::<ManagedWalletInfo>::new(Network::Regtest)));

    let mut client_handle = create_and_start_client(&ctx.client_config, Arc::clone(&wallet)).await;

    // Wait for at least one FiltersStored event: some filters are on disk,
    // sync is still running. Bail out if sync finishes first — that would
    // collapse this test into the "after sync" case.
    let deadline = tokio::time::sleep(Duration::from_secs(60));
    tokio::pin!(deadline);
    let mut saw_mid_sync = false;
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            result = client_handle.sync_event_receiver.recv() => match result {
                Ok(SyncEvent::FiltersStored { .. }) => {
                    saw_mid_sync = true;
                    break;
                }
                Ok(SyncEvent::SyncComplete { .. }) => {
                    panic!("Sync finished before the wallet could be added mid-sync; \
                            use a longer chain for this test");
                }
                Ok(_) => continue,
                Err(_) => break,
            },
        }
    }
    assert!(saw_mid_sync, "Never observed FiltersStored within the timeout");

    let wallet_id = wallet
        .write()
        .await
        .create_wallet_from_mnemonic(&ctx.dashd.wallet.mnemonic, "", 0, test_account_options())
        .expect("Failed to add wallet mid-sync");

    wait_for_sync(&mut client_handle.progress_receiver, ctx.dashd.initial_height).await;
    let tx_count = count_txs(wallet.read().await.deref(), &wallet_id);
    assert_eq!(
        tx_count, ctx.dashd.wallet.transaction_count,
        "Wallet should contain historical transactions after mid-sync add"
    );

    client_handle.stop().await;
}
