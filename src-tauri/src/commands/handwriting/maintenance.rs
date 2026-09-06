//! Single compaction lane shared by periodic work and explicit completion.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{path::PathBuf, sync::Mutex, time::Duration};

const INTERVAL: Duration = Duration::from_secs(60);
const MAX_CHUNKS: usize = 512;
const DECODED_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Default)]
struct MaintenanceState {
    running: bool,
    dirty: bool,
}

#[derive(Default, Clone)]
pub struct HandwritingStore {
    pub(super) lock: Arc<Mutex<()>>,
    maintenance: Arc<Mutex<MaintenanceState>>,
    compaction: Arc<tokio::sync::Mutex<()>>,
    wake: Arc<tokio::sync::Notify>,
    background: Arc<AtomicBool>,
}

impl HandwritingStore {
    pub(super) fn schedule_compaction(&self, path: PathBuf) {
        if self.background.load(Ordering::Relaxed) {
            self.request_completion(path);
        } else {
            self.schedule_periodic(path);
        }
    }

    #[cfg(target_os = "android")]
    pub(crate) fn set_background(&self, path: PathBuf, background: bool) {
        self.background.store(background, Ordering::Relaxed);
        if background && path.exists() {
            self.request_completion(path);
        }
    }

    fn request_completion(&self, path: PathBuf) {
        let store = self.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = store.finish_compaction(path).await {
                tracing::warn!(?error, "Handwriting completion deferred");
            }
        });
    }

    fn schedule_periodic(&self, path: PathBuf) {
        {
            let mut state = self.maintenance.lock().expect("maintenance mutex poisoned");
            state.dirty = true;
            if state.running {
                return;
            }
            state.running = true;
        }
        let store = self.clone();
        tokio::spawn(async move {
            loop {
                let wake = store.wake.notified();
                tokio::pin!(wake);
                wake.as_mut().enable();
                {
                    let mut state = store
                        .maintenance
                        .lock()
                        .expect("maintenance mutex poisoned");
                    if !state.dirty {
                        state.running = false;
                        break;
                    }
                }
                // Continued writing does not postpone this deadline. Explicit
                // completion wakes an obsolete sleeping worker so it can stop.
                tokio::select! {
                    _ = tokio::time::sleep(INTERVAL) => {},
                    _ = &mut wake => {},
                }
                let _lane = store.compaction.lock().await;
                {
                    let mut state = store
                        .maintenance
                        .lock()
                        .expect("maintenance mutex poisoned");
                    if !state.dirty {
                        state.running = false;
                        break;
                    }
                    state.dirty = false;
                }
                let job_path = path.clone();
                let result = tokio::task::spawn_blocking(move || {
                    storage::compact_with_limits(&job_path, MAX_CHUNKS, DECODED_BYTES)
                })
                .await;
                let more = match result {
                    Ok(Ok(packed)) => packed,
                    failure => {
                        tracing::warn!(
                            ?failure,
                            "Handwriting compaction deferred; local writes remain durable"
                        );
                        // Retry transient failures at the next minute, never in a tight loop.
                        true
                    }
                };
                let mut state = store
                    .maintenance
                    .lock()
                    .expect("maintenance mutex poisoned");
                state.dirty |= more;
                if !state.dirty {
                    state.running = false;
                    break;
                }
            }
        });
    }

    pub(super) async fn finish_compaction(&self, path: PathBuf) -> CommandResult<()> {
        let _lane = self.compaction.lock().await;
        self.maintenance
            .lock()
            .expect("maintenance mutex poisoned")
            .dirty = false;
        let job_path = path.clone();
        let result = tokio::task::spawn_blocking(move || {
            // Each job remains bounded. Already sealed, singleton and non-shrinking
            // inputs are valid terminal states, not a reason to block publication.
            while storage::compact_with_limits(&job_path, MAX_CHUNKS, DECODED_BYTES)? {}
            Ok(())
        })
        .await
        .map_err(err)
        .and_then(|r| r);
        if result.is_err() {
            self.schedule_periodic(path);
        } else {
            let state = self.maintenance.lock().expect("maintenance mutex poisoned");
            if state.running {
                self.wake.notify_waiters();
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn repeated_edits_do_not_debounce_the_minute_and_completion_cancels_idle_work() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        let store = HandwritingStore::default();
        store.schedule_compaction(path.clone());
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(59)).await;
        store.schedule_compaction(path.clone());
        assert!(!path.exists());
        // Hold the lane so the deadline can be observed without racing disk IO.
        let lane = store.compaction.lock().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(store.maintenance.lock().unwrap().running);
        drop(lane);
        tokio::task::yield_now().await;
        assert!(!store.maintenance.lock().unwrap().dirty);
        store.finish_compaction(path.clone()).await.unwrap();
        tokio::task::yield_now().await;
        assert!(path.exists());
        assert!(!store.maintenance.lock().unwrap().dirty);
        // This also tests a repeated no-change lifecycle request.
        store.finish_compaction(path.clone()).await.unwrap();
        tokio::task::yield_now().await;
        store.schedule_compaction(path.clone());
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(59)).await;
        // Completion of the old worker must not leave a notification permit
        // that immediately runs the next editing session's minute job.
        assert!(store.maintenance.lock().unwrap().dirty);
        store.finish_compaction(path).await.unwrap();
    }

    #[tokio::test]
    async fn completion_propagates_failure_and_retains_retry_work() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.sqlite3");
        std::fs::write(&path, b"broken").unwrap();
        let store = HandwritingStore::default();
        assert!(store.finish_compaction(path).await.is_err());
        assert!(store.maintenance.lock().unwrap().dirty);
        assert!(store.maintenance.lock().unwrap().running);
    }
}
