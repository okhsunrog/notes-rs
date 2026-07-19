//! Tokenize + stem (Snowball) for FTS5 indexing.
//!
//! We split on non-letter boundaries (Unicode-aware), then route each token to the
//! Russian or English stemmer depending on script. Stemmed tokens are joined with
//! spaces so the FTS5 `unicode61` tokenizer at index time sees the already-stemmed
//! forms.
use rust_stemmers::{Algorithm, Stemmer};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum SearchTokenMode {
    #[default]
    Plain,
    Prefix,
}

fn russian() -> &'static Stemmer {
    static S: OnceLock<Stemmer> = OnceLock::new();
    S.get_or_init(|| Stemmer::create(Algorithm::Russian))
}

fn english() -> &'static Stemmer {
    static S: OnceLock<Stemmer> = OnceLock::new();
    S.get_or_init(|| Stemmer::create(Algorithm::English))
}

fn classify_script(s: &str) -> Script {
    let mut has_cyr = false;
    let mut has_latin = false;
    for c in s.chars() {
        if !c.is_alphabetic() {
            continue;
        }
        match c {
            'А'..='я' | 'Ё' | 'ё' => has_cyr = true,
            'a'..='z' | 'A'..='Z' => has_latin = true,
            _ => {}
        }
    }
    match (has_cyr, has_latin) {
        (true, _) => Script::Cyrillic,
        (false, true) => Script::Latin,
        _ => Script::Other,
    }
}

enum Script {
    Cyrillic,
    Latin,
    Other,
}

/// Stems each alphabetic token; preserves everything else as-is between tokens
/// so the FTS5 tokenizer can still split on punctuation/whitespace later.
pub fn stem(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut buf = String::new();
    for c in lower.chars() {
        if c.is_alphabetic() {
            buf.push(c);
        } else {
            if !buf.is_empty() {
                push_stemmed(&mut out, &buf);
                buf.clear();
            }
            out.push(c);
        }
    }
    if !buf.is_empty() {
        push_stemmed(&mut out, &buf);
    }
    out
}

fn push_stemmed(out: &mut String, token: &str) {
    let stemmed = match classify_script(token) {
        Script::Cyrillic => russian().stem(token).into_owned(),
        Script::Latin => english().stem(token).into_owned(),
        Script::Other => token.to_string(),
    };
    out.push_str(&stemmed);
}

/// Convert ordinary user text into a safe FTS5 query.
///
/// Search boxes accept natural language, not raw FTS5 syntax. Quoting every
/// stemmed token prevents punctuation such as `?`, `-`, `:` and unmatched
/// quotes from being interpreted as operators or column selectors.
pub fn stem_search_query(text: &str) -> String {
    stem_search_query_with_mode(text, SearchTokenMode::Plain)
}

pub fn stem_search_query_with_mode(text: &str, mode: SearchTokenMode) -> String {
    let tokens = search_tokens(text);
    let last_index = tokens.len().checked_sub(1);
    let clauses = tokens
        .into_iter()
        .enumerate()
        .filter_map(|(index, raw)| {
            let stemmed = stem(&raw);
            if stemmed.is_empty() {
                return None;
            }
            if mode == SearchTokenMode::Prefix && Some(index) == last_index {
                let raw = raw.to_lowercase();
                Some(format!("(\"{raw}\"* OR \"{stemmed}\"*)"))
            } else {
                Some(format!("\"{stemmed}\""))
            }
        })
        .collect::<Vec<_>>();
    clauses.join(if mode == SearchTokenMode::Prefix {
        " AND "
    } else {
        " "
    })
}

/// Build the deliberately narrow fallback used by page-title search after both
/// strict FTS and literal substring matching miss. It keeps every query token
/// required, but adds one- and two-character-shorter prefixes for long Cyrillic
/// tokens so common forms such as `покупок` can reach the indexed `покупк` stem.
pub(crate) fn relaxed_stem_prefix_search_query(text: &str) -> String {
    const MIN_RAW_CHARS: usize = 6;
    const MIN_PREFIX_CHARS: usize = 4;
    const MAX_TRIM_CHARS: usize = 2;

    let tokens = search_tokens(text);
    let last_index = tokens.len().checked_sub(1);
    let mut relaxed = false;
    let clauses = tokens
        .into_iter()
        .enumerate()
        .filter_map(|(index, raw)| {
            let stemmed = stem(&raw);
            if stemmed.is_empty() {
                return None;
            }

            let mut alternatives = if Some(index) == last_index {
                vec![
                    format!("\"{}\"*", raw.to_lowercase()),
                    format!("\"{stemmed}\"*"),
                ]
            } else {
                vec![format!("\"{stemmed}\"")]
            };
            if is_cyrillic_token(&raw) && raw.chars().count() >= MIN_RAW_CHARS {
                let stemmed_chars = stemmed.chars().collect::<Vec<_>>();
                for trim in 1..=MAX_TRIM_CHARS {
                    let Some(prefix_len) = stemmed_chars.len().checked_sub(trim) else {
                        continue;
                    };
                    if prefix_len < MIN_PREFIX_CHARS {
                        continue;
                    }
                    let prefix = stemmed_chars[..prefix_len].iter().collect::<String>();
                    alternatives.push(format!("\"{prefix}\"*"));
                    relaxed = true;
                }
            }
            alternatives.dedup();
            Some(if alternatives.len() == 1 {
                alternatives.pop().expect("one query alternative")
            } else {
                format!("({})", alternatives.join(" OR "))
            })
        })
        .collect::<Vec<_>>();

    if relaxed {
        clauses.join(" AND ")
    } else {
        String::new()
    }
}

