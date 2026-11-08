use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use crossbeam_channel::{Receiver, Sender, unbounded};
use memchr::memchr;
use thiserror::Error;

use super::{
    document::{BinaryDocument, ByteRange, DocumentError},
    search::PatternError,
};

const FRAME_CHUNK_SIZE: u64 = 4 * 1024 * 1024;
const FRAME_BATCH_SIZE: usize = 256;
const MAX_FRAMES: usize = 1_000_000;
const MAX_CANDIDATES: usize = 4_000_000;
const MAX_SYNC_LEN: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BytePattern {
    pub bytes: Vec<u8>,
    pub mask: Vec<u8>,
}

impl BytePattern {
    #[doc = "判断模式在指定位置是否与数据匹配。"]
    fn matches_at(&self, data: &[u8], start: usize) -> bool {
        self.bytes
            .iter()
            .zip(&self.mask)
            .enumerate()
            .all(|(index, (byte, mask))| data[start + index] & mask == byte & mask)
    }
}

#[doc = "解析支持 `??` 通配的十六进制同步码。"]
pub fn parse_mask_pattern(input: &str) -> Result<BytePattern, PatternError> {
    let compact: String = input
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    if compact.is_empty() {
        return Err(PatternError::Empty);
    }
    if !compact.len().is_multiple_of(2) || !compact.is_ascii() {
        return Err(PatternError::InvalidByte(compact));
    }
    let mut bytes = Vec::with_capacity(compact.len() / 2);
    let mut mask = Vec::with_capacity(compact.len() / 2);
    for index in (0..compact.len()).step_by(2) {
        let pair = &compact[index..index + 2];
        if pair == "??" {
            bytes.push(0);
            mask.push(0x00);
            continue;
        }
        match u8::from_str_radix(pair, 16) {
            Ok(byte) => {
                bytes.push(byte);
                mask.push(0xFF);
            }
            Err(_) => return Err(PatternError::InvalidByte(pair.to_owned())),
        }
    }
    Ok(BytePattern { bytes, mask })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameRule {
    pub sync: BytePattern,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameRuleError {
    #[error("同步码至少需要一个确定的字节")]
    WildcardOnlySync,
    #[error("同步码最长 {MAX_SYNC_LEN} 字节")]
    SyncTooLong,
}

impl FrameRule {
    #[doc = "校验帧规则参数，避免切分阶段出现无意义的扫描。"]
    pub fn validate(&self) -> Result<(), FrameRuleError> {
        if self.sync.mask.iter().all(|mask| *mask == 0x00) {
            return Err(FrameRuleError::WildcardOnlySync);
        }
        if self.sync.bytes.len() > MAX_SYNC_LEN {
            return Err(FrameRuleError::SyncTooLong);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameStatus {
    Ok,
    BadLength,
    Truncated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameIndex {
    pub start: u64,
    pub len: u64,
    pub status: FrameStatus,
}

#[derive(Debug)]
pub enum FrameEvent {
    Progress { scanned: u64, total: u64 },
    Frames(Vec<FrameIndex>),
    Finished { frame_count: usize, truncated: bool },
    Cancelled,
    Failed(String),
}

pub struct FrameTask {
    receiver: Receiver<FrameEvent>,
    cancelled: Arc<AtomicBool>,
}

impl FrameTask {
    pub fn try_iter(&self) -> crossbeam_channel::TryIter<'_, FrameEvent> {
        self.receiver.try_iter()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[doc = "在后台线程中按帧规则切分文档，结果分批通过事件通道返回。"]
pub fn start_frame_analysis(
    document: Arc<BinaryDocument>,
    rule: FrameRule,
    scope: ByteRange,
) -> FrameTask {
    let (sender, receiver) = unbounded();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    thread::Builder::new()
        .name("bes-frame".to_owned())
        .spawn(move || run_frame_analysis(document, rule, scope, worker_cancelled, sender))
        .expect("failed to spawn frame thread");
    FrameTask {
        receiver,
        cancelled,
    }
}

fn run_frame_analysis(
    document: Arc<BinaryDocument>,
    rule: FrameRule,
    scope: ByteRange,
    cancelled: Arc<AtomicBool>,
    sender: Sender<FrameEvent>,
) {
    let mut batch = Vec::with_capacity(FRAME_BATCH_SIZE);
    let result = split_frames(
        &document,
        &rule,
        scope,
        &cancelled,
        SplitLimits::DEFAULT,
        |frame| {
            batch.push(frame);
            if batch.len() == FRAME_BATCH_SIZE {
                let _ = sender.send(FrameEvent::Frames(std::mem::take(&mut batch)));
            }
        },
        |scanned, total| {
            let _ = sender.send(FrameEvent::Progress { scanned, total });
        },
    );
    match result {
        Ok(SplitOutcome::Completed {
            frame_count,
            truncated,
        }) => {
            if !batch.is_empty() {
                let _ = sender.send(FrameEvent::Frames(batch));
            }
            let _ = sender.send(FrameEvent::Finished {
                frame_count,
                truncated,
            });
        }
        Ok(SplitOutcome::Cancelled) => {
            let _ = sender.send(FrameEvent::Cancelled);
        }
        Err(error) => {
            let _ = sender.send(FrameEvent::Failed(error.to_string()));
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct SplitLimits {
    max_frames: usize,
    max_candidates: usize,
}

impl SplitLimits {
    const DEFAULT: Self = Self {
        max_frames: MAX_FRAMES,
        max_candidates: MAX_CANDIDATES,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SplitOutcome {
    Completed { frame_count: usize, truncated: bool },
    Cancelled,
}

#[doc = "按帧规则顺序切分文档，逐帧回调并上报扫描进度。"]
fn split_frames(
    document: &BinaryDocument,
    rule: &FrameRule,
    scope: ByteRange,
    cancelled: &AtomicBool,
    limits: SplitLimits,
    mut on_frame: impl FnMut(FrameIndex),
    mut on_progress: impl FnMut(u64, u64),
) -> Result<SplitOutcome, DocumentError> {
    let limit = scope
        .end()
        .unwrap_or_else(|| document.len())
        .min(document.len());
    let start = scope.start.min(limit);
    let total = limit - start;
    let overlap = (rule.sync.bytes.len() as u64).saturating_sub(1);
    let mut frame_count = 0;
    let mut candidates = 0;
    let mut truncated = false;
    let mut frame_start = None;
    let mut offset = start;

    let Some(anchor) = rule.sync.mask.iter().position(|mask| *mask == 0xFF) else {
        return Ok(SplitOutcome::Completed {
            frame_count: 0,
            truncated: false,
        });
    };

    'chunks: while offset < limit {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(SplitOutcome::Cancelled);
        }
        let primary_end = (offset + FRAME_CHUNK_SIZE).min(limit);
        let read_len = (primary_end - offset + overlap).min(limit - offset);
        let bytes = document.read(ByteRange::new(offset, read_len))?;
        let mut search_from = 0;
        while let Some(relative) = find_sync(&bytes, &rule.sync, anchor, search_from) {
            let hit = offset + relative as u64;
            if hit >= primary_end {
                break;
            }
            search_from = relative + 1;
            candidates += 1;
            if candidates > limits.max_candidates {
                truncated = true;
                break 'chunks;
            }
            if let Some(previous) = frame_start {
                if frame_count >= limits.max_frames {
                    truncated = true;
                    break 'chunks;
                }
                on_frame(FrameIndex {
                    start: previous,
                    len: hit - previous,
                    status: FrameStatus::Ok,
                });
                frame_count += 1;
            }
            frame_start = Some(hit);
        }
        offset = primary_end;
        on_progress(offset - start, total);
    }

    if !truncated && let Some(frame_start) = frame_start {
        if frame_count < limits.max_frames {
            on_frame(FrameIndex {
                start: frame_start,
                len: limit - frame_start,
                status: FrameStatus::Ok,
            });
            frame_count += 1;
        } else {
            truncated = true;
        }
    }

    Ok(SplitOutcome::Completed {
        frame_count,
        truncated,
    })
}

fn find_sync(data: &[u8], pattern: &BytePattern, anchor: usize, from: usize) -> Option<usize> {
    let sync_len = pattern.bytes.len();
    let mut index = from;
    while index + sync_len <= data.len() {
        let found = memchr(pattern.bytes[anchor], &data[index + anchor..])?;
        let candidate = index + found;
        if candidate + sync_len > data.len() {
            return None;
        }
        if pattern.matches_at(data, candidate) {
            return Some(candidate);
        }
        index = candidate + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use std::{io::Write, sync::atomic::AtomicBool};

    use tempfile::NamedTempFile;

    use super::{
        FRAME_CHUNK_SIZE, FrameIndex, FrameRule, FrameRuleError, FrameStatus, SplitLimits,
        SplitOutcome, parse_mask_pattern, split_frames,
    };
    use crate::eframe_app::{BinaryDocument, ByteRange};

    fn rule() -> FrameRule {
        FrameRule {
            sync: parse_mask_pattern("AA 55").unwrap(),
        }
    }

    fn frame_bytes(payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xAA, 0x55];
        bytes.extend_from_slice(payload);
        bytes
    }

    fn file_with(data: &[u8]) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(data).unwrap();
        file.flush().unwrap();
        file
    }

    fn split_with(
        file: &NamedTempFile,
        rule: &FrameRule,
        limits: SplitLimits,
    ) -> (Vec<FrameIndex>, SplitOutcome) {
        let document = BinaryDocument::open(file.path()).unwrap();
        let mut frames = Vec::new();
        let outcome = split_frames(
            &document,
            rule,
            ByteRange::new(0, document.len()),
            &AtomicBool::new(false),
            limits,
            |frame| frames.push(frame),
            |_, _| {},
        )
        .unwrap();
        (frames, outcome)
    }

    fn split(file: &NamedTempFile, rule: &FrameRule) -> Vec<FrameIndex> {
        let (frames, outcome) = split_with(file, rule, SplitLimits::DEFAULT);
        assert_eq!(
            outcome,
            SplitOutcome::Completed {
                frame_count: frames.len(),
                truncated: false,
            }
        );
        frames
    }

    #[test]
    fn parses_sync_pattern_with_wildcards() {
        let pattern = parse_mask_pattern("aa ?? 01").unwrap();
        assert_eq!(pattern.bytes, [0xAA, 0x00, 0x01]);
        assert_eq!(pattern.mask, [0xFF, 0x00, 0xFF]);
    }

    #[test]
    fn rejects_invalid_sync_patterns() {
        assert!(parse_mask_pattern("A").is_err());
        assert!(parse_mask_pattern("5G").is_err());
        assert!(parse_mask_pattern("   ").is_err());
        assert!(parse_mask_pattern("阿a").is_err());
    }

    #[test]
    fn rejects_invalid_rules() {
        assert!(rule().validate().is_ok());

        let mut wildcard = rule();
        wildcard.sync = parse_mask_pattern("??").unwrap();
        assert_eq!(wildcard.validate(), Err(FrameRuleError::WildcardOnlySync));

        let mut long = rule();
        long.sync = parse_mask_pattern(&"AA ".repeat(33)).unwrap();
        assert_eq!(long.validate(), Err(FrameRuleError::SyncTooLong));
    }

    #[test]
    fn splits_frames_between_sync_codes() {
        let data = [
            frame_bytes(&[0x01; 4]),
            frame_bytes(&[]),
            frame_bytes(&[0x02; 10]),
        ]
        .concat();
        let file = file_with(&data);

        let frames = split(&file, &rule());

        assert_eq!(frames.len(), 3);
        assert_eq!(
            (frames[0].start, frames[0].len, frames[0].status),
            (0, 6, FrameStatus::Ok)
        );
        assert_eq!(
            (frames[1].start, frames[1].len, frames[1].status),
            (6, 2, FrameStatus::Ok)
        );
        assert_eq!(
            (frames[2].start, frames[2].len, frames[2].status),
            (8, 12, FrameStatus::Ok)
        );
    }

    #[test]
    fn treats_each_sync_code_as_a_frame_boundary() {
        let file = file_with(&frame_bytes(&[0x01, 0xAA, 0x55, 0x02]));

        let frames = split(&file, &rule());

        assert_eq!(frames.len(), 2);
        assert_eq!((frames[0].start, frames[0].len), (0, 3));
        assert_eq!((frames[1].start, frames[1].len), (3, 3));
    }

    #[test]
    fn ignores_bytes_before_the_first_sync_code() {
        let mut data = vec![0x00, 0x01, 0x02];
        data.extend_from_slice(&frame_bytes(&[0x07; 4]));
        let file = file_with(&data);

        let frames = split(&file, &rule());

        assert_eq!(frames.len(), 1);
        assert_eq!((frames[0].start, frames[0].len), (3, 6));
    }

    #[test]
    fn last_frame_extends_to_end_of_file() {
        let mut data = frame_bytes(&[0x03; 2]);
        data.extend_from_slice(&frame_bytes(&[0x09; 3]));
        let file = file_with(&data);

        let frames = split(&file, &rule());

        assert_eq!(frames.len(), 2);
        assert_eq!(
            (frames[1].start, frames[1].len, frames[1].status),
            (4, 5, FrameStatus::Ok)
        );
        assert_eq!(frames[1].start + frames[1].len, data.len() as u64);
    }

    #[test]
    fn finds_sync_code_across_chunk_boundary() {
        let boundary = FRAME_CHUNK_SIZE as usize;
        let mut data = vec![0; boundary - 1];
        data.extend_from_slice(&[0xAA, 0x55, 1, 2, 3, 0xAA, 0x55, 4]);
        let file = file_with(&data);

        let frames = split(&file, &rule());

        assert_eq!(frames.len(), 2);
        assert_eq!((frames[0].start, frames[0].len), ((boundary - 1) as u64, 5));
        assert_eq!((frames[1].start, frames[1].len), ((boundary + 4) as u64, 3));
    }

    #[test]
    fn treats_empty_file_and_no_match_as_zero_frames() {
        assert!(split(&file_with(&[]), &rule()).is_empty());
        assert!(split(&file_with(&[0x11; 64]), &rule()).is_empty());
    }

    #[test]
    fn honours_frame_and_candidate_limits() {
        let file = file_with(&[frame_bytes(&[]), frame_bytes(&[]), frame_bytes(&[])].concat());
        let (frames, outcome) = split_with(
            &file,
            &rule(),
            SplitLimits {
                max_frames: 2,
                ..SplitLimits::DEFAULT
            },
        );
        assert_eq!(frames.len(), 2);
        assert_eq!(
            outcome,
            SplitOutcome::Completed {
                frame_count: 2,
                truncated: true,
            }
        );

        let bad = file_with(&[0xAA, 0x55, 0, 0xAA, 0x55, 0, 0xAA, 0x55, 0]);
        let (frames, outcome) = split_with(
            &bad,
            &rule(),
            SplitLimits {
                max_candidates: 2,
                ..SplitLimits::DEFAULT
            },
        );
        assert_eq!(frames.len(), 1);
        assert!(frames.iter().all(|frame| frame.status == FrameStatus::Ok));
        assert_eq!(
            outcome,
            SplitOutcome::Completed {
                frame_count: 1,
                truncated: true,
            }
        );
    }

    #[test]
    fn cancel_stops_analysis() {
        let file = file_with(&frame_bytes(&[0x01; 8]));
        let document = BinaryDocument::open(file.path()).unwrap();
        let mut frames = Vec::new();

        let outcome = split_frames(
            &document,
            &rule(),
            ByteRange::new(0, document.len()),
            &AtomicBool::new(true),
            SplitLimits::DEFAULT,
            |frame| frames.push(frame),
            |_, _| {},
        )
        .unwrap();

        assert_eq!(outcome, SplitOutcome::Cancelled);
        assert!(frames.is_empty());
    }
}
