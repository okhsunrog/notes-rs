use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    DiagnosticCode, DocumentFormat, FileNameFormat, ImportDiagnostic, LogseqConfig,
    LogseqConstruct, LogseqConstructKind, LogseqConstructOwner, LogseqDocumentSource,
    LogseqJournalDate, LogseqPreamble, LogseqSourceBlock, ManifestEntry, ParsedLogseqDocument,
    Sha256Digest, SourceKind, SourcePosition, SourceRange,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogseqParseErrorCode {
    UnsupportedEntryKind,
    UnsupportedDocumentFormat,
    SourceSizeMismatch,
    SourceHashMismatch,
    InvalidUtf8,
    EntryOutsideConfiguredDirectory,
    InvalidPageFilename,
    EmptyPageTitle,
    InvalidJournalFilename,
}

#[derive(Debug, Error)]
pub enum LogseqParseError {
    #[error("manifest entry {relative_path} is not a page or journal: {kind:?}")]
    UnsupportedEntryKind {
        relative_path: String,
        kind: SourceKind,
    },
    #[error("manifest entry {relative_path} is not Markdown")]
    UnsupportedDocumentFormat { relative_path: String },
    #[error(
        "source size changed for {relative_path}: manifest has {expected_bytes} bytes, parser received {actual_bytes}"
    )]
    SourceSizeMismatch {
        relative_path: String,
        expected_bytes: u64,
        actual_bytes: u64,
    },
    #[error("source hash changed for {relative_path}")]
    SourceHashMismatch { relative_path: String },
    #[error("source {relative_path} is not valid UTF-8 at byte {valid_up_to}")]
    InvalidUtf8 {
        relative_path: String,
        valid_up_to: u64,
    },
    #[error("{kind:?} entry {relative_path} is outside configured directory {directory}")]
    EntryOutsideConfiguredDirectory {
        relative_path: String,
        kind: SourceKind,
        directory: String,
    },
    #[error("page source has an invalid Markdown filename: {relative_path}")]
    InvalidPageFilename { relative_path: String },
    #[error("page filename decodes to an empty title: {relative_path}")]
    EmptyPageTitle { relative_path: String },
    #[error(
        "journal source must be a direct YYYY_MM_DD.md file with a valid date: {relative_path}"
    )]
    InvalidJournalFilename { relative_path: String },
}

impl LogseqParseError {
    pub const fn code(&self) -> LogseqParseErrorCode {
        match self {
            Self::UnsupportedEntryKind { .. } => LogseqParseErrorCode::UnsupportedEntryKind,
            Self::UnsupportedDocumentFormat { .. } => {
                LogseqParseErrorCode::UnsupportedDocumentFormat
            }
            Self::SourceSizeMismatch { .. } => LogseqParseErrorCode::SourceSizeMismatch,
            Self::SourceHashMismatch { .. } => LogseqParseErrorCode::SourceHashMismatch,
            Self::InvalidUtf8 { .. } => LogseqParseErrorCode::InvalidUtf8,
            Self::EntryOutsideConfiguredDirectory { .. } => {
                LogseqParseErrorCode::EntryOutsideConfiguredDirectory
            }
            Self::InvalidPageFilename { .. } => LogseqParseErrorCode::InvalidPageFilename,
            Self::EmptyPageTitle { .. } => LogseqParseErrorCode::EmptyPageTitle,
            Self::InvalidJournalFilename { .. } => LogseqParseErrorCode::InvalidJournalFilename,
        }
    }
}

/// Parse exact bytes represented by a page or journal manifest entry.
///
/// Size and SHA-256 are checked before UTF-8 parsing, so a dry-run cannot parse
/// bytes different from the immutable scanner manifest.
pub fn parse_logseq_markdown(
    entry: &ManifestEntry,
    bytes: &[u8],
    config: &LogseqConfig,
) -> Result<ParsedLogseqDocument, LogseqParseError> {
    if !matches!(entry.kind, SourceKind::Page | SourceKind::Journal) {
        return Err(LogseqParseError::UnsupportedEntryKind {
            relative_path: entry.relative_path.clone(),
            kind: entry.kind,
        });
    }
    if entry.document_format != Some(DocumentFormat::Markdown) {
        return Err(LogseqParseError::UnsupportedDocumentFormat {
            relative_path: entry.relative_path.clone(),
        });
    }
    if bytes.len() as u64 != entry.size_bytes {
        return Err(LogseqParseError::SourceSizeMismatch {
            relative_path: entry.relative_path.clone(),
            expected_bytes: entry.size_bytes,
            actual_bytes: bytes.len() as u64,
        });
    }
    let digest = Sha256Digest::from_bytes(Sha256::digest(bytes).into());
    if digest != entry.sha256 {
        return Err(LogseqParseError::SourceHashMismatch {
            relative_path: entry.relative_path.clone(),
        });
    }
    let markdown = std::str::from_utf8(bytes).map_err(|error| LogseqParseError::InvalidUtf8 {
        relative_path: entry.relative_path.clone(),
        valid_up_to: error.valid_up_to() as u64,
    })?;
    let source = document_source(entry, config)?;
    Ok(SourceParser::new(&entry.relative_path, markdown).parse(source))
}

