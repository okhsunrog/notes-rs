use std::fs;

use notes_import::{
    DiagnosticCode, DocumentFormat, IdentityContext, ImportBlockIdentity, ImportBlockSource,
    ImportPageKind, ImportReferenceResolution, ImportReferenceTargetKind,
    ImportReferenceUnresolvedReason, ImportRerunDecision, ImportTaskState, LogseqTaskMarker,
    PrepareImportError, PrepareImportErrorCode, PrepareLimits, SourceKind,
    compare_import_provenance, parse_logseq_markdown, prepare_import, prepare_import_with_limits,
    scan_logseq_graph,
};
use tempfile::TempDir;
use uuid::Uuid;

const FIRST_BLOCK_UUID: &str = "11111111-1111-4111-8111-111111111111";
const SECOND_BLOCK_UUID: &str = "22222222-2222-4222-8222-222222222222";

#[test]
fn stable_page_and_block_ids_do_not_depend_on_absolute_path_or_input_order() {
    let first = graph(&[("pages/Project.md", "- First\n  - Child\n")]);
    let second = graph(&[("pages/Project.md", "- First\n  - Child\n")]);
    let (first_manifest, first_documents) = scan_and_parse(&first);
    let (second_manifest, mut second_documents) = scan_and_parse(&second);
    second_documents.reverse();
    let identity = identity();

    let first_plan =
        prepare_import(&first_documents, identity, &first_manifest).expect("prepare first graph");
    let second_plan = prepare_import(&second_documents, identity, &second_manifest)
        .expect("prepare relocated graph");

    assert_eq!(first_manifest, second_manifest);
    assert_eq!(first_plan.pages, second_plan.pages);
    assert_eq!(first_plan.provenance, second_plan.provenance);
    assert_eq!(
        first_plan.pages[0].uuid,
        Uuid::new_v5(&identity.import_namespace_uuid, b"project")
    );
    assert_ne!(
        first_plan.pages[0].blocks[0].uuid,
        first_plan.pages[0].blocks[1].uuid
    );
    assert!(matches!(
        &first_plan.pages[0].blocks[1].provenance.source,
        ImportBlockSource::Structural {
            structural_path,
            ..
        } if structural_path == &[0, 0]
    ));
}

#[test]
fn journal_uuid_exactly_matches_the_workspace_date_rule() {
    let graph = graph(&[("journals/2026_07_17.md", "- Daily note\n")]);
    let (manifest, documents) = scan_and_parse(&graph);
    let identity = IdentityContext {
        workspace_uuid: Uuid::from_u128(1),
        import_namespace_uuid: Uuid::from_u128(2),
    };
    let plan = prepare_import(&documents, identity, &manifest).expect("prepare journal");
    let page = &plan.pages[0];

    assert!(matches!(page.kind, ImportPageKind::Journal { .. }));
    assert_eq!(
        page.uuid,
        Uuid::parse_str("17ed8d52-fb7d-5c84-a2f4-1bb93bf969d1").expect("golden UUID")
    );
    assert_eq!(
        plan.identity_maps.journal_dates.get("2026-07-17"),
        Some(&page.uuid)
    );
    for alias in ["2026-07-17", "2026_07_17", "jul 17th, 2026"] {
        assert_eq!(plan.identity_maps.page_titles.get(alias), Some(&page.uuid));
    }
}

