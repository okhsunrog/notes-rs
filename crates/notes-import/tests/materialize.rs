use std::fs;
use std::sync::Arc;

use data_url::DataUrl;
use notes_import::{
    DocumentFormat, IdentityContext, ImportMediaResolution, MaterializeMediaErrorCode,
    MediaMaterializationIssue, MediaMaterializationLimits, PreparedImport, PreservedMediaReason,
    Sha256Digest, SourceKind, materialize_source_media, materialize_source_media_with_limits,
    parse_logseq_markdown, prepare_import, scan_logseq_graph,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;

const PNG_DATA: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVQI12P4//8/AAX+Av7czFnnAAAAAElFTkSuQmCC";

#[test]
fn materializes_local_and_inline_bytes_deduplicates_and_targets_preamble_block() {
    let png = inline_png_bytes();
    let source = format!(
        "![preamble](../assets/pixel.png)\n- ![local](../assets/pixel.png \"Local title\") ![inline]({PNG_DATA})\n"
    );
    let graph = graph(&source, &[("assets/pixel.png", &png)]);
    let prepared = prepare(&graph);

    let first = materialize_source_media(&prepared, graph.path()).expect("materialize media");
    let second = materialize_source_media(&prepared, graph.path()).expect("repeat materialization");

    assert_eq!(first, second, "the complete plan is deterministic");
    assert_eq!(
        first.blobs.len(),
        1,
        "equal canonical bytes are deduplicated"
    );
    assert_eq!(&*first.blobs[0].bytes, png.as_slice());
    assert_eq!(Arc::strong_count(&first.blobs[0].bytes), 1);
    assert_eq!(first.blobs[0].size_bytes, 69);
    assert_eq!(
        first.attachments.len(),
        2,
        "identity is owner plus blob hash"
    );
    for attachment in &first.attachments {
        let (kind, owner_uuid) = match attachment.owner {
            notes_import::ImportMediaOwner::Page { page_uuid } => ("page", page_uuid),
            notes_import::ImportMediaOwner::Block { block_uuid } => ("block", block_uuid),
        };
        let expected = Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!(
                "tangleaf:attachment:{kind}:{owner_uuid}:{}",
                attachment.sha256
            )
            .as_bytes(),
        );
        assert_eq!(attachment.attachment_uuid, expected);
    }
    assert_eq!(first.rewrites.len(), 3);
    assert!(first.preserved_references.is_empty());
    assert!(first.diagnostics.is_empty());

    let page = &prepared.pages[0];
    let preamble = page
        .blocks
        .iter()
        .find(|block| block.markdown.starts_with("![preamble]"))
        .expect("synthetic preamble block");
    let preamble_rewrite = first
        .rewrites
        .iter()
        .find(|rewrite| {
            prepared.media_references[rewrite.reference_index as usize]
                .owner_markdown_spelling
                .starts_with("![preamble]")
        })
        .expect("preamble rewrite");
    assert_eq!(preamble_rewrite.markdown_block_uuid, preamble.uuid);
    assert!(matches!(
        preamble_rewrite.attachment_owner,
        notes_import::ImportMediaOwner::Page { page_uuid } if page_uuid == page.uuid
    ));
    assert_eq!(
        preamble_rewrite.replacement_markdown,
        format!("![preamble]({})", preamble_rewrite.attachment_uri)
    );
    assert!(
        first.rewrites.iter().all(|rewrite| rewrite.attachment_uri
            == format!("notes-attachment:{}", rewrite.attachment_uuid))
    );
    let titled = first
        .rewrites
        .iter()
        .find(|rewrite| {
            prepared.media_references[rewrite.reference_index as usize]
                .owner_markdown_spelling
                .starts_with("![local]")
        })
        .expect("titled local image");
    assert_eq!(
        titled.replacement_markdown,
        format!("![local]({} \"Local title\")", titled.attachment_uri)
    );
}

#[test]
fn preserves_preclassified_remote_missing_and_unsafe_references_verbatim() {
    let source = "- ![remote](https://private.invalid/x.png)\n  ![missing](../assets/missing.png)\n  ![unsafe](/etc/passwd)\n";
    let graph = graph(source, &[]);
    let prepared = prepare(&graph);
    let plan = materialize_source_media(&prepared, graph.path()).expect("materialize media");

    assert!(plan.blobs.is_empty());
    assert!(plan.attachments.is_empty());
    assert!(plan.rewrites.is_empty());
    assert!(
        plan.diagnostics.is_empty(),
        "classification diagnostics already exist"
    );
    assert_eq!(plan.preserved_references.len(), 3);
    for preserved in &plan.preserved_references {
        assert_eq!(preserved.reason, PreservedMediaReason::AlreadyClassified);
        let original = &prepared.media_references[preserved.reference_index as usize];
        assert_eq!(preserved.owner, original.owner);
        assert!(!original.raw_spelling.is_empty());
        assert!(!original.owner_markdown_spelling.is_empty());
    }
}

