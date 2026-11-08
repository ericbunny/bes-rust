use std::{
    fs::File,
    io,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};

use lru::LruCache;
use thiserror::Error;

const PAGE_SIZE: usize = 64 * 1024;
const DEFAULT_CACHE_PAGES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub len: u64,
}

impl ByteRange {
    pub fn new(start: u64, len: u64) -> Self {
        Self { start, len }
    }

    pub fn end(self) -> Option<u64> {
        self.start.checked_add(self.len)
    }
}

#[derive(Debug, Error)]
pub enum DocumentError {
    #[error("无法访问文件：{0}")]
    Io(#[from] io::Error),
    #[error("读取范围超出文件：offset={start:#X}, length={len}, file_length={file_len}")]
    OutOfBounds { start: u64, len: u64, file_len: u64 },
    #[error("读取范围过大，无法放入当前进程内存")]
    RangeTooLarge,
    #[error("文件已被外部修改，请重新打开")]
    FileChanged,
}

#[derive(Debug)]
pub struct BinaryDocument {
    file: File,
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
    cache: Mutex<LruCache<u64, Arc<[u8]>>>,
}

impl BinaryDocument {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DocumentError> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path)?;
        let metadata = file.metadata()?;
        let cache_size = NonZeroUsize::new(DEFAULT_CACHE_PAGES).expect("cache size is non-zero");

        Ok(Self {
            file,
            path,
            len: metadata.len(),
            modified: metadata.modified().ok(),
            cache: Mutex::new(LruCache::new(cache_size)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn read(&self, range: ByteRange) -> Result<Vec<u8>, DocumentError> {
        let end = range.end().ok_or(DocumentError::OutOfBounds {
            start: range.start,
            len: range.len,
            file_len: self.len,
        })?;
        if end > self.len {
            return Err(DocumentError::OutOfBounds {
                start: range.start,
                len: range.len,
                file_len: self.len,
            });
        }
        let output_len = usize::try_from(range.len).map_err(|_| DocumentError::RangeTooLarge)?;
        if output_len == 0 {
            return Ok(Vec::new());
        }

        let mut output = Vec::with_capacity(output_len);
        let mut offset = range.start;
        while offset < end {
            let page_index = offset / PAGE_SIZE as u64;
            let page_offset = (offset % PAGE_SIZE as u64) as usize;
            let page = self.page(page_index)?;
            let remaining =
                usize::try_from(end - offset).map_err(|_| DocumentError::RangeTooLarge)?;
            let count = remaining.min(page.len().saturating_sub(page_offset));
            if count == 0 {
                return Err(DocumentError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "file became shorter while reading",
                )));
            }
            output.extend_from_slice(&page[page_offset..page_offset + count]);
            offset += count as u64;
        }
        Ok(output)
    }

    pub fn check_unchanged(&self) -> Result<(), DocumentError> {
        let metadata = self.file.metadata()?;
        if metadata.len() != self.len || metadata.modified().ok() != self.modified {
            return Err(DocumentError::FileChanged);
        }
        Ok(())
    }

    fn page(&self, page_index: u64) -> Result<Arc<[u8]>, DocumentError> {
        if let Some(page) = self
            .cache
            .lock()
            .expect("page cache poisoned")
            .get(&page_index)
        {
            return Ok(page.clone());
        }

        let offset = page_index * PAGE_SIZE as u64;
        let remaining = self.len.saturating_sub(offset);
        let wanted = remaining.min(PAGE_SIZE as u64) as usize;
        let mut page = vec![0; wanted];
        let mut filled = 0;
        while filled < wanted {
            let count = read_at(&self.file, &mut page[filled..], offset + filled as u64)?;
            if count == 0 {
                break;
            }
            filled += count;
        }
        page.truncate(filled);
        let page = Arc::<[u8]>::from(page);
        self.cache
            .lock()
            .expect("page cache poisoned")
            .put(page_index, Arc::clone(&page));
        Ok(page)
    }
}

#[cfg(windows)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buffer, offset)
}

#[cfg(unix)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buffer, offset)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::{BinaryDocument, ByteRange, DocumentError, PAGE_SIZE};

    #[test]
    fn reads_across_page_boundary() {
        let mut file = NamedTempFile::new().unwrap();
        let data: Vec<u8> = (0..PAGE_SIZE + 32).map(|index| index as u8).collect();
        file.write_all(&data).unwrap();
        let document = BinaryDocument::open(file.path()).unwrap();

        let bytes = document
            .read(ByteRange::new((PAGE_SIZE - 8) as u64, 24))
            .unwrap();

        assert_eq!(bytes, data[PAGE_SIZE - 8..PAGE_SIZE + 16]);
    }

    #[test]
    fn rejects_out_of_bounds_range() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&[1, 2, 3]).unwrap();
        let document = BinaryDocument::open(file.path()).unwrap();

        let error = document.read(ByteRange::new(2, 2)).unwrap_err();

        assert!(matches!(error, DocumentError::OutOfBounds { .. }));
    }
}
