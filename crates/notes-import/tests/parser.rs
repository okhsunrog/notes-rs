use std::fs;

use notes_import::{
    DiagnosticCode, DocumentFormat, FileNameFormat, LogseqConfig, LogseqConstructKind,
    LogseqConstructOwner, LogseqDocumentSource, LogseqParseError, LogseqParseErrorCode,
    ManifestEntry, Sha256Digest, SourceKind, parse_logseq_markdown, scan_logseq_graph,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/parser");
const PAGE_PATH: &str = "pages/project___architecture%20notes.md";

#[test]
fn parses_representative_logseq_page_losslessly() {
    let report = scan_logseq_graph(FIXTURE).expect("scan parser fixture");
    let entry = report
        .manifest
        .entries()
        .iter()
        .find(|entry| entry.relative_path == PAGE_PATH)
        .expect("page manifest entry");
    let bytes = fs::read(format!("{FIXTURE}/{PAGE_PATH}")).expect("read page fixture");
    let document = parse_logseq_markdown(entry, &bytes, report.manifest.config())
        .expect("parse representative page");

    assert_eq!(
        document.source,
        LogseqDocumentSource::Page {
            file_title: "project/architecture notes".to_owned(),
        }
    );
    assert_eq!(document.blocks.len(), 4);
    assert_eq!(document.blocks[0].parent_index, None);
    assert_eq!(document.blocks[0].depth, 0);
    assert_eq!(document.blocks[1].parent_index, Some(0));
    assert_eq!(document.blocks[1].depth, 1);
    assert_eq!(document.blocks[2].parent_index, Some(0));
    assert_eq!(document.blocks[2].sibling_index, 1);
    assert_eq!(document.blocks[3].parent_index, None);
    assert_eq!(document.blocks[3].sibling_index, 1);

    let root = &document.blocks[0];
    assert!(root.markdown.starts_with("TODO Root block\nid:: 123e4567"));
    assert!(root.markdown.contains("* ordinary Markdown bullet"));
    assert!(root.markdown.contains("1. ordinary numbered Markdown item"));
    assert!(root.markdown.contains("| Name | Value |"));
    assert!(root.markdown.contains("E = mc^2"));
    assert!(root.markdown.contains(":LOGBOOK:"));

    let code_child = &document.blocks[1];
    assert!(code_child.markdown.contains("```sh"));
    assert!(
        code_child
            .markdown
            .contains("- this is fenced code, not a structural block")
    );
    assert_eq!(document.blocks[3].markdown, "## Heading block");

    let preamble = document.preamble.as_ref().expect("page preamble");
    assert_eq!(preamble.markdown, "title:: Project Architecture");
    let reconstructed = std::iter::once(preamble.raw_source.as_str())
        .chain(
            document
                .blocks
                .iter()
                .map(|block| block.raw_source.as_str()),
        )
        .collect::<String>();
    assert_eq!(reconstructed, document.raw_markdown);

    assert!(document.constructs.iter().any(|construct| matches!(
        &construct.kind,
        LogseqConstructKind::Property { name, value, .. }
            if name == "title" && value == "Project Architecture"
    ) && construct.owner
        == LogseqConstructOwner::Preamble));
    assert!(document.constructs.iter().any(|construct| matches!(
        &construct.kind,
        LogseqConstructKind::Property { name, value, .. }
            if name == "id" && value == "123e4567-e89b-12d3-a456-426614174000"
    )));
    assert!(document.constructs.iter().any(|construct| matches!(
        &construct.kind,
        LogseqConstructKind::TaskMarker { marker } if marker == "TODO"
    )));
    assert!(
        document
            .constructs
            .iter()
            .any(|construct| matches!(construct.kind, LogseqConstructKind::UnorderedListItem))
    );
    assert!(
        document
            .constructs
            .iter()
            .any(|construct| matches!(construct.kind, LogseqConstructKind::OrderedListItem))
    );
    assert_eq!(
        document
            .constructs
            .iter()
            .filter(|construct| matches!(construct.kind, LogseqConstructKind::TableRow))
            .count(),
        2
    );
    assert!(document.constructs.iter().any(|construct| matches!(
        &construct.kind,
        LogseqConstructKind::FenceStart { info, .. } if info.as_deref() == Some("sh")
    )));
    assert!(
        document
            .constructs
            .iter()
            .any(|construct| matches!(construct.kind, LogseqConstructKind::Heading { level: 2 }))
    );

    let macro_warning = document
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == DiagnosticCode::PreservedMacro)
        .expect("macro warning");
    let macro_range = macro_warning.range.expect("macro range");
    assert_eq!(
        &document.raw_markdown
            [macro_range.start.byte_offset as usize..macro_range.end.byte_offset as usize],
        "{{video https://example.invalid/watch}}"
    );
    assert!(
        document
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::MixedIndentation)
    );
    assert!(document.diagnostics.iter().all(|diagnostic| {
        diagnostic.relative_path.as_deref() == Some(PAGE_PATH)
            && diagnostic.range.is_some_and(|range| {
                range.start.line > 0
                    && range.start.column > 0
                    && range.end.byte_offset <= bytes.len() as u64
            })
            && !diagnostic.message.contains("example.invalid")
            && !diagnostic
                .message
                .contains("123e4567-e89b-12d3-a456-426614174000")
    }));
}

