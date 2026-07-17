use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use unicode_normalization::UnicodeNormalization;

/// Opaque revision of one editable content field.
/// Revisions use the same canonical HLC representation as the sync engine, but
/// callers can only round-trip the value they received from a read. This keeps
/// optimistic-concurrency checks typed without exposing HLC parsing to UI code.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, specta::Type)]
#[serde(transparent)]
pub struct ContentRevision(#[specta(type = String)] crate::Hlc);

impl fmt::Display for ContentRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for ContentRevision {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

impl<'de> Deserialize<'de> for ContentRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl rusqlite::types::ToSql for ContentRevision {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        rusqlite::types::ToSql::to_sql(&self.0)
    }
}

impl rusqlite::types::FromSql for ContentRevision {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        value.as_str()?.parse().map_err(|error: anyhow::Error| {
            rusqlite::types::FromSqlError::Other(error.into_boxed_dyn_error())
        })
    }
}

/// Opaque revision of a complete page document projection.
/// The value is always the canonical lowercase hexadecimal representation of
/// one SHA-256 digest. Callers may compare and round-trip it, but the digest
/// framing remains an implementation detail of the document snapshot service.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, specta::Type)]
#[serde(transparent)]
pub struct DocumentRevision(#[specta(type = String)] String);

impl DocumentRevision {
    pub(crate) fn from_digest(bytes: [u8; 32]) -> Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut value = String::with_capacity(64);
        for byte in bytes {
            value.push(char::from(HEX[usize::from(byte >> 4)]));
            value.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DocumentRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for DocumentRevision {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        anyhow::ensure!(
            value.len() == 64,
            "document revision must contain 64 hexadecimal characters"
        );
        anyhow::ensure!(
            value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "document revision must be canonical lowercase hexadecimal"
        );
        Ok(Self(value.to_owned()))
    }
}

impl<'de> Deserialize<'de> for DocumentRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl rusqlite::types::ToSql for DocumentRevision {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(self.0.as_str().into())
    }
}

impl rusqlite::types::FromSql for DocumentRevision {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        value.as_str()?.parse().map_err(|error: anyhow::Error| {
            rusqlite::types::FromSqlError::Other(error.into_boxed_dyn_error())
        })
    }
}

pub fn normalize_title(title: &str) -> String {
    title.trim().nfkc().flat_map(char::to_lowercase).collect()
}

/// A canonical, durable page alias used by references independently of the
/// page's current display title. Construction always applies the same Unicode
/// normalization as page titles, so aliases have one wire and storage form.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, specta::Type)]
#[serde(transparent)]
pub struct PageAlias(String);