#[test]
fn changed_missing_and_non_file_sources_are_fatal() {
    for mutation in ["changed", "missing", "directory"] {
        let graph = graph(
            "- ![asset](../assets/value.bin)\n",
            &[("assets/value.bin", b"old")],
        );
        let prepared = prepare(&graph);
        let path = graph.path().join("assets/value.bin");
        match mutation {
            "changed" => fs::write(&path, b"new").expect("change source"),
            "missing" => fs::remove_file(&path).expect("remove source"),
            "directory" => {
                fs::remove_file(&path).expect("remove source");
                fs::create_dir(&path).expect("replace with directory");
            }
            _ => unreachable!(),
        }
        let error = materialize_source_media(&prepared, graph.path()).expect_err("fatal drift");
        let expected = match mutation {
            "changed" => MediaMaterializationIssue::SourceChanged,
            "missing" => MediaMaterializationIssue::SourceNotFound,
            "directory" => MediaMaterializationIssue::SourceNotRegularFile,
            _ => unreachable!(),
        };
        assert_eq!(error.reference_issue(), Some(expected));
        assert_eq!(error.code(), MaterializeMediaErrorCode::ReferenceFailed);
    }
}

#[test]
fn repeats_path_traversal_defense_even_for_a_mutated_prepared_plan() {
    let graph = graph(
        "- ![asset](../assets/value.bin)\n",
        &[("assets/value.bin", b"safe")],
    );
    let outside = graph
        .path()
        .parent()
        .expect("temp parent")
        .join("outside.bin");
    fs::write(&outside, b"outside").expect("outside file");
    let mut prepared = prepare(&graph);
    let reference = &mut prepared.media_references[0];
    reference.resolution = ImportMediaResolution::LocalManifest {
        relative_path: "../outside.bin".into(),
        size_bytes: 7,
        sha256: digest(b"outside"),
    };

    let error = materialize_source_media(&prepared, graph.path()).expect_err("fatal traversal");
    assert_eq!(
        error.reference_issue(),
        Some(MediaMaterializationIssue::PathEscape)
    );
    let _ = fs::remove_file(outside);
}

#[cfg(unix)]
#[test]
fn rejects_source_and_root_symlinks_instead_of_following_them() {
    use std::os::unix::fs::symlink;

    let graph = graph(
        "- ![asset](../assets/value.bin)\n",
        &[("assets/value.bin", b"safe")],
    );
    let prepared = prepare(&graph);
    let outside_dir = TempDir::new().expect("outside dir");
    let outside = outside_dir.path().join("outside.bin");
    fs::write(&outside, b"safe").expect("outside file");
    let asset = graph.path().join("assets/value.bin");
    fs::remove_file(&asset).expect("remove source");
    symlink(&outside, &asset).expect("replace with symlink");

    let error = materialize_source_media(&prepared, graph.path()).expect_err("fatal symlink");
    assert_eq!(
        error.reference_issue(),
        Some(MediaMaterializationIssue::SymlinkNotAllowed)
    );

    let root_link_parent = TempDir::new().expect("link parent");
    let root_link = root_link_parent.path().join("graph-link");
    symlink(graph.path(), &root_link).expect("root symlink");
    let error = materialize_source_media(&prepared, &root_link).expect_err("reject root symlink");
    assert_eq!(
        error.code(),
        MaterializeMediaErrorCode::RootSymlinkNotAllowed
    );
}