#[test]
fn parses_strict_journal_source_and_nested_blocks() {
    let report = scan_logseq_graph(FIXTURE).expect("scan parser fixture");
    let entry = report
        .manifest
        .entries()
        .iter()
        .find(|entry| entry.relative_path == "journals/2026_07_17.md")
        .expect("journal manifest entry");
    let bytes =
        fs::read(format!("{FIXTURE}/journals/2026_07_17.md")).expect("read journal fixture");
    let document =
        parse_logseq_markdown(entry, &bytes, report.manifest.config()).expect("parse journal");

    assert!(matches!(
        &document.source,
        LogseqDocumentSource::Journal { date } if date.as_str() == "2026-07-17"
    ));
    assert_eq!(document.blocks.len(), 2);
    assert_eq!(document.blocks[1].parent_index, Some(0));
}

#[test]
fn date_looking_page_is_not_reclassified_as_journal() {
    let bytes = b"- ordinary page";
    let entry = manifest_entry(SourceKind::Page, "pages/2026_07_17.md", bytes);
    let document = parse_logseq_markdown(&entry, bytes, &LogseqConfig::default())
        .expect("parse date-looking page");
    assert_eq!(
        document.source,
        LogseqDocumentSource::Page {
            file_title: "2026_07_17".to_owned(),
        }
    );
}

#[test]
fn page_extension_matches_the_scanners_case_insensitive_markdown_contract() {
    let bytes = b"- page";
    let entry = manifest_entry(SourceKind::Page, "pages/Mixed.MD", bytes);
    let document = parse_logseq_markdown(&entry, bytes, &LogseqConfig::default())
        .expect("scanner-recognized Markdown extension must parse");
    assert_eq!(
        document.source,
        LogseqDocumentSource::Page {
            file_title: "Mixed".to_owned(),
        }
    );

    let journal = manifest_entry(SourceKind::Journal, "journals/2026_07_17.MD", bytes);
    assert_eq!(
        parse_logseq_markdown(&journal, bytes, &LogseqConfig::default())
            .expect_err("journal filename remains deliberately strict")
            .code(),
        LogseqParseErrorCode::InvalidJournalFilename
    );
}

#[test]
fn page_filename_must_decode_to_a_nonempty_identity() {
    let bytes = b"- page";
    let config = LogseqConfig {
        file_name_format: FileNameFormat::TripleLowbar,
        ..LogseqConfig::default()
    };
    let empty = manifest_entry(SourceKind::Page, "pages/___.md", bytes);
    assert_eq!(
        parse_logseq_markdown(&empty, bytes, &config)
            .expect_err("empty decoded title")
            .code(),
        LogseqParseErrorCode::EmptyPageTitle
    );

    let missing_stem = manifest_entry(SourceKind::Page, "pages/.md", bytes);
    assert_eq!(
        parse_logseq_markdown(&missing_stem, bytes, &config)
            .expect_err("missing filename stem")
            .code(),
        LogseqParseErrorCode::InvalidPageFilename
    );
}

