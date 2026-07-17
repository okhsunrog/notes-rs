use std::path::PathBuf;

use notes_import::{SourceKind, scan_logseq_graph};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DrawingConversionInputs {
    source_manifest_sha256: String,
    drawings: Vec<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source_root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "usage: drawing_conversion_inputs <logseq-graph>",
            )
        })?;
    let scan = scan_logseq_graph(source_root)?;
    let drawings = scan
        .manifest
        .entries()
        .iter()
        .filter(|entry| entry.kind == SourceKind::Drawing)
        .map(|entry| entry.relative_path.clone())
        .collect();
    serde_json::to_writer(
        std::io::stdout().lock(),
        &DrawingConversionInputs {
            source_manifest_sha256: scan.manifest.sha256().to_string(),
            drawings,
        },
    )?;
    Ok(())
}
