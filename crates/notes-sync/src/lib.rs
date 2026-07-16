//! Sync protocol boundary for notes-rs.
//!
//! The op format, HLC, apply integration, and client state machine land in
//! Phases 1 and 2. This crate exists in Phase 0 so both hosts have a stable
//! dependency boundary before behavior changes.

/// Wire-format version reserved by the approved sync architecture.
pub const FORMAT_VERSION: u32 = 1;
