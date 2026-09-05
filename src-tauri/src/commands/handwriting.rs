//! Device-local scratch sheet for the handwriting input experiment.
//! It deliberately does not enter the synced note model before that format is designed.
use super::*;
use serde::Deserialize;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_POINTS: usize = 150_000;

#[derive(Default)]
pub struct HandwritingStore(pub Arc<Mutex<()>>);

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

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InkDraft {
    pub version: u32,
    pub width: u32,
    pub height: u32,
    pub strokes: Vec<InkStroke>,
}

impl Default for InkDraft {
    fn default() -> Self {
        Self {
            version: 1,
            width: 1000,
            height: 1400,
            strokes: Vec::new(),
        }
    }
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct InkDraftSnapshot {
    pub draft: InkDraft,
    pub revision: Option<String>,
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
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InkDraftSnapshot {
                draft: InkDraft::default(),
                revision: None,
            });
        }
        Err(error) => return Err(err(error)),
    };
    if metadata.len() > MAX_BYTES {
        return Err(CommandError::invalid(
            "Saved handwriting sheet is too large",
        ));
    }
    let bytes = std::fs::read(path).map_err(err)?;
    let draft = serde_json::from_slice(&bytes).map_err(err)?;
    validate(&draft)?;
    Ok(InkDraftSnapshot {
        draft,
        revision: Some(format!("{:x}", Sha256::digest(&bytes))),
    })
}

fn write_draft(
    path: &Path,
    draft: InkDraft,
    expected_revision: Option<String>,
) -> CommandResult<String> {
    validate(&draft)?;
    let current = read_draft(path)?;
    if current.revision != expected_revision {
        return Err(CommandError::conflict(
            "The handwriting draft changed. Reopen it before saving.",
        ));
    }
    let bytes = serde_json::to_vec(&draft).map_err(err)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(CommandError::invalid("Handwriting sheet is too large"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| CommandError::invalid("Invalid draft path"))?;
    std::fs::create_dir_all(parent).map_err(err)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(err)?;
    temporary.write_all(&bytes).map_err(err)?;
    temporary.as_file().sync_all().map_err(err)?;
    temporary.persist(path).map_err(err)?;
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(err)?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
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
        .join("handwriting/draft-v1.json");
    let lock = store.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock
            .lock()
            .map_err(|error| err(anyhow::anyhow!(error.to_string())))?;
        read_draft(&path)
    })
    .await
    .map_err(err)?
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
        .join("handwriting/draft-v1.json");
    let lock = store.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock
            .lock()
            .map_err(|error| err(anyhow::anyhow!(error.to_string())))?;
        write_draft(&path, draft, expected_revision)
    })
    .await
    .map_err(err)?
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
    fn preserves_strokes_and_rejects_stale_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ink/draft.json");
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
        let path = dir.path().join("draft.json");
        let revision = write_draft(&path, sample(), None).unwrap();
        let mut invalid = sample();
        invalid.strokes[0].points[0].x = f64::NAN;
        assert!(write_draft(&path, invalid, Some(revision.clone())).is_err());
        assert_eq!(read_draft(&path).unwrap().revision, Some(revision));
        std::fs::write(&path, b"broken file").unwrap();
        assert!(write_draft(&path, InkDraft::default(), None).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken file");
    }
}
