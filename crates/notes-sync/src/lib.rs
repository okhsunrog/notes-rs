//! Sync protocol boundary for notes-rs.
//!
//! Phase 1 exposes the versioned operation wire types and apply boundary here.
//! The HLC merge policy and client state machine are implemented in Phase 2.

pub use notes_core::operation::FORMAT_VERSION;
pub use notes_core::operation::{
    AttachmentAdd, AttachmentRemove, EdgeAdd, EdgeRemove, NodeCreate, NodeDelete, NodeMove,
    NodeSetContent, NodeSetTitle,
};
pub use notes_core::{ApplyOutcome, Op, OpKind, Origin, apply, apply_batch, local_ops};
