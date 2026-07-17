//! Pure import discovery and planning primitives.
//!
//! This crate deliberately has no persistence, AI, host, or UI dependencies.
//! Scanning a graph only reads source files and produces a deterministic
//! manifest; applying that manifest belongs to a separate boundary.

mod config;
mod diagnostic;
mod manifest;
mod scanner;

pub use config::{
    ConfigError, FileNameFormat, LogseqConfig, ParsedLogseqConfig, RelativeDirectory,
    parse_logseq_config,
};
pub use diagnostic::{
    DiagnosticCode, DiagnosticSeverity, ImportDiagnostic, SourcePosition, SourceRange,
};
pub use manifest::{DocumentFormat, GraphManifest, ManifestEntry, Sha256Digest, SourceKind};
pub use scanner::{
    GraphScanReport, ManifestVerification, ScanError, ScanErrorCode, ScanLimits, scan_logseq_graph,
    scan_logseq_graph_with_limits, verify_logseq_manifest, verify_logseq_manifest_with_limits,
};
