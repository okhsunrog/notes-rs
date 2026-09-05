//! Syntax-aware discovery of tangleaf page and block references in Markdown source.

use std::ops::Range;

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReferenceKind {
    WikiLink,
    BlockReference,
}

impl ReferenceKind {
    fn open(self) -> &'static str {
        match self {
            Self::WikiLink => "[[",
            Self::BlockReference => "((",
        }
    }

    fn close(self) -> &'static str {
        match self {
            Self::WikiLink => "]]",
            Self::BlockReference => "))",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceOccurrence {
    pub kind: ReferenceKind,
    pub source_range: Range<usize>,
    pub target_range: Range<usize>,
}

impl ReferenceOccurrence {
    /// Returns the target from the same source passed to [`scan_references`].
    ///
    /// The scanner guarantees that `target_range` is on UTF-8 boundaries.
    #[must_use]
    pub fn target_text<'source>(&self, source: &'source str) -> &'source str {
        &source[self.target_range.clone()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MalformedReferenceReason {
    EmptyTarget,
    Unclosed,
    Newline,
    NestedDelimiter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MalformedReference {
    pub kind: ReferenceKind,
    pub source_range: Range<usize>,
    pub reason: MalformedReferenceReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReferenceScan {
    pub occurrences: Vec<ReferenceOccurrence>,
    pub malformed: Vec<MalformedReference>,
}

/// Scans references from Markdown source without resolving or deduplicating them.
///
/// Inline/fenced code, raw HTML events, image contents, and link/image destinations are
/// intentionally excluded. Ordinary link labels remain Markdown content. Logseq page embeds
/// (`![[Page]]`) retain their page-reference semantics.
#[must_use]
pub fn scan_references(markdown: &str) -> ReferenceScan {
    let mut scan = ReferenceScan::default();
    let mut excluded = Vec::new();
    let mut link_labels = Vec::new();
    let mut ordinary_link_depth = 0_u32;
    let mut image_depth = 0_u32;
    for (event, range) in Parser::new_ext(markdown, Options::ENABLE_WIKILINKS).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_))
            | Event::Code(_)
            | Event::Html(_)
            | Event::InlineHtml(_) => excluded.push(range),
            Event::Start(Tag::Image { link_type, .. })
                if !matches!(link_type, LinkType::WikiLink { .. }) =>
            {
                excluded.push(range);
                image_depth += 1;
            }
            Event::End(TagEnd::Image) if image_depth > 0 => image_depth -= 1,
            Event::Start(Tag::Link { link_type, .. })
                if !matches!(link_type, LinkType::WikiLink { .. }) =>
            {
                excluded.push(range);
                ordinary_link_depth += 1;
            }
            Event::End(TagEnd::Link) if ordinary_link_depth > 0 => ordinary_link_depth -= 1,
            Event::Text(_) if ordinary_link_depth > 0 && image_depth == 0 => {
                link_labels.push(range);
            }
            _ => {}
        }
    }

    excluded.sort_by_key(|range| (range.start, range.end));
    let excluded = excluded
        .into_iter()
        .fold(Vec::<Range<usize>>::new(), |mut merged, range| {
            if let Some(previous) = merged.last_mut()
                && range.start <= previous.end
            {
                previous.end = previous.end.max(range.end);
            } else {
                merged.push(range);
            }
            merged
        });
    let mut start = 0;
    for range in excluded {
        if start < range.start {
            scan_text_range(markdown, start..range.start, &mut scan);
        }
        start = start.max(range.end);
    }
    if start < markdown.len() {
        scan_text_range(markdown, start..markdown.len(), &mut scan);
    }
    for range in link_labels {
        scan_text_range(markdown, range, &mut scan);
    }

    scan.occurrences.sort_by_key(|reference| {
        (
            reference.source_range.start,
            reference.source_range.end,
            reference.kind,
        )
    });
    scan.malformed.sort_by_key(|reference| {
        (
            reference.source_range.start,
            reference.source_range.end,
            reference.kind,
            reference.reason,
        )
    });
    scan
}

