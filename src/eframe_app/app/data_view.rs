use std::ops::Range;

use eframe::egui::{
    self, Align2, Color32, FontId, Rect, RichText, ScrollArea, Sense, TextStyle, Vec2,
};

use super::{
    ASCII_CELL_WIDTH, BesApp, DisplayMode, NavigationTarget, accent_text, muted_text, subtle_text,
    ui_palette,
};
use crate::eframe_app::{
    BinaryDocument, ByteRange, FrameIndex,
    selection::{Selection, SelectionMode, SelectionPoint},
};

const MAX_VIRTUAL_SCROLL_ROWS: usize = 1_000_000;

impl BesApp {
    #[doc = "按当前显示模式绘制可滚动的二进制数据视图。"]
    pub(super) fn hex_view(&mut self, ui: &mut egui::Ui) {
        let Some(document) = self.document.as_ref() else {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.label(
                        RichText::new("📂")
                            .size(48.0)
                            .color(accent_text(ui)),
                    );
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new("打开或拖拽文件开始分析")
                            .size(18.0)
                            .color(muted_text(ui)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("支持任意二进制文件格式")
                            .size(13.0)
                            .color(subtle_text(ui)),
                    );
                });
            });
            return;
        };
        if document.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("空文件")
                        .size(16.0)
                        .color(subtle_text(ui)),
                );
            });
            return;
        }
        if self.display_mode == DisplayMode::Bitmap {
            self.bitmap_view(ui);
            return;
        }
        if self.display_mode == DisplayMode::VariableFrame {
            self.variable_frame_view(ui);
            return;
        }

        let bytes_per_row = self.bytes_per_row.max(1);
        let row_count_u64 = document.len().div_ceil(bytes_per_row as u64);
        let row_count = usize::try_from(row_count_u64).unwrap_or(usize::MAX);
        let scroll_row_count = virtual_scroll_row_count(row_count);
        let row_height = ui.text_style_height(&TextStyle::Monospace) + 5.0;
        let matching = &self.search_results;
        let display_mode = self.display_mode;
        let vertical_axis_mode = self.vertical_axis_mode;
        let navigation = self.pending_navigation.take();
        let content_width = Self::AXIS_WIDTH
            + match display_mode {
                DisplayMode::Bitmap => unreachable!(),
                DisplayMode::FixedBitText => bytes_per_row as f32 * Self::BIT_TEXT_CELL_WIDTH,
                DisplayMode::FixedHex => bytes_per_row as f32 * Self::BYTE_CELL_WIDTH,
                DisplayMode::Character => bytes_per_row as f32 * ASCII_CELL_WIDTH,
                DisplayMode::HexCharacterMapping => {
                    bytes_per_row as f32 * (Self::BYTE_CELL_WIDTH + ASCII_CELL_WIDTH)
                }
                DisplayMode::VariableFrame => unreachable!(),
            };
        let mut scroll = ScrollArea::both().auto_shrink([false, false]);
        scroll = match navigation {
            Some(NavigationTarget::FirstRow) => scroll.vertical_scroll_offset(0.0),
            Some(NavigationTarget::LastRow) => scroll.vertical_scroll_offset(
                last_row_scroll_offset(scroll_row_count, row_height, ui.spacing().item_spacing.y),
            ),
            Some(NavigationTarget::FirstColumn) => scroll.horizontal_scroll_offset(0.0),
            Some(NavigationTarget::LastColumn) => scroll.horizontal_scroll_offset(content_width),
            None => scroll,
        };
        scroll.show_rows(ui, row_height, scroll_row_count, |ui, rows| {
            let rows = visible_document_rows(rows, scroll_row_count, row_count);
            for row in rows {
                let start = row as u64 * bytes_per_row as u64;
                let len = (document.len() - start).min(bytes_per_row as u64);
                let Ok(bytes) = document.read(ByteRange::new(start, len)) else {
                    ui.colored_label(Color32::RED, "读取失败");
                    continue;
                };
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let (axis_placeholder, _) = ui.allocate_exact_size(
                        Vec2::new(Self::AXIS_WIDTH, row_height),
                        Sense::hover(),
                    );
                    match display_mode {
                        DisplayMode::Bitmap => unreachable!(),
                        DisplayMode::VariableFrame => unreachable!(),
                        DisplayMode::FixedBitText => paint_text_cells(
                            ui,
                            &bytes,
                            start,
                            bytes_per_row,
                            row_height,
                            Self::BIT_TEXT_CELL_WIDTH,
                            &mut self.cursor,
                            matching,
                            self.selection_mode,
                            &mut self.selection,
                            document.len(),
                            format_bit_text,
                        ),
                        DisplayMode::FixedHex => paint_text_cells(
                            ui,
                            &bytes,
                            start,
                            bytes_per_row,
                            row_height,
                            Self::BYTE_CELL_WIDTH,
                            &mut self.cursor,
                            matching,
                            self.selection_mode,
                            &mut self.selection,
                            document.len(),
                            |byte| format!("{byte:02X}"),
                        ),
                        DisplayMode::Character => paint_character_cells(
                            ui,
                            &bytes,
                            start,
                            bytes_per_row,
                            row_height,
                            &mut self.cursor,
                            matching,
                            self.selection_mode,
                            &mut self.selection,
                            document.len(),
                        ),
                        DisplayMode::HexCharacterMapping => {
                            paint_text_cells(
                                ui,
                                &bytes,
                                start,
                                bytes_per_row,
                                row_height,
                                Self::BYTE_CELL_WIDTH,
                                &mut self.cursor,
                                matching,
                                self.selection_mode,
                                &mut self.selection,
                                document.len(),
                                |byte| format!("{byte:02X}"),
                            );
                            paint_character_cells(
                                ui,
                                &bytes,
                                start,
                                bytes_per_row,
                                row_height,
                                &mut self.cursor,
                                matching,
                                self.selection_mode,
                                &mut self.selection,
                                document.len(),
                            );
                        }
                    }
                    let axis_rect = fixed_axis_rect(ui.clip_rect(), axis_placeholder);
                    ui.painter()
                        .rect_filled(axis_rect, 0.0, ui.visuals().panel_fill);
                    paint_axis_divider(ui, axis_rect);
                    ui.painter().text(
                        axis_rect.right_center() - Vec2::new(4.0, 0.0),
                        Align2::RIGHT_CENTER,
                        vertical_axis_mode.value(row, start, len),
                        FontId::monospace(ui.text_style_height(&TextStyle::Monospace)),
                        ui.visuals().text_color(),
                    );
                });
            }
        });
    }

    #[doc = "按帧下标绘制不定帧长视图，每行按需读取并显示一个完整帧。"]
    pub(super) fn variable_frame_view(&mut self, ui: &mut egui::Ui) {
        if self.frames.is_empty() {
            let message = if self.frame_task.is_some() {
                "正在切分帧…"
            } else {
                "请在工具栏“帧”分组填写同步码，然后点击“切分”"
            };
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new(message)
                        .size(15.0)
                        .color(subtle_text(ui)),
                );
            });
            return;
        }
        let Some(document) = self.document.as_ref() else {
            return;
        };
        let frame_count = self.frames.len();
        let max_frame_columns = usize::try_from(self.frame_longest_len).unwrap_or(usize::MAX);
        let row_height = ui.text_style_height(&TextStyle::Monospace) + 5.0;
        let matching = &self.search_results;
        let vertical_axis_mode = self.vertical_axis_mode;
        let content_width = Self::AXIS_WIDTH + max_frame_columns as f32 * Self::BYTE_CELL_WIDTH;
        let navigation = self.pending_navigation.take();
        let frame_scroll = self.pending_frame_scroll.take();
        let mut scroll = ScrollArea::both().auto_shrink([false, false]);
        if let Some(index) = frame_scroll {
            scroll = scroll.vertical_scroll_offset(frame_scroll_offset(
                index.min(frame_count - 1),
                row_height,
                ui.spacing().item_spacing.y,
            ));
        }
        scroll = match navigation {
            Some(NavigationTarget::FirstColumn) => scroll.horizontal_scroll_offset(0.0),
            Some(NavigationTarget::LastColumn) => scroll.horizontal_scroll_offset(content_width),
            _ => scroll,
        };
        scroll.show_rows(ui, row_height, frame_count, |ui, rows| {
            for index in rows {
                let Some(frame) = self.frames.get(index).copied() else {
                    continue;
                };
                let frame_columns = frame_column_count(frame);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let (axis_placeholder, _) = ui.allocate_exact_size(
                        Vec2::new(Self::AXIS_WIDTH, row_height),
                        Sense::hover(),
                    );
                    if paint_frame_cells(
                        ui,
                        document,
                        frame,
                        frame_columns,
                        row_height,
                        &mut self.cursor,
                        matching,
                        document.len(),
                    )
                    .is_err()
                    {
                        ui.colored_label(Color32::RED, "读取失败");
                    }
                    let axis_rect = fixed_axis_rect(ui.clip_rect(), axis_placeholder);
                    ui.painter()
                        .rect_filled(axis_rect, 0.0, ui.visuals().panel_fill);
                    paint_axis_divider(ui, axis_rect);
                    ui.painter().text(
                        axis_rect.right_center() - Vec2::new(4.0, 0.0),
                        Align2::RIGHT_CENTER,
                        vertical_axis_mode.value(index, frame.start, frame.len),
                        FontId::monospace(ui.text_style_height(&TextStyle::Monospace)),
                        ui.visuals().text_color(),
                    );
                });
            }
        });
    }

    #[doc = "以位图形式绘制可见范围内的二进制数据。"]
    pub(super) fn bitmap_view(&mut self, ui: &mut egui::Ui) {
        let Some(document) = self.document.as_ref() else {
            return;
        };
        let bytes_per_row = self.bytes_per_row.max(1);
        let row_count_u64 = document.len().div_ceil(bytes_per_row as u64);
        let row_count = usize::try_from(row_count_u64).unwrap_or(usize::MAX);
        let scroll_row_count = virtual_scroll_row_count(row_count);
        let content_width = Self::AXIS_WIDTH + bytes_per_row as f32 * 8.0 * Self::BITMAP_PIXEL_SIZE;
        let matching = &self.search_results;
        let vertical_axis_mode = self.vertical_axis_mode;

        let navigation = self.pending_navigation.take();
        let mut scroll = ScrollArea::both().auto_shrink([false, false]);
        scroll = match navigation {
            Some(NavigationTarget::FirstRow) => scroll.vertical_scroll_offset(0.0),
            Some(NavigationTarget::LastRow) => {
                scroll.vertical_scroll_offset(last_row_scroll_offset(
                    scroll_row_count,
                    Self::BITMAP_PIXEL_SIZE,
                    ui.spacing().item_spacing.y,
                ))
            }
            Some(NavigationTarget::FirstColumn) => scroll.horizontal_scroll_offset(0.0),
            Some(NavigationTarget::LastColumn) => scroll.horizontal_scroll_offset(content_width),
            None => scroll,
        };
        scroll.show_rows(ui, Self::BITMAP_PIXEL_SIZE, scroll_row_count, |ui, rows| {
            let rows = visible_document_rows(rows, scroll_row_count, row_count);
            let (content_rect, response) = ui.allocate_exact_size(
                Vec2::new(content_width, rows.len() as f32 * Self::BITMAP_PIXEL_SIZE),
                Sense::click_and_drag(),
            );
            let bitmap_rect = Rect::from_min_max(
                egui::Pos2::new(content_rect.left() + Self::AXIS_WIDTH, content_rect.top()),
                content_rect.max,
            );
            let axis_step = (ui.text_style_height(&TextStyle::Monospace) / Self::BITMAP_PIXEL_SIZE)
                .ceil()
                .max(1.0) as usize;
            let visible_bits = visible_column_range(
                bitmap_rect,
                ui.clip_rect(),
                bytes_per_row * 8,
                Self::BITMAP_PIXEL_SIZE,
            );
            if visible_bits.is_empty() || rows.is_empty() {
                return;
            }

            let first_byte = visible_bits.start / 8;
            let last_byte = visible_bits.end.div_ceil(8).min(bytes_per_row);
            let read_start = rows.start as u64 * bytes_per_row as u64 + first_byte as u64;
            let read_end = ((rows.end - 1) as u64 * bytes_per_row as u64 + last_byte as u64)
                .min(document.len());
            let Ok(data) = document.read(ByteRange::new(
                read_start,
                read_end.saturating_sub(read_start),
            )) else {
                ui.colored_label(Color32::RED, "读取失败");
                return;
            };

            let image_width = visible_bits.len();
            let image_height = rows.len();
            let mut pixels = vec![Color32::WHITE; image_width * image_height];
            for (image_y, row) in rows.clone().enumerate() {
                for (image_x, bit_column) in visible_bits.clone().enumerate() {
                    let byte_column = bit_column / 8;
                    let offset = row as u64 * bytes_per_row as u64 + byte_column as u64;
                    if offset >= document.len() {
                        continue;
                    }
                    let byte = data[(offset - read_start) as usize];
                    let is_set = byte & (1 << (7 - bit_column % 8)) != 0;
                    let selected = self.cursor == Some(offset)
                        || self.selection.is_some_and(|selection| {
                            selection.contains(offset, bytes_per_row, document.len())
                        });
                    let is_match = matching.binary_search(&offset).is_ok();
                    pixels[image_y * image_width + image_x] =
                        bitmap_pixel_color(is_set, selected, is_match);
                }
            }

            let image = egui::ColorImage::new([image_width, image_height], pixels);
            if let Some(texture) = &mut self.bitmap_texture {
                texture.set(image, egui::TextureOptions::NEAREST);
            } else {
                self.bitmap_texture = Some(ui.ctx().load_texture(
                    "bes_bitmap_view",
                    image,
                    egui::TextureOptions::NEAREST,
                ));
            }
            let texture = self.bitmap_texture.as_ref().expect("texture was created");
            let image_rect = Rect::from_min_size(
                bitmap_rect.min
                    + Vec2::new(visible_bits.start as f32 * Self::BITMAP_PIXEL_SIZE, 0.0),
                Vec2::new(
                    image_width as f32 * Self::BITMAP_PIXEL_SIZE,
                    image_height as f32 * Self::BITMAP_PIXEL_SIZE,
                ),
            );
            ui.painter().image(
                texture.id(),
                image_rect,
                Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );

            let axis_rect = fixed_axis_rect(
                ui.clip_rect(),
                Rect::from_min_size(
                    content_rect.min,
                    Vec2::new(Self::AXIS_WIDTH, content_rect.height()),
                ),
            );
            ui.painter()
                .rect_filled(axis_rect, 0.0, ui.visuals().panel_fill);
            paint_axis_divider(ui, axis_rect);
            let first_axis_row = rows.start.div_ceil(axis_step) * axis_step;
            for axis_row in (first_axis_row..rows.end).step_by(axis_step) {
                let start = axis_row as u64 * bytes_per_row as u64;
                let len = (document.len() - start).min(bytes_per_row as u64);
                ui.painter().text(
                    egui::Pos2::new(
                        axis_rect.right() - 4.0,
                        axis_rect.top() + (axis_row - rows.start) as f32 * Self::BITMAP_PIXEL_SIZE,
                    ),
                    Align2::RIGHT_TOP,
                    vertical_axis_mode.value(axis_row, start, len),
                    FontId::monospace(12.0),
                    ui.visuals().text_color(),
                );
            }

            update_selection_from_response(
                ui,
                response,
                bitmap_rect,
                8.0 * Self::BITMAP_PIXEL_SIZE,
                Self::BITMAP_PIXEL_SIZE,
                rows.start as u64 * bytes_per_row as u64,
                &mut self.cursor,
                self.selection_mode,
                &mut self.selection,
                bytes_per_row,
                document.len(),
            );
        });
    }
}