#[test]
fn utf8_bom_is_transparent_to_markdown_syntax_but_preserved_in_raw_source() {
    let property_source = "\u{feff}title:: BOM page\n- body";
    let entry = manifest_entry(
        SourceKind::Page,
        "pages/bom-property.md",
        property_source.as_bytes(),
    );
    let document =
        parse_logseq_markdown(&entry, property_source.as_bytes(), &LogseqConfig::default())
            .expect("parse BOM-prefixed property");
    assert_eq!(
        document.preamble.as_ref().expect("preamble").markdown,
        "title:: BOM page"
    );
    assert!(document.constructs.iter().any(|construct| matches!(
        &construct.kind,
        LogseqConstructKind::Property { name, value, .. }
            if name == "title" && value == "BOM page"
    )));
    assert_eq!(reconstruct_source(&document), property_source);

    let block_source = "\u{feff}- first block";
    let entry = manifest_entry(
        SourceKind::Page,
        "pages/bom-block.md",
        block_source.as_bytes(),
    );
    let document = parse_logseq_markdown(&entry, block_source.as_bytes(), &LogseqConfig::default())
        .expect("parse BOM-prefixed block");
    assert!(document.preamble.is_none());
    assert_eq!(document.blocks.len(), 1);
    assert_eq!(document.blocks[0].markdown, "first block");
    assert_eq!(document.blocks[0].raw_source, block_source);
}

#[test]
fn journal_filename_must_be_direct_strict_and_calendar_valid() {
    let bytes = b"- journal";
    for path in [
        "journals/2023_02_29.md",
        "journals/2026-07-17.md",
        "journals/2026_07_17.markdown",
        "journals/archive/2026_07_17.md",
    ] {
        let entry = manifest_entry(SourceKind::Journal, path, bytes);
        let error = parse_logseq_markdown(&entry, bytes, &LogseqConfig::default())
            .expect_err("invalid journal filename");
        assert_eq!(
            error.code(),
            LogseqParseErrorCode::InvalidJournalFilename,
            "{path}"
        );
    }
}

#[test]
fn parser_rejects_bytes_that_differ_from_manifest() {
    let original = b"- original";
    let entry = manifest_entry(SourceKind::Page, "pages/integrity.md", original);
    let size_error =
        parse_logseq_markdown(&entry, b"- original plus bytes", &LogseqConfig::default())
            .expect_err("size mismatch");
    assert!(matches!(
        size_error,
        LogseqParseError::SourceSizeMismatch { .. }
    ));

    let hash_error = parse_logseq_markdown(&entry, b"- OriginaL", &LogseqConfig::default())
        .expect_err("hash mismatch");
    assert!(matches!(
        hash_error,
        LogseqParseError::SourceHashMismatch { .. }
    ));
}

#[test]
fn invalid_utf8_is_reported_after_manifest_integrity_validation() {
    let bytes = [b'-', b' ', 0xff];
    let entry = manifest_entry(SourceKind::Page, "pages/invalid.md", &bytes);
    let error =
        parse_logseq_markdown(&entry, &bytes, &LogseqConfig::default()).expect_err("invalid UTF-8");
    assert!(matches!(
        error,
        LogseqParseError::InvalidUtf8 { valid_up_to: 2, .. }
    ));
}

#[test]
fn no_final_newline_and_long_physical_line_are_preserved() {
    let fixture = include_bytes!("fixtures/parser/pages/project___architecture%20notes.md");
    let without_final_newline = fixture
        .strip_suffix(b"\n")
        .expect("fixture normally has a final newline for source control");
    let entry = manifest_entry(SourceKind::Page, PAGE_PATH, without_final_newline);
    let config = LogseqConfig {
        file_name_format: FileNameFormat::TripleLowbar,
        ..LogseqConfig::default()
    };
    let document = parse_logseq_markdown(&entry, without_final_newline, &config)
        .expect("parse no-final-newline fixture");
    assert!(!document.raw_markdown.ends_with('\n'));
    assert_eq!(
        document.blocks.last().expect("last block").raw_source,
        "- ## Heading block"
    );

    let long_body = "x".repeat(32 * 1024);
    let long_source = format!("- {long_body}");
    let long_entry = manifest_entry(SourceKind::Page, "pages/long.md", long_source.as_bytes());
    let long_document = parse_logseq_markdown(
        &long_entry,
        long_source.as_bytes(),
        &LogseqConfig::default(),
    )
    .expect("parse long physical line");
    assert_eq!(long_document.blocks[0].markdown.len(), long_body.len());
}

