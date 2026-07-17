use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

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
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
        $(#[$meta])*
        pub enum $name {
            $($variant),+
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
    /// Closed set of node shapes understood by storage, sync and the UI.
    #[serde(rename_all = "lowercase")]
    pub enum NodeKind {
        Page => "page",
        Block => "block",
        Tag => "tag",
        Entity => "entity",
        Attachment => "attachment",
    }
}

string_enum! {
    #[serde(rename_all = "lowercase")]
    pub enum ReorderDirection {
        Up => "up",
        Down => "down",
    }
}
