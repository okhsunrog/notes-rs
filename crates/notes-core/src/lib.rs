//! Host-independent notes-rs domain model and SQLite storage engine.

pub mod db;
pub mod operation;
pub mod sqlite;
pub mod stem;

pub use operation::{ApplyOutcome, Op, OpKind, Origin, apply, apply_batch, local_ops};
pub use sqlite::Connection;