#[test]
fn preserves_valid_block_uuid_removes_only_identity_and_resolves_raw_references() {
    let graph = graph(&[
        (
            "pages/Home.md",
            &format!(
                "title:: Home Display\n- TODO Link [[Other]] and (({FIRST_BLOCK_UUID}))\n  id:: {SECOND_BLOCK_UUID}\n  color:: blue\n- `[[Other]]` and \\[[Other]]\n- [label](<https://example.invalid/[[Other]]>)\n- ```md\n  [[Other]]\n  ```\n"
            ),
        ),
        (
            "pages/Other.md",
            &format!("- Target\n  id:: {FIRST_BLOCK_UUID}\n  custom:: kept\n"),
        ),
    ]);
    let (manifest, documents) = scan_and_parse(&graph);
    let plan = prepare_import(&documents, identity(), &manifest).expect("prepare references");
    assert!(plan.is_committable(), "{:?}", plan.report.diagnostics);

    let home = plan
        .pages
        .iter()
        .find(|page| {
            matches!(
                &page.kind,
                ImportPageKind::Note { title } if title == "Home Display"
            )
        })
        .expect("home page");
    assert_eq!(home.blocks[0].markdown, "title:: Home Display");
    assert!(matches!(
        home.blocks[0].provenance.source,
        ImportBlockSource::Preamble
    ));
    let first_block = home
        .blocks
        .iter()
        .find(|block| {
            matches!(
                block.provenance.source,
                ImportBlockSource::Structural {
                    source_block_index: 0,
                    ..
                }
            )
        })
        .expect("first structural block");
    assert_eq!(
        first_block.uuid,
        Uuid::parse_str(SECOND_BLOCK_UUID).expect("source UUID")
    );
    assert_eq!(
        first_block.markdown,
        format!("Link [[Other]] and (({FIRST_BLOCK_UUID}))\ncolor:: blue")
    );
    assert!(matches!(
        first_block.provenance.identity,
        ImportBlockIdentity::Preserved { .. }
    ));
    assert_eq!(
        first_block
            .task
            .as_ref()
            .map(|task| (task.source_marker, task.target_state)),
        Some((LogseqTaskMarker::Todo, ImportTaskState::Todo))
    );

    assert_eq!(plan.references.len(), 2);
    assert_eq!(
        plan.references
            .iter()
            .map(|reference| reference.raw_spelling.as_str())
            .collect::<Vec<_>>(),
        ["[[Other]]", format!("(({FIRST_BLOCK_UUID}))").as_str()]
    );
    assert!(plan.references.iter().all(|reference| matches!(
        reference.resolution,
        ImportReferenceResolution::Resolved { .. }
    )));
    assert!(plan.references.iter().any(|reference| matches!(
        reference.resolution,
        ImportReferenceResolution::Resolved {
            target_kind: ImportReferenceTargetKind::Block,
            ..
        }
    )));
    assert_eq!(plan.report.preserved_block_uuid_count, 2);
}

#[test]
fn malformed_opener_does_not_hide_a_later_valid_reference() {
    let graph = graph(&[
        ("pages/Home.md", "- [[broken and [[Other]]\n"),
        ("pages/Other.md", "- Target\n"),
    ]);
    let (manifest, documents) = scan_and_parse(&graph);
    let plan = prepare_import(&documents, identity(), &manifest).expect("prepare references");

    assert_eq!(plan.references.len(), 1);
    assert_eq!(plan.references[0].raw_spelling, "[[Other]]");
    assert!(matches!(
        plan.references[0].resolution,
        ImportReferenceResolution::Resolved { .. }
    ));
}

#[test]
fn maps_every_supported_task_marker_and_strips_only_the_head_marker() {
    let markers = [
        ("TODO", LogseqTaskMarker::Todo, ImportTaskState::Todo),
        ("DOING", LogseqTaskMarker::Doing, ImportTaskState::Doing),
        ("NOW", LogseqTaskMarker::Now, ImportTaskState::Now),
        ("LATER", LogseqTaskMarker::Later, ImportTaskState::Later),
        ("DONE", LogseqTaskMarker::Done, ImportTaskState::Done),
        ("WAIT", LogseqTaskMarker::Wait, ImportTaskState::Waiting),
        (
            "WAITING",
            LogseqTaskMarker::Waiting,
            ImportTaskState::Waiting,
        ),
        (
            "CANCELLED",
            LogseqTaskMarker::Cancelled,
            ImportTaskState::Cancelled,
        ),
        (
            "CANCELED",
            LogseqTaskMarker::Canceled,
            ImportTaskState::Cancelled,
        ),
        (
            "IN-PROGRESS",
            LogseqTaskMarker::InProgress,
            ImportTaskState::Doing,
        ),
    ];
    let source = markers
        .iter()
        .map(|(marker, _, _)| format!("- {marker} keep {marker} inside\n"))
        .collect::<String>();
    let graph = graph(&[("pages/Tasks.md", &source)]);
    let (manifest, documents) = scan_and_parse(&graph);
    let plan = prepare_import(&documents, identity(), &manifest).expect("prepare tasks");

    for (block, (raw, source_marker, target_state)) in plan.pages[0].blocks.iter().zip(markers) {
        let mapping = block.task.as_ref().expect("typed task mapping");
        assert_eq!(mapping.source_marker, source_marker);
        assert_eq!(mapping.target_state, target_state);
        assert_eq!(block.markdown, format!("keep {raw} inside"));
    }
}

