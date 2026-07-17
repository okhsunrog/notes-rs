use std::fmt;

use serde::Serialize;

use crate::{ImportDiagnostic, SourceRange};

/// Source identity discovered from the manifest path. This is not yet the
/// notes-core page identity used by the apply stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogseqDocumentSource {
    Page { file_title: String },
    Journal { date: LogseqJournalDate },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct LogseqJournalDate(String);

impl LogseqJournalDate {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn from_parts(year: u16, month: u8, day: u8) -> Self {
        Self(format!("{year:04}-{month:02}-{day:02}"))
    }
}

impl fmt::Display for LogseqJournalDate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Loss-aware source AST for one Logseq Markdown file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedLogseqDocument {
    pub relative_path: String,
    pub source: LogseqDocumentSource,
    /// Exact validated UTF-8 source, including original line endings.
    pub raw_markdown: String,
    pub preamble: Option<LogseqPreamble>,
    /// Blocks in physical source order. Parent indices address this vector.
    pub blocks: Vec<LogseqSourceBlock>,
    pub constructs: Vec<LogseqConstruct>,
    pub diagnostics: Vec<ImportDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogseqPreamble {
    /// Markdown normalized to `\n` between physical lines.
    pub markdown: String,
    /// Exact preamble bytes as UTF-8 text.
    pub raw_source: String,
    pub source_range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogseqSourceBlock {
    pub index: u64,
    pub parent_index: Option<u64>,
    pub sibling_index: u64,
    pub depth: u64,
    /// Exact whitespace before the structural `-` marker.
    pub indentation: String,
    /// Deterministic visual indentation width; a tab is two columns.
    pub indentation_columns: u64,
    /// Structural marker removed; continuation indentation normalized.
    pub markdown: String,
    /// Exact source fragment owned by this block, including line endings.
    pub raw_source: String,
    pub source_range: SourceRange,
    pub content_range: SourceRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogseqConstructOwner {
    Preamble,
    Block { index: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogseqConstructKind {
    Heading {
        level: u8,
    },
    Property {
        name: String,
        /// Trimmed semantic value used by the identity/reference mapping stage.
        value: String,
        /// Exact range of `value`; empty values use a zero-width range.
        value_range: SourceRange,
    },
    TableRow,
    Latex,
    Macro,
    Logbook,
    FenceStart {
        marker: String,
        info: Option<String>,
    },
    FenceEnd {
        marker: String,
    },
    TaskMarker {
        marker: String,
    },
    UnorderedListItem,
    OrderedListItem,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogseqConstruct {
    pub owner: LogseqConstructOwner,
    pub kind: LogseqConstructKind,
    pub source_range: SourceRange,
}
