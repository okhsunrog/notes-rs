use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::de;
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::{DiagnosticCode, ImportDiagnostic};

const PAGES_DIRECTORY: &str = "pages";
const JOURNALS_DIRECTORY: &str = "journals";
const ASSETS_DIRECTORY: &str = "assets";

/// Logseq's on-disk page filename convention.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileNameFormat {
    #[default]
    Legacy,
    TripleLowbar,
}

/// A normalized relative directory below the graph root.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct RelativeDirectory(String);

impl RelativeDirectory {
    pub fn new(value: impl AsRef<str>) -> Result<Self, ConfigError> {
        let value = value.as_ref();
        validate_relative_directory(value)?;
        Ok(Self(value.replace('\\', "/")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_path_buf(&self) -> PathBuf {
        self.0.split('/').collect()
    }
}

impl fmt::Display for RelativeDirectory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RelativeDirectory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// The small, intentionally whitelisted subset of `logseq/config.edn` that
/// affects source discovery and page identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogseqConfig {
    pub pages_directory: RelativeDirectory,
    pub journals_directory: RelativeDirectory,
    pub assets_directory: RelativeDirectory,
    pub file_name_format: FileNameFormat,
}

impl Default for LogseqConfig {
    fn default() -> Self {
        Self {
            pages_directory: RelativeDirectory(PAGES_DIRECTORY.to_owned()),
            journals_directory: RelativeDirectory(JOURNALS_DIRECTORY.to_owned()),
            assets_directory: RelativeDirectory(ASSETS_DIRECTORY.to_owned()),
            file_name_format: FileNameFormat::Legacy,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLogseqConfig {
    pub config: LogseqConfig,
    pub diagnostics: Vec<ImportDiagnostic>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ConfigError {
    #[error("malformed EDN at byte {offset}: {message}")]
    Malformed { offset: usize, message: String },
    #[error("setting {key} occurs more than once")]
    DuplicateSetting { key: String },
    #[error("setting {key} must be {expected}")]
    InvalidValueType { key: String, expected: &'static str },
    #[error("unsupported value {value} for setting {key}")]
    UnsupportedValue { key: String, value: String },
    #[error("invalid relative directory {value:?}: {reason}")]
    InvalidDirectory { value: String, reason: String },
}

/// Parse only import-relevant literal settings from a Logseq EDN config.
///
/// Unknown forms are structurally skipped. They are never evaluated or
/// deserialized into executable Clojure data.
pub fn parse_logseq_config(source: &str) -> Result<ParsedLogseqConfig, ConfigError> {
    let mut parser = EdnParser::new(source);
    let mut config = LogseqConfig::default();
    let mut seen = Vec::<String>::new();

    parser.skip_trivia()?;
    parser.expect('{')?;

    loop {
        parser.skip_trivia()?;
        if parser.consume_if('}') {
            break;
        }
        if parser.is_eof() {
            return Err(parser.malformed("unterminated top-level map"));
        }

        let key = parser.parse_map_key()?;
        parser.skip_trivia()?;
        if parser.peek() == Some('}') || parser.is_eof() {
            return Err(parser.malformed("top-level map has a key without a value"));
        }

        let Some(key) = key else {
            parser.skip_form()?;
            continue;
        };

        if !is_whitelisted(&key) {
            parser.skip_form()?;
            continue;
        }
        if seen.iter().any(|seen_key| seen_key == &key) {
            return Err(ConfigError::DuplicateSetting { key });
        }
        seen.push(key.clone());

        match key.as_str() {
            ":pages-directory" => {
                if let Some(value) = parser.parse_optional_string(&key)? {
                    config.pages_directory = RelativeDirectory::new(value)?;
                }
            }
            ":journals-directory" => {
                if let Some(value) = parser.parse_optional_string(&key)? {
                    config.journals_directory = RelativeDirectory::new(value)?;
                }
            }
            ":assets-directory" => {
                if let Some(value) = parser.parse_optional_string(&key)? {
                    config.assets_directory = RelativeDirectory::new(value)?;
                }
            }
            ":file/name-format" => {
                let value = parser.parse_optional_keyword(&key)?;
                if let Some(value) = value {
                    config.file_name_format = match value.as_str() {
                        ":legacy" => FileNameFormat::Legacy,
                        ":triple-lowbar" => FileNameFormat::TripleLowbar,
                        _ => {
                            return Err(ConfigError::UnsupportedValue { key, value });
                        }
                    };
                }
            }
            _ => unreachable!("whitelist and setting parser must stay in sync"),
        }
    }

    parser.skip_trivia()?;
    if !parser.is_eof() {
        return Err(parser.malformed("unexpected data after top-level map"));
    }

    Ok(ParsedLogseqConfig {
        config,
        diagnostics: Vec::new(),
    })
}

pub(crate) fn config_not_found_diagnostic() -> ImportDiagnostic {
    ImportDiagnostic::warning(
        DiagnosticCode::ConfigNotFound,
        Some("logseq/config.edn".to_owned()),
        "Logseq config was not found; standard source directories and legacy filenames are assumed",
        Some("Add logseq/config.edn or verify the dry-run source directories".to_owned()),
    )
}

fn is_whitelisted(key: &str) -> bool {
    matches!(
        key,
        ":pages-directory" | ":journals-directory" | ":assets-directory" | ":file/name-format"
    )
}

fn validate_relative_directory(value: &str) -> Result<(), ConfigError> {
    if value.is_empty() {
        return Err(invalid_directory(value, "directory cannot be empty"));
    }
    if value.chars().any(char::is_control) {
        return Err(invalid_directory(
            value,
            "control characters are not allowed",
        ));
    }
    if value.starts_with('/')
        || value.starts_with('\\')
        || value.starts_with("//")
        || value.starts_with("\\\\")
        || value.as_bytes().get(1) == Some(&b':')
    {
        return Err(invalid_directory(value, "directory must be relative"));
    }

    let normalized = value.replace('\\', "/");
    if normalized.split('/').any(|part| part.is_empty()) {
        return Err(invalid_directory(
            value,
            "empty path components are not allowed",
        ));
    }
    if normalized
        .split('/')
        .any(|part| part.as_bytes().get(1) == Some(&b':'))
    {
        return Err(invalid_directory(
            value,
            "Windows drive prefixes are not allowed in any component",
        ));
    }
    if normalized.split('/').any(|part| matches!(part, "." | "..")) {
        return Err(invalid_directory(
            value,
            "current and parent path components are not allowed",
        ));
    }

    if Path::new(&normalized)
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid_directory(
            value,
            "directory is not a normal relative path",
        ));
    }
    Ok(())
}

fn invalid_directory(value: &str, reason: &str) -> ConfigError {
    ConfigError::InvalidDirectory {
        value: value.to_owned(),
        reason: reason.to_owned(),
    }
}

struct EdnParser<'source> {
    source: &'source str,
    position: usize,
}

impl<'source> EdnParser<'source> {
    fn new(source: &'source str) -> Self {
        Self {
            source,
            position: if source.starts_with('\u{feff}') {
                '\u{feff}'.len_utf8()
            } else {
                0
            },
        }
    }

    fn is_eof(&self) -> bool {
        self.position >= self.source.len()
    }

    fn peek(&self) -> Option<char> {
        self.source[self.position..].chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.position += character.len_utf8();
        Some(character)
    }

    fn consume_if(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: char) -> Result<(), ConfigError> {
        if self.consume_if(expected) {
            Ok(())
        } else {
            Err(self.malformed(format!("expected {expected:?}")))
        }
    }

    fn malformed(&self, message: impl Into<String>) -> ConfigError {
        ConfigError::Malformed {
            offset: self.position,
            message: message.into(),
        }
    }

    fn skip_trivia(&mut self) -> Result<(), ConfigError> {
        loop {
            while matches!(self.peek(), Some(character) if character.is_whitespace() || character == ',')
            {
                self.advance();
            }

            if self.consume_if(';') {
                while !matches!(self.peek(), None | Some('\n') | Some('\r')) {
                    self.advance();
                }
                continue;
            }

            if self.source[self.position..].starts_with("#_") {
                self.position += 2;
                self.skip_trivia()?;
                self.skip_form()?;
                continue;
            }
            break;
        }
        Ok(())
    }

    fn parse_map_key(&mut self) -> Result<Option<String>, ConfigError> {
        if self.peek() == Some(':') {
            Ok(Some(self.parse_atom()?))
        } else {
            self.skip_form()?;
            Ok(None)
        }
    }

    fn parse_optional_string(&mut self, key: &str) -> Result<Option<String>, ConfigError> {
        self.skip_trivia()?;
        if self.starts_with_atom("nil") {
            self.parse_atom()?;
            return Ok(None);
        }
        if self.peek() != Some('"') {
            return Err(ConfigError::InvalidValueType {
                key: key.to_owned(),
                expected: "a string or nil",
            });
        }
        self.parse_string().map(Some)
    }

    fn parse_optional_keyword(&mut self, key: &str) -> Result<Option<String>, ConfigError> {
        self.skip_trivia()?;
        if self.starts_with_atom("nil") {
            self.parse_atom()?;
            return Ok(None);
        }
        if self.peek() != Some(':') {
            return Err(ConfigError::InvalidValueType {
                key: key.to_owned(),
                expected: "a keyword or nil",
            });
        }
        self.parse_atom().map(Some)
    }

    fn starts_with_atom(&self, expected: &str) -> bool {
        self.source[self.position..].starts_with(expected)
            && self.source[self.position + expected.len()..]
                .chars()
                .next()
                .is_none_or(is_delimiter)
    }

    fn parse_string(&mut self) -> Result<String, ConfigError> {
        self.expect('"')?;
        let mut output = String::new();
        loop {
            let Some(character) = self.advance() else {
                return Err(self.malformed("unterminated string"));
            };
            match character {
                '"' => return Ok(output),
                '\\' => {
                    let Some(escaped) = self.advance() else {
                        return Err(self.malformed("unterminated string escape"));
                    };
                    match escaped {
                        '"' => output.push('"'),
                        '\\' => output.push('\\'),
                        'n' => output.push('\n'),
                        'r' => output.push('\r'),
                        't' => output.push('\t'),
                        'b' => output.push('\u{0008}'),
                        'f' => output.push('\u{000c}'),
                        'u' => output.push(self.parse_unicode_escape()?),
                        _ => return Err(self.malformed("unsupported string escape")),
                    }
                }
                _ => output.push(character),
            }
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<char, ConfigError> {
        let start = self.position;
        let mut value = 0_u32;
        for _ in 0..4 {
            let Some(character) = self.advance() else {
                return Err(self.malformed("incomplete unicode escape"));
            };
            let Some(digit) = character.to_digit(16) else {
                return Err(self.malformed("invalid unicode escape"));
            };
            value = value * 16 + digit;
        }
        char::from_u32(value).ok_or_else(|| ConfigError::Malformed {
            offset: start,
            message: "unicode escape is not a scalar value".to_owned(),
        })
    }

    fn parse_atom(&mut self) -> Result<String, ConfigError> {
        let start = self.position;
        while matches!(self.peek(), Some(character) if !is_delimiter(character)) {
            self.advance();
        }
        if start == self.position {
            return Err(self.malformed("expected an EDN token"));
        }
        Ok(self.source[start..self.position].to_owned())
    }

    fn skip_form(&mut self) -> Result<(), ConfigError> {
        self.skip_trivia()?;
        let Some(character) = self.peek() else {
            return Err(self.malformed("expected an EDN form"));
        };
        match character {
            '{' => self.skip_collection('{', '}'),
            '[' => self.skip_collection('[', ']'),
            '(' => self.skip_collection('(', ')'),
            '"' => self.parse_string().map(drop),
            '\\' => {
                self.advance();
                self.parse_atom().map(drop)
            }
            '\'' | '`' | '@' => {
                self.advance();
                self.skip_form()
            }
            '~' => {
                self.advance();
                self.consume_if('@');
                self.skip_form()
            }
            '^' => {
                self.advance();
                self.skip_form()?;
                self.skip_form()
            }
            '#' => self.skip_dispatch_form(),
            '}' | ']' | ')' => Err(self.malformed("unexpected closing delimiter")),
            _ => self.parse_atom().map(drop),
        }
    }

    fn skip_collection(&mut self, opening: char, closing: char) -> Result<(), ConfigError> {
        self.expect(opening)?;
        loop {
            self.skip_trivia()?;
            match self.peek() {
                Some(character) if character == closing => {
                    self.advance();
                    return Ok(());
                }
                Some('}' | ']' | ')') => {
                    return Err(self.malformed("mismatched closing delimiter"));
                }
                None => return Err(self.malformed("unterminated collection")),
                Some(_) => self.skip_form()?,
            }
        }
    }

    fn skip_dispatch_form(&mut self) -> Result<(), ConfigError> {
        self.expect('#')?;
        match self.peek() {
            Some('{') => self.skip_collection('{', '}'),
            Some('(') => self.skip_collection('(', ')'),
            Some('"') => self.parse_string().map(drop),
            Some('#') => self.parse_atom().map(drop),
            Some(_) => {
                self.parse_atom()?;
                self.skip_form()
            }
            None => Err(self.malformed("incomplete dispatch form")),
        }
    }
}

fn is_delimiter(character: char) -> bool {
    character.is_whitespace()
        || matches!(
            character,
            ',' | ';' | '"' | '{' | '}' | '[' | ']' | '(' | ')' | '\'' | '`' | '~' | '@'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_whitelisted_literals_and_skips_executable_forms() {
        let parsed = parse_logseq_config(
            r#"{
                ;; :pages-directory "commented-out"
                :pages-directory "content/pages"
                :journals-directory "daily"
                :assets-directory "media"
                :file/name-format :triple-lowbar
                :default-queries [{:query [:find (pull ?b [*]) :where [?b :block/name ?n]]}]
                :query/result-transforms (fn [result] (sort-by :block/name result))
                :ignored-tag #custom/value {:nested true}
            }"#,
        )
        .expect("valid config");

        assert_eq!(parsed.config.pages_directory.as_str(), "content/pages");
        assert_eq!(parsed.config.journals_directory.as_str(), "daily");
        assert_eq!(parsed.config.assets_directory.as_str(), "media");
        assert_eq!(parsed.config.file_name_format, FileNameFormat::TripleLowbar);
    }

    #[test]
    fn reader_discard_does_not_shift_map_pairs() {
        let parsed = parse_logseq_config(
            r#"{:unknown #_ {:discarded true} (fn [] :actual)
                #_ :discarded-key #_ "discarded-value"
                :file/name-format :triple-lowbar}"#,
        )
        .expect("valid config");

        assert_eq!(parsed.config.file_name_format, FileNameFormat::TripleLowbar);
    }

    #[test]
    fn rejects_parent_and_absolute_directories() {
        for directory in ["../pages", "nested/../pages", "/tmp/pages", "C:\\pages"] {
            let edn_directory = directory.replace('\\', "\\\\");
            let source = format!(r#"{{:pages-directory "{edn_directory}"}}"#);
            assert!(matches!(
                parse_logseq_config(&source),
                Err(ConfigError::InvalidDirectory { .. })
            ));
        }
    }

    #[test]
    fn rejects_duplicate_import_settings() {
        assert!(matches!(
            parse_logseq_config(r#"{:pages-directory "one" :pages-directory "two"}"#),
            Err(ConfigError::DuplicateSetting { .. })
        ));
    }

    #[test]
    fn accepts_a_utf8_byte_order_mark() {
        let parsed = parse_logseq_config("\u{feff}{:file/name-format :triple-lowbar}")
            .expect("config with BOM");
        assert_eq!(parsed.config.file_name_format, FileNameFormat::TripleLowbar);
    }
}
