use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::LogseqConfig;

const MANIFEST_VERSION: u32 = 1;
const HASH_DOMAIN: &[u8] = b"tangleaf/logseq-manifest/v1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Config,
    Page,
    Journal,
    Asset,
    Drawing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentFormat {
    Markdown,
    Org,
    Unknown,
}

/// SHA-256 bytes serialized as a lowercase hexadecimal string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(64);
        for byte in self.0 {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        output
    }

    fn parse_hex(value: &str) -> Result<Self, &'static str> {
        if value.len() != 64 {
            return Err("SHA-256 digest must contain exactly 64 hexadecimal characters");
        }
        let mut bytes = [0_u8; 32];
        let (pairs, _) = value.as_bytes().as_chunks::<2>();
        for (index, &[high, low]) in pairs.iter().enumerate() {
            bytes[index] = (hex_value(high)? << 4) | hex_value(low)?;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl Serialize for Sha256Digest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DigestVisitor;

        impl Visitor<'_> for DigestVisitor {
            type Value = Sha256Digest;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a lowercase or uppercase 64-character SHA-256 digest")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Sha256Digest::parse_hex(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(DigestVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub kind: SourceKind,
    pub document_format: Option<DocumentFormat>,
    pub relative_path: String,
    pub size_bytes: u64,
    pub sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphManifest {
    version: u32,
    config: LogseqConfig,
    entries: Vec<ManifestEntry>,
    total_bytes: u64,
    sha256: Sha256Digest,
}

impl GraphManifest {
    pub const fn version(&self) -> u32 {
        self.version
    }

    pub const fn config(&self) -> &LogseqConfig {
        &self.config
    }

    pub fn entries(&self) -> &[ManifestEntry] {
        &self.entries
    }

    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub const fn sha256(&self) -> Sha256Digest {
        self.sha256
    }

    pub(crate) fn build(config: LogseqConfig, mut entries: Vec<ManifestEntry>) -> Self {
        entries.sort_by(|left, right| {
            left.relative_path
                .cmp(&right.relative_path)
                .then(left.kind.cmp(&right.kind))
        });
        let total_bytes = entries.iter().map(|entry| entry.size_bytes).sum();
        let sha256 = hash_manifest(&config, &entries);
        Self {
            version: MANIFEST_VERSION,
            config,
            entries,
            total_bytes,
            sha256,
        }
    }
}

fn hash_manifest(config: &LogseqConfig, entries: &[ManifestEntry]) -> Sha256Digest {
    let mut hasher = Sha256::new();
    hasher.update(HASH_DOMAIN);
    hasher.update(MANIFEST_VERSION.to_be_bytes());
    update_text(&mut hasher, config.pages_directory.as_str());
    update_text(&mut hasher, config.journals_directory.as_str());
    update_text(&mut hasher, config.assets_directory.as_str());
    hasher.update([match config.file_name_format {
        crate::FileNameFormat::Legacy => 0,
        crate::FileNameFormat::TripleLowbar => 1,
    }]);
    hasher.update((entries.len() as u64).to_be_bytes());

    for entry in entries {
        hasher.update([source_kind_tag(entry.kind)]);
        hasher.update([entry.document_format.map_or(0, document_format_tag)]);
        update_text(&mut hasher, &entry.relative_path);
        hasher.update(entry.size_bytes.to_be_bytes());
        hasher.update(entry.sha256.as_bytes());
    }

    Sha256Digest::from_bytes(hasher.finalize().into())
}

fn update_text(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

const fn source_kind_tag(kind: SourceKind) -> u8 {
    match kind {
        SourceKind::Config => 1,
        SourceKind::Page => 2,
        SourceKind::Journal => 3,
        SourceKind::Asset => 4,
        SourceKind::Drawing => 5,
    }
}

const fn document_format_tag(format: DocumentFormat) -> u8 {
    match format {
        DocumentFormat::Markdown => 1,
        DocumentFormat::Org => 2,
        DocumentFormat::Unknown => 3,
    }
}

fn hex_value(value: u8) -> Result<u8, &'static str> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err("SHA-256 digest contains a non-hexadecimal character"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_has_a_stable_json_representation() {
        let digest = Sha256Digest::from_bytes([0xab; 32]);
        let json = serde_json::to_string(&digest).expect("serialize digest");
        assert_eq!(json, format!("\"{}\"", "ab".repeat(32)));
        assert_eq!(
            serde_json::from_str::<Sha256Digest>(&json).expect("deserialize digest"),
            digest
        );
    }
}
