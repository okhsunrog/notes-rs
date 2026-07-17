use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use unicode_normalization::UnicodeNormalization;

pub fn normalize_title(title: &str) -> String {
    title.trim().nfkc().flat_map(char::to_lowercase).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseEnumError {
    type_name: &'static str,
    value: String,
}

impl fmt::Display for ParseEnumError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unsupported {}: {}", self.type_name, self.value)
    }
}

impl std::error::Error for ParseEnumError {}

macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $value:literal),+ $(,)?
        }
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, specta::Type)]
        $(#[$meta])*
        pub enum $name {
            $(#[serde(rename = $value)] $variant),+
        }

        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = ParseEnumError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => Err(ParseEnumError {
                        type_name: stringify!($name),
                        value: value.to_owned(),
                    }),
                }
            }
        }

        impl rusqlite::types::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                Ok(self.as_str().into())
            }
        }

        impl rusqlite::types::FromSql for $name {
            fn column_result(
                value: rusqlite::types::ValueRef<'_>,
            ) -> rusqlite::types::FromSqlResult<Self> {
                value
                    .as_str()?
                    .parse()
                    .map_err(|error| rusqlite::types::FromSqlError::Other(Box::new(error)))
            }
        }
    };
}

string_enum! {
    /// The durable structural layout of a page. Reading is pane-local
    /// presentation state and deliberately does not cross this boundary.
    #[serde(rename_all = "snake_case")]
    pub enum PageLayout {
        Outline => "outline",
        Document => "document",
    }
}

string_enum! {
    #[serde(rename_all = "lowercase")]
    pub enum ObjectKind {
        Page => "page",
        Block => "block",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", content = "uuid", rename_all = "lowercase")]
pub enum AttachmentOwner {
    Page(uuid::Uuid),
    Block(uuid::Uuid),
}

impl AttachmentOwner {
    pub const fn kind(self) -> ObjectKind {
        match self {
            Self::Page(_) => ObjectKind::Page,
            Self::Block(_) => ObjectKind::Block,
        }
    }

    pub const fn uuid(self) -> uuid::Uuid {
        match self {
            Self::Page(uuid) | Self::Block(uuid) => uuid,
        }
    }
}

string_enum! {
    /// Semantic Markdown shape of a block. Outline bullets are editor chrome
    /// and deliberately do not change this value.
    #[serde(rename_all = "snake_case")]
    pub enum BlockStyle {
        Paragraph => "paragraph",
        Bullet => "bullet",
        Numbered => "numbered",
        Task => "task",
        Heading1 => "heading_1",
        Heading2 => "heading_2",
        Heading3 => "heading_3",
        Quote => "quote",
        Code => "code",
        Divider => "divider",
    }
}

/// A lexical, sync-safe sibling ordering key. Keys are fixed-width uppercase
/// hexadecimal integers, so SQLite TEXT ordering is also numeric ordering.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, specta::Type)]
#[serde(transparent)]
pub struct OrderKey(String);

impl OrderKey {
    pub const STEP: u64 = 1 << 32;

    pub fn first() -> Self {
        Self::from_ordinal(1)
    }

    pub fn from_ordinal(ordinal: usize) -> Self {
        Self::from_value((ordinal as u64).saturating_mul(Self::STEP))
    }

    pub fn after(&self) -> Self {
        Self::from_value(self.value().saturating_add(Self::STEP))
    }

    pub fn value(&self) -> u64 {
        u64::from_str_radix(&self.0, 16).expect("OrderKey is validated when constructed or read")
    }

    fn from_value(value: u64) -> Self {
        Self(format!("{value:016X}"))
    }
}

impl Default for OrderKey {
    fn default() -> Self {
        Self::first()
    }
}

impl fmt::Display for OrderKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for OrderKey {
    type Err = ParseEnumError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let valid = value.len() == 16
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte));
        if valid {
            Ok(Self(value.to_owned()))
        } else {
            Err(ParseEnumError {
                type_name: "OrderKey",
                value: value.to_owned(),
            })
        }
    }
}

impl<'de> Deserialize<'de> for OrderKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl rusqlite::types::ToSql for OrderKey {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(self.0.as_str().into())
    }
}

impl rusqlite::types::FromSql for OrderKey {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        value
            .as_str()?
            .parse()
            .map_err(|error| rusqlite::types::FromSqlError::Other(Box::new(error)))
    }
}

string_enum! {
    #[serde(rename_all = "lowercase")]
    pub enum ReorderDirection {
        Up => "up",
        Down => "down",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_style_uses_one_canonical_wire_and_sql_value() {
        for style in [
            BlockStyle::Paragraph,
            BlockStyle::Bullet,
            BlockStyle::Numbered,
            BlockStyle::Task,
            BlockStyle::Heading1,
            BlockStyle::Heading2,
            BlockStyle::Heading3,
            BlockStyle::Quote,
            BlockStyle::Code,
            BlockStyle::Divider,
        ] {
            assert_eq!(
                serde_json::to_value(style).unwrap(),
                serde_json::Value::String(style.as_str().into())
            );
        }
    }
}
