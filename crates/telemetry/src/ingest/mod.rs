//! Ingest: an OTLP export turned into rows of `spans`, each span read by the
//! contract ([`crate::contract`]) — its fields into columns, its content
//! dropped at the door, what the inventory does not name dropped with it —
//! and the answer that names what was refused or dropped, and why.

pub mod batch;
pub mod claims;
pub mod decode;
pub mod live;
pub mod report;
pub mod span;
