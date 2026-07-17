use std::fs;

use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use notes_import::{
    DocumentFormat, DrawingConversionBrowserName, DrawingConversionBundle, DrawingConversionEntry,
    DrawingConversionFailure, DrawingConversionFailureCode, DrawingConversionMime,
    DrawingConversionOutput, DrawingConversionSource, DrawingConversionSourceRoot,
    DrawingConversionSourceRootKind, DrawingConversionStatus, DrawingConverterIdentity,
    IdentityContext, LoadDrawingConversionErrorCode, MediaMaterializationIssue,
    MediaMaterializationLimits, PreparedImport, PreservedMediaReason, Sha256Digest, SourceKind,
    load_drawing_conversion_publication, load_drawing_conversion_publication_with_limit,
    materialize_source_media, materialize_source_media_with_drawing_conversions,
    materialize_source_media_with_drawing_conversions_and_limits, parse_logseq_markdown,
    prepare_import, scan_logseq_graph,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;

const DRAWING_BYTES: &[u8] = br#"{"type":"excalidraw","version":2,"elements":[]}"#;

enum TestResult {
    Converted(DrawingConversionOutput),
    SkippedEmpty,
    Failed(DrawingConversionFailure),
}

#[test]
fn valid_publication_converts_and_deduplicates_repeated_drawing_references() {
    let graph = graph("- [[draws/diagram.excalidraw]] and [[draws/diagram.excalidraw]]\n");
    let prepared = prepare(&graph);
    let png = png(1, 1);
    let publication = publication(&prepared, TestResult::Converted(output(&png, 1, 1)), &png);
    let loaded = load_drawing_conversion_publication(publication.path()).expect("load bundle");
    let plan = materialize_source_media_with_drawing_conversions(&prepared, graph.path(), &loaded)
        .expect("materialize converted drawing");

    assert_eq!(plan.blobs.len(), 1);
    assert_eq!(plan.attachments.len(), 1);
    assert_eq!(plan.rewrites.len(), 2);
    assert!(plan.preserved_references.is_empty());
    assert!(plan.diagnostics.is_empty());
    assert!(
        plan.attachments
            .iter()
            .all(|attachment| attachment.mime == "image/png")
    );
    assert!(plan.rewrites.iter().all(|rewrite| {
        rewrite.replacement_markdown.starts_with("![diagram]")
            && rewrite.replacement_markdown.contains("notes-attachment:")
    }));
}

#[test]
fn legacy_api_and_absent_failed_skipped_results_preserve_exact_tokens() {
    let graph = graph("- [[draws/diagram.excalidraw]]\n");
    let prepared = prepare(&graph);
    let legacy = materialize_source_media(&prepared, graph.path()).expect("legacy path");
    assert_preserved(
        &legacy,
        MediaMaterializationIssue::DeferredExcalidrawConversion,
    );

    let png = png(1, 1);
    for (result, expected, has_detail) in [
        (
            TestResult::SkippedEmpty,
            MediaMaterializationIssue::DrawingConversionSkippedEmpty,
            false,
        ),
        (
            TestResult::Failed(DrawingConversionFailure {
                code: DrawingConversionFailureCode::RenderFailed,
                message: "renderer rejected the scene".into(),
            }),
            MediaMaterializationIssue::DrawingConversionFailed,
            true,
        ),
    ] {
        let root = publication(&prepared, result, &png);
        let loaded = load_drawing_conversion_publication(root.path()).expect("load bundle");
        let plan =
            materialize_source_media_with_drawing_conversions(&prepared, graph.path(), &loaded)
                .expect("preserve result");
        assert_preserved(&plan, expected);
        assert_eq!(plan.diagnostics[0].detail.is_some(), has_detail);
    }

    let missing_root = TempDir::new().expect("publication");
    write_bundle(missing_root.path(), &bundle(&prepared, Vec::new()));
    let loaded = load_drawing_conversion_publication(missing_root.path()).expect("load empty");
    let missing =
        materialize_source_media_with_drawing_conversions(&prepared, graph.path(), &loaded)
            .expect("preserve missing");
    assert_preserved(
        &missing,
        MediaMaterializationIssue::DrawingConversionMissing,
    );
}

#[test]
fn binds_bundle_and_each_entry_to_the_exact_graph_and_source_bytes() {
    let source_graph = graph("- [[draws/diagram.excalidraw]]\n");
    let prepared = prepare(&source_graph);
    let png = png(1, 1);

    let wrong_graph = graph("- another page\n");
    let wrong_prepared = prepare(&wrong_graph);
    let root = publication(&prepared, converted(&png, 1, 1), &png);
    let mut value: serde_json::Value = serde_json::from_slice(
        &fs::read(root.path().join("conversion-bundle.json")).expect("read bundle"),
    )
    .expect("bundle json");
    value["sourceRoot"]["manifestSha256"] = wrong_prepared
        .provenance
        .source_manifest
        .sha256()
        .to_hex()
        .into();
    fs::write(
        root.path().join("conversion-bundle.json"),
        serde_json::to_vec_pretty(&value).expect("json"),
    )
    .expect("rewrite bundle");
    let loaded = load_drawing_conversion_publication(root.path()).expect("load bundle");
    let mismatch =
        materialize_source_media_with_drawing_conversions(&prepared, source_graph.path(), &loaded)
            .expect("manifest mismatch is non-fatal");
    assert_preserved(
        &mismatch,
        MediaMaterializationIssue::DrawingBundleManifestMismatch,
    );

    let root = publication(&prepared, converted(&png, 1, 1), &png);
    fs::write(
        source_graph.path().join("draws/diagram.excalidraw"),
        b"changed but same source is no longer trusted",
    )
    .expect("mutate drawing");
    let loaded = load_drawing_conversion_publication(root.path()).expect("load bundle");
    let changed =
        materialize_source_media_with_drawing_conversions(&prepared, source_graph.path(), &loaded)
            .expect("source mismatch is non-fatal");
    assert_preserved(
        &changed,
        MediaMaterializationIssue::DrawingConversionSourceMismatch,
    );

    let graph_with_orphan = graph("- [[draws/diagram.excalidraw]]\n");
    fs::write(
        graph_with_orphan.path().join("draws/orphan.excalidraw"),
        DRAWING_BYTES,
    )
    .expect("orphan drawing");
    let prepared = prepare(&graph_with_orphan);
    let orphan = prepared
        .provenance
        .source_manifest
        .entries()
        .iter()
        .find(|entry| entry.relative_path == "draws/orphan.excalidraw")
        .expect("orphan manifest entry");
    let root = TempDir::new().expect("publication");
    write_bundle(
        root.path(),
        &bundle(
            &prepared,
            vec![DrawingConversionEntry {
                source: DrawingConversionSource {
                    relative_path: orphan.relative_path.clone(),
                    size_bytes: orphan.size_bytes,
                    sha256: orphan.sha256,
                },
                status: DrawingConversionStatus::SkippedEmpty,
                output: None,
                error: None,
            }],
        ),
    );
    let loaded = load_drawing_conversion_publication(root.path()).expect("load extra source");
    let valid_orphan = materialize_source_media_with_drawing_conversions(
        &prepared,
        graph_with_orphan.path(),
        &loaded,
    )
    .expect("an exact unreferenced manifest drawing is harmless");
    assert_preserved(
        &valid_orphan,
        MediaMaterializationIssue::DrawingConversionMissing,
    );

    let mut value: serde_json::Value = serde_json::from_slice(
        &fs::read(root.path().join("conversion-bundle.json")).expect("read bundle"),
    )
    .expect("bundle json");
    value["drawings"][0]["source"]["relativePath"] =
        serde_json::json!("draws/not-in-manifest.excalidraw");
    fs::write(
        root.path().join("conversion-bundle.json"),
        serde_json::to_vec_pretty(&value).expect("json"),
    )
    .expect("rewrite bundle");
    let loaded = load_drawing_conversion_publication(root.path()).expect("load unknown source");
    let unexpected = materialize_source_media_with_drawing_conversions(
        &prepared,
        graph_with_orphan.path(),
        &loaded,
    )
    .expect("unknown source is non-fatal");
    assert_preserved(
        &unexpected,
        MediaMaterializationIssue::DrawingBundleUnexpectedSource,
    );
}

#[test]
fn rejects_tampered_or_invalid_png_artifacts_without_rewriting_source_markdown() {
    let graph = graph("- [[draws/diagram.excalidraw]]\n");
    let prepared = prepare(&graph);
    let png = png(1, 1);

    for mutation in ["changed", "missing", "invalid_png", "wrong_dimensions"] {
        let dimensions = if mutation == "wrong_dimensions" {
            (2, 1)
        } else {
            (1, 1)
        };
        let root = publication(&prepared, converted(&png, dimensions.0, dimensions.1), &png);
        let artifact = root.path().join("artifacts/diagram.png");
        match mutation {
            "changed" => fs::write(&artifact, b"same length is irrelevant").expect("tamper"),
            "missing" => fs::remove_file(&artifact).expect("remove"),
            "invalid_png" => {
                let bytes = b"not a png";
                fs::write(&artifact, bytes).expect("invalid png");
                let mut value: serde_json::Value = serde_json::from_slice(
                    &fs::read(root.path().join("conversion-bundle.json")).expect("read bundle"),
                )
                .expect("bundle json");
                value["drawings"][0]["output"]["sizeBytes"] = (bytes.len() as u64).into();
                value["drawings"][0]["output"]["sha256"] = digest(bytes).to_hex().into();
                fs::write(
                    root.path().join("conversion-bundle.json"),
                    serde_json::to_vec_pretty(&value).expect("json"),
                )
                .expect("rewrite bundle");
            }
            "wrong_dimensions" => {}
            _ => unreachable!(),
        }
        let loaded = load_drawing_conversion_publication(root.path()).expect("load bundle");
        let plan =
            materialize_source_media_with_drawing_conversions(&prepared, graph.path(), &loaded)
                .expect("artifact failure is non-fatal");
        let expected = match mutation {
            "changed" => MediaMaterializationIssue::DrawingArtifactChanged,
            "missing" => MediaMaterializationIssue::DrawingArtifactNotFound,
            "invalid_png" => MediaMaterializationIssue::DrawingArtifactInvalidPng,
            "wrong_dimensions" => MediaMaterializationIssue::DrawingArtifactDimensionsMismatch,
            _ => unreachable!(),
        };
        assert_preserved(&plan, expected);
    }
}

#[cfg(unix)]
#[test]
fn rejects_artifact_symlinks_and_publications_inside_the_source_graph() {
    use std::os::unix::fs::symlink;

    let graph = graph("- [[draws/diagram.excalidraw]]\n");
    let prepared = prepare(&graph);
    let png = png(1, 1);
    let root = publication(&prepared, converted(&png, 1, 1), &png);
    let artifact = root.path().join("artifacts/diagram.png");
    let outside = root.path().join("outside.png");
    fs::write(&outside, &png).expect("outside file");
    fs::remove_file(&artifact).expect("remove artifact");
    symlink(&outside, &artifact).expect("artifact symlink");
    let loaded = load_drawing_conversion_publication(root.path()).expect("load bundle");
    let plan = materialize_source_media_with_drawing_conversions(&prepared, graph.path(), &loaded)
        .expect("symlink is non-fatal");
    assert_preserved(
        &plan,
        MediaMaterializationIssue::DrawingArtifactSymlinkNotAllowed,
    );

    let inside = graph.path().join("conversion");
    fs::create_dir(&inside).expect("inside publication");
    let entry = drawing_entry(&prepared, converted(&png, 1, 1));
    fs::create_dir_all(inside.join("artifacts")).expect("artifact dir");
    fs::write(inside.join("artifacts/diagram.png"), &png).expect("artifact");
    write_bundle(&inside, &bundle(&prepared, vec![entry]));
    let loaded = load_drawing_conversion_publication(&inside).expect("load inside bundle");
    let plan = materialize_source_media_with_drawing_conversions(&prepared, graph.path(), &loaded)
        .expect("overlap is non-fatal");
    assert_preserved(
        &plan,
        MediaMaterializationIssue::DrawingPublicationOverlapsSource,
    );

    let valid = publication(&prepared, converted(&png, 1, 1), &png);
    let links = TempDir::new().expect("link root");
    let root_link = links.path().join("publication-link");
    symlink(valid.path(), &root_link).expect("publication symlink");
    assert_eq!(
        load_drawing_conversion_publication(&root_link)
            .expect_err("reject root symlink")
            .code(),
        LoadDrawingConversionErrorCode::RootSymlinkNotAllowed
    );

    let bundle_path = valid.path().join("conversion-bundle.json");
    let real_bundle = valid.path().join("real-bundle.json");
    fs::rename(&bundle_path, &real_bundle).expect("move bundle");
    symlink(&real_bundle, &bundle_path).expect("bundle symlink");
    assert_eq!(
        load_drawing_conversion_publication(valid.path())
            .expect_err("reject bundle symlink")
            .code(),
        LoadDrawingConversionErrorCode::BundleSymlinkNotAllowed
    );
}

#[test]
fn enforces_shared_blob_budgets_and_exact_image_boundaries() {
    let graph = graph("- [[draws/diagram.excalidraw]]\n");
    let prepared = prepare(&graph);
    let png = png(2, 1);
    let root = publication(&prepared, converted(&png, 2, 1), &png);
    let loaded = load_drawing_conversion_publication(root.path()).expect("load bundle");
    let defaults = MediaMaterializationLimits::default();
    assert_eq!(defaults.max_drawing_image_width, 8_192);
    assert_eq!(defaults.max_drawing_image_height, 8_192);
    assert_eq!(defaults.max_drawing_image_pixels, 25_000_000);

    let exact = materialize_source_media_with_drawing_conversions_and_limits(
        &prepared,
        graph.path(),
        &loaded,
        &MediaMaterializationLimits {
            max_blob_bytes: png.len() as u64,
            max_total_blob_bytes: png.len() as u64,
            max_drawing_image_width: 2,
            max_drawing_image_height: 1,
            max_drawing_image_pixels: 2,
            ..defaults
        },
    )
    .expect("exact boundaries are accepted");
    assert_eq!(exact.rewrites.len(), 1);

    for (limits, expected) in [
        (
            MediaMaterializationLimits {
                max_blob_bytes: (png.len() - 1) as u64,
                ..defaults
            },
            MediaMaterializationIssue::BlobTooLarge,
        ),
        (
            MediaMaterializationLimits {
                max_drawing_image_width: 1,
                ..defaults
            },
            MediaMaterializationIssue::DrawingArtifactDimensionLimit,
        ),
        (
            MediaMaterializationLimits {
                max_drawing_image_pixels: 1,
                ..defaults
            },
            MediaMaterializationIssue::DrawingArtifactPixelLimit,
        ),
    ] {
        let plan = materialize_source_media_with_drawing_conversions_and_limits(
            &prepared,
            graph.path(),
            &loaded,
            &limits,
        )
        .expect("invalid conversion is preserved");
        assert_preserved(&plan, expected);
    }
}

#[test]
fn loader_rejects_unknown_schema_codes_unsafe_paths_duplicates_and_size_overflow() {
    let graph = graph("- [[draws/diagram.excalidraw]]\n");
    let prepared = prepare(&graph);
    let png = png(1, 1);
    let valid = publication(&prepared, converted(&png, 1, 1), &png);
    assert_eq!(
        load_drawing_conversion_publication_with_limit(valid.path(), 1)
            .expect_err("bundle limit")
            .code(),
        LoadDrawingConversionErrorCode::BundleTooLarge
    );

    for (field, value, expected) in [
        (
            "schemaVersion",
            serde_json::json!(2),
            LoadDrawingConversionErrorCode::UnsupportedSchemaVersion,
        ),
        (
            "sourcePath",
            serde_json::json!("../secret.excalidraw"),
            LoadDrawingConversionErrorCode::InvalidRelativePath,
        ),
        (
            "outputPath",
            serde_json::json!("../escape.png"),
            LoadDrawingConversionErrorCode::InvalidRelativePath,
        ),
        (
            "failureCode",
            serde_json::json!("unknown_code"),
            LoadDrawingConversionErrorCode::InvalidJson,
        ),
    ] {
        let root = TempDir::new().expect("publication");
        fs::create_dir(root.path().join("artifacts")).expect("artifacts");
        fs::write(root.path().join("artifacts/diagram.png"), &png).expect("png");
        let mut json = serde_json::to_value(bundle(
            &prepared,
            vec![drawing_entry(&prepared, converted(&png, 1, 1))],
        ))
        .expect("json");
        match field {
            "schemaVersion" => json["schemaVersion"] = value,
            "sourcePath" => json["drawings"][0]["source"]["relativePath"] = value,
            "outputPath" => json["drawings"][0]["output"]["relativePath"] = value,
            "failureCode" => {
                json["drawings"][0] = serde_json::json!({
                    "source": json["drawings"][0]["source"].clone(),
                    "status": "failed",
                    "error": {"code": value, "message": "safe"}
                });
            }
            _ => unreachable!(),
        }
        fs::write(
            root.path().join("conversion-bundle.json"),
            serde_json::to_vec(&json).expect("json bytes"),
        )
        .expect("bundle");
        assert_eq!(
            load_drawing_conversion_publication(root.path())
                .expect_err("reject malformed publication")
                .code(),
            expected,
            "field {field}"
        );
    }

    let root = TempDir::new().expect("publication");
    let entry = drawing_entry(&prepared, converted(&png, 1, 1));
    write_bundle(root.path(), &bundle(&prepared, vec![entry.clone(), entry]));
    assert_eq!(
        load_drawing_conversion_publication(root.path())
            .expect_err("duplicate source")
            .code(),
        LoadDrawingConversionErrorCode::DuplicateSource
    );
}

fn assert_preserved(
    plan: &notes_import::MediaMaterializationPlan,
    issue: MediaMaterializationIssue,
) {
    assert!(plan.blobs.is_empty());
    assert!(plan.attachments.is_empty());
    assert!(plan.rewrites.is_empty());
    assert_eq!(plan.preserved_references.len(), 1);
    assert_eq!(
        plan.preserved_references[0].reason,
        PreservedMediaReason::DeferredConversion
    );
    assert_eq!(plan.diagnostics.len(), 1);
    assert_eq!(plan.diagnostics[0].issue, issue);
}

fn converted(bytes: &[u8], width: u32, height: u32) -> TestResult {
    TestResult::Converted(output(bytes, width, height))
}

fn output(bytes: &[u8], width: u32, height: u32) -> DrawingConversionOutput {
    DrawingConversionOutput {
        relative_path: "artifacts/diagram.png".into(),
        size_bytes: bytes.len() as u64,
        sha256: digest(bytes),
        width,
        height,
        mime_type: DrawingConversionMime::ImagePng,
    }
}

fn publication(prepared: &PreparedImport, result: TestResult, output_bytes: &[u8]) -> TempDir {
    let root = TempDir::new().expect("publication");
    fs::create_dir(root.path().join("artifacts")).expect("artifact directory");
    fs::write(root.path().join("artifacts/diagram.png"), output_bytes).expect("artifact");
    write_bundle(
        root.path(),
        &bundle(prepared, vec![drawing_entry(prepared, result)]),
    );
    root
}

fn write_bundle(root: &std::path::Path, bundle: &DrawingConversionBundle) {
    fs::write(
        root.join("conversion-bundle.json"),
        serde_json::to_vec_pretty(bundle).expect("serialize bundle"),
    )
    .expect("write bundle");
}

fn bundle(
    prepared: &PreparedImport,
    drawings: Vec<DrawingConversionEntry>,
) -> DrawingConversionBundle {
    DrawingConversionBundle {
        schema_version: 1,
        converter: DrawingConverterIdentity {
            name: "notes-rs-excalidraw-converter".into(),
            version: "1.0.0".into(),
            excalidraw_version: "0.18.0".into(),
            playwright_version: "1.55.0".into(),
            browser_name: DrawingConversionBrowserName::Chromium,
            browser_version: "140.0".into(),
        },
        source_root: DrawingConversionSourceRoot {
            kind: DrawingConversionSourceRootKind::Redacted,
            manifest_sha256: prepared.provenance.source_manifest.sha256(),
        },
        drawings,
    }
}

fn drawing_entry(prepared: &PreparedImport, result: TestResult) -> DrawingConversionEntry {
    let reference = prepared
        .media_references
        .iter()
        .find(|reference| reference.kind == notes_import::ImportMediaKind::LegacyExcalidraw)
        .expect("drawing reference");
    let notes_import::ImportMediaResolution::LocalManifest {
        relative_path,
        size_bytes,
        sha256,
    } = &reference.resolution
    else {
        panic!("local drawing")
    };
    let (status, output, error) = match result {
        TestResult::Converted(output) => (DrawingConversionStatus::Converted, Some(output), None),
        TestResult::SkippedEmpty => (DrawingConversionStatus::SkippedEmpty, None, None),
        TestResult::Failed(error) => (DrawingConversionStatus::Failed, None, Some(error)),
    };
    DrawingConversionEntry {
        source: DrawingConversionSource {
            relative_path: relative_path.clone(),
            size_bytes: *size_bytes,
            sha256: *sha256,
        },
        status,
        output,
        error,
    }
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let pixels = vec![0x7f; (width * height * 4) as usize];
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(&pixels, width, height, ExtendedColorType::Rgba8)
        .expect("encode PNG");
    bytes
}

fn digest(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::from_bytes(Sha256::digest(bytes).into())
}

fn graph(source: &str) -> TempDir {
    let temp = TempDir::new().expect("temp graph");
    fs::create_dir_all(temp.path().join("pages")).expect("pages");
    fs::create_dir_all(temp.path().join("draws")).expect("draws");
    fs::write(temp.path().join("pages/Media.md"), source).expect("page");
    fs::write(temp.path().join("draws/diagram.excalidraw"), DRAWING_BYTES).expect("drawing");
    temp
}

fn prepare(graph: &TempDir) -> PreparedImport {
    let report = scan_logseq_graph(graph.path()).expect("scan graph");
    let documents = report
        .manifest
        .entries()
        .iter()
        .filter(|entry| {
            matches!(entry.kind, SourceKind::Page | SourceKind::Journal)
                && entry.document_format == Some(DocumentFormat::Markdown)
        })
        .map(|entry| {
            let bytes = fs::read(graph.path().join(&entry.relative_path)).expect("document");
            parse_logseq_markdown(entry, &bytes, report.manifest.config()).expect("parse")
        })
        .collect::<Vec<_>>();
    prepare_import(
        &documents,
        IdentityContext {
            workspace_uuid: Uuid::from_u128(0x301),
            import_namespace_uuid: Uuid::from_u128(0x401),
        },
        &report.manifest,
    )
    .expect("prepare")
}