#[test]
fn invalid_multiple_and_duplicate_block_ids_are_blocking_and_remain_visible() {
    let graph = graph(&[(
        "pages/Bad.md",
        &format!(
            "- invalid\n  id:: not-a-uuid\n- duplicate one\n  id:: {FIRST_BLOCK_UUID}\n- duplicate two\n  id:: {FIRST_BLOCK_UUID}\n- multiple\n  id:: {SECOND_BLOCK_UUID}\n  ID:: 33333333-3333-4333-8333-333333333333\n"
        ),
    )]);
    let (manifest, documents) = scan_and_parse(&graph);
    let plan = prepare_import(&documents, identity(), &manifest).expect("prepare bad identities");

    assert!(!plan.is_committable());
    assert!(plan.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::InvalidBlockUuid
            && diagnostic.message == "block identity property is not a non-nil UUID"
    }));
    assert!(
        plan.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::MultipleBlockIdentityProperties
        })
    );
    assert!(
        plan.report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == DiagnosticCode::DuplicateBlockUuid)
            .count()
            >= 2
    );
    for block in &plan.pages[0].blocks {
        assert!(block.markdown.contains("id::") || block.markdown.contains("ID::"));
        assert!(matches!(
            block.provenance.identity,
            ImportBlockIdentity::RejectedSourceProperty
        ));
    }
    assert!(plan.identity_maps.block_source_uuids.is_empty());
}

#[test]
fn reports_both_logseq_identity_and_destination_nfkc_collisions() {
    let graph = graph(&[
        ("pages/A.md", "- uppercase\n"),
        ("pages/a.md", "- lowercase\n"),
        ("pages/ff.md", "- ascii\n"),
        ("pages/ﬀ.md", "- ligature\n"),
    ]);
    let (manifest, documents) = scan_and_parse(&graph);
    let plan = prepare_import(&documents, identity(), &manifest).expect("prepare collisions");

    assert!(!plan.is_committable());
    assert!(
        plan.report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::DuplicatePageIdentity)
    );
    assert!(plan.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::DuplicatePageTitle
            && diagnostic
                .message
                .contains("destination title normalization")
    }));
    let ff = plan
        .pages
        .iter()
        .find(|page| page.source_identity == "ff")
        .expect("ASCII source page");
    let ligature = plan
        .pages
        .iter()
        .find(|page| page.source_identity == "ﬀ")
        .expect("NFC source page");
    assert_ne!(ff.uuid, ligature.uuid);
}

#[test]
fn ambiguous_and_unknown_references_keep_spelling_and_have_ranged_diagnostics() {
    let graph = graph(&[
        ("pages/One.md", "title:: Shared\n- one\n"),
        ("pages/Two.md", "title:: Shared\n- two\n"),
        (
            "pages/Refs.md",
            "- Unicode [[ Shared ]] unknown [[Missing]] invalid ((nope))\n",
        ),
    ]);
    let (manifest, documents) = scan_and_parse(&graph);
    let plan = prepare_import(&documents, identity(), &manifest).expect("prepare unresolved refs");

    assert_eq!(plan.references.len(), 3);
    assert!(plan.references.iter().any(|reference| matches!(
        reference.resolution,
        ImportReferenceResolution::Unresolved {
            reason: ImportReferenceUnresolvedReason::Ambiguous
        }
    )));
    assert!(plan.references.iter().any(|reference| matches!(
        reference.resolution,
        ImportReferenceResolution::Unresolved {
            reason: ImportReferenceUnresolvedReason::Unknown
        }
    )));
    assert!(plan.references.iter().any(|reference| matches!(
        reference.resolution,
        ImportReferenceResolution::Unresolved {
            reason: ImportReferenceUnresolvedReason::Invalid
        }
    )));
    assert!(plan.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::AmbiguousPageReference && diagnostic.range.is_some()
    }));
    assert!(
        plan.report
            .diagnostics
            .iter()
            .filter(|diagnostic| matches!(
                diagnostic.code,
                DiagnosticCode::AmbiguousPageReference
                    | DiagnosticCode::UnresolvedPageReference
                    | DiagnosticCode::InvalidBlockReference
            ))
            .all(|diagnostic| !diagnostic.message.contains("Shared")
                && !diagnostic.message.contains("Missing")
                && !diagnostic.message.contains("nope"))
    );
}

