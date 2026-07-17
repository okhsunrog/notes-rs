use std::fs;

use notes_import::{
    DiagnosticCode, DocumentFormat, IdentityContext, ImportInlineImageMime,
    ImportMediaBlockedReason, ImportMediaKind, ImportMediaOwner, ImportMediaResolution,
    ImportMediaUnsupportedReason, PrepareImportErrorCode, PrepareLimits, SourceKind,
    parse_logseq_markdown, prepare_import, prepare_import_with_limits, scan_logseq_graph,
};
use tempfile::TempDir;
use uuid::Uuid;

const BLOCK_UUID: &str = "11111111-1111-4111-8111-111111111111";
const PNG_DATA: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVQI12P4//8/AAX+Av7czFnnAAAAAElFTkSuQmCC";
const JPEG_DATA: &str = "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAYEBQYFBAYGBQYHBwYIChAKCgkJChQODwwQFxQYGBcUFhYaHSUfGhsjHBYWICwgIyYnKSopGR8tMC0oMCUoKSj/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAACP/EABQQAQAAAAAAAAAAAAAAAAAAAAD/2gAIAQEAAD8AVL//2Q==";

#[test]
fn classifies_commonmark_images_and_drawings_with_exact_typed_ownership_and_ranges() {
    let source = format!(
        "![preamble](../assets/pre.png \"Preamble title\")\n\
- TODO first ![local](../assets/a%20b.png 'Local title') and ![local](../assets/a%20b.png 'Local title')\n\
  id:: {BLOCK_UUID}\n\
  plus ![plus](../assets/a+b.png) drawing [[draws/diagram.excalidraw]]\n\
- ![reference][asset]\n\
  \n\
  [asset]: ../assets/reference.png \"Reference title\"\n\
- `![inline-code](../assets/orphan.png)` and \\![escaped](../assets/orphan.png)\n\
- ```md\n\
  ![fenced](../assets/orphan.png)\n\
  [[draws/orphan.excalidraw]]\n\
  ```\n"
    );
    let graph = graph(
        &source,
        &[
            ("assets/pre.png", b"pre"),
            ("assets/a b.png", b"space"),
            ("assets/a+b.png", b"plus"),
            ("assets/reference.png", b"reference"),
            ("assets/orphan.png", b"orphan"),
            ("draws/diagram.excalidraw", br#"{"type":"excalidraw"}"#),
            ("draws/orphan.excalidraw", br#"{"type":"excalidraw"}"#),
        ],
    );
    let plan = prepare(&graph);

    assert_eq!(plan.report.media_reference_count, 6);
    assert_eq!(plan.report.markdown_image_count, 5);
    assert_eq!(plan.report.legacy_excalidraw_count, 1);
    assert_eq!(plan.report.local_media_reference_count, 6);
    assert_eq!(plan.report.unreferenced_asset_count, 1);
    assert_eq!(plan.report.unreferenced_drawing_count, 1);
    assert!(
        plan.references.is_empty(),
        "drawing must not become a page ref"
    );

    let page = &plan.pages[0];
    let preamble = plan
        .media_references
        .iter()
        .find(|reference| {
            reference.owner
                == ImportMediaOwner::Page {
                    page_uuid: page.uuid,
                }
        })
        .expect("page-owned preamble image");
    assert_eq!(
        preamble.owner_markdown_spelling,
        "![preamble](../assets/pre.png \"Preamble title\")"
    );

    let task_block = page
        .blocks
        .iter()
        .find(|block| block.uuid == Uuid::parse_str(BLOCK_UUID).expect("UUID"))
        .expect("task block");
    assert!(task_block.markdown.starts_with("first ![local]"));
    assert_eq!(
        plan.media_references
            .iter()
            .filter(|reference| {
                reference.owner
                    == ImportMediaOwner::Block {
                        block_uuid: task_block.uuid,
                    }
                    && reference.owner_markdown_spelling.starts_with("![local]")
            })
            .count(),
        2,
        "repeated identical tokens remain distinct ordered references"
    );
    assert!(plan.media_references.iter().any(|reference| {
        matches!(
            &reference.resolution,
            ImportMediaResolution::LocalManifest { relative_path, .. }
                if relative_path == "assets/a+b.png"
        )
    }));
    assert!(plan.media_references.iter().any(|reference| {
        reference.kind == ImportMediaKind::LegacyExcalidraw
            && matches!(
                &reference.resolution,
                ImportMediaResolution::LocalManifest { relative_path, .. }
                    if relative_path == "draws/diagram.excalidraw"
            )
    }));
    assert!(plan.media_references.iter().any(|reference| {
        reference.owner_markdown_spelling == "![reference][asset]"
            && matches!(
                &reference.resolution,
                ImportMediaResolution::LocalManifest { relative_path, .. }
                    if relative_path == "assets/reference.png"
            )
    }));

    for reference in &plan.media_references {
        let start = reference.source_range.start.byte_offset as usize;
        let end = reference.source_range.end.byte_offset as usize;
        assert_eq!(&source[start..end], reference.raw_spelling);

        let owner_markdown = match reference.owner {
            ImportMediaOwner::Page { page_uuid } => {
                assert_eq!(page_uuid, page.uuid);
                &page.blocks[0].markdown
            }
            ImportMediaOwner::Block { block_uuid } => {
                &page
                    .blocks
                    .iter()
                    .find(|block| block.uuid == block_uuid)
                    .expect("media owner block")
                    .markdown
            }
        };
        let owner_start = reference.owner_markdown_range.start_byte as usize;
        let owner_end = reference.owner_markdown_range.end_byte as usize;
        assert_eq!(
            &owner_markdown[owner_start..owner_end],
            reference.owner_markdown_spelling
        );
    }
}

#[test]
fn classifies_scheme_before_path_and_blocks_local_path_attacks() {
    let source = "- ![ready](../assets/ready.png)\n\
  ![remote](https://private.invalid/assets/not-local.png)\n\
  ![missing](../assets/missing.png)\n\
  ![file](file:///tmp/private.png)\n\
  ![script](javascript:alert(1))\n\
  ![protocol](//private.invalid/image.png)\n\
  ![absolute](/etc/passwd)\n\
  ![windows](C:%5Cprivate%5Cimage.png)\n\
  ![backslash](..%5Cassets%5Cready.png)\n\
  ![escape](%2e%2e%2f%2e%2e%2fetc/passwd)\n\
  ![malformed](../assets/bad%ZZ.png)\n\
  ![utf8](../assets/bad%FF.png)\n\
  ![query](../assets/ready.png?secret=1)\n\
  ![encoded-scheme](%68ttps%3A%2F%2Fprivate.invalid%2Fassets%2Fx.png)\n\
  ![unc](%5C%5Cserver%5Cshare%5Cx.png)\n\
  ![control](../assets/a%00.png)\n\
  ![encoded-protocol](%2F%2Fprivate.invalid%2Fx.png)\n\
  ![encoded-fragment](../assets/ready.png%23private)\n";
    let graph = graph(source, &[("assets/ready.png", b"ready")]);
    let plan = prepare(&graph);

    assert_eq!(plan.report.media_reference_count, 18);
    assert_eq!(plan.report.local_media_reference_count, 1);
    assert_eq!(plan.report.blocked_remote_media_reference_count, 1);
    assert_eq!(plan.report.missing_media_reference_count, 1);
    assert_eq!(plan.report.blocked_unsafe_media_reference_count, 15);
    assert_eq!(plan.report.unreferenced_asset_count, 0);
    assert!(plan.media_references.iter().any(|reference| matches!(
        reference.resolution,
        ImportMediaResolution::RemoteBlocked { .. }
    )));
    assert!(plan.media_references.iter().any(|reference| matches!(
        reference.resolution,
        ImportMediaResolution::Blocked {
            reason: ImportMediaBlockedReason::PathEscape
        }
    )));
    assert!(plan.media_references.iter().any(|reference| matches!(
        reference.resolution,
        ImportMediaResolution::Blocked {
            reason: ImportMediaBlockedReason::MalformedPercentEncoding
        }
    )));
    assert!(plan.media_references.iter().any(|reference| matches!(
        reference.resolution,
        ImportMediaResolution::Blocked {
            reason: ImportMediaBlockedReason::InvalidPercentEncodedUtf8
        }
    )));
    assert!(plan.media_references.iter().any(|reference| matches!(
        reference.resolution,
        ImportMediaResolution::Blocked {
            reason: ImportMediaBlockedReason::QueryOrFragment
        }
    )));
    for reason in [
        ImportMediaBlockedReason::ControlCharacter,
        ImportMediaBlockedReason::WindowsOrUncPath,
        ImportMediaBlockedReason::ProtocolRelative,
    ] {
        assert!(plan.media_references.iter().any(|reference| {
            reference.resolution == ImportMediaResolution::Blocked { reason }
        }));
    }
    assert_eq!(
        plan.report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == DiagnosticCode::MissingMediaSource)
            .count(),
        1,
        "an HTTPS URL containing /assets/ is remote, never missing"
    );
    assert_private_values_absent_from_diagnostics(&plan.report.diagnostics);
}

#[test]
fn validates_bounded_png_and_jpeg_data_urls_without_retaining_decoded_bytes() {
    let source = format!(
        "- ![png]({PNG_DATA})\n\
  ![jpeg]({JPEG_DATA})\n\
  ![gif](data:image/gif;base64,R0lGODlh)\n\
  ![bad-base64](data:image/png;base64,%%%%)\n\
  ![bad-signature](data:image/png;base64,AAAA)\n\
  ![truncated-png](data:image/png;base64,iVBORw0KGgo=)\n\
  ![truncated-jpeg](data:image/jpeg;base64,/9j/)\n\
  ![fragment]({PNG_DATA}#private)\n\
  ![invalid-data](data:image/png;base64)\n"
    );
    let graph = graph(&source, &[]);
    let plan = prepare(&graph);

    assert_eq!(plan.report.media_reference_count, 9);
    assert_eq!(plan.report.inline_media_reference_count, 2);
    assert_eq!(plan.report.unsupported_media_reference_count, 7);
    assert!(plan.media_references.iter().any(|reference| matches!(
        reference.resolution,
        ImportMediaResolution::InlineData {
            mime: ImportInlineImageMime::Png,
            decoded_size_bytes: 69,
            ..
        }
    )));
    assert!(plan.media_references.iter().any(|reference| matches!(
        reference.resolution,
        ImportMediaResolution::InlineData {
            mime: ImportInlineImageMime::Jpeg,
            decoded_size_bytes: 160,
            ..
        }
    )));
    for reason in [
        ImportMediaUnsupportedReason::InlineMime,
        ImportMediaUnsupportedReason::InlineEncoding,
        ImportMediaUnsupportedReason::InlineSignatureMismatch,
        ImportMediaUnsupportedReason::InlineImageDecode,
        ImportMediaUnsupportedReason::InlineFragment,
        ImportMediaUnsupportedReason::InvalidDataUrl,
    ] {
        assert!(plan.media_references.iter().any(|reference| {
            reference.resolution == ImportMediaResolution::Unsupported { reason }
        }));
    }
    assert_private_values_absent_from_diagnostics(&plan.report.diagnostics);

    let (manifest, documents) = scan_and_parse(&graph);
    let limited = prepare_import_with_limits(
        &documents,
        identity(),
        &manifest,
        &PrepareLimits {
            max_inline_media_bytes: 4,
            ..PrepareLimits::default()
        },
    )
    .expect("oversized inline data is preserved as typed unsupported media");
    assert!(limited.media_references.iter().any(|reference| matches!(
        reference.resolution,
        ImportMediaResolution::Unsupported {
            reason: ImportMediaUnsupportedReason::InlineTooLarge
        }
    )));

    let dimensions_limited = prepare_import_with_limits(
        &documents,
        identity(),
        &manifest,
        &PrepareLimits {
            max_inline_image_width: 0,
            ..PrepareLimits::default()
        },
    )
    .expect("inline image dimension limit produces typed unsupported media");
    assert!(
        dimensions_limited
            .media_references
            .iter()
            .any(|reference| matches!(
                reference.resolution,
                ImportMediaResolution::Unsupported {
                    reason: ImportMediaUnsupportedReason::InlineDimensions
                }
            ))
    );

    let pixels_limited = prepare_import_with_limits(
        &documents,
        identity(),
        &manifest,
        &PrepareLimits {
            max_inline_image_pixels: 0,
            ..PrepareLimits::default()
        },
    )
    .expect("inline image pixel limit produces typed unsupported media");
    assert!(
        pixels_limited
            .media_references
            .iter()
            .any(|reference| matches!(
                reference.resolution,
                ImportMediaResolution::Unsupported {
                    reason: ImportMediaUnsupportedReason::InlinePixelLimit
                }
            ))
    );
}

#[test]
fn enforces_media_count_and_token_size_limits_before_materialization() {
    let graph = graph(
        "- ![first](../assets/first.png) ![second](../assets/second.png)\n",
        &[
            ("assets/first.png", b"first"),
            ("assets/second.png", b"second"),
        ],
    );
    let (manifest, documents) = scan_and_parse(&graph);

    for (limits, expected) in [
        (
            PrepareLimits {
                max_media_references_per_document: 1,
                ..PrepareLimits::default()
            },
            PrepareImportErrorCode::MediaReferenceLimitExceeded,
        ),
        (
            PrepareLimits {
                max_media_references: 1,
                max_media_references_per_document: 100,
                ..PrepareLimits::default()
            },
            PrepareImportErrorCode::TotalMediaReferenceLimitExceeded,
        ),
        (
            PrepareLimits {
                max_media_reference_bytes: 8,
                ..PrepareLimits::default()
            },
            PrepareImportErrorCode::MediaReferenceTooLong,
        ),
    ] {
        let error = prepare_import_with_limits(&documents, identity(), &manifest, &limits)
            .expect_err("configured media planner limit");
        assert_eq!(error.code(), expected);
    }
}

#[test]
fn enforces_global_reference_and_diagnostic_budgets_inside_document_collection() {
    let graph = graph(
        "- ![first](https://private.invalid/first.png) ![second](https://private.invalid/second.png)\n",
        &[],
    );
    let (manifest, documents) = scan_and_parse(&graph);

    let global_error = prepare_import_with_limits(
        &documents,
        identity(),
        &manifest,
        &PrepareLimits {
            max_media_references: 1,
            max_media_references_per_document: 100,
            max_diagnostics: 1,
            ..PrepareLimits::default()
        },
    )
    .expect_err("global media limit must stop collection within one document");
    assert_eq!(
        global_error.code(),
        PrepareImportErrorCode::TotalMediaReferenceLimitExceeded
    );

    let diagnostic_error = prepare_import_with_limits(
        &documents,
        identity(),
        &manifest,
        &PrepareLimits {
            max_media_references: 100,
            max_media_references_per_document: 100,
            max_diagnostics: 0,
            ..PrepareLimits::default()
        },
    )
    .expect_err("diagnostic budget must stop collection before a diagnostic is pushed");
    assert_eq!(
        diagnostic_error.code(),
        PrepareImportErrorCode::DiagnosticLimitExceeded
    );
}

#[test]
fn percent_decoding_is_once_only_and_plus_remains_a_literal_filename_byte() {
    let source = "- ![plus](../assets/a+b.png)\n\
  ![space](../assets/a%20b.png)\n\
  ![encoded-percent](../assets/a%2520b.png)\n";
    let graph = graph(
        source,
        &[
            ("assets/a+b.png", b"plus"),
            ("assets/a b.png", b"space"),
            ("assets/a%20b.png", b"encoded percent"),
        ],
    );
    let plan = prepare(&graph);
    let paths = plan
        .media_references
        .iter()
        .map(|reference| match &reference.resolution {
            ImportMediaResolution::LocalManifest { relative_path, .. } => relative_path.as_str(),
            resolution => panic!("unexpected resolution: {resolution:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        ["assets/a+b.png", "assets/a b.png", "assets/a%20b.png"]
    );
}

fn assert_private_values_absent_from_diagnostics(diagnostics: &[notes_import::ImportDiagnostic]) {
    for diagnostic in diagnostics {
        let rendered = format!(
            "{} {}",
            diagnostic.message,
            diagnostic.remediation.as_deref().unwrap_or_default()
        );
        for private in [
            "private.invalid",
            "data:",
            "iVBORw0KGgo",
            "/etc/passwd",
            "missing.png",
        ] {
            assert!(!rendered.contains(private), "diagnostic leaked {private}");
        }
    }
}

fn prepare(graph: &TempDir) -> notes_import::PreparedImport {
    let (manifest, documents) = scan_and_parse(graph);
    prepare_import(&documents, identity(), &manifest).expect("prepare media graph")
}

fn identity() -> IdentityContext {
    IdentityContext {
        workspace_uuid: Uuid::from_u128(0x300),
        import_namespace_uuid: Uuid::from_u128(0x400),
    }
}

fn graph(source: &str, files: &[(&str, &[u8])]) -> TempDir {
    let temp = TempDir::new().expect("temp graph");
    fs::create_dir_all(temp.path().join("pages")).expect("create pages");
    fs::write(temp.path().join("pages/Media.md"), source).expect("write page");
    for (relative_path, bytes) in files {
        let path = temp.path().join(relative_path);
        fs::create_dir_all(path.parent().expect("file parent")).expect("create file parent");
        fs::write(path, bytes).expect("write graph file");
    }
    temp
}

fn scan_and_parse(
    graph: &TempDir,
) -> (
    notes_import::GraphManifest,
    Vec<notes_import::ParsedLogseqDocument>,
) {
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
            let bytes = fs::read(graph.path().join(&entry.relative_path)).expect("read document");
            parse_logseq_markdown(entry, &bytes, report.manifest.config()).expect("parse document")
        })
        .collect();
    (report.manifest, documents)
}