fn scan_text_range(markdown: &str, range: Range<usize>, scan: &mut ReferenceScan) {
    let mut cursor = range.start;
    while cursor + 1 < range.end {
        let Some((start, kind)) = next_opener(markdown, cursor, range.end) else {
            break;
        };
        if is_escaped(markdown.as_bytes(), start) {
            cursor = start + kind.open().len();
            continue;
        }
        let next = scan_candidate(markdown, start, range.end, kind, scan);
        cursor = next.max(start + kind.open().len());
    }
}

fn scan_candidate(
    markdown: &str,
    start: usize,
    end: usize,
    kind: ReferenceKind,
    scan: &mut ReferenceScan,
) -> usize {
    let target_start = start + kind.open().len();
    let rest = &markdown[target_start..end];
    let close = rest.find(kind.close()).map(|offset| target_start + offset);
    let newline = rest.find(['\n', '\r']).map(|offset| target_start + offset);
    let nested = next_opener(markdown, target_start, close.unwrap_or(end));

    if let Some(newline) = newline
        && close.is_none_or(|close| newline < close)
    {
        scan.malformed.push(MalformedReference {
            kind,
            source_range: start..newline,
            reason: MalformedReferenceReason::Newline,
        });
        return newline + markdown[newline..].chars().next().map_or(1, char::len_utf8);
    }

    if let Some((nested_start, nested_kind)) = nested {
        let Some(inner_close) = close else {
            scan.malformed.push(MalformedReference {
                kind,
                source_range: start..nested_start,
                reason: MalformedReferenceReason::Unclosed,
            });
            return nested_start;
        };
        if nested_kind != kind {
            let reference_end = inner_close + kind.close().len();
            scan.malformed.push(MalformedReference {
                kind,
                source_range: start..reference_end,
                reason: MalformedReferenceReason::NestedDelimiter,
            });
            return reference_end;
        }
        let outer_close = markdown[inner_close + kind.close().len()..end]
            .find(kind.close())
            .map(|offset| inner_close + kind.close().len() + offset);
        if let Some(outer_close) = outer_close {
            let reference_end = outer_close + kind.close().len();
            scan.malformed.push(MalformedReference {
                kind,
                source_range: start..reference_end,
                reason: MalformedReferenceReason::NestedDelimiter,
            });
            return reference_end;
        }
        scan.malformed.push(MalformedReference {
            kind,
            source_range: start..nested_start,
            reason: MalformedReferenceReason::Unclosed,
        });
        return nested_start;
    }

    let Some(close) = close else {
        scan.malformed.push(MalformedReference {
            kind,
            source_range: start..end,
            reason: MalformedReferenceReason::Unclosed,
        });
        return target_start;
    };
    let reference_end = close + kind.close().len();
    let inner = &markdown[target_start..close];
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        scan.malformed.push(MalformedReference {
            kind,
            source_range: start..reference_end,
            reason: MalformedReferenceReason::EmptyTarget,
        });
        return reference_end;
    }
    let leading = inner.len() - inner.trim_start().len();
    let target_start = target_start + leading;
    let target_end = target_start + trimmed.len();
    scan.occurrences.push(ReferenceOccurrence {
        kind,
        source_range: start..reference_end,
        target_range: target_start..target_end,
    });
    reference_end
}

fn next_opener(markdown: &str, start: usize, end: usize) -> Option<(usize, ReferenceKind)> {
    let source = &markdown[start..end];
    let wiki = source
        .find("[[")
        .map(|offset| (start + offset, ReferenceKind::WikiLink));
    let block = source
        .find("((")
        .map(|offset| (start + offset, ReferenceKind::BlockReference));
    match (wiki, block) {
        (Some(wiki), Some(block)) => Some(if wiki.0 <= block.0 { wiki } else { block }),
        (Some(wiki), None) => Some(wiki),
        (None, Some(block)) => Some(block),
        (None, None) => None,
    }
}

