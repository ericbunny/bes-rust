pub mod eframe_app;
pub mod utils;

pub use eframe_app::{
    BesApp, BinaryDocument, ByteRange, DocumentError, SearchEvent, SearchTask, parse_hex_pattern,
    start_search,
};
