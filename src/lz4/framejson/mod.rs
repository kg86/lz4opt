//! Parses LZ4 frames into a physical JSON model and restores the original
//! bytes from that model.
//!
//! [`parse`] reads a stream of LZ4 frames into a [`Root`], which
//! [`Root::to_json`] renders as the indented JSON document the `lz4frame-json
//! inspect` command prints. [`restore`] reads such a document and writes the
//! frame bytes it describes.

mod block_parser;
mod errors;
mod model;
mod parser;
mod restore;
mod restore_schema;

pub use errors::{ParseCause, ParseError, RestoreError, SchemaError};
pub use model::*;
pub use parser::{
    parse, MAGIC_NUMBER_LEGACY, MAGIC_NUMBER_LZ4, MAGIC_NUMBER_SKIPPABLE_MAX, MAGIC_NUMBER_SKIPPABLE_MIN,
};
pub use restore::restore;
pub use restore_schema::decode_restore_document;

#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod roundtrip_tests;
#[cfg(test)]
mod stream_tests;
#[cfg(test)]
mod test_fixtures;