/// Decode a Logseq page filename stem with the configured filename scheme.
pub fn decode_logseq_page_title(file_stem: &str, format: FileNameFormat) -> String {
    match format {
        FileNameFormat::TripleLowbar => {
            let namespaced = file_stem.replace("___", "/");
            let decoded = decode_ascii_percent_triplets(&namespaced);
            decoded
                .split('/')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("/")
        }
        FileNameFormat::Legacy => {
            let namespaced = file_stem.replace('.', "/");
            decode_uri_component(&namespaced).unwrap_or(namespaced)
        }
    }
}

fn document_source(
    entry: &ManifestEntry,
    config: &LogseqConfig,
) -> Result<LogseqDocumentSource, LogseqParseError> {
    match entry.kind {
        SourceKind::Page => {
            let remainder = path_below_directory(
                &entry.relative_path,
                config.pages_directory.as_str(),
                entry.kind,
            )?;
            let filename = remainder
                .rsplit('/')
                .next()
                .filter(|filename| !filename.is_empty())
                .ok_or_else(|| LogseqParseError::InvalidPageFilename {
                    relative_path: entry.relative_path.clone(),
                })?;
            let file_stem = markdown_file_stem(filename).ok_or_else(|| {
                LogseqParseError::InvalidPageFilename {
                    relative_path: entry.relative_path.clone(),
                }
            })?;
            let file_title = decode_logseq_page_title(file_stem, config.file_name_format);
            if file_title.is_empty() {
                return Err(LogseqParseError::EmptyPageTitle {
                    relative_path: entry.relative_path.clone(),
                });
            }
            Ok(LogseqDocumentSource::Page { file_title })
        }
        SourceKind::Journal => {
            let remainder = path_below_directory(
                &entry.relative_path,
                config.journals_directory.as_str(),
                entry.kind,
            )?;
            if remainder.contains('/') {
                return Err(LogseqParseError::InvalidJournalFilename {
                    relative_path: entry.relative_path.clone(),
                });
            }
            let date = parse_journal_filename(remainder).ok_or_else(|| {
                LogseqParseError::InvalidJournalFilename {
                    relative_path: entry.relative_path.clone(),
                }
            })?;
            Ok(LogseqDocumentSource::Journal { date })
        }
        SourceKind::Config | SourceKind::Asset | SourceKind::Drawing => {
            Err(LogseqParseError::UnsupportedEntryKind {
                relative_path: entry.relative_path.clone(),
                kind: entry.kind,
            })
        }
    }
}

fn path_below_directory<'path>(
    relative_path: &'path str,
    directory: &str,
    kind: SourceKind,
) -> Result<&'path str, LogseqParseError> {
    let remainder = relative_path
        .strip_prefix(directory)
        .and_then(|remainder| remainder.strip_prefix('/'))
        .filter(|remainder| !remainder.is_empty());
    remainder.ok_or_else(|| LogseqParseError::EntryOutsideConfiguredDirectory {
        relative_path: relative_path.to_owned(),
        kind,
        directory: directory.to_owned(),
    })
}

