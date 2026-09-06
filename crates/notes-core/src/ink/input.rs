//! Shared input model and validation for the handwriting adapter.
use super::{CommandError, CommandResult};
use serde::{Deserialize, Serialize};
pub(super) const MAX_POINTS: usize = 150_000;

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InkPoint {
    // validate() rejects non-finite values before persistence or return to the webview.
    #[specta(type = specta_typescript::Number)]
    pub x: f64,
    #[specta(type = specta_typescript::Number)]
    pub y: f64,
    #[specta(type = specta_typescript::Number)]
    pub pressure: f64,
    #[specta(type = specta_typescript::Number)]
    pub tilt_x: f64,
    #[specta(type = specta_typescript::Number)]
    pub tilt_y: f64,
    #[specta(type = specta_typescript::Number)]
    pub time: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InkStroke {
    pub id: uuid::Uuid,
    #[specta(type = specta_typescript::Number)]
    pub width: f64,
    pub points: Vec<InkPoint>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum InkBackground {
    #[default]
    Plain,
    Grid,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InkDraft {
    pub version: u32,
    pub width: u32,
    pub height: u32,
    pub strokes: Vec<InkStroke>,
    #[serde(default)]
    pub background: InkBackground,
}

/// An IPC update against an acknowledged immutable document root.
#[derive(Debug, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InkDraftPatch {
    pub order: Vec<uuid::Uuid>,
    pub upserts: Vec<InkStroke>,
    pub background: InkBackground,
}

impl InkDraft {
    pub fn validate(&self) -> CommandResult<()> {
        validate(self)
    }
}

impl Default for InkDraft {
    fn default() -> Self {
        Self {
            version: 1,
            width: 1000,
            height: 1400,
            strokes: Vec::new(),
            background: InkBackground::Plain,
        }
    }
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct InkDraftSnapshot {
    pub draft: InkDraft,
    pub revision: Option<String>,
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct InkHistorySnapshot {
    pub snapshot: InkDraftSnapshot,
    pub can_undo: bool,
    pub can_redo: bool,
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum InkHistoryUpdate {
    Snapshot {
        history: InkHistorySnapshot,
    },
    Patch {
        patch: InkDraftPatch,
        base_revision: Option<String>,
        revision: Option<String>,
        can_undo: bool,
        can_redo: bool,
    },
}

pub(super) fn validate(draft: &InkDraft) -> CommandResult<()> {
    if draft.version != 1 || draft.width != 1000 || draft.height != 1400 {
        return Err(CommandError::invalid(
            "Unsupported handwriting sheet format",
        ));
    }
    let mut count = 0;
    let mut ids = std::collections::HashSet::new();
    for stroke in &draft.strokes {
        count += stroke.points.len();
        if count > MAX_POINTS
            || stroke.points.is_empty()
            || stroke.id.is_nil()
            || !ids.insert(stroke.id)
            || !stroke.width.is_finite()
            || !(0.5..=20.0).contains(&stroke.width)
        {
            return Err(CommandError::invalid(
                "Invalid or oversized handwriting sheet",
            ));
        }
        for point in &stroke.points {
            if ![
                point.x,
                point.y,
                point.pressure,
                point.tilt_x,
                point.tilt_y,
                point.time,
            ]
            .iter()
            .all(|value| value.is_finite())
                || !(0.0..=1000.0).contains(&point.x)
                || !(0.0..=1400.0).contains(&point.y)
                || !(0.0..=1.0).contains(&point.pressure)
                || !(-90.0..=90.0).contains(&point.tilt_x)
                || !(-90.0..=90.0).contains(&point.tilt_y)
                || point.time < 0.0
            {
                return Err(CommandError::invalid("Invalid handwriting point"));
            }
        }
    }
    Ok(())
}
