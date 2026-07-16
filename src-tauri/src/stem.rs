//! Tokenize + stem (Snowball) for FTS5 indexing.
//!
//! We split on non-letter boundaries (Unicode-aware), then route each token to the
//! Russian or English stemmer depending on script. Stemmed tokens are joined with
//! spaces so the FTS5 `unicode61` tokenizer at index time sees the already-stemmed
//! forms.
use rust_stemmers::{Algorithm, Stemmer};
use std::sync::OnceLock;

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
    let mut tokens = Vec::new();
    let mut token = String::new();
    for character in text.chars() {
        if character.is_alphanumeric() {
            token.push(character);
        } else if !token.is_empty() {
            tokens.push(stem(&token));
            token.clear();
        }
    }
    if !token.is_empty() {
        tokens.push(stem(&token));
    }
    tokens
        .into_iter()
        .filter(|token| !token.is_empty())
        .map(|token| format!("\"{token}\""))
        .collect::<Vec<_>>()
        .join(" ")
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
    fn mixed() {
        let a = stem("Programming в Москве");
        let b = stem("программ in москва");
        // These won't be exactly equal but both should be lower-cased and stemmed.
        assert!(!a.is_empty() && !b.is_empty());
    }
}