fn markdown_file_stem(filename: &str) -> Option<&str> {
    let (stem, extension) = filename.rsplit_once('.')?;
    (!stem.is_empty()
        && (extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")))
    .then_some(stem)
}

fn parse_journal_filename(filename: &str) -> Option<LogseqJournalDate> {
    let stem = filename.strip_suffix(".md")?;
    let bytes = stem.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'_'
        || bytes[7] != b'_'
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        return None;
    }
    let year = parse_ascii_u16(&bytes[0..4])?;
    let month = parse_ascii_u8(&bytes[5..7])?;
    let day = parse_ascii_u8(&bytes[8..10])?;
    if year == 0 || !(1..=12).contains(&month) || !(1..=days_in_month(year, month)).contains(&day) {
        return None;
    }
    Some(LogseqJournalDate::from_parts(year, month, day))
}

fn parse_ascii_u16(bytes: &[u8]) -> Option<u16> {
    bytes.iter().try_fold(0_u16, |value, byte| {
        value
            .checked_mul(10)?
            .checked_add(u16::from(byte.checked_sub(b'0')?))
    })
}

fn parse_ascii_u8(bytes: &[u8]) -> Option<u8> {
    bytes.iter().try_fold(0_u8, |value, byte| {
        value.checked_mul(10)?.checked_add(byte.checked_sub(b'0')?)
    })
}

const fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

const fn is_leap_year(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn decode_ascii_percent_triplets(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len());
    let mut position = 0;
    while position < bytes.len() {
        if bytes[position] == b'%'
            && position + 2 < bytes.len()
            && let (Some(high), Some(low)) = (
                hex_value(bytes[position + 1]),
                hex_value(bytes[position + 2]),
            )
        {
            let decoded = (high << 4) | low;
            if decoded.is_ascii() {
                output.push(char::from(decoded));
            } else {
                output.push_str(&value[position..position + 3]);
            }
            position += 3;
            continue;
        }
        let character = value[position..]
            .chars()
            .next()
            .expect("position is inside the string");
        output.push(character);
        position += character.len_utf8();
    }
    output
}

fn decode_uri_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut position = 0;
    while position < bytes.len() {
        if bytes[position] == b'%' {
            let high = hex_value(*bytes.get(position + 1)?)?;
            let low = hex_value(*bytes.get(position + 2)?)?;
            decoded.push((high << 4) | low);
            position += 3;
        } else {
            let character = value[position..].chars().next()?;
            let mut buffer = [0_u8; 4];
            decoded.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            position += character.len_utf8();
        }
    }
    String::from_utf8(decoded).ok()
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

struct SourceParser<'source> {
    relative_path: &'source str,
    source: &'source str,
    preamble: Option<PreambleBuilder>,
    blocks: Vec<BlockBuilder>,
    constructs: Vec<LogseqConstruct>,
    diagnostics: Vec<ImportDiagnostic>,
    ancestors: Vec<Ancestor>,
    sibling_counts: BTreeMap<Option<u64>, u64>,
    indentation_style: Option<IndentationStyle>,
    fence: Option<FenceState>,
}

impl<'source> SourceParser<'source> {
    fn new(relative_path: &'source str, source: &'source str) -> Self {
        Self {
            relative_path,
            source,
            preamble: None,
            blocks: Vec::new(),
            constructs: Vec::new(),
            diagnostics: Vec::new(),
            ancestors: Vec::new(),
            sibling_counts: BTreeMap::new(),
            indentation_style: None,
            fence: None,
        }
    }

    fn parse(mut self, document_source: LogseqDocumentSource) -> ParsedLogseqDocument {
        for line in PhysicalLines::new(self.source) {
            self.parse_line(line);
        }
        if let Some(fence) = self.fence.take() {
            self.diagnostics.push(ImportDiagnostic::warning_at(
                DiagnosticCode::UnclosedFence,
                self.relative_path,
                fence.opening_range,
                "Fenced code block is not closed; its source was preserved verbatim",
                Some("Close the Markdown fence before importing if this was accidental".to_owned()),
            ));
        }
        self.constructs.sort_by_key(|construct| {
            (
                construct.source_range.start.byte_offset,
                construct.source_range.end.byte_offset,
            )
        });
        self.diagnostics.sort_by(|left, right| {
            left.range
                .map(|range| range.start.byte_offset)
                .cmp(&right.range.map(|range| range.start.byte_offset))
                .then(left.code.cmp(&right.code))
        });

        ParsedLogseqDocument {
            relative_path: self.relative_path.to_owned(),
            source: document_source,
            raw_markdown: self.source.to_owned(),
            preamble: self.preamble.map(PreambleBuilder::finish),
            blocks: self.blocks.into_iter().map(BlockBuilder::finish).collect(),
            constructs: self.constructs,
            diagnostics: self.diagnostics,
        }
    }