#[test]
fn materialization_limits_are_fatal_and_preflighted_before_source_reads() {
    let graph = graph(
        "- ![a](../assets/a.bin) ![b](../assets/b.bin)\n",
        &[("assets/a.bin", b"aa"), ("assets/b.bin", b"bb")],
    );
    let prepared = prepare(&graph);

    let per_blob = materialize_source_media_with_limits(
        &prepared,
        graph.path(),
        &MediaMaterializationLimits {
            max_blob_bytes: 1,
            ..MediaMaterializationLimits::default()
        },
    )
    .expect_err("per-blob budget is fatal");
    assert_eq!(
        per_blob.reference_issue(),
        Some(MediaMaterializationIssue::BlobTooLarge)
    );

    let total = materialize_source_media_with_limits(
        &prepared,
        graph.path(),
        &MediaMaterializationLimits {
            max_total_blob_bytes: 3,
            ..MediaMaterializationLimits::default()
        },
    )
    .expect_err("total budget is fatal");
    assert_eq!(
        total.reference_issue(),
        Some(MediaMaterializationIssue::TotalBlobBytesExceeded)
    );

    let count = materialize_source_media_with_limits(
        &prepared,
        graph.path(),
        &MediaMaterializationLimits {
            max_blobs: 1,
            ..MediaMaterializationLimits::default()
        },
    )
    .expect_err("blob count is fatal");
    assert_eq!(
        count.reference_issue(),
        Some(MediaMaterializationIssue::BlobLimitExceeded)
    );

    fs::remove_file(graph.path().join("assets/a.bin")).expect("remove first source");
    let preflight = materialize_source_media_with_limits(
        &prepared,
        graph.path(),
        &MediaMaterializationLimits {
            max_total_blob_bytes: 1,
            ..MediaMaterializationLimits::default()
        },
    )
    .expect_err("budget must fail before filesystem read");
    assert_eq!(
        preflight.reference_issue(),
        Some(MediaMaterializationIssue::TotalBlobBytesExceeded)
    );
}

#[test]
fn revalidates_inline_hash_and_owner_markdown_before_rewrite() {
    let graph = graph(&format!("- ![inline]({PNG_DATA})\n"), &[]);
    let mut wrong_hash = prepare(&graph);
    if let ImportMediaResolution::InlineData { decoded_sha256, .. } =
        &mut wrong_hash.media_references[0].resolution
    {
        *decoded_sha256 = Sha256Digest::from_bytes([0x55; 32]);
    } else {
        panic!("expected inline reference");
    }
    let error = materialize_source_media(&wrong_hash, graph.path()).expect_err("fatal hash drift");
    assert_eq!(
        error.reference_issue(),
        Some(MediaMaterializationIssue::SourceChanged)
    );

    let mut wrong_owner = prepare(&graph);
    wrong_owner.pages[0].blocks[0]
        .markdown
        .replace_range(0..1, "x");
    let error =
        materialize_source_media(&wrong_owner, graph.path()).expect_err("fatal owner drift");
    assert_eq!(
        error.reference_issue(),
        Some(MediaMaterializationIssue::InvalidPreparedReference)
    );
}

#[test]
fn cache_never_reuses_bytes_for_conflicting_prepared_evidence() {
    let graph = graph(
        "- ![first](../assets/value.bin) ![second](../assets/value.bin)\n",
        &[("assets/value.bin", b"same")],
    );
    let mut prepared = prepare(&graph);
    let ImportMediaResolution::LocalManifest { sha256, .. } =
        &mut prepared.media_references[1].resolution
    else {
        panic!("expected local media");
    };
    *sha256 = Sha256Digest::from_bytes([0x77; 32]);

    let error =
        materialize_source_media(&prepared, graph.path()).expect_err("conflicting evidence");
    assert_eq!(
        error.reference_issue(),
        Some(MediaMaterializationIssue::SourceChanged)
    );
}

#[test]
fn equal_bytes_under_different_paths_share_one_retained_arc_and_one_blob_budget() {
    let bytes = b"identical media";
    let graph = graph(
        "- ![first](../assets/first.bin) ![second](../assets/second.bin)\n",
        &[("assets/first.bin", bytes), ("assets/second.bin", bytes)],
    );
    let prepared = prepare(&graph);
    let plan = materialize_source_media_with_limits(
        &prepared,
        graph.path(),
        &MediaMaterializationLimits {
            max_total_blob_bytes: bytes.len() as u64,
            max_blobs: 1,
            ..MediaMaterializationLimits::default()
        },
    )
    .expect("content-addressed budget counts retained bytes once");

    assert_eq!(plan.blobs.len(), 1);
    assert_eq!(
        plan.attachments.len(),
        1,
        "same owner and hash is one attachment"
    );
    assert_eq!(plan.rewrites.len(), 2);
    assert_eq!(Arc::strong_count(&plan.blobs[0].bytes), 1);
    assert_eq!(&*plan.blobs[0].bytes, bytes);
}

