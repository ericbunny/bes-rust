mod app;
mod document;
mod frame;
mod search;
mod selection;

pub use app::BesApp;
pub use document::{BinaryDocument, ByteRange, DocumentError};
pub use frame::{FrameIndex, FrameRule, FrameStatus};
pub use search::{SearchEvent, SearchTask, parse_hex_pattern, start_search};
