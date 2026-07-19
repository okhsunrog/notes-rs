//! Deterministic provider-input chunking.
//!
//! These are policy constants, not provider limits. The eval harness is the
//! tool for tuning them; keep the values centralized so a measured change is
//! an explicit input-format decision.

pub const SINGLE_VECTOR_MAX_CHARS: usize = 2_000;
pub const CHUNK_BODY_MIN_CHARS: usize = 1_200;
pub const CHUNK_BODY_TARGET_CHARS: usize = 1_400;
pub const CHUNK_BODY_MAX_CHARS: usize = 1_600;
pub const MAX_PROVIDER_INPUT_CHARS: usize = 8_000;
pub const PROVIDER_TRUNCATION_MARKER: &str = "\n[notes-rs: input truncated]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChunk {
    pub chunk_index: u32,
    pub text: String,
}

pub fn split_for_embedding(full_text: &str, header: &str) -> Vec<TextChunk> {
    if full_text.chars().count() <= SINGLE_VECTOR_MAX_CHARS {
        return vec![TextChunk {
            chunk_index: 0,
            text: cap_provider_input(full_text),
        }];
    }

    let body = header
        .is_empty()
        .then_some(full_text)
        .or_else(|| full_text.strip_prefix(header)?.strip_prefix('\n'))
        .unwrap_or(full_text);
    split_body_with_limits(
        body,
        CHUNK_BODY_MIN_CHARS,
        CHUNK_BODY_TARGET_CHARS,
        CHUNK_BODY_MAX_CHARS,
    )
    .into_iter()
    .enumerate()
    .map(|(chunk_index, body)| {
        let text = if header.is_empty() {
            body
        } else {
            format!("{header}\n{body}")
        };
        TextChunk {
            chunk_index: chunk_index as u32,
            text: cap_provider_input(&text),
        }
    })
    .collect()
}

