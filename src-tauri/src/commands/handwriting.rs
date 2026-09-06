//! Device-local scratch sheet for the handwriting input experiment.
//! It deliberately does not enter the synced note model before that format is designed.
use super::*;
use serde::Deserialize;
use std::path::Path;
use std::sync::Mutex;

mod storage;
const MAX_POINTS: usize = 150_000;

#[derive(Default)]
struct MaintenanceState {
    running: bool,
    dirty: bool,
}
#[derive(Default)]
pub struct HandwritingStore {
    lock: Arc<Mutex<()>>,
    maintenance: Arc<Mutex<MaintenanceState>>,
}
impl HandwritingStore {
    fn schedule_compaction(&self, path: std::path::PathBuf) {
        {
            let mut state = self.maintenance.lock().expect("maintenance mutex poisoned");
            state.dirty = true;
            if state.running {
                return;
            }
            state.running = true;
        }
        let maintenance = self.maintenance.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                // A fixed interval: continued writing never postpones the next batch.
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                maintenance
                    .lock()
                    .expect("maintenance mutex poisoned")
                    .dirty = false;
                let job_path = path.clone();
                let packed =
                    match tauri::async_runtime::spawn_blocking(move || storage::compact(&job_path))
                        .await
                    {
                        Ok(Ok(packed)) => packed,
                        result => {
                            tracing::warn!(
                                ?result,
                                "Handwriting compaction deferred; saved history is unchanged"
                            );
                            false
                        }
                    };
                let mut state = maintenance.lock().expect("maintenance mutex poisoned");
                if !state.dirty && !packed {
                    state.running = false;
                    break;
                }
            }
        });
    }
}

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

#[tauri::command]
#[specta::specta]
pub async fn handwriting_history(
    app: AppHandle,
    store: State<'_, HandwritingStore>,
    redo: Option<bool>,
    expected_revision: Option<String>,
) -> CommandResult<InkHistoryUpdate> {
    let path = app
        .path()
        .app_data_dir()
        .map_err(err)?
        .join("handwriting/ink-v1.sqlite3");
    let lock = store.lock.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock
            .lock()
            .map_err(|e| err(anyhow::anyhow!(e.to_string())))?;
        storage::navigate_update(&path, redo, expected_revision)
    })
    .await
    .map_err(err)??;
    Ok(result)
}

fn validate(draft: &InkDraft) -> CommandResult<()> {
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

fn read_draft(path: &Path) -> CommandResult<InkDraftSnapshot> {
    storage::read(path)
}

fn write_draft(
    path: &Path,
    draft: InkDraft,
    expected_revision: Option<String>,
) -> CommandResult<String> {
    validate(&draft)?;
    storage::patch(
        path,
        InkDraftPatch {
            order: draft.strokes.iter().map(|s| s.id).collect(),
            upserts: draft.strokes,
            background: draft.background,
        },
        expected_revision,
    )
}

fn write_patch(
    path: &Path,
    patch: InkDraftPatch,
    expected_revision: Option<String>,
) -> CommandResult<String> {
    storage::patch(path, patch, expected_revision)
}

#[tauri::command]
#[specta::specta]
pub async fn save_handwriting_patch(
    app: AppHandle,
    store: State<'_, HandwritingStore>,
    patch: InkDraftPatch,
    expected_revision: Option<String>,
) -> CommandResult<String> {
    let path = app
        .path()
        .app_data_dir()
        .map_err(err)?
        .join("handwriting/ink-v1.sqlite3");
    let lock = store.lock.clone();
    let maintenance_path = path.clone();
    let geometry_changed = !patch.upserts.is_empty();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock
            .lock()
            .map_err(|error| err(anyhow::anyhow!(error.to_string())))?;
        write_patch(&path, patch, expected_revision)
    })
    .await
    .map_err(err)??;
    if geometry_changed {
        store.schedule_compaction(maintenance_path);
    }
    Ok(result)
}

#[tauri::command]
#[specta::specta]
pub async fn load_handwriting_draft(
    app: AppHandle,
    store: State<'_, HandwritingStore>,
) -> CommandResult<InkDraftSnapshot> {
    let path = app
        .path()
        .app_data_dir()
        .map_err(err)?
        .join("handwriting/ink-v1.sqlite3");
    let lock = store.lock.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock
            .lock()
            .map_err(|error| err(anyhow::anyhow!(error.to_string())))?;
        read_draft(&path)
    })
    .await
    .map_err(err)??;
    Ok(result)
}