#[test]
fn unclosed_fence_is_preserved_with_a_ranged_warning() {
    let bytes = b"- code\n  ```rust\n  - still code";
    let entry = manifest_entry(SourceKind::Page, "pages/unclosed.md", bytes);
    let document = parse_logseq_markdown(&entry, bytes, &LogseqConfig::default())
        .expect("parse unclosed fence");

    assert_eq!(document.blocks.len(), 1);
    assert!(document.blocks[0].markdown.contains("- still code"));
    let warning = document
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == DiagnosticCode::UnclosedFence)
        .expect("unclosed fence warning");
    assert_eq!(warning.range.expect("range").start.line, 2);
}

#[test]
fn fences_follow_commonmark_indentation_and_backtick_info_rules() {
    let over_indented = b"- root\n      ```not-a-fence\n  - child";
    let entry = manifest_entry(SourceKind::Page, "pages/over-indented.md", over_indented);
    let document = parse_logseq_markdown(&entry, over_indented, &LogseqConfig::default())
        .expect("parse over-indented delimiter");
    assert_eq!(document.blocks.len(), 2);
    assert!(
        !document
            .constructs
            .iter()
            .any(|construct| matches!(construct.kind, LogseqConstructKind::FenceStart { .. }))
    );

    let invalid_info = b"- root\n  ```bad`info\n  - child";
    let entry = manifest_entry(
        SourceKind::Page,
        "pages/invalid-fence-info.md",
        invalid_info,
    );
    let document = parse_logseq_markdown(&entry, invalid_info, &LogseqConfig::default())
        .expect("parse invalid backtick fence info");
    assert_eq!(document.blocks.len(), 2);
    assert!(
        !document
            .constructs
            .iter()
            .any(|construct| matches!(construct.kind, LogseqConstructKind::FenceStart { .. }))
    );

    let over_indented_close = b"- root\n  ```\n      ```\n  - fenced bullet\n  ````\n- next";
    let entry = manifest_entry(
        SourceKind::Page,
        "pages/over-indented-close.md",
        over_indented_close,
    );
    let document = parse_logseq_markdown(&entry, over_indented_close, &LogseqConfig::default())
        .expect("parse over-indented closing delimiter");
    assert_eq!(document.blocks.len(), 2);
    assert!(document.blocks[0].markdown.contains("- fenced bullet"));
    assert_eq!(
        document
            .constructs
            .iter()
            .filter(|construct| matches!(construct.kind, LogseqConstructKind::FenceStart { .. }))
            .count(),
        1
    );
    assert_eq!(
        document
            .constructs
            .iter()
            .filter(|construct| matches!(construct.kind, LogseqConstructKind::FenceEnd { .. }))
            .count(),
        1
    );
    let closing = document
        .constructs
        .iter()
        .find(|construct| matches!(construct.kind, LogseqConstructKind::FenceEnd { .. }))
        .expect("closing fence construct");
    assert!(matches!(
        &closing.kind,
        LogseqConstructKind::FenceEnd { marker } if marker == "````"
    ));
    assert_eq!(
        source_slice(
            std::str::from_utf8(over_indented_close).expect("fixture UTF-8"),
            closing.source_range,
        ),
        "````"
    );
}

#[test]
fn nested_noncanonical_continuations_are_loss_aware() {
    let bytes = b"- root\n\t- child\n\tcontinuation missing padding\nunindented continuation";
    let entry = manifest_entry(SourceKind::Page, "pages/continuations.md", bytes);
    let document = parse_logseq_markdown(&entry, bytes, &LogseqConfig::default())
        .expect("parse noncanonical continuations");

    assert_eq!(document.blocks.len(), 2);
    assert_eq!(
        document.blocks[1].markdown,
        "child\ncontinuation missing padding\nunindented continuation"
    );
    assert_eq!(
        document
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.code == DiagnosticCode::NonCanonicalContinuationIndentation
            })
            .count(),
        2
    );
    assert_eq!(reconstruct_source(&document), document.raw_markdown);
}

