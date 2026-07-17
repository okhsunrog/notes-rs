use std::fs;

use notes_import::{
    DiagnosticCode, DocumentFormat, FileNameFormat, ManifestVerification, ScanError, ScanErrorCode,
    ScanLimits, SourceKind, scan_logseq_graph, scan_logseq_graph_with_limits,
    verify_logseq_manifest,
};
use tempfile::TempDir;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/basic");

#[test]
fn scans_fixture_deterministically_with_portable_paths() {
    let first = scan_logseq_graph(FIXTURE).expect("scan fixture");
    let second = scan_logseq_graph(FIXTURE).expect("scan fixture again");

    assert_eq!(first, second);
    assert_eq!(
        first.manifest.config().file_name_format,
        FileNameFormat::TripleLowbar
    );
    assert_eq!(
        first.manifest.config().pages_directory.as_str(),
        "content/pages"
    );
    assert_eq!(first.manifest.config().journals_directory.as_str(), "daily");
    assert_eq!(first.manifest.config().assets_directory.as_str(), "media");

    let paths = first
        .manifest
        .entries()
        .iter()
        .map(|entry| entry.relative_path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        [
            "content/pages/project___architecture.md",
            "daily/2026_07_17.md",
            "draws/diagram.excalidraw",
            "logseq/config.edn",
            "media/pixel.png",
        ]
    );
    assert!(first.diagnostics.is_empty());

    let page = &first.manifest.entries()[0];
    assert_eq!(page.kind, SourceKind::Page);
    assert_eq!(page.document_format, Some(DocumentFormat::Markdown));

    let encoded = serde_json::to_string_pretty(&first.manifest).expect("serialize manifest");
    assert!(encoded.contains("\"triple_lowbar\""));
    assert!(encoded.contains(&first.manifest.sha256().to_string()));
}

#[test]
fn identical_graph_at_a_different_absolute_path_has_the_same_manifest() {
    let first = scan_logseq_graph(FIXTURE).expect("scan fixture");
    let temp = TempDir::new().expect("temp dir");
    copy_tree(std::path::Path::new(FIXTURE), temp.path());
    let relocated = scan_logseq_graph(temp.path()).expect("scan relocated fixture");

    assert_eq!(first.manifest, relocated.manifest);
}

#[test]
fn content_changes_change_the_manifest_hash() {
    let temp = TempDir::new().expect("temp dir");
    copy_tree(std::path::Path::new(FIXTURE), temp.path());
    let before = scan_logseq_graph(temp.path()).expect("scan before mutation");
    fs::write(
        temp.path().join("content/pages/project___architecture.md"),
        "- changed\n",
    )
    .expect("change fixture copy");
    let after = scan_logseq_graph(temp.path()).expect("scan after mutation");

    assert_ne!(before.manifest.sha256(), after.manifest.sha256());
    assert!(matches!(
        verify_logseq_manifest(temp.path(), &before.manifest).expect("verify changed graph"),
        ManifestVerification::Changed { current, .. }
            if current.manifest == after.manifest
    ));
}

#[test]
fn unchanged_manifest_verifies_exactly() {
    let scan = scan_logseq_graph(FIXTURE).expect("scan fixture");
    let verification = verify_logseq_manifest(FIXTURE, &scan.manifest).expect("verify fixture");
    assert!(matches!(
        &verification,
        ManifestVerification::Exact { current } if current == &scan
    ));
    let json = serde_json::to_value(verification).expect("serialize verification");
    assert_eq!(json["status"], "exact");
}

#[test]
fn missing_standard_directories_are_loss_aware_warnings() {
    let temp = TempDir::new().expect("temp dir");
    let report = scan_logseq_graph(temp.path()).expect("scan empty graph");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::ConfigNotFound)
    );
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == DiagnosticCode::SourceDirectoryNotFound)
            .count(),
        3
    );
}

#[test]
fn overlapping_configured_directories_are_rejected() {
    let temp = TempDir::new().expect("temp dir");
    fs::create_dir_all(temp.path().join("logseq")).expect("create config dir");
    fs::write(
        temp.path().join("logseq/config.edn"),
        r#"{:pages-directory "notes" :journals-directory "notes/daily"}"#,
    )
    .expect("write config");

    assert!(matches!(
        scan_logseq_graph(temp.path()),
        Err(ScanError::DirectoryOverlap { .. })
    ));
}

