use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;
use std::time::Instant;

use dash_spv::sync::SyncEvent;
use dash_spv::EventHandler;
use tokio::sync::Notify;

/// Milestones we time, in pipeline order. Stable string keys so timings are easy to correlate.
pub const MILESTONE_BLOCK_HEADERS: &str = "block_headers_complete";
pub const MILESTONE_FILTER_HEADERS: &str = "filter_headers_complete";
pub const MILESTONE_FILTERS: &str = "filters_complete";
pub const MILESTONE_SYNC: &str = "sync_complete";

#[derive(Debug)]
pub struct RunMetrics {
    /// Total wall-clock from start to `sync_complete` (or to abort/timeout).
    total_ms: u64,
    /// Whether the run reached `sync_complete` within the timeout. If false, the run timed out
    completed: bool,
    block_headers_ms: Option<u64>,
    filter_headers_ms: Option<u64>,
    filters_ms: Option<u64>,
    /// How long after block-header completion the filter headers finished
    filter_headers_lag_ms: Option<u64>,
    error: Option<String>,
}

impl fmt::Display for RunMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = |o: Option<u64>| o.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string());

        writeln!(f, "=== dash-spv sync ===")?;
        writeln!(
            f,
            "completed:         {}{}",
            self.completed,
            if self.completed {
                ""
            } else {
                " (TIMED OUT)"
            }
        )?;
        writeln!(f, "total_ms:          {}", self.total_ms)?;
        writeln!(f, "block_headers_ms:  {}", ms(self.block_headers_ms))?;
        writeln!(f, "filter_headers_ms: {}", ms(self.filter_headers_ms))?;
        writeln!(f, "filters_ms:        {}", ms(self.filters_ms))?;
        write!(f, "filter_hdr_lag_ms: {}", ms(self.filter_headers_lag_ms))?;
        if let Some(e) = &self.error {
            write!(f, "\nerror:             {e}")?;
        }

        Ok(())
    }
}

/// Collects timing for one run by subscribing to the client's event stream.
pub struct BenchEventHandler {
    start: Instant,
    inner: Mutex<Inner>,
    /// Notified once the run reaches `sync_complete`.
    done: Notify,
}

struct Inner {
    /// label -> elapsed_ms at first occurrence.
    milestones: BTreeMap<&'static str, u64>,
    error: Option<String>,
    completed: bool,
}

impl BenchEventHandler {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            inner: Mutex::new(Inner {
                milestones: BTreeMap::new(),
                error: None,
                completed: false,
            }),
            done: Notify::new(),
        }
    }

    fn record(&self, label: &'static str) {
        let elapsed = self.start.elapsed().as_millis() as u64;
        let mut inner = self.inner.lock().unwrap();
        inner.milestones.entry(label).or_insert(elapsed);
    }

    /// Wait until the run signals completion. Returns immediately if already done.
    pub async fn wait_done(&self) {
        if self.inner.lock().unwrap().completed {
            return;
        }
        self.done.notified().await;
    }

    /// Snapshot the collected timing into a [`RunMetrics`]. `total_ms` is the elapsed time
    /// at the moment of snapshotting (call after `wait_done` or timeout).
    pub fn snapshot(&self) -> RunMetrics {
        let inner = self.inner.lock().unwrap();
        let bh = inner.milestones.get(MILESTONE_BLOCK_HEADERS).copied();
        let fh = inner.milestones.get(MILESTONE_FILTER_HEADERS).copied();
        let fl = inner.milestones.get(MILESTONE_FILTERS).copied();
        let sc = inner.milestones.get(MILESTONE_SYNC).copied();

        RunMetrics {
            total_ms: sc.unwrap_or_else(|| self.start.elapsed().as_millis() as u64),
            completed: inner.completed,
            block_headers_ms: bh,
            filter_headers_ms: fh,
            filters_ms: fl,
            filter_headers_lag_ms: match (bh, fh) {
                (Some(b), Some(f)) => Some(f.saturating_sub(b)),
                _ => None,
            },
            error: inner.error.clone(),
        }
    }
}

impl Default for BenchEventHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl EventHandler for BenchEventHandler {
    fn on_sync_event(&self, event: &SyncEvent) {
        match event {
            SyncEvent::BlockHeaderSyncComplete {
                ..
            } => self.record(MILESTONE_BLOCK_HEADERS),
            SyncEvent::FilterHeadersSyncComplete {
                ..
            } => self.record(MILESTONE_FILTER_HEADERS),
            SyncEvent::FiltersSyncComplete {
                ..
            } => self.record(MILESTONE_FILTERS),
            SyncEvent::SyncComplete {
                ..
            } => {
                self.record(MILESTONE_SYNC);
                let mut inner = self.inner.lock().unwrap();
                inner.completed = true;
                drop(inner);
                self.done.notify_waiters();
            }
            _ => {}
        }
    }

    fn on_error(&self, error: &str) {
        let mut inner = self.inner.lock().unwrap();
        if inner.error.is_none() {
            inner.error = Some(error.to_string());
        }
    }
}
