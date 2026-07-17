use anyhow::Context;
use notes_core::Connection;
use notes_core::db::{self, Block, Page, SearchHit};
use notes_core::{BlockStyle, ContentRevision, PageLayout, ReorderDirection, TaskState};
use notes_protocol::{
    AiIndexStatus, AiProviderProbeResult, AiProviderSettingsUpdate, AiRuntimeSettings, ChatEvent,
    ChatTurn,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
#[cfg(not(target_os = "android"))]
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tauri_specta::Event;
use tokio_util::sync::CancellationToken;

mod ai;
pub use ai::*;
mod attachments;
pub use attachments::*;
mod data;
pub use data::*;
mod lifecycle;
pub use lifecycle::*;
mod journals;
pub use journals::*;
mod logseq_import;
pub use logseq_import::*;
mod pages;
pub use pages::*;
mod outliner;
pub use outliner::*;
mod system;
pub use system::*;

#[cfg(target_os = "android")]
use tauri_plugin_mobile_system::MobileSystemExt;

pub type CommandResult<T> = Result<T, CommandError>;

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CommandErrorCode {
    InvalidInput,
    NotFound,
    Conflict,
    Unavailable,
    Internal,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: CommandErrorCode,
    pub message: String,
}

impl CommandError {
    fn new(code: CommandErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(CommandErrorCode::InvalidInput, message)
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self::new(CommandErrorCode::Conflict, message)
    }
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self::invalid(message)
    }
}