pub fn cap_provider_input(input: &str) -> String {
    if input.chars().count() <= MAX_PROVIDER_INPUT_CHARS {
        return input.to_owned();
    }
    let marker_chars = PROVIDER_TRUNCATION_MARKER.chars().count();
    let content_chars = MAX_PROVIDER_INPUT_CHARS.saturating_sub(marker_chars);
    let byte_limit = byte_index_at_char(input, content_chars);
    format!(
        "{}{}",
        input[..byte_limit].trim_end(),
        PROVIDER_TRUNCATION_MARKER
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BoundaryKind {
    BlankLine,
    StructuredLine,
    Line,
}

#[derive(Debug, Clone, Copy)]
struct Boundary {
    byte_index: usize,
    char_index: usize,
    kind: BoundaryKind,
    in_code_fence_after: bool,
}

fn split_body_with_limits(
    body: &str,
    min_chars: usize,
    target_chars: usize,
    max_chars: usize,
) -> Vec<String> {
    debug_assert!(min_chars <= target_chars && target_chars <= max_chars);
    if body.trim().is_empty() {
        return vec![String::new()];
    }
    let mut chunks = Vec::new();
    let mut remaining = body.trim();
    let mut in_code_fence = false;
    while remaining.chars().count() > max_chars {
        let boundaries = natural_boundaries(remaining, max_chars, in_code_fence);
        let selected = [
            BoundaryKind::BlankLine,
            BoundaryKind::StructuredLine,
            BoundaryKind::Line,
        ]
        .into_iter()
        .find_map(|kind| {
            boundaries
                .iter()
                .filter(|boundary| {
                    boundary.kind == kind && (min_chars..=max_chars).contains(&boundary.char_index)
                })
                .min_by_key(|boundary| boundary.char_index.abs_diff(target_chars))
                .copied()
        })
        .or_else(|| {
            [
                BoundaryKind::BlankLine,
                BoundaryKind::StructuredLine,
                BoundaryKind::Line,
            ]
            .into_iter()
            .find_map(|kind| {
                boundaries
                    .iter()
                    .filter(|boundary| boundary.kind == kind && boundary.char_index <= max_chars)
                    .max_by_key(|boundary| boundary.char_index)
                    .copied()
            })
        });
        let byte_index = selected.map_or_else(
            || byte_index_at_char(remaining, max_chars),
            |boundary| {
                in_code_fence = boundary.in_code_fence_after;
                boundary.byte_index
            },
        );
        let (chunk, tail) = remaining.split_at(byte_index);
        chunks.push(chunk.trim().to_owned());
        remaining = tail.trim_start();
    }
    if !remaining.is_empty() {
        chunks.push(remaining.trim().to_owned());
    }
    chunks
}

fn natural_boundaries(value: &str, max_chars: usize, mut in_code_fence: bool) -> Vec<Boundary> {
    let mut boundaries = Vec::new();
    let mut byte_index = 0;
    let mut char_index = 0;
    for line in value.split_inclusive('\n') {
        let line_chars = line.chars().count();
        if char_index + line_chars > max_chars {
            break;
        }
        byte_index += line.len();
        char_index += line_chars;
        let trimmed = line.trim();
        let fence_line = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        let structured = in_code_fence || fence_line || looks_like_table_row(trimmed);
        let kind = if structured {
            BoundaryKind::StructuredLine
        } else if trimmed.is_empty() {
            BoundaryKind::BlankLine
        } else {
            BoundaryKind::Line
        };
        if fence_line {
            in_code_fence = !in_code_fence;
        }
        boundaries.push(Boundary {
            byte_index,
            char_index,
            kind,
            in_code_fence_after: in_code_fence,
        });
    }
    boundaries
}

fn looks_like_table_row(line: &str) -> bool {
    line.matches('|').count() >= 2
}

fn byte_index_at_char(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map_or(value.len(), |(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_line_boundaries_win_over_nearer_plain_lines() {
        let body = "aaaa\nbbbb\ncccc\n\ndddd\neeee\nffff";
        assert_eq!(
            split_body_with_limits(body, 10, 12, 20),
            vec!["aaaa\nbbbb\ncccc", "dddd\neeee\nffff"]
        );
    }

    #[test]
    fn large_table_without_blank_lines_splits_only_between_rows() {
        let header = "Roadmap";
        let body = (0..300)
            .map(|index| format!("| row {index:03} | value {index:03} |"))
            .collect::<Vec<_>>()
            .join("\n");
        let full = format!("{header}\n{body}");
        let chunks = split_for_embedding(&full, header);

        assert!(chunks.len() > 1);
        for (index, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.chunk_index, index as u32);
            let chunk_body = chunk.text.strip_prefix("Roadmap\n").unwrap();
            assert!(chunk_body.lines().all(|line| line.starts_with("| row ")));
        }
    }

    #[test]
    fn sub_minimum_line_boundaries_win_over_hard_cuts() {
        let body = (0..4)
            .map(|index| format!("{}-{index}", "x".repeat(897)))
            .collect::<Vec<_>>()
            .join("\n");
        let chunks = split_for_embedding(&body, "");

        assert_eq!(chunks.len(), 4);
        assert!(chunks.iter().all(|chunk| chunk.text.lines().count() == 1));
        assert!(
            chunks
                .iter()
                .enumerate()
                .all(|(index, chunk)| chunk.text.ends_with(&format!("-{index}")))
        );
    }

    #[test]
    fn fence_state_and_structured_blank_lines_survive_an_in_fence_split() {
        let body = format!(
            "```\n{}\n{}\n\n{}\n{}\n```",
            "x".repeat(1_395),
            "a".repeat(1_198),
            "b".repeat(199),
            "c".repeat(500),
        );
        let chunks = split_for_embedding(&body, "");

        assert!(chunks.len() >= 3);
        assert_eq!(chunks[0].text.chars().count(), 1_399);
        assert_eq!(chunks[1].text.chars().count(), 1_399);
        assert!(
            chunks[1]
                .text
                .contains(&format!("{}\n\n{}", "a".repeat(1_198), "b".repeat(199)))
        );

        let inside_fence = natural_boundaries("\nnext\n```\n", 32, true);
        assert_eq!(inside_fence[0].kind, BoundaryKind::StructuredLine);
        assert!(inside_fence[0].in_code_fence_after);
        assert!(!inside_fence.last().unwrap().in_code_fence_after);
    }

    #[test]
    fn code_fence_and_unicode_split_at_valid_utf8_line_boundaries() {
        let header = "Код";
        let body = format!("```rust\n{}\n```", "let привет = 1;\n".repeat(180));
        let full = format!("{header}\n{body}");
        let chunks = split_for_embedding(&full, header);

        assert!(chunks.len() > 1);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.text.is_char_boundary(chunk.text.len()))
        );
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.text.chars().count() <= MAX_PROVIDER_INPUT_CHARS)
        );
    }

    #[test]
    fn blank_line_runs_do_not_create_empty_chunks() {
        let header = "Page";
        let body = format!("{}\n\n\n{}", "first ".repeat(260), "second ".repeat(260));
        let chunks = split_for_embedding(&format!("{header}\n{body}"), header);

        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|chunk| !chunk.text.trim().is_empty()));
    }

    #[test]
    fn exactly_at_tier_one_threshold_stays_one_vector() {
        let text = "x".repeat(SINGLE_VECTOR_MAX_CHARS);
        assert_eq!(split_for_embedding(&text, "").len(), 1);
    }

    #[test]
    fn single_enormous_line_uses_a_hard_character_cut() {
        let body = "Ж".repeat(CHUNK_BODY_MAX_CHARS * 2 + 1);
        let chunks = split_for_embedding(&body, "");

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].text.chars().count(), CHUNK_BODY_MAX_CHARS);
        assert_eq!(chunks[1].text.chars().count(), CHUNK_BODY_MAX_CHARS);
        assert_eq!(chunks[2].text, "Ж");
    }
}
