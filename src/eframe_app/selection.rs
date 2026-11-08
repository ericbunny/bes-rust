use std::{
    io::{BufWriter, Write},
    path::Path,
};

use super::document::{BinaryDocument, ByteRange};

const EXPORT_SOURCE_CHUNK_SIZE: u64 = 8 * 1024 * 1024;
const EXPORT_OUTPUT_BUFFER_SIZE: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SelectionMode {
    Continuous,
    Block,
    Row,
    Column,
}

impl SelectionMode {
    pub(super) const ALL: [Self; 4] = [Self::Continuous, Self::Block, Self::Row, Self::Column];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Continuous => "连续选取",
            Self::Block => "块选取",
            Self::Row => "行选取",
            Self::Column => "列选取",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SelectionPoint {
    pub(super) row: usize,
    pub(super) column: usize,
    pub(super) offset: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Selection {
    pub(super) mode: SelectionMode,
    pub(super) anchor: SelectionPoint,
    pub(super) focus: SelectionPoint,
}

impl Selection {
    pub(super) fn contains(self, offset: u64, bytes_per_row: usize, document_len: u64) -> bool {
        if offset >= document_len {
            return false;
        }
        let row = (offset / bytes_per_row as u64) as usize;
        let column = (offset % bytes_per_row as u64) as usize;
        let rows = self.row_range();
        let columns = self.column_range();
        match self.mode {
            SelectionMode::Continuous => {
                let start = self.anchor.offset.min(self.focus.offset);
                let end = self.anchor.offset.max(self.focus.offset);
                (start..=end).contains(&offset)
            }
            SelectionMode::Block => rows.contains(&row) && columns.contains(&column),
            SelectionMode::Row => rows.contains(&row),
            SelectionMode::Column => columns.contains(&column),
        }
    }

    pub(super) fn row_range(self) -> std::ops::RangeInclusive<usize> {
        self.anchor.row.min(self.focus.row)..=self.anchor.row.max(self.focus.row)
    }

    pub(super) fn column_range(self) -> std::ops::RangeInclusive<usize> {
        self.anchor.column.min(self.focus.column)..=self.anchor.column.max(self.focus.column)
    }

    #[cfg(test)]
    pub(super) fn export(
        self,
        document: &BinaryDocument,
        bytes_per_row: usize,
        path: &Path,
    ) -> Result<u64, String> {
        self.export_with_progress(document, bytes_per_row, path, |_| {})
    }

    pub(super) fn export_with_progress(
        self,
        document: &BinaryDocument,
        bytes_per_row: usize,
        path: &Path,
        mut on_progress: impl FnMut(u64),
    ) -> Result<u64, String> {
        let file = std::fs::File::create(path).map_err(|error| error.to_string())?;
        let mut output = BufWriter::with_capacity(EXPORT_OUTPUT_BUFFER_SIZE, file);
        let mut written = 0;
        let document_len = document.len();
        if document_len == 0 {
            return Ok(0);
        }

        let bytes_per_row = bytes_per_row.max(1);
        match self.mode {
            SelectionMode::Continuous => {
                let start = self
                    .anchor
                    .offset
                    .min(self.focus.offset)
                    .min(document_len - 1);
                let end = self
                    .anchor
                    .offset
                    .max(self.focus.offset)
                    .min(document_len - 1)
                    + 1;
                write_contiguous(
                    document,
                    &mut output,
                    start,
                    end,
                    &mut written,
                    &mut on_progress,
                )?;
            }
            SelectionMode::Row => {
                let row_count = document_len.div_ceil(bytes_per_row as u64);
                let rows = self.row_range();
                let first_row = (*rows.start() as u64).min(row_count);
                let last_row = (*rows.end() as u64).min(row_count - 1);
                if first_row <= last_row {
                    let start = first_row * bytes_per_row as u64;
                    let end = ((last_row + 1) * bytes_per_row as u64).min(document_len);
                    write_contiguous(
                        document,
                        &mut output,
                        start,
                        end,
                        &mut written,
                        &mut on_progress,
                    )?;
                }
            }
            SelectionMode::Block | SelectionMode::Column => {
                let row_count = document_len.div_ceil(bytes_per_row as u64);
                let (first_row, last_row) = if self.mode == SelectionMode::Column {
                    (0, row_count - 1)
                } else {
                    let rows = self.row_range();
                    (
                        (*rows.start() as u64).min(row_count),
                        (*rows.end() as u64).min(row_count - 1),
                    )
                };
                if first_row <= last_row {
                    self.write_columns(
                        document,
                        &mut output,
                        bytes_per_row,
                        first_row..=last_row,
                        &mut written,
                        &mut on_progress,
                    )?;
                }
            }
        }
        output.flush().map_err(|error| error.to_string())?;
        Ok(written)
    }

    pub(super) fn export_len(self, document_len: u64, bytes_per_row: usize) -> u64 {
        if document_len == 0 {
            return 0;
        }
        let bytes_per_row = bytes_per_row.max(1) as u64;
        match self.mode {
            SelectionMode::Continuous => {
                let start = self
                    .anchor
                    .offset
                    .min(self.focus.offset)
                    .min(document_len - 1);
                let end = self
                    .anchor
                    .offset
                    .max(self.focus.offset)
                    .min(document_len - 1);
                end - start + 1
            }
            SelectionMode::Row => {
                let row_count = document_len.div_ceil(bytes_per_row);
                let rows = self.row_range();
                let first_row = (*rows.start() as u64).min(row_count);
                let last_row = (*rows.end() as u64).min(row_count - 1);
                if first_row > last_row {
                    0
                } else {
                    ((last_row + 1) * bytes_per_row).min(document_len) - first_row * bytes_per_row
                }
            }
            SelectionMode::Block | SelectionMode::Column => {
                let row_count = document_len.div_ceil(bytes_per_row);
                let (first_row, last_row) = if self.mode == SelectionMode::Column {
                    (0, row_count - 1)
                } else {
                    let rows = self.row_range();
                    (
                        (*rows.start() as u64).min(row_count),
                        (*rows.end() as u64).min(row_count - 1),
                    )
                };
                if first_row > last_row {
                    return 0;
                }
                let columns = self.column_range();
                let first_column = *columns.start() as u64;
                let last_column = *columns.end() as u64;
                let row_width = (last_column + 1)
                    .min(bytes_per_row)
                    .saturating_sub(first_column);
                let mut total = (last_row - first_row + 1).saturating_mul(row_width);
                let document_last_row = row_count - 1;
                if last_row == document_last_row {
                    let last_row_len = document_len - document_last_row * bytes_per_row;
                    let last_row_width = (last_column + 1)
                        .min(last_row_len)
                        .saturating_sub(first_column);
                    total = total.saturating_sub(row_width) + last_row_width;
                }
                total
            }
        }
    }

    fn write_columns(
        self,
        document: &BinaryDocument,
        output: &mut impl Write,
        bytes_per_row: usize,
        rows: std::ops::RangeInclusive<u64>,
        written: &mut u64,
        on_progress: &mut impl FnMut(u64),
    ) -> Result<(), String> {
        let first_row = *rows.start();
        let last_row = *rows.end();
        let columns = self.column_range();
        let first_column = *columns.start();
        let last_column = *columns.end();
        let rows_per_chunk = (EXPORT_SOURCE_CHUNK_SIZE / bytes_per_row as u64).max(1);
        let mut chunk_first_row = first_row;
        while chunk_first_row <= last_row {
            let chunk_last_row = (chunk_first_row + rows_per_chunk - 1).min(last_row);
            let start = chunk_first_row * bytes_per_row as u64;
            let end = ((chunk_last_row + 1) * bytes_per_row as u64).min(document.len());
            let source = document
                .read(ByteRange::new(start, end - start))
                .map_err(|error| error.to_string())?;
            let selected_capacity =
                (chunk_last_row - chunk_first_row + 1) as usize * (last_column - first_column + 1);
            let mut selected = Vec::with_capacity(selected_capacity.min(source.len()));
            for row in source.chunks(bytes_per_row) {
                if first_column < row.len() {
                    let selected_end = (last_column + 1).min(row.len());
                    selected.extend_from_slice(&row[first_column..selected_end]);
                }
            }
            output
                .write_all(&selected)
                .map_err(|error| error.to_string())?;
            *written += selected.len() as u64;
            on_progress(*written);
            chunk_first_row = chunk_last_row + 1;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn curve_series(
        self,
        document: &BinaryDocument,
        bytes_per_row: usize,
        merge_columns: bool,
    ) -> Result<Vec<CurveSeries>, String> {
        self.curve_series_limited(document, bytes_per_row, merge_columns, usize::MAX)
    }

    pub(super) fn curve_series_limited(
        self,
        document: &BinaryDocument,
        bytes_per_row: usize,
        merge_columns: bool,
        max_points: usize,
    ) -> Result<Vec<CurveSeries>, String> {
        if !matches!(self.mode, SelectionMode::Column | SelectionMode::Block) {
            return Err("只有列选取和块选取支持曲线绘制".to_owned());
        }
        let columns = self.column_range();
        let first_column = *columns.start();
        let last_column = *columns.end();
        let column_count = last_column - first_column + 1;
        if merge_columns && column_count > 8 {
            return Err("合并绘制最多支持连续 8 列".to_owned());
        }
        let document_rows = document.len().div_ceil(bytes_per_row as u64) as usize;
        let rows = if self.mode == SelectionMode::Column {
            0..document_rows
        } else {
            let selected = self.row_range();
            *selected.start()..(*selected.end() + 1).min(document_rows)
        };
        let mut series = if merge_columns {
            vec![CurveSeries {
                name: format!("列 {first_column}..{last_column}（大端合并）"),
                points: Vec::new(),
            }]
        } else {
            (first_column..=last_column)
                .map(|column| CurveSeries {
                    name: format!("列 {column}"),
                    points: Vec::new(),
                })
                .collect()
        };
        let row_count = rows.len();
        let sampled_rows = sampled_indices(rows, row_count.min(max_points.max(2)));
        for row in sampled_rows {
            let start = row as u64 * bytes_per_row as u64 + first_column as u64;
            if start >= document.len() {
                break;
            }
            let available = (document.len() - start).min(column_count as u64);
            if available < column_count as u64 {
                continue;
            }
            let bytes = document
                .read(ByteRange::new(start, column_count as u64))
                .map_err(|error| error.to_string())?;
            if merge_columns {
                let value = bytes.iter().fold(0_u64, |value, byte| {
                    value.wrapping_shl(8) | u64::from(*byte)
                });
                series[0].points.push([row as f64, value as f64]);
            } else {
                for (index, byte) in bytes.iter().enumerate() {
                    series[index].points.push([row as f64, f64::from(*byte)]);
                }
            }
        }
        Ok(series)
    }
}

fn sampled_indices(
    rows: std::ops::Range<usize>,
    sample_count: usize,
) -> impl Iterator<Item = usize> {
    let row_count = rows.len();
    let first = rows.start;
    (0..sample_count).map(move |index| {
        if sample_count <= 1 {
            first
        } else {
            first + index * (row_count - 1) / (sample_count - 1)
        }
    })
}

fn write_contiguous(
    document: &BinaryDocument,
    output: &mut impl Write,
    start: u64,
    end: u64,
    written: &mut u64,
    on_progress: &mut impl FnMut(u64),
) -> Result<(), String> {
    let mut offset = start;
    while offset < end {
        let len = (end - offset).min(EXPORT_SOURCE_CHUNK_SIZE);
        let bytes = document
            .read(ByteRange::new(offset, len))
            .map_err(|error| error.to_string())?;
        output
            .write_all(&bytes)
            .map_err(|error| error.to_string())?;
        *written += bytes.len() as u64;
        on_progress(*written);
        offset += len;
    }
    Ok(())
}

pub(super) struct CurveSeries {
    pub(super) name: String,
    pub(super) points: Vec<[f64; 2]>,
}

#[cfg(test)]
mod tests {
    use std::{io::Write, sync::Arc};

    use tempfile::NamedTempFile;

    use crate::BinaryDocument;

    use super::{Selection, SelectionMode, SelectionPoint};

    fn selection(mode: SelectionMode) -> Selection {
        Selection {
            mode,
            anchor: SelectionPoint {
                row: 1,
                column: 2,
                offset: 6,
            },
            focus: SelectionPoint {
                row: 3,
                column: 3,
                offset: 15,
            },
        }
    }

    #[test]
    fn selection_modes_match_expected_cells() {
        assert!(selection(SelectionMode::Continuous).contains(9, 4, 32));
        assert!(!selection(SelectionMode::Continuous).contains(5, 4, 32));
        assert!(selection(SelectionMode::Block).contains(10, 4, 32));
        assert!(!selection(SelectionMode::Block).contains(9, 4, 32));
        assert!(selection(SelectionMode::Row).contains(8, 4, 32));
        assert!(!selection(SelectionMode::Row).contains(16, 4, 32));
        assert!(selection(SelectionMode::Column).contains(22, 4, 32));
        assert!(!selection(SelectionMode::Column).contains(21, 4, 32));
    }

    fn document() -> (NamedTempFile, Arc<BinaryDocument>) {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&(0_u8..16).collect::<Vec<_>>()).unwrap();
        let document = Arc::new(BinaryDocument::open(file.path()).unwrap());
        (file, document)
    }

    #[test]
    fn exports_each_selection_mode_in_row_major_order() {
        let (_source, document) = document();
        let expected = [
            (
                SelectionMode::Continuous,
                vec![6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
            ),
            (SelectionMode::Block, vec![6, 7, 10, 11, 14, 15]),
            (SelectionMode::Row, (4_u8..16).collect()),
            (SelectionMode::Column, vec![2, 3, 6, 7, 10, 11, 14, 15]),
        ];
        for (mode, expected) in expected {
            let output = NamedTempFile::new().unwrap();
            selection(mode).export(&document, 4, output.path()).unwrap();
            assert_eq!(std::fs::read(output.path()).unwrap(), expected);
        }
    }

    #[test]
    fn column_export_handles_a_partial_final_row() {
        let mut source = NamedTempFile::new().unwrap();
        source.write_all(&(0_u8..18).collect::<Vec<_>>()).unwrap();
        let document = BinaryDocument::open(source.path()).unwrap();
        let output = NamedTempFile::new().unwrap();
        let selection = Selection {
            mode: SelectionMode::Column,
            anchor: SelectionPoint {
                row: 0,
                column: 0,
                offset: 0,
            },
            focus: SelectionPoint {
                row: 0,
                column: 1,
                offset: 1,
            },
        };

        selection.export(&document, 4, output.path()).unwrap();

        assert_eq!(
            std::fs::read(output.path()).unwrap(),
            [0, 1, 4, 5, 8, 9, 12, 13, 16, 17]
        );
    }

    #[test]
    fn export_lengths_match_written_bytes_for_all_selection_modes() {
        let (_source, document) = document();
        for mode in SelectionMode::ALL {
            let selection = selection(mode);
            let output = NamedTempFile::new().unwrap();
            let written = selection.export(&document, 4, output.path()).unwrap();
            assert_eq!(selection.export_len(document.len(), 4), written, "{mode:?}");
        }
    }

    #[test]
    fn export_progress_reaches_the_exact_partial_row_total() {
        let mut source = NamedTempFile::new().unwrap();
        source.write_all(&(0_u8..18).collect::<Vec<_>>()).unwrap();
        let document = BinaryDocument::open(source.path()).unwrap();
        let output = NamedTempFile::new().unwrap();
        let selection = Selection {
            mode: SelectionMode::Column,
            anchor: SelectionPoint {
                row: 0,
                column: 0,
                offset: 0,
            },
            focus: SelectionPoint {
                row: 0,
                column: 1,
                offset: 1,
            },
        };
        let mut progress = Vec::new();

        let written = selection
            .export_with_progress(&document, 4, output.path(), |value| progress.push(value))
            .unwrap();

        assert_eq!(selection.export_len(document.len(), 4), 10);
        assert_eq!(written, 10);
        assert_eq!(progress.last(), Some(&10));
    }

    #[test]
    fn column_selection_builds_split_and_big_endian_merged_curves() {
        let (_source, document) = document();
        let selection = Selection {
            mode: SelectionMode::Column,
            anchor: SelectionPoint {
                row: 0,
                column: 1,
                offset: 1,
            },
            focus: SelectionPoint {
                row: 3,
                column: 2,
                offset: 14,
            },
        };
        let split = selection.curve_series(&document, 4, false).unwrap();
        assert_eq!(
            split[0].points,
            [[0.0, 1.0], [1.0, 5.0], [2.0, 9.0], [3.0, 13.0]]
        );
        assert_eq!(
            split[1].points,
            [[0.0, 2.0], [1.0, 6.0], [2.0, 10.0], [3.0, 14.0]]
        );

        let merged = selection.curve_series(&document, 4, true).unwrap();
        assert_eq!(
            merged[0].points,
            [
                [0.0, 0x0102 as f64],
                [1.0, 0x0506 as f64],
                [2.0, 0x090A as f64],
                [3.0, 0x0D0E as f64]
            ]
        );
    }

    #[test]
    fn block_selection_limits_curve_to_selected_rows() {
        let (_source, document) = document();
        let selection = Selection {
            mode: SelectionMode::Block,
            anchor: SelectionPoint {
                row: 1,
                column: 1,
                offset: 5,
            },
            focus: SelectionPoint {
                row: 2,
                column: 2,
                offset: 10,
            },
        };

        let split = selection.curve_series(&document, 4, false).unwrap();
        assert_eq!(split[0].points, [[1.0, 5.0], [2.0, 9.0]]);
        assert_eq!(split[1].points, [[1.0, 6.0], [2.0, 10.0]]);
    }

    #[test]
    fn split_curves_include_all_selected_columns_and_rows() {
        const COLUMN_COUNT: usize = 40;
        const ROW_COUNT: usize = 5_000;

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&vec![0x5A; COLUMN_COUNT * ROW_COUNT])
            .unwrap();
        let document = BinaryDocument::open(file.path()).unwrap();
        let selection = Selection {
            mode: SelectionMode::Column,
            anchor: SelectionPoint {
                row: 0,
                column: 0,
                offset: 0,
            },
            focus: SelectionPoint {
                row: ROW_COUNT - 1,
                column: COLUMN_COUNT - 1,
                offset: (COLUMN_COUNT * ROW_COUNT - 1) as u64,
            },
        };

        let series = selection
            .curve_series(&document, COLUMN_COUNT, false)
            .unwrap();

        assert_eq!(series.len(), COLUMN_COUNT);
        assert!(series.iter().all(|curve| curve.points.len() == ROW_COUNT));
        assert_eq!(series[0].points[ROW_COUNT - 1], [4_999.0, 90.0]);
    }

    #[test]
    fn limited_curve_series_caps_points_and_preserves_endpoints() {
        const ROW_COUNT: usize = 10_000;
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&vec![0x5A; ROW_COUNT]).unwrap();
        let document = BinaryDocument::open(file.path()).unwrap();
        let selection = Selection {
            mode: SelectionMode::Column,
            anchor: SelectionPoint {
                row: 0,
                column: 0,
                offset: 0,
            },
            focus: SelectionPoint {
                row: ROW_COUNT - 1,
                column: 0,
                offset: (ROW_COUNT - 1) as u64,
            },
        };

        let series = selection
            .curve_series_limited(&document, 1, false, 1_000)
            .unwrap();

        assert_eq!(series[0].points.len(), 1_000);
        assert_eq!(series[0].points.first(), Some(&[0.0, 90.0]));
        assert_eq!(series[0].points.last(), Some(&[9_999.0, 90.0]));
    }
}