fn search_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    for character in text.chars() {
        if character.is_alphanumeric() {
            token.push(character);
        } else if !token.is_empty() {
            tokens.push(std::mem::take(&mut token));
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

fn is_cyrillic_token(token: &str) -> bool {
    token
        .chars()
        .all(|character| matches!(character, 'А'..='я' | 'Ё' | 'ё'))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn russian_stem() {
        // Noun cases for "Москва" all reduce to the same stem.
        let s1 = stem("Москва Москвы Москве Москву");
        let stems: Vec<&str> = s1.split_whitespace().collect();
        assert!(
            stems.windows(2).all(|w| w[0] == w[1]),
            "expected all same, got {stems:?}"
        );
    }
    #[test]
    fn english_stem() {
        assert_eq!(stem("running runs"), stem("run run"));
    }
    #[test]
    fn natural_search_query_quotes_tokens_and_drops_fts_syntax() {
        assert_eq!(
            stem_search_query("Why offline-first? title:Rust"),
            "\"whi\" \"offlin\" \"first\" \"titl\" \"rust\""
        );
        assert_eq!(stem_search_query("???"), "");
    }

    #[test]
    fn prefix_search_matches_a_longer_word() {
        let database = rusqlite::Connection::open_in_memory().expect("open database");
        database
            .execute_batch(
                "CREATE VIRTUAL TABLE search_test USING fts5(content);
                 INSERT INTO search_test(content) VALUES ('programming');",
            )
            .expect("create FTS fixture");

        let query = stem_search_query_with_mode("prog", SearchTokenMode::Prefix);
        let count: i64 = database
            .query_row(
                "SELECT count(*) FROM search_test WHERE search_test MATCH ?1",
                [query],
                |row| row.get(0),
            )
            .expect("run prefix search");
        assert_eq!(count, 1);
    }

    #[test]
    fn prefix_search_quotes_tokens_and_drops_operators() {
        assert_eq!(
            stem_search_query_with_mode("prog OR title:Rust*", SearchTokenMode::Prefix),
            "\"prog\" AND \"or\" AND \"titl\" AND (\"rust\"* OR \"rust\"*)"
        );
        assert_eq!(stem_search_query_with_mode("", SearchTokenMode::Prefix), "");
        assert_eq!(
            stem_search_query_with_mode("***", SearchTokenMode::Prefix),
            ""
        );

        let database = rusqlite::Connection::open_in_memory().expect("open database");
        database
            .execute_batch(
                "CREATE VIRTUAL TABLE multi_token_test USING fts5(content);
                 INSERT INTO multi_token_test(content) VALUES ('firmware esp32');",
            )
            .expect("create multi-token FTS fixture");
        let query = stem_search_query_with_mode("esp32 firm", SearchTokenMode::Prefix);
        let count: i64 = database
            .query_row(
                "SELECT count(*) FROM multi_token_test WHERE multi_token_test MATCH ?1",
                [query],
                |row| row.get(0),
            )
            .expect("run multi-token prefix search");
        assert_eq!(count, 1);
    }

    #[test]
    fn relaxed_prefix_is_bounded_to_long_cyrillic_tokens() {
        assert_eq!(
            relaxed_stem_prefix_search_query("покупок"),
            "(\"покупок\"* OR \"покупо\"* OR \"покуп\"*)"
        );
        assert_eq!(relaxed_stem_prefix_search_query("абвгд"), "");
        assert_eq!(relaxed_stem_prefix_search_query("abcdef"), "");
        assert_eq!(
            relaxed_stem_prefix_search_query("абвгде"),
            "(\"абвгде\"* OR \"абвгд\"* OR \"абвг\"*)"
        );
    }

    #[test]
    fn mixed() {
        let a = stem("Programming в Москве");
        let b = stem("программ in москва");
        // These won't be exactly equal but both should be lower-cased and stemmed.
        assert!(!a.is_empty() && !b.is_empty());
    }
}