    fn parse_line(&mut self, line: PhysicalLine<'source>) {
        if let Some(fence) = self.fence.clone() {
            let logical = self.append_continuation_or_preamble(line);
            if let Some(closing) = closing_fence(logical.text, &fence) {
                self.constructs.push(LogseqConstruct {
                    owner: logical.owner,
                    kind: LogseqConstructKind::FenceEnd {
                        marker: fence.marker.to_string().repeat(closing.length),
                    },
                    source_range: line_range(
                        self.source,
                        &line,
                        logical.absolute_start + closing.offset,
                        logical.absolute_start + closing.offset + closing.length,
                    ),
                });
                self.fence = None;
            }
            return;
        }

        let (syntax_text, source_prefix_bytes) = line_syntax(&line);
        let logical = if let Some(structural) = structural_bullet(syntax_text) {
            self.start_block(line, structural, source_prefix_bytes)
        } else {
            self.append_continuation_or_preamble(line)
        };

        if let Some(opening) = opening_fence(logical.text) {
            let range = line_range(
                self.source,
                &line,
                logical.absolute_start + opening.offset,
                logical.absolute_start + opening.offset + opening.length,
            );
            self.constructs.push(LogseqConstruct {
                owner: logical.owner,
                kind: LogseqConstructKind::FenceStart {
                    marker: opening.marker.to_string().repeat(opening.length),
                    info: (!opening.info.is_empty()).then(|| opening.info.to_owned()),
                },
                source_range: range,
            });
            self.fence = Some(FenceState {
                marker: opening.marker,
                length: opening.length,
                opening_range: range,
            });
        } else {
            self.collect_line_constructs(&line, logical);
        }
    }

    fn start_block(
        &mut self,
        line: PhysicalLine<'source>,
        structural: StructuralBullet<'source>,
        source_prefix_bytes: usize,
    ) -> LogicalLine<'source> {
        let indentation_columns = indentation_columns(structural.indentation);
        self.report_indentation(
            &line,
            line.start + source_prefix_bytes,
            structural.indentation,
            indentation_columns,
        );

        while self
            .ancestors
            .last()
            .is_some_and(|ancestor| ancestor.indentation_columns >= indentation_columns)
        {
            self.ancestors.pop();
        }
        let parent = self.ancestors.last().copied();
        if indentation_columns > 0 && parent.is_none() {
            self.diagnostics.push(ImportDiagnostic::warning_at(
                DiagnosticCode::NonCanonicalIndentation,
                self.relative_path,
                line_range(
                    self.source,
                    &line,
                    line.start + source_prefix_bytes,
                    line.start + source_prefix_bytes + structural.indentation.len(),
                ),
                "Indented block has no preceding less-indented parent; it remains a root block",
                Some("Align the block with a parent or remove its leading indentation".to_owned()),
            ));
        }
        let parent_index = parent.map(|ancestor| ancestor.block_index);
        let depth = parent.map_or(0, |ancestor| ancestor.depth + 1);
        let sibling_index = self.sibling_counts.entry(parent_index).or_default();
        let current_sibling = *sibling_index;
        *sibling_index += 1;
        let index = self.blocks.len() as u64;

        let source_range = range_to_raw_end(self.source, &line, line.start);
        let content_start = line.start + source_prefix_bytes + structural.body_offset;
        let content_range = range_to_raw_end(self.source, &line, content_start);
        self.blocks.push(BlockBuilder {
            index,
            parent_index,
            sibling_index: current_sibling,
            depth,
            indentation: structural.indentation.to_owned(),
            indentation_columns,
            markdown_lines: vec![structural.body.to_owned()],
            raw_source: self.source[line.start..line.end].to_owned(),
            source_range,
            content_range,
        });
        self.ancestors.push(Ancestor {
            indentation_columns,
            block_index: index,
            depth,
        });