#[test]
fn title_property_changes_do_not_change_normal_page_identity() {
    let first = graph(&[("pages/Stable.md", "title:: First title\n- Body\n")]);
    let second = graph(&[("pages/Stable.md", "title:: Second title\n- Body\n")]);
    let (first_manifest, first_documents) = scan_and_parse(&first);
    let (second_manifest, second_documents) = scan_and_parse(&second);
    let first_plan = prepare_import(&first_documents, identity(), &first_manifest).expect("first");
    let second_plan =
        prepare_import(&second_documents, identity(), &second_manifest).expect("second");

    assert_eq!(first_plan.pages[0].uuid, second_plan.pages[0].uuid);
    assert_ne!(first_plan.pages[0].kind, second_plan.pages[0].kind);
}

#[test]
fn exact_manifest_is_noop_but_changed_manifest_is_explicitly_refused() {
    let graph = graph(&[("pages/Stable.md", "- Body\n")]);
    let (manifest, documents) = scan_and_parse(&graph);
    let identity = identity();
    let plan = prepare_import(&documents, identity, &manifest).expect("prepare");

    assert_eq!(
        compare_import_provenance(&plan.provenance, identity, &manifest),
        ImportRerunDecision::ExactNoOp
    );
    fs::write(graph.path().join("pages/Stable.md"), "- Changed\n").expect("change source");
    let (changed_manifest, _) = scan_and_parse(&graph);
    assert!(matches!(
        compare_import_provenance(&plan.provenance, identity, &changed_manifest),
        ImportRerunDecision::RefuseChangedManifest { .. }
    ));
    assert_eq!(
        compare_import_provenance(
            &plan.provenance,
            IdentityContext {
                import_namespace_uuid: Uuid::from_u128(99),
                ..identity
            },
            &manifest,
        ),
        ImportRerunDecision::RefuseIdentityContext
    );
}

#[test]
fn malformed_or_mismatched_preparsed_input_is_rejected_with_typed_errors() {
    let graph = graph(&[("pages/Stable.md", "- Body\n")]);
    let (manifest, mut documents) = scan_and_parse(&graph);
    let missing = prepare_import(&[], identity(), &manifest).expect_err("missing document");
    assert_eq!(
        missing.code(),
        PrepareImportErrorCode::MissingParsedDocument
    );

    let duplicate = prepare_import(
        &[documents[0].clone(), documents[0].clone()],
        identity(),
        &manifest,
    )
    .expect_err("duplicate document");
    assert_eq!(
        duplicate.code(),
        PrepareImportErrorCode::DuplicateParsedDocument
    );

    documents[0].raw_markdown.push_str("changed");
    assert!(matches!(
        prepare_import(&documents, identity(), &manifest),
        Err(PrepareImportError::SourceManifestMismatch { .. })
    ));

    let nil = prepare_import(
        &[],
        IdentityContext {
            workspace_uuid: Uuid::nil(),
            import_namespace_uuid: Uuid::from_u128(1),
        },
        &manifest,
    )
    .expect_err("nil workspace");
    assert_eq!(nil.code(), PrepareImportErrorCode::NilWorkspaceUuid);
}

