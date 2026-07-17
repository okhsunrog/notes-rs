use serde::{Deserialize, Serialize};

/// Severity of a non-fatal issue discovered while preparing an import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

/// Stable, machine-readable classification for an import diagnostic.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    ConfigNotFound,
    SourceDirectoryNotFound,
    UnsupportedDocumentFormat,
    MixedIndentation,
    NonCanonicalIndentation,
    NonCanonicalContinuationIndentation,
    UnclosedFence,
    PreservedMacro,
    EmptyPageTitle,
    MultiplePageTitles,
    DuplicatePageIdentity,
    DuplicatePageTitle,
    DuplicateJournalDate,
    DuplicateTargetUuid,
    InvalidBlockUuid,
    MultipleBlockIdentityProperties,
    DuplicateBlockUuid,
    UnresolvedPageReference,
    AmbiguousPageReference,
    InvalidBlockReference,
    UnresolvedBlockReference,
    UnsupportedNestedWikilink,
    MissingMediaSource,
    RemoteMediaBlocked,
    UnsafeMediaSource,
    UnsupportedInlineMedia,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SourcePosition {
    /// One-based physical line number.
    pub line: u64,
    /// One-based Unicode scalar column.
    pub column: u64,
    /// Zero-based byte offset in the original UTF-8 source.
    pub byte_offset: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SourceRange {
    pub start: SourcePosition,
    pub end: SourcePosition,
}

/// A loss-aware diagnostic suitable for a dry-run report.
/// `relative_path` is always relative to the selected graph root. Diagnostic
/// messages never contain note bodies or values from unknown Logseq forms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportDiagnostic {
    pub severity: DiagnosticSeverity,
    pub code: DiagnosticCode,
    pub relative_path: Option<String>,
    pub range: Option<SourceRange>,
    pub message: String,
    pub remediation: Option<String>,
}

impl ImportDiagnostic {
    pub(crate) fn error(
        code: DiagnosticCode,
        relative_path: Option<String>,
        message: impl Into<String>,
        remediation: Option<String>,
    ) -> Self {
        Self {
            severity: DiagnosticSeverity::Error,
            code,
            relative_path,
            range: None,
            message: message.into(),
            remediation,
        }
    }

    pub(crate) fn error_at(
        code: DiagnosticCode,
        relative_path: impl Into<String>,
        range: SourceRange,
        message: impl Into<String>,
        remediation: Option<String>,
    ) -> Self {
        Self {
            severity: DiagnosticSeverity::Error,
            code,
            relative_path: Some(relative_path.into()),
            range: Some(range),
            message: message.into(),
            remediation,
        }
    }

    pub(crate) fn warning(
        code: DiagnosticCode,
        relative_path: Option<String>,
        message: impl Into<String>,
        remediation: Option<String>,
    ) -> Self {
        Self {
            severity: DiagnosticSeverity::Warning,
            code,
            relative_path,
            range: None,
            message: message.into(),
            remediation,
        }
    }

    pub(crate) fn warning_at(
        code: DiagnosticCode,
        relative_path: impl Into<String>,
        range: SourceRange,
        message: impl Into<String>,
        remediation: Option<String>,
    ) -> Self {
        Self {
            severity: DiagnosticSeverity::Warning,
            code,
            relative_path: Some(relative_path.into()),
            range: Some(range),
            message: message.into(),
            remediation,
        }
    }
}