#[test]
fn configured_source_cannot_capture_config_or_fixed_drawings() {
    for config in [
        r#"{:pages-directory "logseq"}"#,
        r#"{:assets-directory "draws"}"#,
    ] {
        let temp = TempDir::new().expect("temp dir");
        fs::create_dir_all(temp.path().join("logseq")).expect("create config dir");
        fs::write(temp.path().join("logseq/config.edn"), config).expect("write config");
        assert!(matches!(
            scan_logseq_graph(temp.path()),
            Err(ScanError::DirectoryOverlap { .. })
        ));
    }
}

#[test]
fn unsupported_document_is_hashed_and_reported() {
    let temp = TempDir::new().expect("temp dir");
    fs::create_dir_all(temp.path().join("pages")).expect("create pages");
    fs::write(temp.path().join("pages/legacy.org"), "* preserved bytes\n")
        .expect("write org document");
    let report = scan_logseq_graph(temp.path()).expect("scan graph");

    assert!(report.manifest.entries().iter().any(|entry| {
        entry.relative_path == "pages/legacy.org"
            && entry.document_format == Some(DocumentFormat::Org)
    }));
    assert!(report.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::UnsupportedDocumentFormat
            && diagnostic.relative_path.as_deref() == Some("pages/legacy.org")
    }));
}

#[test]
fn known_recovery_directories_are_skipped_but_other_dot_paths_are_kept() {
    let temp = TempDir::new().expect("temp dir");
    let pages = temp.path().join("pages");
    fs::create_dir_all(pages.join(".custom")).expect("create custom dot directory");
    fs::write(pages.join(".important.md"), "kept dotfile").expect("write meaningful dotfile");
    fs::write(pages.join(".custom/kept.md"), "kept nested dot path")
        .expect("write meaningful nested dotfile");

    let ignored_directories = [
        ".git",
        ".obsidian",
        ".recycle",
        "backup",
        "bak",
        "version-history",
        "version-files",
    ];
    for directory in ignored_directories {
        let ignored = pages.join(directory);
        fs::create_dir_all(&ignored).expect("create ignored directory");
        fs::write(ignored.join("ignored.md"), format!("ignored {directory}"))
            .expect("write ignored bytes");
    }

    let before = scan_logseq_graph(temp.path()).expect("scan graph with recovery data");
    let paths = before
        .manifest
        .entries()
        .iter()
        .map(|entry| entry.relative_path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(paths, ["pages/.custom/kept.md", "pages/.important.md"]);

    for directory in ignored_directories {
        fs::write(
            pages.join(directory).join("ignored.md"),
            format!("changed ignored bytes in {directory}"),
        )
        .expect("change ignored bytes");
    }
    let after = scan_logseq_graph(temp.path()).expect("rescan graph with changed recovery data");
    assert_eq!(before.manifest, after.manifest);
}

#[test]
fn entry_limit_is_typed_and_deterministic() {
    let temp = graph_with_page("one.md", b"one");
    let limits = ScanLimits {
        max_entries: 0,
        ..ScanLimits::default()
    };
    let error = scan_logseq_graph_with_limits(temp.path(), &limits).expect_err("entry limit");

    assert_eq!(error.code(), ScanErrorCode::EntryLimitExceeded);
    assert!(matches!(error, ScanError::EntryLimitExceeded { limit: 0 }));
}

#[test]
fn entry_limit_also_bounds_empty_directory_fanout() {
    let temp = TempDir::new().expect("temp dir");
    fs::create_dir_all(temp.path().join("pages/one")).expect("create first empty directory");
    fs::create_dir_all(temp.path().join("pages/two")).expect("create second empty directory");
    let limits = ScanLimits {
        max_entries: 1,
        ..ScanLimits::default()
    };

    let error = scan_logseq_graph_with_limits(temp.path(), &limits).expect_err("entry fanout");
    assert_eq!(error.code(), ScanErrorCode::EntryLimitExceeded);
}

#[test]
fn per_file_limit_is_typed_and_exact_boundary_is_allowed() {
    let temp = graph_with_page("five.md", b"12345");
    let failing = ScanLimits {
        max_file_bytes: 4,
        ..ScanLimits::default()
    };
    let error = scan_logseq_graph_with_limits(temp.path(), &failing).expect_err("file limit");
    assert_eq!(error.code(), ScanErrorCode::FileTooLarge);
    assert!(matches!(
        error,
        ScanError::FileTooLarge {
            relative_path,
            limit_bytes: 4,
        } if relative_path == "pages/five.md"
    ));

    let exact = ScanLimits {
        max_file_bytes: 5,
        max_total_bytes: 5,
        ..ScanLimits::default()
    };
    let report = scan_logseq_graph_with_limits(temp.path(), &exact).expect("exact boundaries");
    assert_eq!(report.manifest.total_bytes(), 5);
}

#[test]
fn total_bytes_limit_is_typed() {
    let temp = graph_with_page("five.md", b"12345");
    let limits = ScanLimits {
        max_total_bytes: 4,
        ..ScanLimits::default()
    };
    let error = scan_logseq_graph_with_limits(temp.path(), &limits).expect_err("total limit");

    assert_eq!(error.code(), ScanErrorCode::TotalBytesLimitExceeded);
    assert!(matches!(
        error,
        ScanError::TotalBytesLimitExceeded { limit_bytes: 4 }
    ));
}

#[test]
fn config_limit_is_separate_and_typed() {
    let temp = TempDir::new().expect("temp dir");
    fs::create_dir_all(temp.path().join("logseq")).expect("create config directory");
    fs::write(temp.path().join("logseq/config.edn"), "{}").expect("write minimal config");
    let limits = ScanLimits {
        max_config_bytes: 1,
        ..ScanLimits::default()
    };
    let error = scan_logseq_graph_with_limits(temp.path(), &limits).expect_err("config limit");

    assert_eq!(error.code(), ScanErrorCode::ConfigTooLarge);
    assert!(matches!(
        error,
        ScanError::ConfigTooLarge { limit_bytes: 1 }
    ));
}

#[cfg(unix)]
#[test]
fn symlinks_to_files_or_directories_are_rejected() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("temp dir");
    let outside = TempDir::new().expect("outside temp dir");
    fs::create_dir_all(temp.path().join("pages")).expect("create pages");
    fs::create_dir_all(temp.path().join("journals")).expect("create journals");
    fs::create_dir_all(temp.path().join("assets")).expect("create assets");
    fs::write(outside.path().join("secret.md"), "not part of graph").expect("write outside file");
    symlink(
        outside.path().join("secret.md"),
        temp.path().join("pages/escape.md"),
    )
    .expect("create symlink");

    assert!(matches!(
        scan_logseq_graph(temp.path()),
        Err(ScanError::SymlinkNotAllowed { relative_path })
            if relative_path == "pages/escape.md"
    ));
}