#[test]
fn empty_structural_blocks_are_not_dropped() {
    let bytes = b"-\n\t-\n- content";
    let entry = manifest_entry(SourceKind::Page, "pages/empty-blocks.md", bytes);
    let document = parse_logseq_markdown(&entry, bytes, &LogseqConfig::default())
        .expect("parse empty structural blocks");

    assert_eq!(document.blocks.len(), 3);
    assert_eq!(document.blocks[0].markdown, "");
    assert_eq!(document.blocks[1].markdown, "");
    assert_eq!(document.blocks[1].parent_index, Some(0));
    assert_eq!(document.blocks[2].parent_index, None);
    assert_eq!(reconstruct_source(&document), document.raw_markdown);
}

#[test]
fn dash_list_marker_inside_the_structural_block_body_is_semantic() {
    let bytes = b"- - semantic Markdown item";
    let entry = manifest_entry(SourceKind::Page, "pages/semantic-list.md", bytes);
    let document = parse_logseq_markdown(&entry, bytes, &LogseqConfig::default())
        .expect("parse semantic list marker");

    assert_eq!(document.blocks.len(), 1);
    assert_eq!(document.blocks[0].markdown, "- semantic Markdown item");
    assert!(
        document
            .constructs
            .iter()
            .any(|construct| matches!(construct.kind, LogseqConstructKind::UnorderedListItem))
    );
}

#[test]
fn unicode_macro_ranges_are_exact_and_source_ordered() {
    let source = "- 😀 {{one}} λ {{two}}";
    let entry = manifest_entry(SourceKind::Page, "pages/macros.md", source.as_bytes());
    let document = parse_logseq_markdown(&entry, source.as_bytes(), &LogseqConfig::default())
        .expect("parse Unicode macros");
    let macro_ranges = document
        .constructs
        .iter()
        .filter_map(|construct| {
            matches!(construct.kind, LogseqConstructKind::Macro).then_some(construct.source_range)
        })
        .collect::<Vec<_>>();
    assert_eq!(macro_ranges.len(), 2);
    assert_eq!(source_slice(source, macro_ranges[0]), "{{one}}");
    assert_eq!(source_slice(source, macro_ranges[1]), "{{two}}");
    assert_eq!(macro_ranges[0].start.column, 5);
    assert!(document.constructs.windows(2).all(|constructs| {
        constructs[0].source_range.start.byte_offset <= constructs[1].source_range.start.byte_offset
    }));
}