#[test]
fn legacy_excalidraw_is_deferred_and_preserved_raw() {
    let graph = graph(
        "- [[draws/diagram.excalidraw]]\n",
        &[("draws/diagram.excalidraw", br#"{"type":"excalidraw"}"#)],
    );
    let prepared = prepare(&graph);
    let plan = materialize_source_media(&prepared, graph.path()).expect("deferred drawing");

    assert!(plan.blobs.is_empty());
    assert!(plan.attachments.is_empty());
    assert!(plan.rewrites.is_empty());
    assert_eq!(plan.preserved_references.len(), 1);
    assert_eq!(
        plan.preserved_references[0].reason,
        PreservedMediaReason::DeferredConversion
    );
    assert_eq!(
        plan.diagnostics[0].issue,
        MediaMaterializationIssue::DeferredExcalidrawConversion
    );
    let original =
        &prepared.media_references[plan.preserved_references[0].reference_index as usize];
    assert_eq!(original.raw_spelling, "[[draws/diagram.excalidraw]]");
}

#[test]
fn rewrite_matches_commonmark_escaped_angle_destination_and_reference_title() {
    let graph = graph(
        "- ![angled](<../assets/a\\>b.png> \"Angle title\")\n- ![referenced][asset]\n  \n  [asset]: ../assets/reference.png \"Reference title\"\n",
        &[
            ("assets/a>b.png", b"angled"),
            ("assets/reference.png", b"referenced"),
        ],
    );
    let prepared = prepare(&graph);
    let plan = materialize_source_media(&prepared, graph.path()).expect("materialize syntax");

    let angled = plan
        .rewrites
        .iter()
        .find(|rewrite| {
            prepared.media_references[rewrite.reference_index as usize]
                .owner_markdown_spelling
                .starts_with("![angled]")
        })
        .expect("angled rewrite");
    assert_eq!(
        angled.replacement_markdown,
        format!("![angled](<{}> \"Angle title\")", angled.attachment_uri)
    );
    let referenced = plan
        .rewrites
        .iter()
        .find(|rewrite| {
            prepared.media_references[rewrite.reference_index as usize].owner_markdown_spelling
                == "![referenced][asset]"
        })
        .expect("reference-style rewrite");
    assert_eq!(
        referenced.replacement_markdown,
        format!(
            "![referenced]({} \"Reference title\")",
            referenced.attachment_uri
        )
    );
}

#[test]
fn duplicate_prepared_identities_are_fatal_before_materialization() {
    let graph = graph("- first\n- second\n", &[]);
    let mut prepared = prepare(&graph);
    prepared.pages[0].blocks[1].uuid = prepared.pages[0].blocks[0].uuid;
    let error =
        materialize_source_media(&prepared, graph.path()).expect_err("duplicate block UUID");
    assert_eq!(error.code(), MaterializeMediaErrorCode::InvalidPreparedPlan);
}

#[test]
fn validates_root_and_limit_contracts_before_reading() {
    let graph = graph("- plain\n", &[]);
    let prepared = prepare(&graph);
    let file = graph.path().join("not-a-directory");
    fs::write(&file, b"file").expect("root file");
    assert_eq!(
        materialize_source_media(&prepared, &file)
            .expect_err("root file")
            .code(),
        MaterializeMediaErrorCode::RootNotDirectory
    );
    assert_eq!(
        materialize_source_media(&prepared, graph.path().join("missing"))
            .expect_err("missing root")
            .code(),
        MaterializeMediaErrorCode::RootNotFound
    );
    assert_eq!(
        materialize_source_media_with_limits(
            &prepared,
            graph.path(),
            &MediaMaterializationLimits {
                max_rewrites: 0,
                ..MediaMaterializationLimits::default()
            }
        )
        .expect_err("zero limit")
        .code(),
        MaterializeMediaErrorCode::InvalidLimits
    );
}

fn inline_png_bytes() -> Vec<u8> {
    let data = DataUrl::process(PNG_DATA).expect("valid data URL");
    let mut bytes = Vec::new();
    let fragment = data
        .decode(|chunk| {
            bytes.extend_from_slice(chunk);
            Ok::<_, ()>(())
        })
        .expect("decode data URL");
    assert!(fragment.is_none());
    bytes
}

fn digest(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::from_bytes(Sha256::digest(bytes).into())
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
            let bytes = fs::read(graph.path().join(&entry.relative_path)).expect("read document");
            parse_logseq_markdown(entry, &bytes, report.manifest.config()).expect("parse document")
        })
        .collect::<Vec<_>>();
    prepare_import(
        &documents,
        IdentityContext {
            workspace_uuid: Uuid::from_u128(0x300),
            import_namespace_uuid: Uuid::from_u128(0x400),
        },
        &report.manifest,
    )
    .expect("prepare graph")
}