impl From<&str> for CommandError {
    fn from(message: &str) -> Self {
        Self::invalid(message)
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CommandError {}

pub struct AppState {
    pub conn: Connection,
    pub blob_store: notes_blob::BlobStore,
    pub remote_ai: Option<notes_sync::HttpTransport>,
    pub chat_cancellations: Arc<std::sync::Mutex<HashMap<uuid::Uuid, CancellationToken>>>,
}

/// The single frontend invalidation stream for persisted Rust state.
/// Payloads carry affected IDs when a command can identify them; whole-workspace
/// replacements (import/sync) deliberately request a full cache refresh.
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DomainEvent {
    PagesChanged {
        page_uuids: Vec<uuid::Uuid>,
    },
    BlocksChanged {
        block_uuids: Vec<uuid::Uuid>,
        container_uuids: Vec<uuid::Uuid>,
    },
    PagesDeleted {
        page_uuids: Vec<uuid::Uuid>,
    },
    BlocksDeleted {
        block_uuids: Vec<uuid::Uuid>,
        container_uuids: Vec<uuid::Uuid>,
    },
    GraphChanged {
        content_uuids: Vec<uuid::Uuid>,
    },
    StructureChanged {
        block_uuids: Vec<uuid::Uuid>,
    },
    AttachmentsChanged {
        owner_uuids: Vec<uuid::Uuid>,
    },
    HistoryChanged,
    SettingsChanged,
    SyncStatusChanged,
    ServerAiChanged,
    WorkspaceChanged,
}

#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "domain:event")]
pub struct DomainEventMessage(pub DomainEvent);

#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "app:ready")]
pub struct StartupReadyEvent;

#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
#[tauri_specta(event_name = "app:startup-error")]
pub struct StartupErrorEvent {
    pub message: String,
}

pub(crate) fn emit_domain(app: &AppHandle, event: DomainEvent) {
    if let Err(error) = DomainEventMessage(event).emit(app) {
        tracing::warn!(%error, "failed to emit domain event");
    }
}

pub(crate) fn emit_pages_changed(app: &AppHandle, pages: &[Page]) {
    emit_domain(
        app,
        DomainEvent::PagesChanged {
            page_uuids: pages.iter().map(|page| page.uuid).collect(),
        },
    );
}

pub(crate) fn emit_blocks_changed(
    app: &AppHandle,
    blocks: &[Block],
    additional_container_uuids: impl IntoIterator<Item = uuid::Uuid>,
) {
    let container_uuids = blocks
        .iter()
        .map(|block| block.parent_uuid.unwrap_or(block.page_uuid))
        .chain(additional_container_uuids)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    emit_domain(
        app,
        DomainEvent::BlocksChanged {
            block_uuids: blocks.iter().map(|block| block.uuid).collect(),
            container_uuids,
        },
    );
}

/// Registered immediately in setup so the frontend can ask whether the heavy
/// initialization (embedder download, db open) has finished. Without this,
/// commands invoked during the ~minutes-long first-run model fetch fail with
/// an opaque "state not managed" error.
pub struct Startup {
    pub status: Arc<RwLock<StartupStatus>>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum StartupStatus {
    Starting { message: String },
    Ready,
    Error { message: String },
}

fn err<E: std::fmt::Display + 'static>(error: E) -> CommandError {
    let any = &error as &dyn std::any::Any;
    if let Some(error) = any.downcast_ref::<anyhow::Error>() {
        let message = format!("{error:#}");
        if let Some(error) = error.downcast_ref::<notes_core::CoreError>() {
            let code = match error {
                notes_core::CoreError::InvalidInput(_) => CommandErrorCode::InvalidInput,
                notes_core::CoreError::NotFound(_) => CommandErrorCode::NotFound,
                notes_core::CoreError::Conflict(_) | notes_core::CoreError::SyncConflict(_) => {
                    CommandErrorCode::Conflict
                }
                notes_core::CoreError::Database(_) => CommandErrorCode::Internal,
            };
            return CommandError::new(code, message);
        }
        if let Some(error) = notes_sync::transport_error(error) {
            let code = match error {
                notes_sync::TransportError::Unauthorized => CommandErrorCode::Unavailable,
                notes_sync::TransportError::Conflict(_) => CommandErrorCode::Conflict,
                notes_sync::TransportError::Http {
                    code:
                        Some(
                            notes_protocol::ApiErrorCode::InvalidRequest
                            | notes_protocol::ApiErrorCode::PayloadTooLarge,
                        ),
                    ..
                } => CommandErrorCode::InvalidInput,
                notes_sync::TransportError::Http {
                    code: Some(notes_protocol::ApiErrorCode::NotFound),
                    ..
                } => CommandErrorCode::NotFound,
                notes_sync::TransportError::Http {
                    code: Some(notes_protocol::ApiErrorCode::Unavailable),
                    ..
                } => CommandErrorCode::Unavailable,
                notes_sync::TransportError::Http { status: 400, .. } => {
                    CommandErrorCode::InvalidInput
                }
                notes_sync::TransportError::Http { status: 404, .. } => CommandErrorCode::NotFound,
                notes_sync::TransportError::Http { status: 409, .. } => CommandErrorCode::Conflict,
                notes_sync::TransportError::Http { status, .. } if *status >= 500 => {
                    CommandErrorCode::Unavailable
                }
                notes_sync::TransportError::Http { .. } => CommandErrorCode::Internal,
            };
            return CommandError::new(code, message);
        }
        if notes_sync::is_transport_failure(error) {
            return CommandError::new(CommandErrorCode::Unavailable, message);
        }
        return CommandError::new(CommandErrorCode::Internal, message);
    }
    if let Some(error) = any.downcast_ref::<notes_core::CoreError>() {
        let code = match error {
            notes_core::CoreError::InvalidInput(_) => CommandErrorCode::InvalidInput,
            notes_core::CoreError::NotFound(_) => CommandErrorCode::NotFound,
            notes_core::CoreError::Conflict(_) | notes_core::CoreError::SyncConflict(_) => {
                CommandErrorCode::Conflict
            }
            notes_core::CoreError::Database(_) => CommandErrorCode::Internal,
        };
        return CommandError::new(code, error.to_string());
    }
    CommandError::new(CommandErrorCode::Internal, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_errors_expose_stable_categories() {
        assert!(matches!(
            err(anyhow::Error::from(notes_core::CoreError::invalid(
                "title is required"
            )))
            .code,
            CommandErrorCode::InvalidInput
        ));
        assert!(matches!(
            err(anyhow::Error::from(notes_core::CoreError::not_found(
                "block was not found"
            )))
            .code,
            CommandErrorCode::NotFound
        ));
        assert!(matches!(
            err(anyhow::Error::from(notes_core::CoreError::conflict(
                "request is already active"
            )))
            .code,
            CommandErrorCode::Conflict
        ));
        assert!(matches!(
            err(anyhow::Error::from(
                notes_sync::TransportError::Unauthorized
            ))
            .code,
            CommandErrorCode::Unavailable
        ));
        assert!(matches!(
            err(anyhow::Error::from(notes_sync::TransportError::Http {
                status: 400,
                code: Some(notes_protocol::ApiErrorCode::InvalidRequest),
                message: "invalid provider configuration".into(),
            }))
            .code,
            CommandErrorCode::InvalidInput
        ));
    }
}