pub(super) fn fixed_axis_rect(viewport: Rect, scrolled_axis_rect: Rect) -> Rect {
    Rect::from_min_size(
        egui::Pos2::new(viewport.left(), scrolled_axis_rect.top()),
        scrolled_axis_rect.size(),
    )
}

pub(super) fn last_row_scroll_offset(row_count: usize, row_height: f32, row_spacing: f32) -> f32 {
    ((row_height + row_spacing) * row_count as f32 - row_spacing).max(0.0)
}

pub(super) fn virtual_scroll_row_count(document_row_count: usize) -> usize {
    document_row_count.min(MAX_VIRTUAL_SCROLL_ROWS)
}

pub(super) fn visible_document_rows(
    visible_scroll_rows: Range<usize>,
    scroll_row_count: usize,
    document_row_count: usize,
) -> Range<usize> {
    let visible_count = visible_scroll_rows.len().min(document_row_count);
    let max_document_start = document_row_count.saturating_sub(visible_count);
    let max_scroll_start = scroll_row_count.saturating_sub(visible_count);
    let start = if max_scroll_start == 0 {
        0
    } else {
        (visible_scroll_rows.start as u128 * max_document_start as u128 / max_scroll_start as u128)
            as usize
    };
    start..start + visible_count
}

fn paint_axis_divider(ui: &egui::Ui, axis_rect: Rect) {
    let x = axis_rect.right() - 0.5;
    ui.painter().line_segment(
        [
            egui::pos2(x, axis_rect.top()),
            egui::pos2(x, axis_rect.bottom()),
        ],
        ui.visuals().window_stroke,
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_text_cells(
    ui: &mut egui::Ui,
    bytes: &[u8],
    start: u64,
    total_columns: usize,
    row_height: f32,
    cell_width: f32,
    cursor: &mut Option<u64>,
    matching: &[u64],
    selection_mode: SelectionMode,
    selection: &mut Option<Selection>,
    document_len: u64,
    format_byte: impl Fn(u8) -> String,
) {
    let (content_rect, response) = ui.allocate_exact_size(
        Vec2::new(total_columns as f32 * cell_width, row_height),
        Sense::click_and_drag(),
    );
    for column in visible_column_range(content_rect, ui.clip_rect(), bytes.len(), cell_width) {
        let offset = start + column as u64;
        let cell_rect = Rect::from_min_size(
            content_rect.min + Vec2::new(column as f32 * cell_width, 0.0),
            Vec2::new(cell_width, row_height),
        );
        let selection_highlighted = selection
            .is_some_and(|selection| selection.contains(offset, total_columns, document_len));
        paint_cell_background(
            ui,
            cell_rect,
            *cursor == Some(offset),
            selection_highlighted,
            matching,
            offset,
        );
        ui.painter().text(
            cell_rect.center(),
            Align2::CENTER_CENTER,
            format_byte(bytes[column]),
            FontId::monospace(14.0),
            if selection_highlighted || *cursor == Some(offset) {
                ui.visuals().selection.stroke.color
            } else {
                ui.visuals().text_color()
            },
        );
    }
    update_selection_from_response(
        ui,
        response,
        content_rect,
        cell_width,
        row_height,
        start,
        cursor,
        selection_mode,
        selection,
        total_columns,
        document_len,
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_character_cells(
    ui: &mut egui::Ui,
    bytes: &[u8],
    start: u64,
    total_columns: usize,
    row_height: f32,
    cursor: &mut Option<u64>,
    matching: &[u64],
    selection_mode: SelectionMode,
    selection: &mut Option<Selection>,
    document_len: u64,
) {
    let (content_rect, response) = ui.allocate_exact_size(
        Vec2::new(total_columns as f32 * ASCII_CELL_WIDTH, row_height),
        Sense::click_and_drag(),
    );
    let visible = visible_column_range(content_rect, ui.clip_rect(), bytes.len(), ASCII_CELL_WIDTH);
    let mut text = String::with_capacity(visible.len());
    for column in visible.clone() {
        let offset = start + column as u64;
        let cell_rect = Rect::from_min_size(
            content_rect.min + Vec2::new(column as f32 * ASCII_CELL_WIDTH, 0.0),
            Vec2::new(ASCII_CELL_WIDTH, row_height),
        );
        paint_cell_background(
            ui,
            cell_rect,
            *cursor == Some(offset),
            selection
                .is_some_and(|selection| selection.contains(offset, total_columns, document_len)),
            matching,
            offset,
        );
        text.push(display_character(bytes[column]));
    }
    if !text.is_empty() {
        ui.painter().text(
            content_rect.min + Vec2::new(visible.start as f32 * ASCII_CELL_WIDTH, row_height / 2.0),
            Align2::LEFT_CENTER,
            text,
            FontId::monospace(14.0),
            ui.visuals().text_color(),
        );
    }
    update_selection_from_response(
        ui,
        response,
        content_rect,
        ASCII_CELL_WIDTH,
        row_height,
        start,
        cursor,
        selection_mode,
        selection,
        total_columns,
        document_len,
    );
}

fn bitmap_pixel_color(is_set: bool, selected: bool, is_match: bool) -> Color32 {
    match (selected, is_match, is_set) {
        (true, _, true) => Color32::from_rgb(70, 25, 150),
        (true, _, false) => Color32::from_rgb(205, 180, 255),
        (false, true, true) => Color32::from_rgb(130, 90, 0),
        (false, true, false) => Color32::from_rgb(255, 225, 120),
        (false, false, true) => Color32::BLACK,
        (false, false, false) => Color32::WHITE,
    }
}

pub(super) fn selection_background_color() -> Color32 {
    Color32::from_rgba_unmultiplied(70, 145, 220, 105)
}

fn paint_cell_background(
    ui: &egui::Ui,
    rect: Rect,
    cursor: bool,
    selected: bool,
    matching: &[u64],
    offset: u64,
) {
    let color = if selected {
        Some(selection_background_color())
    } else if cursor {
        Some(ui.visuals().selection.bg_fill)
    } else if matching.binary_search(&offset).is_ok() {
        Some(ui_palette(ui).match_fill)
    } else {
        None
    };
    if let Some(color) = color {
        ui.painter().rect_filled(rect, 2.0, color);
    }
}

#[allow(clippy::too_many_arguments)]
fn update_selection_from_response(
    ui: &egui::Ui,
    response: egui::Response,
    content_rect: Rect,
    cell_width: f32,
    row_height: f32,
    start: u64,
    cursor: &mut Option<u64>,
    mode: SelectionMode,
    selection: &mut Option<Selection>,
    bytes_per_row: usize,
    document_len: u64,
) {
    if response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let scroll_delta = edge_auto_scroll_delta(pointer, ui.clip_rect(), 32.0, 24.0);
        if scroll_delta != Vec2::ZERO {
            ui.scroll_with_delta(scroll_delta);
            ui.ctx().request_repaint();
        }
    }
    if !(response.clicked() || response.drag_started() || response.dragged()) {
        return;
    }
    let Some(pointer) = response.interact_pointer_pos() else {
        return;
    };
    let anchor_row = (start / bytes_per_row as u64) as isize;
    let row_delta = ((pointer.y - content_rect.top()) / row_height).floor() as isize;
    let row_count = document_len.div_ceil(bytes_per_row as u64) as isize;
    let row = (anchor_row + row_delta).clamp(0, row_count.saturating_sub(1)) as usize;
    let raw_column = ((pointer.x - content_rect.left()) / cell_width).floor() as isize;
    let column = raw_column.clamp(0, bytes_per_row.saturating_sub(1) as isize) as usize;
    let offset =
        (row as u64 * bytes_per_row as u64 + column as u64).min(document_len.saturating_sub(1));
    let point = SelectionPoint {
        row: (offset / bytes_per_row as u64) as usize,
        column: (offset % bytes_per_row as u64) as usize,
        offset,
    };
    *cursor = Some(offset);
    if response.clicked() || response.drag_started() {
        *selection = Some(Selection {
            mode,
            anchor: point,
            focus: point,
        });
    } else if let Some(selection) = selection
        .as_mut()
        .filter(|selection| selection.mode == mode)
    {
        selection.focus = point;
    }
}

pub(super) fn edge_auto_scroll_delta(
    pointer: egui::Pos2,
    viewport: Rect,
    edge_size: f32,
    max_speed: f32,
) -> Vec2 {
    fn axis_delta(position: f32, min: f32, max: f32, edge_size: f32, max_speed: f32) -> f32 {
        if position < min + edge_size {
            ((min + edge_size - position) / edge_size).clamp(0.0, 1.0) * max_speed
        } else if position > max - edge_size {
            -((position - (max - edge_size)) / edge_size).clamp(0.0, 1.0) * max_speed
        } else {
            0.0
        }
    }

    Vec2::new(
        axis_delta(
            pointer.x,
            viewport.left(),
            viewport.right(),
            edge_size,
            max_speed,
        ),
        axis_delta(
            pointer.y,
            viewport.top(),
            viewport.bottom(),
            edge_size,
            max_speed,
        ),
    )
}

pub(super) fn display_character(byte: u8) -> char {
    if byte.is_ascii_graphic() || byte == b' ' || byte >= 0xA0 {
        char::from(byte)
    } else {
        '·'
    }
}

pub(super) fn format_bit_text(byte: u8) -> String {
    format!("{byte:08b}")
}

fn visible_column_range(
    content: Rect,
    clip: Rect,
    column_count: usize,
    column_width: f32,
) -> std::ops::Range<usize> {
    let first = ((clip.left() - content.left()).max(0.0) / column_width).floor() as usize;
    let last = ((clip.right() - content.left()).max(0.0) / column_width).ceil() as usize;
    first.min(column_count)..last.min(column_count)
}

#[allow(clippy::too_many_arguments)]
fn paint_frame_cells(
    ui: &mut egui::Ui,
    document: &BinaryDocument,
    frame: FrameIndex,
    total_columns: usize,
    row_height: f32,
    cursor: &mut Option<u64>,
    matching: &[u64],
    document_len: u64,
) -> Result<(), crate::eframe_app::DocumentError> {
    let (content_rect, response) = ui.allocate_exact_size(
        Vec2::new(total_columns as f32 * BesApp::BYTE_CELL_WIDTH, row_height),
        Sense::click_and_drag(),
    );
    let hex_rect = Rect::from_min_size(
        content_rect.min,
        Vec2::new(total_columns as f32 * BesApp::BYTE_CELL_WIDTH, row_height),
    );
    let hex_columns = visible_column_range(
        hex_rect,
        ui.clip_rect(),
        total_columns,
        BesApp::BYTE_CELL_WIDTH,
    );
    let hex_bytes = document.read(ByteRange::new(
        frame.start + hex_columns.start as u64,
        hex_columns.len() as u64,
    ))?;
    for (column, byte) in hex_columns.zip(hex_bytes) {
        let offset = frame.start + column as u64;
        let cell_rect = Rect::from_min_size(
            hex_rect.min + Vec2::new(column as f32 * BesApp::BYTE_CELL_WIDTH, 0.0),
            Vec2::new(BesApp::BYTE_CELL_WIDTH, row_height),
        );
        paint_cell_background(
            ui,
            cell_rect,
            *cursor == Some(offset),
            false,
            matching,
            offset,
        );
        ui.painter().text(
            cell_rect.center(),
            Align2::CENTER_CENTER,
            format!("{byte:02X}"),
            FontId::monospace(14.0),
            if *cursor == Some(offset) {
                ui.visuals().selection.stroke.color
            } else {
                ui.visuals().text_color()
            },
        );
    }

    ui.painter().line_segment(
        [content_rect.left_bottom(), content_rect.right_bottom()],
        ui.visuals().window_stroke,
    );

    if (response.clicked() || response.dragged())
        && let Some(pointer) = response.interact_pointer_pos()
        && let Some(offset) =
            frame_click_offset(pointer.x, hex_rect, frame, total_columns, document_len)
    {
        *cursor = Some(offset);
    }
    Ok(())
}

fn frame_column_count(frame: FrameIndex) -> usize {
    usize::try_from(frame.len).unwrap_or(usize::MAX)
}

#[doc = "把帧视图中的鼠标横坐标换算为帧内字节偏移，落在帧尾之后返回 `None`。"]
pub(super) fn frame_click_offset(
    pointer_x: f32,
    hex_rect: Rect,
    frame: FrameIndex,
    bytes_per_row: usize,
    document_len: u64,
) -> Option<u64> {
    if !(hex_rect.left()..hex_rect.right()).contains(&pointer_x) {
        return None;
    }
    let column = ((pointer_x - hex_rect.left()) / BesApp::BYTE_CELL_WIDTH).floor() as usize;
    let visible = usize::try_from(frame.len)
        .unwrap_or(usize::MAX)
        .min(bytes_per_row);
    if column >= visible {
        return None;
    }
    let offset = frame.start + column as u64;
    (offset < document_len).then_some(offset)
}

#[doc = "返回把指定帧滚动到视口顶部所需的垂直滚动偏移。"]
pub(super) fn frame_scroll_offset(index: usize, row_height: f32, row_spacing: f32) -> f32 {
    index as f32 * (row_height + row_spacing)
}