impl PageAlias {
    pub fn new(value: &str) -> Result<Self, ParseEnumError> {
        let value = normalize_title(value);
        if value.is_empty() {
            return Err(ParseEnumError {
                type_name: "PageAlias",
                value,
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PageAlias {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for PageAlias {
    type Err = ParseEnumError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for PageAlias {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl rusqlite::types::ToSql for PageAlias {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(self.0.as_str().into())
    }
}

impl rusqlite::types::FromSql for PageAlias {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        value
            .as_str()?
            .parse()
            .map_err(|error| rusqlite::types::FromSqlError::Other(Box::new(error)))
    }
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

/// A calendar day without a timezone or time-of-day component.
/// The private canonical string keeps the Tauri/Specta wire type simple while
/// construction, serde, and SQLite reads all pass through strict validation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, specta::Type)]
#[serde(transparent)]
pub struct JournalDate(String);

impl JournalDate {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JournalDate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for JournalDate {
    type Err = ParseEnumError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = value.as_bytes();
        let has_canonical_shape = bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes
                .iter()
                .enumerate()
                .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit());
        let valid_date = has_canonical_shape
            && chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .is_ok_and(|date| date.format("%Y-%m-%d").to_string() == value);
        if valid_date {
            Ok(Self(value.to_owned()))
        } else {
            Err(ParseEnumError {
                type_name: "JournalDate",
                value: value.to_owned(),
            })
        }
    }
}

impl<'de> Deserialize<'de> for JournalDate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl rusqlite::types::ToSql for JournalDate {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(self.0.as_str().into())
    }
}

impl rusqlite::types::FromSql for JournalDate {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        value
            .as_str()?
            .parse()
            .map_err(|error| rusqlite::types::FromSqlError::Other(Box::new(error)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageKind {
    Note,
    Journal { date: JournalDate },
}

impl PageKind {
    pub const fn is_journal(&self) -> bool {
        matches!(self, Self::Journal { .. })
    }
}

/// A typed projection over the page catalog. Normal note navigation uses
/// `Notes`; explicit callers may request Journals or the complete catalog.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum PageListFilter {
    #[default]
    Notes,
    Journals,
    All,
}

pub fn journal_page_uuid(workspace_uuid: uuid::Uuid, date: &JournalDate) -> uuid::Uuid {
    uuid::Uuid::new_v5(&workspace_uuid, date.as_str().as_bytes())
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
    /// Durable workflow state carried only by a task block.
    #[serde(rename_all = "snake_case")]
    pub enum TaskState {
        Todo => "todo",
        Doing => "doing",
        Now => "now",
        Later => "later",
        Done => "done",
        Waiting => "waiting",
        Cancelled => "cancelled",
    }
}

impl TaskState {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }

    /// Checkbox semantics are deliberately binary: any open workflow state is
    /// completed as `Done`, while a terminal state is reopened as `Todo`.
    pub const fn toggled(self) -> Self {
        if self.is_terminal() {
            Self::Todo
        } else {
            Self::Done
        }
    }
}

/// Semantic Markdown shape of a block. Task state is part of the style value,
/// so neither the Rust nor generated TypeScript contract can represent a task
/// without state or attach task state to a non-task block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BlockStyle {
    Paragraph,
    Bullet,
    Numbered,
    Task {
        state: TaskState,
    },
    #[serde(rename = "heading_1")]
    Heading1,
    #[serde(rename = "heading_2")]
    Heading2,
    #[serde(rename = "heading_3")]
    Heading3,
    Quote,
    Code,
    Divider,
}

impl<'de> Deserialize<'de> for BlockStyle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Default)]
        enum StateField {
            #[default]
            Missing,
            Present(Option<TaskState>),
        }

        fn deserialize_state_field<'de, D>(deserializer: D) -> Result<StateField, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            Option::<TaskState>::deserialize(deserializer).map(StateField::Present)
        }

        #[derive(Deserialize)]
        struct WireStyle {
            kind: String,
            #[serde(default, deserialize_with = "deserialize_state_field")]
            state: StateField,
            #[serde(flatten)]
            extra: std::collections::BTreeMap<String, serde::de::IgnoredAny>,
        }

        let wire = WireStyle::deserialize(deserializer)?;
        if !wire.extra.is_empty() {
            return Err(serde::de::Error::custom("unknown block style field"));
        }
        match (wire.kind.as_str(), wire.state) {
            ("paragraph", StateField::Missing) => Ok(Self::Paragraph),
            ("bullet", StateField::Missing) => Ok(Self::Bullet),
            ("numbered", StateField::Missing) => Ok(Self::Numbered),
            ("task", StateField::Present(Some(state))) => Ok(Self::Task { state }),
            ("heading_1", StateField::Missing) => Ok(Self::Heading1),
            ("heading_2", StateField::Missing) => Ok(Self::Heading2),
            ("heading_3", StateField::Missing) => Ok(Self::Heading3),
            ("quote", StateField::Missing) => Ok(Self::Quote),
            ("code", StateField::Missing) => Ok(Self::Code),
            ("divider", StateField::Missing) => Ok(Self::Divider),
            ("task", _) => Err(serde::de::Error::custom("task block style requires state")),
            (_, StateField::Present(_)) => Err(serde::de::Error::custom(
                "task state is only valid for task block style",
            )),
            _ => Err(serde::de::Error::custom("unsupported block style kind")),
        }
    }
}

impl BlockStyle {
    pub const fn task(state: TaskState) -> Self {
        Self::Task { state }
    }

    pub const fn task_state(self) -> Option<TaskState> {
        match self {
            Self::Task { state } => Some(state),
            _ => None,
        }
    }

    /// Canonical single-column SQLite representation. The state is encoded in
    /// the same value as the visual style and therefore shares one LWW clock.
    pub const fn storage_value(self) -> &'static str {
        match self {
            Self::Paragraph => "paragraph",
            Self::Bullet => "bullet",
            Self::Numbered => "numbered",
            Self::Task {
                state: TaskState::Todo,
            } => "task:todo",
            Self::Task {
                state: TaskState::Doing,
            } => "task:doing",
            Self::Task {
                state: TaskState::Now,
            } => "task:now",
            Self::Task {
                state: TaskState::Later,
            } => "task:later",
            Self::Task {
                state: TaskState::Done,
            } => "task:done",
            Self::Task {
                state: TaskState::Waiting,
            } => "task:waiting",
            Self::Task {
                state: TaskState::Cancelled,
            } => "task:cancelled",
            Self::Heading1 => "heading_1",
            Self::Heading2 => "heading_2",
            Self::Heading3 => "heading_3",
            Self::Quote => "quote",
            Self::Code => "code",
            Self::Divider => "divider",
        }
    }
}

impl fmt::Display for BlockStyle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.storage_value())
    }
}