#[cfg(unix)]
#[test]
fn configured_directory_symlink_is_rejected_even_when_its_target_is_missing() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("temp dir");
    symlink(temp.path().join("missing"), temp.path().join("pages"))
        .expect("create broken directory symlink");

    assert!(matches!(
        scan_logseq_graph(temp.path()),
        Err(ScanError::SymlinkNotAllowed { relative_path }) if relative_path == "pages"
    ));
}

#[cfg(unix)]
#[test]
fn selected_graph_root_must_not_be_a_symlink() {
    use std::os::unix::fs::symlink;

    let source = TempDir::new().expect("source temp dir");
    let parent = TempDir::new().expect("parent temp dir");
    let link = parent.path().join("graph");
    symlink(source.path(), &link).expect("create graph symlink");

    assert!(matches!(
        scan_logseq_graph(link),
        Err(ScanError::SymlinkNotAllowed { relative_path }) if relative_path == "."
    ));
}

#[cfg(unix)]
#[test]
fn non_utf8_source_paths_are_rejected_instead_of_lossily_normalized() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let temp = TempDir::new().expect("temp dir");
    fs::create_dir_all(temp.path().join("pages")).expect("create pages");
    let invalid_name = OsString::from_vec(vec![b'n', b'o', b't', 0xff, b'.', b'm', b'd']);
    fs::write(temp.path().join("pages").join(invalid_name), "bytes").expect("write non-UTF8 file");

    assert!(matches!(
        scan_logseq_graph(temp.path()),
        Err(ScanError::NonUtf8Path { .. })
    ));
}

fn copy_tree(source: &std::path::Path, destination: &std::path::Path) {
    for entry in fs::read_dir(source).expect("read source fixture") {
        let entry = entry.expect("fixture entry");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("fixture type").is_dir() {
            fs::create_dir_all(&target).expect("create fixture directory");
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy fixture file");
        }
    }
}

fn graph_with_page(name: &str, content: &[u8]) -> TempDir {
    let temp = TempDir::new().expect("temp dir");
    fs::create_dir_all(temp.path().join("pages")).expect("create pages");
    fs::write(temp.path().join("pages").join(name), content).expect("write page");
    temp
}