        LogicalLine {
            owner: LogseqConstructOwner::Block { index },
            text: structural.body,
            absolute_start: content_start,
            block_head: true,
        }
    }

    fn append_continuation_or_preamble(
        &mut self,
        line: PhysicalLine<'source>,
    ) -> LogicalLine<'source> {
        if self.blocks.last().is_some() {
            let inside_fence = self.fence.is_some();
            let (logical, warn_noncanonical) = {
                let block = self.blocks.last_mut().expect("checked as present");
                let normalized = normalize_continuation(line.text, &block.indentation);
                let warn_noncanonical = !inside_fence
                    && !block.indentation.is_empty()
                    && !line.text.trim().is_empty()
                    && !normalized.canonical;
                block.markdown_lines.push(normalized.text.to_owned());
                block
                    .raw_source
                    .push_str(&self.source[line.start..line.end]);
                block.source_range.end = raw_end_position(self.source, &line);
                block.content_range.end = raw_end_position(self.source, &line);
                (
                    LogicalLine {
                        owner: LogseqConstructOwner::Block { index: block.index },
                        text: normalized.text,
                        absolute_start: line.start + normalized.stripped_bytes,
                        block_head: false,
                    },
                    warn_noncanonical,
                )
            };
            if warn_noncanonical {
                self.diagnostics.push(ImportDiagnostic::warning_at(
                    DiagnosticCode::NonCanonicalContinuationIndentation,
                    self.relative_path,
                    leading_source_range(self.source, &line),
                    "Continuation line does not use its block indentation followed by two spaces; source was preserved and normalized conservatively",
                    Some("Indent continuation lines with their block prefix followed by two spaces".to_owned()),
                ));
            }
            logical
        } else {
            let (syntax_text, source_prefix_bytes) = line_syntax(&line);
            let preamble = self.preamble.get_or_insert_with(|| PreambleBuilder {
                markdown_lines: Vec::new(),
                raw_source: String::new(),
                source_range: range_to_raw_end(self.source, &line, line.start),
            });
            preamble.markdown_lines.push(syntax_text.to_owned());
            preamble
                .raw_source
                .push_str(&self.source[line.start..line.end]);
            preamble.source_range.end = raw_end_position(self.source, &line);
            LogicalLine {
                owner: LogseqConstructOwner::Preamble,
                text: syntax_text,
                absolute_start: line.start + source_prefix_bytes,
                block_head: false,
            }
        }
    }

    fn report_indentation(
        &mut self,
        line: &PhysicalLine<'source>,
        syntax_start: usize,
        indentation: &str,
        columns: u64,
    ) {
        let Some(style) = indentation_style(indentation) else {
            return;
        };
        let range = line_range(
            self.source,
            line,
            syntax_start,
            syntax_start + indentation.len(),
        );
        if style == IndentationStyle::Mixed
            || self
                .indentation_style
                .is_some_and(|existing| existing != style)
        {
            self.diagnostics.push(ImportDiagnostic::warning_at(
                DiagnosticCode::MixedIndentation,
                self.relative_path,
                range,
                "Block indentation mixes tabs and spaces; deterministic source nesting was retained",
                Some("Use either tabs or pairs of spaces consistently for structural blocks".to_owned()),
            ));
        }
        if style != IndentationStyle::Mixed {
            self.indentation_style.get_or_insert(style);
        }
        if indentation.bytes().all(|byte| byte == b' ') && !columns.is_multiple_of(2) {
            self.diagnostics.push(ImportDiagnostic::warning_at(
                DiagnosticCode::NonCanonicalIndentation,
                self.relative_path,
                range,
                "Structural block uses an odd number of spaces for indentation",
                Some("Use tabs or two spaces per indentation level".to_owned()),
            ));
        }
    }

    fn collect_line_constructs(
        &mut self,
        line: &PhysicalLine<'source>,
        logical: LogicalLine<'source>,
    ) {
        let leading = leading_whitespace_bytes(logical.text);
        let trimmed = &logical.text[leading..];
        let trimmed_start = logical.absolute_start + leading;

        if let Some(level) = heading_level(trimmed) {
            self.push_construct(
                logical.owner,
                LogseqConstructKind::Heading { level },
                line,
                trimmed_start,
                trimmed_start + usize::from(level),
            );
        }
        if let Some(property) = property_parts(trimmed) {
            let value_range = line_range(
                self.source,
                line,
                trimmed_start + property.value_start,
                trimmed_start + property.value_end,
            );
            self.push_construct(
                logical.owner,
                LogseqConstructKind::Property {
                    name: property.name.to_owned(),
                    value: property.value.to_owned(),
                    value_range,
                },
                line,
                trimmed_start,
                logical.absolute_start + logical.text.len(),
            );
        }
        if trimmed.starts_with('|') {
            self.push_construct(
                logical.owner,
                LogseqConstructKind::TableRow,
                line,
                trimmed_start,
                logical.absolute_start + logical.text.len(),
            );
        }
        if let Some(offset) = logical.text.find("$$") {
            self.push_construct(
                logical.owner,
                LogseqConstructKind::Latex,
                line,
                logical.absolute_start + offset,
                logical.absolute_start + offset + 2,
            );
        }
        if is_logbook_line(trimmed) {
            self.push_construct(
                logical.owner,
                LogseqConstructKind::Logbook,
                line,
                trimmed_start,
                logical.absolute_start + logical.text.len(),
            );
        }
        if logical.block_head
            && let Some(marker) = task_marker(trimmed)
        {
            self.push_construct(
                logical.owner,
                LogseqConstructKind::TaskMarker {
                    marker: marker.to_owned(),
                },
                line,
                trimmed_start,
                trimmed_start + marker.len(),
            );
        }
        if unordered_list_marker(trimmed).is_some() {
            self.push_construct(
                logical.owner,
                LogseqConstructKind::UnorderedListItem,
                line,
                trimmed_start,
                trimmed_start + 1,
            );
        } else if let Some(marker_length) = ordered_list_marker_length(trimmed) {
            self.push_construct(
                logical.owner,
                LogseqConstructKind::OrderedListItem,
                line,
                trimmed_start,
                trimmed_start + marker_length,
            );
        }

        let mut remaining = logical.text;
        let mut consumed_bytes = 0;
        let mut remaining_column = self.source[line.start..logical.absolute_start]
            .chars()
            .count() as u64
            + 1;
        while let Some(start) = remaining.find("{{") {
            let after_start = start + 2;
            let Some(end) = remaining[after_start..].find("}}") else {
                break;
            };
            let end = after_start + end + 2;
            let absolute_start = logical.absolute_start + consumed_bytes + start;
            let absolute_end = logical.absolute_start + consumed_bytes + end;
            let start_column = remaining_column + remaining[..start].chars().count() as u64;
            let end_column = start_column + remaining[start..end].chars().count() as u64;
            let range = SourceRange {
                start: SourcePosition {
                    line: line.number,
                    column: start_column,
                    byte_offset: absolute_start as u64,
                },
                end: SourcePosition {
                    line: line.number,
                    column: end_column,
                    byte_offset: absolute_end as u64,
                },
            };
            self.constructs.push(LogseqConstruct {
                owner: logical.owner,
                kind: LogseqConstructKind::Macro,
                source_range: range,
            });
            self.diagnostics.push(ImportDiagnostic::warning_at(
                DiagnosticCode::PreservedMacro,
                self.relative_path,
                range,
                "Logseq macro is preserved as source syntax for a later typed conversion stage",
                None,
            ));
            consumed_bytes += end;
            remaining_column = end_column;
            remaining = &remaining[end..];
        }
    }

    fn push_construct(
        &mut self,
        owner: LogseqConstructOwner,
        kind: LogseqConstructKind,
        line: &PhysicalLine<'source>,
        start: usize,
        end: usize,
    ) {
        self.constructs.push(LogseqConstruct {
            owner,
            kind,
            source_range: line_range(self.source, line, start, end),
        });
    }
}

