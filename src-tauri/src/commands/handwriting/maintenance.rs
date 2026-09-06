//! Application lifecycle binding for the core ink session scheduler.
use super::*;
use notes_core::ink::runtime::Session;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
#[derive(Default, Clone)]
pub struct HandwritingStore {
    sessions: Arc<Mutex<HashMap<uuid::Uuid, Session>>>,
    lane: Arc<tokio::sync::Mutex<()>>,
    background: Arc<AtomicBool>,
}
impl HandwritingStore {
    pub(super) fn session(&self, app: &AppHandle, page: uuid::Uuid) -> CommandResult<Session> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|e| err(anyhow::anyhow!(e.to_string())))?;
        if let Some(session) = sessions.get(&page) {
            return Ok(session.clone());
        }
        let path = app.path().app_data_dir().map_err(err)?.join("notes.db");
        let handle = app.clone();
        let session = Session::new(
            ink::Store::new(path, page),
            self.lane.clone(),
            device_name(app),
            move |p| {
                emit_domain(
                    &handle,
                    DomainEvent::PagesChanged {
                        page_uuids: vec![p.page_uuid],
                    },
                );
            },
        );
        sessions.insert(page, session.clone());
        Ok(session)
    }
    pub(super) fn is_background(&self) -> bool {
        self.background.load(Ordering::Relaxed)
    }
    pub(super) fn finish_in_background(&self, session: Session) {
        tauri::async_runtime::spawn(async move {
            if let Err(error) = session.complete().await {
                tracing::warn!(?error, "Ink publication deferred");
            }
        });
    }
    pub(crate) fn set_background(&self, background: bool) {
        self.background.store(background, Ordering::Relaxed);
        if background {
            let manager = self.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = manager.complete_all().await {
                    tracing::warn!(?error, "Ink background completion deferred");
                }
            });
        }
    }
    pub(crate) async fn complete_all(&self) -> CommandResult<()> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|e| err(anyhow::anyhow!(e.to_string())))?
            .values()
            .filter(|session| session.needs_completion())
            .cloned()
            .collect::<Vec<_>>();
        let mut failure = None;
        for session in sessions {
            if let Err(error) = session.complete().await {
                if !matches!(error, notes_core::CoreError::NotFound(_)) {
                    failure = Some(err(error));
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
    pub(crate) async fn recover(&self, app: &AppHandle, conn: &Connection) -> CommandResult<()> {
        for page in ink::recoverable_notes(conn).await.map_err(err)? {
            let session = self.session(app, page)?;
            if let Err(error) = session.complete().await {
                tracing::warn!(?error,%page,"Interrupted ink session retained for retry");
            }
        }
        Ok(())
    }
}
pub(super) fn device_name(app: &AppHandle) -> String {
    #[cfg(target_os = "android")]
    if let Ok(name) = app.mobile_system().device_name() {
        return name;
    }
    let _ = app;
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| std::env::consts::OS.into())
}