fn is_escaped(bytes: &[u8], position: usize) -> bool {
    let mut preceding = 0;
    let mut cursor = position;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        preceding += 1;
        cursor -= 1;
    }
    preceding % 2 == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_all_occurrences_without_deduplicating() {
        let markdown = "[[ Roadmap ]] and [[Roadmap]] and ((11111111-1111-4111-8111-111111111111))";
        let scan = scan_references(markdown);

        assert_eq!(scan.occurrences.len(), 3);
        assert_eq!(scan.occurrences[0].target_text(markdown), "Roadmap");
        assert_eq!(scan.occurrences[1].target_text(markdown), "Roadmap");
        assert_eq!(scan.occurrences[2].kind, ReferenceKind::BlockReference);
        assert_eq!(
            &markdown[scan.occurrences[0].source_range.clone()],
            "[[ Roadmap ]]"
        );
        assert_eq!(
            &markdown[scan.occurrences[0].target_range.clone()],
            "Roadmap"
        );
        assert!(scan.malformed.is_empty());
    }

    #[test]
    fn excludes_code_html_link_destinations_and_images() {
        let block = "11111111-1111-4111-8111-111111111111";
        let markdown = format!(
            r#"[[visible]]
`[[inline]]`
```md
[[fenced]]
```
[label (({block}))](<https://example.invalid/[[destination]]>)
![alt](<asset-[[image-destination]].png>)
<span data-ref="[[html]]">ignored</span>
"#
        );
        let scan = scan_references(&markdown);

        assert_eq!(
            scan.occurrences
                .iter()
                .map(|reference| reference.target_text(&markdown))
                .collect::<Vec<_>>(),
            ["visible", block]
        );
    }

    #[test]
    fn ignores_odd_escapes_and_accepts_even_escapes() {
        let markdown = r"\[[ignored]] \\[[kept]] \((ignored)) \\((kept))";
        let scan = scan_references(markdown);

        assert_eq!(
            scan.occurrences
                .iter()
                .map(|reference| reference.target_text(markdown))
                .collect::<Vec<_>>(),
            ["kept", "kept"]
        );
    }

    #[test]
    fn page_embed_retains_page_reference_semantics() {
        let scan = scan_references("![[Embedded page]]");

        assert_eq!(scan.occurrences.len(), 1);
        assert_eq!(
            scan.occurrences[0].target_text("![[Embedded page]]"),
            "Embedded page"
        );
        assert_eq!(scan.occurrences[0].kind, ReferenceKind::WikiLink);
    }

    #[test]
    fn rejects_nested_newline_and_empty_targets() {
        let nested = scan_references("[[outer [[inner]] tail]]");
        assert!(nested.occurrences.is_empty());
        assert_eq!(
            nested.malformed[0].reason,
            MalformedReferenceReason::NestedDelimiter
        );

        let cross_kind = scan_references("[[outer ((11111111-1111-4111-8111-111111111111))]]");
        assert!(cross_kind.occurrences.is_empty());
        assert_eq!(
            cross_kind.malformed[0].reason,
            MalformedReferenceReason::NestedDelimiter
        );

        let multiline = scan_references("[[first\nsecond]]");
        assert!(multiline.occurrences.is_empty());
        assert_eq!(
            multiline.malformed[0].reason,
            MalformedReferenceReason::Newline
        );

        let empty = scan_references("before ((  )) after");
        assert!(empty.occurrences.is_empty());
        assert_eq!(
            empty.malformed[0].reason,
            MalformedReferenceReason::EmptyTarget
        );
    }

    #[test]
    fn recovers_after_an_unclosed_opener() {
        let scan = scan_references("[[broken and [[valid]]");

        assert_eq!(scan.occurrences.len(), 1);
        assert_eq!(
            scan.occurrences[0].target_text("[[broken and [[valid]]"),
            "valid"
        );
        assert_eq!(scan.malformed.len(), 1);
        assert_eq!(scan.malformed[0].reason, MalformedReferenceReason::Unclosed);
    }

    #[test]
    fn excludes_indented_tilde_and_unclosed_fences() {
        let markdown =
            "- [[Visible]]\n  ~~~md\n  [[Tilde fenced]]\n  ~~~\n- ```md\n  [[Unclosed fenced]]\n";
        let scan = scan_references(markdown);

        assert_eq!(scan.occurrences.len(), 1);
        assert_eq!(scan.occurrences[0].target_text(markdown), "Visible");
    }

    #[test]
    fn ranges_remain_utf8_byte_ranges() {
        let markdown = "Привет [[ Страница 🦀 ]]";
        let scan = scan_references(markdown);
        let reference = &scan.occurrences[0];

        assert_eq!(
            &markdown[reference.source_range.clone()],
            "[[ Страница 🦀 ]]"
        );
        assert_eq!(reference.target_text(markdown), "Страница 🦀");
    }
}
