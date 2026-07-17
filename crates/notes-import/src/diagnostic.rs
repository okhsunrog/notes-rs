use serde::{Deserialize, Serialize};

/// Severity of a non-fatal issue discovered while preparing an import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

/// Stable, machine-readable classification for an import diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    ConfigNotFound,
    SourceDirectoryNotFound,
    UnsupportedDocumentFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePosition {
    pub line: u32,
    pub column: u32,
    pub byte_offset: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRange {
    pub start: SourcePosition,
    pub end: SourcePosition,
}

/// A loss-aware diagnostic suitable for a dry-run report.
///
/// `relative_path` is always relative to the selected graph root. Diagnostic
/// messages never contain note bodies or values from unknown Logseq forms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}