#[derive(Debug)]
struct PreambleBuilder {
    markdown_lines: Vec<String>,
    raw_source: String,
    source_range: SourceRange,
}

impl PreambleBuilder {
    fn finish(self) -> LogseqPreamble {
        LogseqPreamble {
            markdown: self.markdown_lines.join("\n"),
            raw_source: self.raw_source,
            source_range: self.source_range,
        }
    }
}

#[derive(Debug)]
struct BlockBuilder {
    index: u64,
    parent_index: Option<u64>,
    sibling_index: u64,
    depth: u64,
    indentation: String,
    indentation_columns: u64,
    markdown_lines: Vec<String>,
    raw_source: String,
    source_range: SourceRange,
    content_range: SourceRange,
}

impl BlockBuilder {
    fn finish(self) -> LogseqSourceBlock {
        LogseqSourceBlock {
            index: self.index,
            parent_index: self.parent_index,
            sibling_index: self.sibling_index,
            depth: self.depth,
            indentation: self.indentation,
            indentation_columns: self.indentation_columns,
            markdown: self.markdown_lines.join("\n"),
            raw_source: self.raw_source,
            source_range: self.source_range,
            content_range: self.content_range,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Ancestor {
    indentation_columns: u64,
    block_index: u64,
    depth: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndentationStyle {
    Tabs,
    Spaces,
    Mixed,
}

#[derive(Debug, Clone)]
struct FenceState {
    marker: char,
    length: usize,
    opening_range: SourceRange,
}

#[derive(Debug, Clone, Copy)]
struct LogicalLine<'source> {
    owner: LogseqConstructOwner,
    text: &'source str,
    absolute_start: usize,
    block_head: bool,
}

#[derive(Debug, Clone, Copy)]
struct StructuralBullet<'source> {
    indentation: &'source str,
    body: &'source str,
    body_offset: usize,
}

#[derive(Debug, Clone, Copy)]
struct NormalizedContinuation<'source> {
    text: &'source str,
    stripped_bytes: usize,
    canonical: bool,
}

#[derive(Debug, Clone, Copy)]
struct FenceOpening<'source> {
    marker: char,
    length: usize,
    offset: usize,
    info: &'source str,
}

#[derive(Debug, Clone, Copy)]
struct FenceClosing {
    offset: usize,
    length: usize,
}

#[derive(Debug, Clone, Copy)]
struct PropertyParts<'source> {
    name: &'source str,
    value: &'source str,
    value_start: usize,
    value_end: usize,
}

