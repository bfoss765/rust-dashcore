//! Process-relative timeline timestamps for pinpointing where sync wall-clock time goes.
//!
//! One process-start [`Instant`] plus the [`tlog!`](crate::tlog) macro: drop `tlog!("...")` at
//! interesting points (before/after a network round-trip, before/after hashing, storage writes, …)
//! and every line is prefixed with the elapsed time since the timeline was anchored. The gap
//! between two consecutive timeline lines is the time spent in between — so you can add marks
//! incrementally and bisect where the time is going.
//!
//! # Usage
//! ```ignore
//! dash_spv::timer::init(); // once, at the top of main() (optional; otherwise anchored lazily)
//! tlog!("about to send GetHeaders (seg {})", seg_id);
//! // ... network round-trip ...
//! tlog!("got {} headers", headers.len());
//! ```
//! Timeline lines use the `timeline` tracing target at INFO — enable them with `timeline=info`
//! in `RUST_LOG` / the subscriber's env filter.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// The instant the timeline started. Anchored on first access; call [`init`] early in `main` to
/// anchor it at process start rather than at the first [`elapsed`]/`tlog!`.
pub static START: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Anchor the timeline now (idempotent). Call once at the top of `main` so `t+0` == process start.
pub fn init() {
    LazyLock::force(&START);
}

/// Time elapsed since the timeline started.
#[inline]
pub fn elapsed() -> Duration {
    START.elapsed()
}

/// Milliseconds elapsed since the timeline started (what [`tlog!`](crate::tlog) prints).
#[inline]
pub fn elapsed_ms() -> u128 {
    START.elapsed().as_millis()
}

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// A named time accumulator for micro-profiling a hot path called too often to `tlog!` per call
/// (e.g. once per cfilter message). Sum time with [`Prof::add`], print all with [`dump_profile`].
pub struct Prof {
    pub name: &'static str,
    ns: AtomicU64,
    calls: AtomicU64,
}

impl Prof {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            ns: AtomicU64::new(0),
            calls: AtomicU64::new(0),
        }
    }
    /// Add one sample.
    #[inline]
    pub fn add(&self, d: Duration) {
        self.ns.fetch_add(d.as_nanos() as u64, Relaxed);
        self.calls.fetch_add(1, Relaxed);
    }
}

// Buckets for the per-cfilter-message hot path (filters/sync_manager.rs::handle_message).
pub static P_HEIGHT_LOOKUP: Prof = Prof::new("filt.height_lookup");
pub static P_RECV_DATA: Prof = Prof::new("filt.receive_with_data");
pub static P_SEND_PENDING: Prof = Prof::new("filt.send_pending");
pub static P_STORE_MATCH: Prof = Prof::new("filt.store_and_match");

// Arrival path: time to lock the shared Arc<Mutex<MessageDispatcher>> and dispatch each message.
// If per-call time balloons with more peers, that Mutex is the ~70k/s serialization point.
pub static P_NET_DISPATCH: Prof = Prof::new("net.dispatch(lock)");

// Reader loop (network/manager.rs) per-iteration breakdown. RLOCK+WLOCK are the two peer-lock
// acquisitions per message; RECV is the receive_message/select; MSG vs IDLE count how often an
// iteration yielded a message vs timed out waiting (IDLE-heavy => peer/network-bound, not reader).
pub static P_READ_RLOCK: Prof = Prof::new("read.rlock");
pub static P_READ_WLOCK: Prof = Prof::new("read.wlock");
pub static P_READ_RECV: Prof = Prof::new("read.recv/select");
pub static P_READ_MSG: Prof = Prof::new("read.got_msg");
pub static P_READ_IDLE: Prof = Prof::new("read.idle_timeout");

// Lock contention probes for the cfilter phase.
// READER_HOLD: how long the reader holds peer.write() across receive_message/select (if long, it
//   blocks the sender which needs the same lock). SENDER_WAIT: how long send_message_to_peer waits
//   to acquire peer.write() (high => reader is hogging it). HM_HDR: per-cfilter header_storage.read
//   acquire wait in handle_message. DISPATCH_WAIT: message_dispatcher Mutex acquire wait.
pub static P_READER_HOLD: Prof = Prof::new("reader.peer_write_HOLD");
pub static P_SENDER_WAIT: Prof = Prof::new("sender.peer_write_WAIT");
pub static P_HM_HDR: Prof = Prof::new("handle_msg.hdr_read_wait");
pub static P_DISPATCH_WAIT: Prof = Prof::new("dispatch.lock_WAIT");

/// Log all profiling accumulators (call once at end of run). Uses the `timeline` target.
pub fn dump_profile() {
    // eprintln so it prints even when the `timeline` target is filtered out (so we can profile with
    // the high-volume per-message marks disabled).
    for p in [
        &P_HEIGHT_LOOKUP,
        &P_RECV_DATA,
        &P_SEND_PENDING,
        &P_STORE_MATCH,
        &P_NET_DISPATCH,
        &P_READ_RLOCK,
        &P_READ_WLOCK,
        &P_READ_RECV,
        &P_READ_MSG,
        &P_READ_IDLE,
        &P_READER_HOLD,
        &P_SENDER_WAIT,
        &P_HM_HDR,
        &P_DISPATCH_WAIT,
    ] {
        let ns = p.ns.load(Relaxed);
        let c = p.calls.load(Relaxed);
        if c > 0 {
            eprintln!(
                "[t+{:>7}ms] PROF {:22} calls={:>8} total={:>6}ms  per={:>7.3}us",
                elapsed_ms(),
                p.name,
                c,
                ns / 1_000_000,
                ns as f64 / c as f64 / 1000.0
            );
        }
    }
}

/// Log a message prefixed with the elapsed time since the process timeline started, e.g.
/// `[t+   1234ms] sent GetHeaders`. Uses the `timeline` tracing target at INFO level.
///
/// Cheap to leave in place, easy to add more of — the point is to sprinkle these around hot spots
/// and read off the gaps between consecutive lines.
#[macro_export]
macro_rules! tlog {
    ($($arg:tt)*) => {
        ::tracing::info!(
            target: "timeline",
            "[t+{:>7}ms] {}",
            $crate::timer::elapsed_ms(),
            format_args!($($arg)*)
        )
    };
}
