//! Host-independent per-note save serialization and shared compaction lane.
use super::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
const INTERVAL: Duration = Duration::from_secs(60);
#[derive(Default)]
struct Pending {
    running: bool,
    dirty: bool,
}
#[derive(Clone)]
pub struct Session {
    store: Store,
    input: Arc<Mutex<()>>,
    modified: Arc<AtomicBool>,
    editing: Arc<AtomicBool>,
    pending: Arc<Mutex<Pending>>,
    lane: Arc<tokio::sync::Mutex<()>>,
    wake: Arc<tokio::sync::Notify>,
    device_name: String,
    on_publish: Arc<dyn Fn(Publish) + Send + Sync>,
}
impl Session {
    pub fn new(
        store: Store,
        lane: Arc<tokio::sync::Mutex<()>>,
        device_name: String,
        on_publish: impl Fn(Publish) + Send + Sync + 'static,
    ) -> Self {
        Self {
            store,
            input: Arc::default(),
            modified: Arc::default(),
            editing: Arc::default(),
            pending: Arc::default(),
            lane,
            wake: Arc::default(),
            device_name,
            on_publish: Arc::new(on_publish),
        }
    }
    pub async fn access<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Store) -> CommandResult<T> + Send + 'static,
    ) -> CommandResult<T> {
        let session = self.clone();
        tokio::task::spawn_blocking(move || {
            let _input = session.input.lock().map_err(err)?;
            f(&session.store)
        })
        .await
        .map_err(err)?
    }
    pub async fn patch(
        &self,
        patch: InkDraftPatch,
        expected: Option<String>,
    ) -> CommandResult<String> {
        let geometry = !patch.upserts.is_empty();
        let modified = self.modified.clone();
        let revision = self
            .access(move |store| {
                let revision = store.patch(patch, expected)?;
                modified.store(true, Ordering::Relaxed);
                Ok(revision)
            })
            .await?;
        if geometry {
            self.schedule();
        }
        Ok(revision)
    }
    pub async fn open(&self, editing: bool) -> CommandResult<InkHistorySnapshot> {
        let active = self.editing.clone();
        self.access(move |store| {
            let history = store.open_editor(editing)?;
            if editing {
                active.store(true, Ordering::Relaxed);
            }
            Ok(history)
        })
        .await
    }
    /// The note this session belongs to, so a host cache can drop it when the
    /// note is gone.
    pub fn page(&self) -> uuid::Uuid {
        self.store.document()
    }
    pub fn needs_completion(&self) -> bool {
        self.modified.load(Ordering::Relaxed) || self.editing.load(Ordering::Relaxed)
    }
    pub async fn history(
        &self,
        redo: bool,
        expected: Option<String>,
    ) -> CommandResult<InkHistoryUpdate> {
        let modified = self.modified.clone();
        self.access(move |store| {
            let result = store.history(Some(redo), expected)?;
            modified.store(true, Ordering::Relaxed);
            Ok(result)
        })
        .await
    }
    fn notify(&self, published: Option<Publish>) {
        if let Some(p) = published {
            (self.on_publish)(p);
        }
    }
    /// The host flushes its JS/native write queue first. Publication intent is
    /// durable before packing; interrupted or failed completion is recoverable.
    pub async fn complete(&self) -> CommandResult<()> {
        self.modified.store(true, Ordering::Relaxed);
        self.access(Store::request_publication).await?;
        let _lane = self.lane.lock().await;
        self.pending.lock().map_err(err)?.dirty = false;
        let device = self.device_name.clone();
        let editing = self.editing.clone();
        let modified = self.modified.clone();
        let result = self
            .access(move |store| {
                while store.compact()? {}
                let published = store.publish(device)?;
                // The publication is already committed at this point. Failing
                // the whole call on the editor flag would hide a version that
                // exists, so the close is reported alongside it instead.
                let closed = store.close_editor();
                if closed.is_ok() {
                    editing.store(false, Ordering::Relaxed);
                    modified.store(false, Ordering::Relaxed);
                }
                Ok((published, closed))
            })
            .await;
        match result {
            Ok((published, closed)) => {
                self.notify(published);
                self.wake.notify_waiters();
                closed.inspect_err(|_| self.schedule())
            }
            Err(error) => {
                self.schedule();
                Err(error)
            }
        }
    }
    fn schedule(&self) {
        {
            let mut state = self.pending.lock().expect("ink scheduler mutex");
            state.dirty = true;
            if state.running {
                return;
            }
            state.running = true;
        }
        let session = self.clone();
        tokio::spawn(async move {
            loop {
                let wake = session.wake.notified();
                tokio::pin!(wake);
                wake.as_mut().enable();
                {
                    let mut state = session.pending.lock().expect("ink scheduler mutex");
                    if !state.dirty {
                        state.running = false;
                        break;
                    }
                }
                tokio::select! {_=tokio::time::sleep(INTERVAL)=>{},_=&mut wake=>{}}
                let _lane = session.lane.lock().await;
                {
                    let mut state = session.pending.lock().expect("ink scheduler mutex");
                    if !state.dirty {
                        state.running = false;
                        break;
                    }
                    state.dirty = false;
                }
                let target = session.clone();
                let result = tokio::task::spawn_blocking(
                    move || -> CommandResult<(bool, Option<Publish>)> {
                        if target.store.publication_requested()? {
                            // An explicit boundary may finish while another note is active.
                            // Serialize this note only; the ordinary minute job never locks input.
                            let _input = target.input.lock().map_err(err)?;
                            while target.store.compact()? {}
                            {
                                let published = target.store.publish(target.device_name.clone())?;
                                target.store.close_editor()?;
                                target.editing.store(false, Ordering::Relaxed);
                                target.modified.store(false, Ordering::Relaxed);
                                Ok((false, published))
                            }
                        } else {
                            Ok((target.store.compact()?, None))
                        }
                    },
                )
                .await
                .map_err(err)
                .and_then(|r| r);
                let more = match result {
                    Ok((more, published)) => {
                        session.notify(published);
                        more
                    }
                    Err(CommandError::NotFound(_)) => false,
                    Err(error) => {
                        tracing::warn!(?error, "Ink maintenance deferred; local writes retained");
                        true
                    }
                };
                let mut state = session.pending.lock().expect("ink scheduler mutex");
                state.dirty |= more;
                if !state.dirty {
                    state.running = false;
                    break;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test]
    async fn completion_publishes_once_and_failed_publication_recovers_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.db");
        let conn = crate::db::open(&path).await.unwrap();
        let id = crate::db::create_handwritten_note_with_ops(&conn, None)
            .await
            .unwrap()
            .value
            .uuid;
        crate::configure_sync(&conn, "https://notes.example.test")
            .await
            .unwrap();
        let store = Store::new(&path, id);
        let published = Arc::new(AtomicUsize::new(0));
        let sink = published.clone();
        let session = Session::new(store.clone(), Arc::default(), "Book".into(), move |_| {
            sink.fetch_add(1, Ordering::Relaxed);
        });
        session
            .patch(
                InkDraftPatch {
                    order: vec![],
                    upserts: vec![],
                    background: InkBackground::Grid,
                },
                None,
            )
            .await
            .unwrap();
        let revision = store.read().unwrap().revision;
        let sql = rusqlite::Connection::open(&path).unwrap();
        sql.execute_batch("CREATE TRIGGER fail_publication BEFORE INSERT ON ink_versions BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(session.complete().await.is_err());
        assert!(store.status().unwrap().publication_requested);
        assert!(store.status().unwrap().unpublished_changes);
        assert_eq!(recoverable_notes(&conn).await.unwrap(), vec![id]);
        sql.execute_batch("DROP TRIGGER fail_publication").unwrap();
        session.complete().await.unwrap();
        session.complete().await.unwrap();
        assert_eq!(published.load(Ordering::Relaxed), 1);
        assert_eq!(store.read().unwrap().revision, revision);
        assert!(recoverable_notes(&conn).await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn explicit_completion_cancels_obsolete_minute_work_without_a_stale_wakeup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.db");
        let conn = crate::db::open(&path).await.unwrap();
        let id = crate::db::create_handwritten_note_with_ops(&conn, None)
            .await
            .unwrap()
            .value
            .uuid;
        let session = Session::new(Store::new(&path, id), Arc::default(), "Book".into(), |_| {});
        tokio::time::pause();
        session.schedule();
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(59)).await;
        session.schedule();
        assert!(session.pending.lock().unwrap().dirty);
        let lane = session.lane.lock().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(session.pending.lock().unwrap().running);
        drop(lane);
        session.complete().await.unwrap();
        tokio::task::yield_now().await;
        assert!(!session.pending.lock().unwrap().dirty);
        session.schedule();
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(59)).await;
        assert!(session.pending.lock().unwrap().dirty);
        session.complete().await.unwrap();
    }
}