#[derive(Debug, Clone, Copy)]
struct PhysicalLine<'source> {
    number: u64,
    start: usize,
    content_end: usize,
    end: usize,
    text: &'source str,
}

struct PhysicalLines<'source> {
    source: &'source str,
    position: usize,
    line_number: u64,
}

impl<'source> PhysicalLines<'source> {
    const fn new(source: &'source str) -> Self {
        Self {
            source,
            position: 0,
            line_number: 1,
        }
    }
}

impl<'source> Iterator for PhysicalLines<'source> {
    type Item = PhysicalLine<'source>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.position >= self.source.len() {
            return None;
        }
        let start = self.position;
        let newline = self.source.as_bytes()[start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|offset| start + offset);
        let end = newline.map_or(self.source.len(), |newline| newline + 1);
        let mut content_end = newline.unwrap_or(end);
        if content_end > start && self.source.as_bytes()[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        let line = PhysicalLine {
            number: self.line_number,
            start,
            content_end,
            end,
            text: &self.source[start..content_end],
        };
        self.position = end;
        self.line_number += 1;
        Some(line)
    }
}

fn line_syntax<'source>(line: &PhysicalLine<'source>) -> (&'source str, usize) {
    if line.start == 0
        && let Some(without_bom) = line.text.strip_prefix('\u{feff}')
    {
        return (without_bom, '\u{feff}'.len_utf8());
    }
    (line.text, 0)
}

fn structural_bullet(line: &str) -> Option<StructuralBullet<'_>> {
    let indentation_length = leading_whitespace_bytes(line);
    let remainder = &line[indentation_length..];
    let after_marker = remainder.strip_prefix('-')?;
    let padding = after_marker
        .as_bytes()
        .first()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t')) as usize;
    if !after_marker.is_empty() && padding == 0 {
        return None;
    }
    Some(StructuralBullet {
        indentation: &line[..indentation_length],
        body: &after_marker[padding..],
        body_offset: indentation_length + 1 + padding,
    })
}

fn normalize_continuation<'line>(
    line: &'line str,
    block_indentation: &str,
) -> NormalizedContinuation<'line> {
    if let Some(remainder) = line.strip_prefix(block_indentation) {
        if let Some(remainder) = remainder.strip_prefix("  ") {
            return NormalizedContinuation {
                text: remainder,
                stripped_bytes: block_indentation.len() + 2,
                canonical: true,
            };
        }
        return NormalizedContinuation {
            text: remainder,
            stripped_bytes: block_indentation.len(),
            canonical: false,
        };
    }
    NormalizedContinuation {
        text: line,
        stripped_bytes: 0,
        canonical: false,
    }
}

fn indentation_columns(indentation: &str) -> u64 {
    indentation.bytes().fold(0_u64, |columns, byte| {
        columns.saturating_add(if byte == b'\t' { 2 } else { 1 })
    })
}

fn indentation_style(indentation: &str) -> Option<IndentationStyle> {
    let tabs = indentation.as_bytes().contains(&b'\t');
    let spaces = indentation.as_bytes().contains(&b' ');
    match (tabs, spaces) {
        (false, false) => None,
        (true, false) => Some(IndentationStyle::Tabs),
        (false, true) => Some(IndentationStyle::Spaces),
        (true, true) => Some(IndentationStyle::Mixed),
    }
}

fn leading_whitespace_bytes(line: &str) -> usize {
    line.as_bytes()
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

fn opening_fence(line: &str) -> Option<FenceOpening<'_>> {
    let offset = commonmark_fence_indentation(line)?;
    let trimmed = &line[offset..];
    let marker = trimmed.chars().next()?;
    if !matches!(marker, '`' | '~') {
        return None;
    }
    let length = trimmed
        .bytes()
        .take_while(|byte| *byte == marker as u8)
        .count();
    if length < 3 {
        return None;
    }
    let info = trimmed[length..].trim();
    if marker == '`' && info.contains('`') {
        return None;
    }
    Some(FenceOpening {
        marker,
        length,
        offset,
        info,
    })
}

fn closing_fence(line: &str, fence: &FenceState) -> Option<FenceClosing> {
    let offset = commonmark_fence_indentation(line)?;
    let trimmed = &line[offset..];
    let count = trimmed
        .bytes()
        .take_while(|byte| *byte == fence.marker as u8)
        .count();
    (count >= fence.length && trimmed[count..].trim().is_empty()).then_some(FenceClosing {
        offset,
        length: count,
    })
}