#[tauri::command]
#[specta::specta]
pub async fn save_handwriting_draft(
    app: AppHandle,
    store: State<'_, HandwritingStore>,
    draft: InkDraft,
    expected_revision: Option<String>,
) -> CommandResult<String> {
    let path = app
        .path()
        .app_data_dir()
        .map_err(err)?
        .join("handwriting/ink-v1.sqlite3");
    let lock = store.lock.clone();
    let maintenance_path = path.clone();
    let geometry_changed = !draft.strokes.is_empty();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock
            .lock()
            .map_err(|error| err(anyhow::anyhow!(error.to_string())))?;
        write_draft(&path, draft, expected_revision)
    })
    .await
    .map_err(err)??;
    if geometry_changed {
        store.schedule_compaction(maintenance_path);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> InkDraft {
        InkDraft {
            strokes: vec![InkStroke {
                id: uuid::Uuid::now_v7(),
                width: 3.0,
                points: vec![InkPoint {
                    x: 10.0,
                    y: 20.0,
                    pressure: 0.7,
                    tilt_x: 12.0,
                    tilt_y: -5.0,
                    time: 25.0,
                }],
            }],
            ..InkDraft::default()
        }
    }

    #[test]
    fn patches_preserve_unchanged_points_delete_and_restore_strokes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        let a = sample().strokes.remove(0);
        let b = sample().strokes.remove(0);
        let original = InkDraft {
            strokes: vec![a.clone(), b.clone()],
            ..InkDraft::default()
        };
        let revision = write_draft(&path, original, None).unwrap();
        let revision = write_patch(
            &path,
            InkDraftPatch {
                order: vec![b.id],
                upserts: vec![],
                background: InkBackground::Grid,
            },
            Some(revision),
        )
        .unwrap();
        let saved = read_draft(&path).unwrap();
        assert_eq!(saved.draft.strokes.len(), 1);
        assert_eq!(
            serde_json::to_value(&saved.draft.strokes[0]).unwrap(),
            serde_json::to_value(&b).unwrap()
        );
        assert!(matches!(saved.draft.background, InkBackground::Grid));
        write_patch(
            &path,
            InkDraftPatch {
                order: vec![b.id, a.id],
                upserts: vec![a.clone()],
                background: InkBackground::Plain,
            },
            Some(revision),
        )
        .unwrap();
        let restored = read_draft(&path).unwrap();
        assert_eq!(restored.draft.strokes[1].id, a.id);
    }

    #[test]
    fn invalid_or_stale_patches_never_change_saved_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        let draft = sample();
        let stroke = draft.strokes[0].clone();
        let revision = write_draft(&path, draft, None).unwrap();
        let before = serde_json::to_value(read_draft(&path).unwrap()).unwrap();
        let mut invalid = stroke.clone();
        invalid.points[0].x = -1.0;
        for patch in [
            InkDraftPatch {
                order: vec![stroke.id, stroke.id],
                upserts: vec![],
                background: InkBackground::Plain,
            },
            InkDraftPatch {
                order: vec![uuid::Uuid::now_v7()],
                upserts: vec![],
                background: InkBackground::Plain,
            },
            InkDraftPatch {
                order: vec![stroke.id],
                upserts: vec![stroke.clone(), stroke.clone()],
                background: InkBackground::Plain,
            },
            InkDraftPatch {
                order: vec![stroke.id],
                upserts: vec![invalid],
                background: InkBackground::Plain,
            },
        ] {
            assert!(write_patch(&path, patch, Some(revision.clone())).is_err());
            assert_eq!(
                serde_json::to_value(read_draft(&path).unwrap()).unwrap(),
                before
            );
        }
        assert!(
            write_patch(
                &path,
                InkDraftPatch {
                    order: vec![],
                    upserts: vec![],
                    background: InkBackground::Plain
                },
                None
            )
            .is_err()
        );
        assert_eq!(
            serde_json::to_value(read_draft(&path).unwrap()).unwrap(),
            before
        );
    }

    #[test]
    fn fresh_database_defaults_to_plain_and_grid_round_trips() {
        let mut draft = InkDraft::default();
        assert!(matches!(draft.background, InkBackground::Plain));
        draft.background = InkBackground::Grid;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        write_draft(&path, draft, None).unwrap();
        assert!(matches!(
            read_draft(&path).unwrap().draft.background,
            InkBackground::Grid
        ));
    }

    #[test]
    fn preserves_strokes_and_rejects_stale_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink/ink.sqlite3");
        assert!(read_draft(&path).unwrap().revision.is_none());
        let revision = write_draft(&path, sample(), None).unwrap();
        let saved = read_draft(&path).unwrap();
        assert_eq!(saved.draft.strokes[0].points[0].pressure, 0.7);
        assert_eq!(saved.revision, Some(revision.clone()));
        assert!(write_draft(&path, InkDraft::default(), None).is_err());
        assert_eq!(read_draft(&path).unwrap().draft.strokes.len(), 1);
        write_draft(&path, InkDraft::default(), Some(revision)).unwrap();
        assert!(read_draft(&path).unwrap().draft.strokes.is_empty());
    }

    #[test]
    fn invalid_data_never_replaces_the_last_saved_draft() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        let revision = write_draft(&path, sample(), None).unwrap();
        let mut invalid = sample();
        invalid.strokes[0].points[0].x = f64::NAN;
        assert!(write_draft(&path, invalid, Some(revision.clone())).is_err());
        assert_eq!(read_draft(&path).unwrap().revision, Some(revision));
        std::fs::write(&path, b"broken file").unwrap();
        assert!(write_draft(&path, InkDraft::default(), None).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken file");
    }
    fn stored_rows(path: &Path, table: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
        let conn = rusqlite::Connection::open(path).unwrap();
        let mut stmt = conn
            .prepare(&format!("SELECT id,data FROM {table} ORDER BY id"))
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn sqlite_reuses_unchanged_chunks_and_preserves_float_bits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        let mut draft = sample();
        draft.strokes[0].points[0].x = -0.0;
        draft.strokes[0].points[0].y = 1.234567890123456;
        draft.strokes[0].points[0].time = 1789000000000.125;
        let original = draft.strokes[0].clone();
        let revision = write_draft(&path, draft, None).unwrap();
        assert!(
            std::fs::read(&path)
                .unwrap()
                .starts_with(b"SQLite format 3\0")
        );
        let chunks = stored_rows(&path, "ink_chunks");
        assert_eq!(chunks.len(), 1);
        let decoded = ink_format::chunk::Chunk::decode(&chunks[0].1).unwrap();
        assert_eq!(decoded.count().unwrap(), 1);
        let another = sample().strokes.remove(0);
        write_patch(
            &path,
            InkDraftPatch {
                order: vec![original.id, another.id],
                upserts: vec![another],
                background: InkBackground::Grid,
            },
            Some(revision),
        )
        .unwrap();
        assert!(stored_rows(&path, "ink_chunks").contains(&chunks[0]));
        let restored = read_draft(&path).unwrap();
        let point = &restored.draft.strokes[0].points[0];
        assert_eq!(point.x.to_bits(), original.points[0].x.to_bits());
        assert_eq!(point.y.to_bits(), original.points[0].y.to_bits());
        assert_eq!(point.time.to_bits(), original.points[0].time.to_bits());
    }

    #[test]
    fn failure_during_head_update_rolls_back_every_new_blob() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        let revision = write_draft(&path, sample(), None).unwrap();
        let records = stored_rows(&path, "ink_records");
        let chunks = stored_rows(&path, "ink_chunks");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TRIGGER fail_commit BEFORE UPDATE ON ink_head BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
        }
        assert!(write_draft(&path, sample(), Some(revision.clone())).is_err());
        assert_eq!(read_draft(&path).unwrap().revision, Some(revision));
        assert_eq!(stored_rows(&path, "ink_records"), records);
        assert_eq!(stored_rows(&path, "ink_chunks"), chunks);
    }
    #[test]
    fn history_survives_reopen_and_branches_without_aba() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        let original = sample();
        let original_id = original.strokes[0].id;
        let first = write_draft(&path, original, None).unwrap();
        let second = write_draft(&path, InkDraft::default(), Some(first.clone())).unwrap();
        let undo = storage::navigate(&path, Some(false), Some(second)).unwrap();
        assert_eq!(undo.snapshot.draft.strokes[0].id, original_id);
        assert!(undo.can_undo && undo.can_redo);
        assert_ne!(undo.snapshot.revision, Some(first.clone()));
        assert!(storage::navigate(&path, Some(false), Some(first)).is_err());
        let reopened = storage::navigate(&path, None, None).unwrap();
        assert!(reopened.can_redo);
        let redo = storage::navigate(&path, Some(true), reopened.snapshot.revision).unwrap();
        assert!(redo.snapshot.draft.strokes.is_empty());
        let undo = storage::navigate(&path, Some(false), redo.snapshot.revision).unwrap();
        write_draft(&path, sample(), undo.snapshot.revision).unwrap();
        let branched = storage::navigate(&path, None, None).unwrap();
        assert!(!branched.can_redo);
        let prior = storage::navigate(&path, Some(false), branched.snapshot.revision).unwrap();
        assert_eq!(prior.snapshot.draft.strokes[0].id, original_id);
        let empty = storage::navigate(&path, Some(false), prior.snapshot.revision).unwrap();
        assert!(empty.snapshot.draft.strokes.is_empty());
        assert!(!empty.can_undo && empty.can_redo);
    }

    #[test]
    fn history_updates_send_only_changed_strokes_and_reject_stale_bases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        let a = sample().strokes.remove(0);
        let b = sample().strokes.remove(0);
        let first = write_draft(
            &path,
            InkDraft {
                strokes: vec![a.clone()],
                ..InkDraft::default()
            },
            None,
        )
        .unwrap();
        let second = storage::patch(
            &path,
            InkDraftPatch {
                order: vec![a.id, b.id],
                upserts: vec![b.clone()],
                background: InkBackground::Plain,
            },
            Some(first),
        )
        .unwrap();
        // Compaction changes storage locations, but must not turn unchanged strokes into upserts.
        storage::compact(&path).unwrap();
        let InkHistoryUpdate::Patch {
            patch,
            base_revision,
            revision,
            can_redo,
            ..
        } = storage::navigate_update(&path, Some(false), Some(second.clone())).unwrap()
        else {
            panic!("expected patch")
        };
        assert_eq!(base_revision, Some(second.clone()));
        assert_eq!(patch.order, vec![a.id]);
        assert!(patch.upserts.is_empty());
        assert!(can_redo);
        assert!(storage::navigate_update(&path, Some(true), Some(second)).is_err());
        let InkHistoryUpdate::Patch {
            patch, revision, ..
        } = storage::navigate_update(&path, Some(true), revision).unwrap()
        else {
            panic!("expected patch")
        };
        assert_eq!(patch.order, vec![a.id, b.id]);
        assert_eq!(patch.upserts.len(), 1);
        assert_eq!(patch.upserts[0].id, b.id);
        let mut edited = b.clone();
        edited.points[0].x = 123.;
        let changed = storage::patch(
            &path,
            InkDraftPatch {
                order: vec![b.id, a.id],
                upserts: vec![edited],
                background: InkBackground::Grid,
            },
            revision,
        )
        .unwrap();
        let InkHistoryUpdate::Patch { patch, .. } =
            storage::navigate_update(&path, Some(false), Some(changed)).unwrap()
        else {
            panic!("expected patch")
        };
        assert_eq!(patch.order, vec![a.id, b.id]);
        assert_eq!(patch.upserts.len(), 1);
        assert_eq!(patch.upserts[0].points[0].x, b.points[0].x);
        assert!(matches!(patch.background, InkBackground::Plain));
        assert!(matches!(
            storage::navigate_update(&path, None, None).unwrap(),
            InkHistoryUpdate::Snapshot { .. }
        ));
    }

    #[test]
    fn history_retains_fifty_actions_and_upgrades_existing_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink.sqlite3");
        let mut revision = Some(write_draft(&path, sample(), None).unwrap());
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "DROP TABLE ink_history; DROP TABLE ink_cursor; PRAGMA user_version=1;",
            )
            .unwrap();
        }
        let initial = storage::navigate(&path, None, None).unwrap();
        assert!(!initial.can_undo && !initial.can_redo);
        for _ in 0..52 {
            revision = Some(write_draft(&path, sample(), revision).unwrap());
        }
        let mut count = 0;
        loop {
            let state = storage::navigate(&path, None, None).unwrap();
            if !state.can_undo {
                break;
            }
            storage::navigate(&path, Some(false), state.snapshot.revision).unwrap();
            count += 1;
        }
        assert_eq!(count, 50);
        assert_eq!(read_draft(&path).unwrap().draft.strokes.len(), 1);
    }
}