#[test]
fn property_ast_exposes_trimmed_values_and_exact_ranges_without_reparsing() {
    let source = "title  ::   Project Name   \nempty::\nspaces::   \nunicode:: \t λ \t\n- body";
    let entry = manifest_entry(SourceKind::Page, "pages/properties.md", source.as_bytes());
    let document = parse_logseq_markdown(&entry, source.as_bytes(), &LogseqConfig::default())
        .expect("parse properties");
    let properties = document
        .constructs
        .iter()
        .filter_map(|construct| match &construct.kind {
            LogseqConstructKind::Property {
                name,
                value,
                value_range,
            } => Some((
                name.as_str(),
                value.as_str(),
                *value_range,
                construct.source_range,
            )),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(properties.len(), 4);
    assert_eq!(
        (properties[0].0, properties[0].1),
        ("title", "Project Name")
    );
    assert_eq!(source_slice(source, properties[0].2), "Project Name");
    assert_eq!(
        source_slice(source, properties[0].3),
        "title  ::   Project Name   "
    );
    assert_eq!((properties[1].0, properties[1].1), ("empty", ""));
    assert_eq!(properties[1].2.start, properties[1].2.end);
    assert_eq!((properties[2].0, properties[2].1), ("spaces", ""));
    assert_eq!(properties[2].2.start, properties[2].2.end);
    assert_eq!((properties[3].0, properties[3].1), ("unicode", "λ"));
    assert_eq!(source_slice(source, properties[3].2), "λ");
}

#[test]
fn property_delimiter_requires_whitespace_or_end_of_line() {
    let source = "valid:: value\nempty::\ntab::\tvalue\nfoo::bar\nurl::https://example.com\n- body";
    let entry = manifest_entry(SourceKind::Page, "pages/properties.md", source.as_bytes());
    let document = parse_logseq_markdown(&entry, source.as_bytes(), &LogseqConfig::default())
        .expect("parse properties");
    let properties = document
        .constructs
        .iter()
        .filter_map(|construct| match &construct.kind {
            LogseqConstructKind::Property { name, value, .. } => {
                Some((name.as_str(), value.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        properties,
        [("valid", "value"), ("empty", ""), ("tab", "value")]
    );
    assert_eq!(reconstruct_source(&document), source);
}

#[test]
fn arbitrary_utf8_and_malformed_structure_never_panics_or_loses_source() {
    const TOKENS: &[&str] = &[
        "-",
        "- ",
        "\t- child",
        "   - odd",
        "  continuation",
        "```",
        "````bad`info",
        "~~~~",
        "{{macro value}}",
        "{{unclosed",
        "}}",
        "id:: 123e4567-e89b-12d3-a456-426614174000",
        "# heading",
        "| a | b |",
        "😀 λ 中",
        "\0",
        "\r",
        "* semantic list",
    ];

    let mut state = 0x4d59_5df4_d0f3_3173_u64;
    for case in 0..256 {
        let mut source = String::new();
        for line in 0..1 + case % 31 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            source.push_str(TOKENS[state as usize % TOKENS.len()]);
            if line + 1 < 1 + case % 31 || state & 1 == 0 {
                source.push('\n');
            }
        }
        let entry = manifest_entry(SourceKind::Page, "pages/generated.md", source.as_bytes());
        let document = parse_logseq_markdown(&entry, source.as_bytes(), &LogseqConfig::default())
            .expect("generated valid UTF-8 source must parse");
        assert_eq!(document.raw_markdown, source);
        assert_eq!(reconstruct_source(&document), source);

        for range in document
            .constructs
            .iter()
            .map(|construct| construct.source_range)
            .chain(
                document
                    .diagnostics
                    .iter()
                    .filter_map(|diagnostic| diagnostic.range),
            )
            .chain(
                document
                    .preamble
                    .iter()
                    .map(|preamble| preamble.source_range),
            )
            .chain(
                document
                    .blocks
                    .iter()
                    .flat_map(|block| [block.source_range, block.content_range]),
            )
        {
            let start = range.start.byte_offset as usize;
            let end = range.end.byte_offset as usize;
            assert!(start <= end && end <= source.len());
            assert!(source.is_char_boundary(start));
            assert!(source.is_char_boundary(end));
        }
    }
}

#[test]
fn configured_directory_boundary_is_enforced() {
    let bytes = b"- page";
    let entry = manifest_entry(SourceKind::Page, "elsewhere/page.md", bytes);
    let error = parse_logseq_markdown(&entry, bytes, &LogseqConfig::default())
        .expect_err("outside pages directory");
    assert_eq!(
        error.code(),
        LogseqParseErrorCode::EntryOutsideConfiguredDirectory
    );
}

#[test]
fn scanner_and_parser_accept_invalid_utf8_as_different_boundaries() {
    let temp = TempDir::new().expect("temp dir");
    fs::create_dir_all(temp.path().join("pages")).expect("create pages");
    fs::write(temp.path().join("pages/invalid.md"), [0xff]).expect("write invalid UTF-8");
    let report = scan_logseq_graph(temp.path()).expect("scanner hashes opaque bytes");
    let entry = report
        .manifest
        .entries()
        .iter()
        .find(|entry| entry.kind == SourceKind::Page)
        .expect("page entry");
    assert!(matches!(
        parse_logseq_markdown(entry, &[0xff], report.manifest.config()),
        Err(LogseqParseError::InvalidUtf8 { .. })
    ));
}

fn manifest_entry(kind: SourceKind, relative_path: &str, bytes: &[u8]) -> ManifestEntry {
    ManifestEntry {
        kind,
        document_format: Some(DocumentFormat::Markdown),
        relative_path: relative_path.to_owned(),
        size_bytes: bytes.len() as u64,
        sha256: Sha256Digest::from_bytes(Sha256::digest(bytes).into()),
    }
}

fn reconstruct_source(document: &notes_import::ParsedLogseqDocument) -> String {
    document
        .preamble
        .iter()
        .map(|preamble| preamble.raw_source.as_str())
        .chain(
            document
                .blocks
                .iter()
                .map(|block| block.raw_source.as_str()),
        )
        .collect()
}

fn source_slice(source: &str, range: notes_import::SourceRange) -> &str {
    &source[range.start.byte_offset as usize..range.end.byte_offset as usize]
}