impl FromStr for BlockStyle {
    type Err = ParseEnumError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let style = match value {
            "paragraph" => Self::Paragraph,
            "bullet" => Self::Bullet,
            "numbered" => Self::Numbered,
            "task:todo" => Self::task(TaskState::Todo),
            "task:doing" => Self::task(TaskState::Doing),
            "task:now" => Self::task(TaskState::Now),
            "task:later" => Self::task(TaskState::Later),
            "task:done" => Self::task(TaskState::Done),
            "task:waiting" => Self::task(TaskState::Waiting),
            "task:cancelled" => Self::task(TaskState::Cancelled),
            "heading_1" => Self::Heading1,
            "heading_2" => Self::Heading2,
            "heading_3" => Self::Heading3,
            "quote" => Self::Quote,
            "code" => Self::Code,
            "divider" => Self::Divider,
            _ => {
                return Err(ParseEnumError {
                    type_name: "BlockStyle",
                    value: value.to_owned(),
                });
            }
        };
        Ok(style)
    }
}

impl rusqlite::types::ToSql for BlockStyle {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(self.storage_value().into())
    }
}

impl rusqlite::types::FromSql for BlockStyle {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        value
            .as_str()?
            .parse()
            .map_err(|error| rusqlite::types::FromSqlError::Other(Box::new(error)))
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
    fn content_revisions_roundtrip_canonical_hlc_and_reject_unvalidated_strings() {
        let wire = "0000018D4A510000-00000002-01900000000070008000000000000001";
        let revision: ContentRevision = serde_json::from_value(serde_json::json!(wire)).unwrap();
        assert_eq!(revision.to_string(), wire.to_ascii_lowercase());
        assert_eq!(
            serde_json::to_value(revision).unwrap(),
            wire.to_ascii_lowercase()
        );
        assert!(
            serde_json::from_value::<ContentRevision>(serde_json::json!("revision-7")).is_err()
        );
    }

    #[test]
    fn block_style_wire_shape_cannot_represent_an_invalid_task() {
        assert_eq!(
            serde_json::to_value(BlockStyle::Paragraph).unwrap(),
            serde_json::json!({ "kind": "paragraph" })
        );
        assert_eq!(
            serde_json::to_value(BlockStyle::task(TaskState::Now)).unwrap(),
            serde_json::json!({ "kind": "task", "state": "now" })
        );
        assert!(
            serde_json::from_value::<BlockStyle>(serde_json::json!({ "kind": "task" })).is_err()
        );
        assert!(
            serde_json::from_value::<BlockStyle>(
                serde_json::json!({ "kind": "paragraph", "state": "done" })
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<BlockStyle>(
                serde_json::json!({ "kind": "paragraph", "state": null })
            )
            .is_err()
        );
    }

    #[test]
    fn task_state_has_explicit_terminal_toggle_behavior() {
        for state in [
            TaskState::Todo,
            TaskState::Doing,
            TaskState::Now,
            TaskState::Later,
            TaskState::Waiting,
        ] {
            assert_eq!(state.toggled(), TaskState::Done);
        }
        assert_eq!(TaskState::Done.toggled(), TaskState::Todo);
        assert_eq!(TaskState::Cancelled.toggled(), TaskState::Todo);
    }

    #[test]
    fn task_state_is_part_of_the_canonical_sql_style_value() {
        for state in [
            TaskState::Todo,
            TaskState::Doing,
            TaskState::Now,
            TaskState::Later,
            TaskState::Done,
            TaskState::Waiting,
            TaskState::Cancelled,
        ] {
            let style = BlockStyle::task(state);
            assert_eq!(style.storage_value().parse::<BlockStyle>().unwrap(), style);
        }
        assert!("task".parse::<BlockStyle>().is_err());
    }

    #[test]
    fn journal_dates_are_strict_civil_iso_dates() {
        for valid in ["2024-02-29", "2026-07-17", "9999-12-31"] {
            let date = valid.parse::<JournalDate>().expect("valid journal date");
            assert_eq!(date.as_str(), valid);
            assert_eq!(
                serde_json::to_string(&date).unwrap(),
                format!("\"{valid}\"")
            );
        }
        for invalid in [
            "2023-02-29",
            "2026-2-03",
            "2026_07_17",
            "2026-13-01",
            " 2026-07-17",
        ] {
            assert!(
                invalid.parse::<JournalDate>().is_err(),
                "accepted {invalid}"
            );
            assert!(serde_json::from_str::<JournalDate>(&format!("\"{invalid}\"")).is_err());
        }
    }

    #[test]
    fn journal_page_identity_depends_only_on_workspace_and_date() {
        let workspace = uuid::Uuid::from_u128(1);
        let date = "2026-07-17".parse::<JournalDate>().unwrap();
        assert_eq!(
            journal_page_uuid(workspace, &date),
            journal_page_uuid(workspace, &date)
        );
        assert_eq!(
            journal_page_uuid(workspace, &date),
            uuid::Uuid::parse_str("17ed8d52-fb7d-5c84-a2f4-1bb93bf969d1").unwrap()
        );
        assert_ne!(
            journal_page_uuid(workspace, &date),
            journal_page_uuid(uuid::Uuid::from_u128(2), &date)
        );
    }
}