#[test]
fn synthetic_preamble_is_a_durable_first_root_block_and_owns_its_references() {
    let graph = graph(&[
        (
            "pages/Home.md",
            "Intro [[Target]]\n- Nested [[Outer [[Inner]] Tail]] then [[Target]]\n",
        ),
        ("pages/Target.md", "- destination\n"),
    ]);
    let (manifest, documents) = scan_and_parse(&graph);
    let plan = prepare_import(&documents, identity(), &manifest).expect("prepare preamble");
    let home = plan
        .pages
        .iter()
        .find(|page| page.source.relative_path == "pages/Home.md")
        .expect("home page");
    let preamble = &home.blocks[0];

    assert_eq!(preamble.sibling_ordinal, 0);
    assert_eq!(preamble.parent_block_uuid, None);
    assert_eq!(preamble.markdown, "Intro [[Target]]");
    assert!(matches!(
        preamble.provenance.source,
        ImportBlockSource::Preamble
    ));
    assert_eq!(home.blocks[1].sibling_ordinal, 1);
    assert_eq!(home.blocks[1].parent_block_uuid, None);
    assert_eq!(plan.report.synthetic_preamble_block_count, 1);

    let preamble_reference = plan
        .references
        .iter()
        .find(|reference| reference.source_range.start.line == 1)
        .expect("preamble reference");
    assert_eq!(preamble_reference.relative_path, "pages/Home.md");
    assert_eq!(preamble_reference.owner.page_uuid, home.uuid);
    assert_eq!(preamble_reference.owner.block_uuid, preamble.uuid);

    let nested = plan
        .references
        .iter()
        .find(|reference| reference.raw_spelling.contains("Outer"))
        .expect("nested spelling preserved");
    assert_eq!(nested.raw_spelling, "[[Outer [[Inner]] Tail]]");
    assert!(matches!(
        nested.resolution,
        ImportReferenceResolution::Unresolved {
            reason: ImportReferenceUnresolvedReason::UnsupportedNested
        }
    ));
    assert!(plan.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::UnsupportedNestedWikilink
            && diagnostic.range == Some(nested.target_range)
    }));
}

#[test]
fn identity_property_removal_is_exact_for_first_middle_last_and_sole_crlf_lines() {
    let source = "- id:: 10000000-0000-4000-8000-000000000001\r\n  tail\r\n- head\r\n  id:: 10000000-0000-4000-8000-000000000002\r\n  tail\r\n- head\r\n  id:: 10000000-0000-4000-8000-000000000003\r\n- id:: 10000000-0000-4000-8000-000000000004";
    let graph = graph(&[("pages/Properties.md", source)]);
    let (manifest, documents) = scan_and_parse(&graph);
    let plan = prepare_import(&documents, identity(), &manifest).expect("prepare properties");
    let page = &plan.pages[0];
    let markdown = page
        .blocks
        .iter()
        .map(|block| block.markdown.as_str())
        .collect::<Vec<_>>();

    assert_eq!(markdown, ["tail", "head\ntail", "head", ""]);
    assert!(page.blocks.iter().all(|block| matches!(
        block.provenance.identity,
        ImportBlockIdentity::Preserved { .. }
    )));
}

#[test]
fn only_canonical_hyphenated_uuid_spelling_is_consumed() {
    let graph = graph(&[(
        "pages/Uuid.md",
        "- simple spelling\n  id:: 11111111111141118111111111111111\n",
    )]);
    let (manifest, documents) = scan_and_parse(&graph);
    let plan = prepare_import(&documents, identity(), &manifest).expect("prepare UUID spelling");

    assert!(!plan.is_committable());
    assert_eq!(plan.report.preserved_block_uuid_count, 0);
    assert!(plan.pages[0].blocks[0].markdown.contains("id::"));
    assert!(
        plan.report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::InvalidBlockUuid)
    );
}

#[test]
fn fallback_block_identity_includes_content_hash() {
    let first = graph(&[("pages/Stable.md", "- first body\n")]);
    let second = graph(&[("pages/Stable.md", "- changed body\n")]);
    let (first_manifest, first_documents) = scan_and_parse(&first);
    let (second_manifest, second_documents) = scan_and_parse(&second);
    let first_plan = prepare_import(&first_documents, identity(), &first_manifest).expect("first");
    let second_plan =
        prepare_import(&second_documents, identity(), &second_manifest).expect("second");

    assert_eq!(first_plan.pages[0].uuid, second_plan.pages[0].uuid);
    assert_ne!(
        first_plan.pages[0].blocks[0].uuid,
        second_plan.pages[0].blocks[0].uuid
    );
}

