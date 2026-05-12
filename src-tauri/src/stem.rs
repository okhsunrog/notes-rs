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

/// Stem an FTS5 *query* string. Same per-token stemming as [`stem`], but
/// preserves FTS5 operator keywords (AND, OR, NOT, NEAR) verbatim so the
/// resulting string remains a valid FTS5 expression. Punctuation, quotes,
/// parens, `*`, `^`, `+`, `-`, and `:` pass through (they're non-alphabetic
/// so the tokenizer already preserves them).
///
/// Quoted phrases like `"block ref"` are stemmed token-by-token inside the
/// quotes — since the index stores stemmed forms, the phrase match needs
/// stemmed tokens too.
pub fn stem_query(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut buf = String::new();
    for c in text.chars() {
        if c.is_alphabetic() {
            buf.push(c);
        } else {
            if !buf.is_empty() {
                push_query_token(&mut out, &buf);
                buf.clear();
            }
            out.push(c);
        }
    }
    if !buf.is_empty() {
        push_query_token(&mut out, &buf);
    }
    out
}

fn push_query_token(out: &mut String, token: &str) {
    // FTS5 operator keywords must stay uppercase to function as operators.
    let upper = token.to_uppercase();
    if matches!(upper.as_str(), "AND" | "OR" | "NOT" | "NEAR") {
        out.push_str(&upper);
        return;
    }
    let lower = token.to_lowercase();
    let stemmed = match classify_script(&lower) {
        Script::Cyrillic => russian().stem(&lower).into_owned(),
        Script::Latin => english().stem(&lower).into_owned(),
        Script::Other => lower,
    };
    out.push_str(&stemmed);
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
    fn query_preserves_operators() {
        assert_eq!(stem_query("running AND fast"), "run AND fast");
        assert_eq!(stem_query("foo OR bar NOT baz"), "foo OR bar NOT baz");
    }

    #[test]
    fn query_preserves_metacharacters() {
        let s = stem_query("\"running fast\"");
        assert!(s.starts_with('"') && s.ends_with('"'), "got {s:?}");
        assert_eq!(stem_query("running*"), "run*");
    }

    #[test]
    fn mixed() {
        let a = stem("Programming в Москве");
        let b = stem("программ in москва");
        // These won't be exactly equal but both should be lower-cased and stemmed.
        assert!(!a.is_empty() && !b.is_empty());
    }
}
