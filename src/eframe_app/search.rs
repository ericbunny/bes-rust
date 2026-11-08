use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use crossbeam_channel::{Receiver, Sender, unbounded};
use memchr::memmem;
use thiserror::Error;

use super::document::{BinaryDocument, ByteRange, DocumentError};

const SEARCH_CHUNK_SIZE: u64 = 4 * 1024 * 1024;
const RESULT_BATCH_SIZE: usize = 256;
const MAX_RESULTS: usize = 1_000_000;

#[derive(Debug)]
pub enum SearchEvent {
    Progress {
        scanned: u64,
        total: u64,
    },
    Matches(Vec<u64>),
    Finished {
        total_matches: usize,
        truncated: bool,
    },
    Cancelled,
    Failed(String),
}

pub struct SearchTask {
    receiver: Receiver<SearchEvent>,
    cancelled: Arc<AtomicBool>,
}

impl SearchTask {
    pub fn try_iter(&self) -> crossbeam_channel::TryIter<'_, SearchEvent> {
        self.receiver.try_iter()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PatternError {
    #[error("请输入至少一个十六进制字节")]
    Empty,
    #[error("无效的十六进制字节：{0}")]
    InvalidByte(String),
}

pub fn parse_hex_pattern(input: &str) -> Result<Vec<u8>, PatternError> {
    let compact: String = input
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    if compact.is_empty() {
        return Err(PatternError::Empty);
    }
    if !compact.len().is_multiple_of(2) {
        return Err(PatternError::InvalidByte(compact));
    }
    (0..compact.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&compact[index..index + 2], 16)
                .map_err(|_| PatternError::InvalidByte(compact[index..index + 2].to_owned()))
        })
        .collect()
}

pub fn start_search(document: Arc<BinaryDocument>, pattern: Vec<u8>) -> SearchTask {
    let (sender, receiver) = unbounded();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    thread::Builder::new()
        .name("bes-search".to_owned())
        .spawn(move || run_search(document, pattern, worker_cancelled, sender))
        .expect("failed to spawn search thread");
    SearchTask {
        receiver,
        cancelled,
    }
}

fn run_search(
    document: Arc<BinaryDocument>,
    pattern: Vec<u8>,
    cancelled: Arc<AtomicBool>,
    sender: Sender<SearchEvent>,
) {
    let result = scan(&document, &pattern, &cancelled, &sender);
    if let Err(error) = result {
        let _ = sender.send(SearchEvent::Failed(error.to_string()));
    }
}

fn scan(
    document: &BinaryDocument,
    pattern: &[u8],
    cancelled: &AtomicBool,
    sender: &Sender<SearchEvent>,
) -> Result<(), DocumentError> {
    if pattern.is_empty() {
        let _ = sender.send(SearchEvent::Finished {
            total_matches: 0,
            truncated: false,
        });
        return Ok(());
    }

    let total = document.len();
    let overlap = pattern.len().saturating_sub(1) as u64;
    let mut offset = 0;
    let mut matches = Vec::with_capacity(RESULT_BATCH_SIZE);
    let mut total_matches = 0;
    let mut truncated = false;

    while offset < total {
        if cancelled.load(Ordering::Relaxed) {
            let _ = sender.send(SearchEvent::Cancelled);
            return Ok(());
        }
        let read_len = (SEARCH_CHUNK_SIZE + overlap).min(total - offset);
        let bytes = document.read(ByteRange::new(offset, read_len))?;
        let primary_end = (offset + SEARCH_CHUNK_SIZE).min(total);
        let finder = memmem::Finder::new(pattern);
        let mut search_start = 0;
        while let Some(found) = finder.find(&bytes[search_start..]) {
            let relative = search_start + found;
            let absolute = offset + relative as u64;
            if absolute >= primary_end || absolute + pattern.len() as u64 > total {
                break;
            }
            matches.push(absolute);
            total_matches += 1;
            if matches.len() == RESULT_BATCH_SIZE {
                let _ = sender.send(SearchEvent::Matches(std::mem::take(&mut matches)));
            }
            if total_matches == MAX_RESULTS {
                truncated = true;
                break;
            }
            search_start = relative + 1;
        }
        if truncated {
            break;
        }
        offset = primary_end;
        let _ = sender.send(SearchEvent::Progress {
            scanned: offset,
            total,
        });
    }
    if !matches.is_empty() {
        let _ = sender.send(SearchEvent::Matches(matches));
    }
    let _ = sender.send(SearchEvent::Finished {
        total_matches,
        truncated,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{io::Write, sync::Arc};

    use tempfile::NamedTempFile;

    use super::{BinaryDocument, SearchEvent, parse_hex_pattern, start_search};

    #[test]
    fn parses_spaced_hex() {
        assert_eq!(parse_hex_pattern("AA 01 ff").unwrap(), [0xAA, 0x01, 0xFF]);
        assert!(parse_hex_pattern("A").is_err());
    }

    #[test]
    fn finds_overlapping_matches() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"ABABA").unwrap();
        let document = Arc::new(BinaryDocument::open(file.path()).unwrap());
        let task = start_search(document, b"ABA".to_vec());
        let events: Vec<_> = task.receiver.iter().collect();
        let matches: Vec<u64> = events
            .into_iter()
            .filter_map(|event| match event {
                SearchEvent::Matches(offsets) => Some(offsets),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(matches, [0, 2]);
    }
}
