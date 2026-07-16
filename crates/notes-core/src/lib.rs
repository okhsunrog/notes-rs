//! Host-independent notes-rs domain model and SQLite storage engine.

pub mod db;
pub mod sqlite;
pub mod stem;

pub use sqlite::Connection;