fn commonmark_fence_indentation(line: &str) -> Option<usize> {
    let spaces = line.bytes().take_while(|byte| *byte == b' ').count();
    (spaces <= 3).then_some(spaces)
}

fn heading_level(line: &str) -> Option<u8> {
    let level = line.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&level)
        || !line
            .as_bytes()
            .get(level)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        return None;
    }
    Some(level as u8)
}

fn property_parts(line: &str) -> Option<PropertyParts<'_>> {
    let delimiter = line.find("::")?;
    let name = line[..delimiter].trim_end();
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return None;
    }
    let raw_value = &line[delimiter + 2..];
    let value = raw_value.trim();
    let value_start = delimiter + 2 + raw_value.len() - raw_value.trim_start().len();
    let value_end = value_start + value.len();
    Some(PropertyParts {
        name,
        value,
        value_start,
        value_end,
    })
}

fn is_logbook_line(line: &str) -> bool {
    matches!(line, ":LOGBOOK:" | ":END:") || line.starts_with("CLOCK:")
}

fn task_marker(line: &str) -> Option<&str> {
    let marker = line.split_whitespace().next()?;
    matches!(
        marker,
        "TODO"
            | "DOING"
            | "NOW"
            | "LATER"
            | "DONE"
            | "WAIT"
            | "WAITING"
            | "CANCELLED"
            | "CANCELED"
            | "IN-PROGRESS"
    )
    .then_some(marker)
}

fn unordered_list_marker(line: &str) -> Option<char> {
    let mut characters = line.chars();
    let marker = characters.next()?;
    (matches!(marker, '-' | '*' | '+') && characters.next().is_some_and(char::is_whitespace))
        .then_some(marker)
}

fn ordered_list_marker_length(line: &str) -> Option<usize> {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0
        && line.as_bytes().get(digits) == Some(&b'.')
        && line
            .as_bytes()
            .get(digits + 1)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t')))
    .then_some(digits + 1)
}

fn line_range(source: &str, line: &PhysicalLine<'_>, start: usize, end: usize) -> SourceRange {
    SourceRange {
        start: position_in_line(source, line, start),
        end: position_in_line(source, line, end),
    }
}

fn leading_source_range(source: &str, line: &PhysicalLine<'_>) -> SourceRange {
    let whitespace = leading_whitespace_bytes(line.text);
    let end = if whitespace > 0 {
        line.start + whitespace
    } else {
        line.start + line.text.chars().next().map_or(0, char::len_utf8)
    };
    line_range(source, line, line.start, end)
}

fn range_to_raw_end(source: &str, line: &PhysicalLine<'_>, start: usize) -> SourceRange {
    SourceRange {
        start: position_in_line(source, line, start),
        end: raw_end_position(source, line),
    }
}

fn position_in_line(source: &str, line: &PhysicalLine<'_>, offset: usize) -> SourcePosition {
    let clamped = offset.clamp(line.start, line.content_end);
    SourcePosition {
        line: line.number,
        column: source[line.start..clamped].chars().count() as u64 + 1,
        byte_offset: clamped as u64,
    }
}

fn raw_end_position(source: &str, line: &PhysicalLine<'_>) -> SourcePosition {
    if line.end > line.content_end {
        SourcePosition {
            line: line.number + 1,
            column: 1,
            byte_offset: line.end as u64,
        }
    } else {
        position_in_line(source, line, line.content_end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triple_lowbar_decoding_matches_logseq_examples() {
        assert_eq!(
            decode_logseq_page_title("project___api%2Fv1%20notes", FileNameFormat::TripleLowbar),
            "project/api/v1 notes"
        );
        assert_eq!(
            decode_logseq_page_title("abc%25%2Fdef", FileNameFormat::TripleLowbar),
            "abc%/def"
        );
        assert_eq!(
            decode_logseq_page_title("___a______b___", FileNameFormat::TripleLowbar),
            "a/b"
        );
        assert_eq!(
            decode_logseq_page_title("abc%2——ef%2Fghi", FileNameFormat::TripleLowbar),
            "abc%2——ef/ghi"
        );
    }

    #[test]
    fn strict_journal_dates_include_calendar_validation() {
        assert_eq!(
            parse_journal_filename("2024_02_29.md"),
            Some(LogseqJournalDate::from_parts(2024, 2, 29))
        );
        for invalid in [
            "2023_02_29.md",
            "2026_13_01.md",
            "2026_07_1.md",
            "2026-07-17.md",
            "2026_07_17.markdown",
        ] {
            assert_eq!(parse_journal_filename(invalid), None, "{invalid}");
        }
    }
}