#[test]
fn oversized_reference_is_rejected_before_copying_its_target() {
    let target = "x".repeat(70 * 1024);
    let source = format!("- [[{target}]]\n");
    let graph = graph(&[("pages/Large.md", &source)]);
    let (manifest, documents) = scan_and_parse(&graph);
    let error = prepare_import(&documents, identity(), &manifest).expect_err("reference limit");

    assert_eq!(error.code(), PrepareImportErrorCode::ReferenceTooLong);
    assert!(matches!(
        error,
        PrepareImportError::ReferenceTooLong {
            relative_path,
            limit_bytes: 65_536,
        } if relative_path == "pages/Large.md"
    ));
}

#[test]
fn unicode_reference_positions_are_linear_and_scalar_based_on_a_dense_line() {
    let count = 5_000_usize;
    let source = format!("- α {}\n", "[[T]] ".repeat(count));
    let graph = graph(&[("pages/Many.md", &source), ("pages/T.md", "- target\n")]);
    let (manifest, documents) = scan_and_parse(&graph);
    let plan = prepare_import(&documents, identity(), &manifest).expect("prepare dense references");
    let references = plan
        .references
        .iter()
        .filter(|reference| reference.relative_path == "pages/Many.md")
        .collect::<Vec<_>>();

    assert_eq!(references.len(), count);
    assert_eq!(references[0].target_range.start.column, 7);
    assert_eq!(
        references
            .last()
            .expect("last reference")
            .target_range
            .start
            .column,
        7 + 6 * (count as u64 - 1)
    );
    assert!(references.iter().all(|reference| matches!(
        reference.resolution,
        ImportReferenceResolution::Resolved { .. }
    )));
}

#[test]
fn configurable_prepare_limits_fail_with_stable_typed_errors() {
    let graph = graph(&[("pages/Limits.md", "- root [[Missing]]\n  - child\n")]);
    let (manifest, documents) = scan_and_parse(&graph);
    let cases = [
        (
            PrepareLimits {
                max_total_document_bytes: 1,
                ..PrepareLimits::default()
            },
            PrepareImportErrorCode::TotalDocumentBytesLimitExceeded,
        ),
        (
            PrepareLimits {
                max_blocks_per_document: 1,
                ..PrepareLimits::default()
            },
            PrepareImportErrorCode::DocumentBlockLimitExceeded,
        ),
        (
            PrepareLimits {
                max_nesting_depth: 0,
                ..PrepareLimits::default()
            },
            PrepareImportErrorCode::NestingDepthExceeded,
        ),
        (
            PrepareLimits {
                max_references_per_document: 0,
                ..PrepareLimits::default()
            },
            PrepareImportErrorCode::ReferenceLimitExceeded,
        ),
        (
            PrepareLimits {
                max_references: 0,
                ..PrepareLimits::default()
            },
            PrepareImportErrorCode::TotalReferenceLimitExceeded,
        ),
        (
            PrepareLimits {
                max_diagnostics: 0,
                ..PrepareLimits::default()
            },
            PrepareImportErrorCode::DiagnosticLimitExceeded,
        ),
    ];

    for (limits, expected) in cases {
        let error = prepare_import_with_limits(&documents, identity(), &manifest, &limits)
            .expect_err("configured limit");
        assert_eq!(error.code(), expected);
    }
}

#[test]
fn malformed_sibling_sequence_and_backslash_before_unicode_never_panic() {
    let graph = graph(&[("pages/Safe.md", "- escaped \\О and [[Safe]]\n- second\n")]);
    let (manifest, mut documents) = scan_and_parse(&graph);
    let plan = prepare_import(&documents, identity(), &manifest).expect("safe Unicode scan");
    assert_eq!(plan.references.len(), 1);

    documents[0].blocks[1].sibling_index = 4;
    let error = prepare_import(&documents, identity(), &manifest).expect_err("sibling gap");
    assert_eq!(error.code(), PrepareImportErrorCode::InvalidSourceAst);
}

fn identity() -> IdentityContext {
    IdentityContext {
        workspace_uuid: Uuid::from_u128(0x100),
        import_namespace_uuid: Uuid::from_u128(0x200),
    }
}

fn graph(files: &[(&str, &str)]) -> TempDir {
    let temp = TempDir::new().expect("temp graph");
    for (relative_path, source) in files {
        let path = temp.path().join(relative_path);
        fs::create_dir_all(path.parent().expect("source parent")).expect("create source directory");
        fs::write(path, source).expect("write source file");
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
