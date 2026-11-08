use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use eframe::egui::{self, Align2, Color32, FontId, Rect, RichText, ScrollArea, TextStyle, Vec2};
use egui_dock::DockState;
use egui_inbox::UiInbox;

use crate::utils::egui_helper::load_fonts;

mod curve_view;
mod data_view;
mod workspace;

#[cfg(test)]
use curve_view::{format_nearest_curve_point, nearest_point_at_x};
#[cfg(test)]
use data_view::{
    display_character, edge_auto_scroll_delta, fixed_axis_rect, format_bit_text,
    frame_click_offset, frame_scroll_offset, last_row_scroll_offset, selection_background_color,
    virtual_scroll_row_count, visible_document_rows,
};
use workspace::default_dock_state;
#[cfg(test)]
use workspace::{open_workspace_tab, toolbar_fraction_for_height};

use super::selection::CurveSeries;
#[cfg(test)]
use super::selection::SelectionPoint;
use super::{
    document::{BinaryDocument, ByteRange},
    frame::{
        FrameEvent, FrameIndex, FrameRule, FrameStatus, FrameTask, parse_mask_pattern,
        start_frame_analysis,
    },
    search::{SearchEvent, SearchTask, parse_hex_pattern, start_search},
    selection::{Selection, SelectionMode},
};

const ASCII_CELL_WIDTH: f32 = 8.5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DisplayMode {
    Bitmap,
    FixedBitText,
    FixedHex,
    Character,
    HexCharacterMapping,
    VariableFrame,
}

impl DisplayMode {
    const ALL: [Self; 6] = [
        Self::Bitmap,
        Self::FixedBitText,
        Self::FixedHex,
        Self::Character,
        Self::HexCharacterMapping,
        Self::VariableFrame,
    ];

    #[doc = "返回显示模式对应的界面标签。"]
    const fn label(self) -> &'static str {
        match self {
            Self::Bitmap => "比特位图",
            Self::FixedBitText => "等长帧比特字符",
            Self::FixedHex => "等长帧十六进制",
            Self::Character => "字符方式",
            Self::HexCharacterMapping => "十六进制字符对应",
            Self::VariableFrame => "不定帧长",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VerticalAxisMode {
    FrameNumber,
    FrameLength,
    StartBit,
}

impl VerticalAxisMode {
    const ALL: [Self; 3] = [Self::FrameNumber, Self::FrameLength, Self::StartBit];

    #[doc = "返回纵轴模式对应的界面标签。"]
    const fn label(self) -> &'static str {
        match self {
            Self::FrameNumber => "帧号",
            Self::FrameLength => "帧长",
            Self::StartBit => "起始比特",
        }
    }

    #[doc = "根据当前纵轴模式格式化指定行的元数据。"]
    fn value(self, row: usize, start: u64, len: u64) -> String {
        match self {
            Self::FrameNumber => row.to_string(),
            Self::FrameLength => len.to_string(),
            Self::StartBit => start.saturating_mul(8).to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PlotView {
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlotRenderer {
    EguiPlot,
    Legacy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NavigationTarget {
    FirstRow,
    LastRow,
    FirstColumn,
    LastColumn,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum WorkspaceTab {
    Toolbar,
    Data,
    Document(u64),
    Inspector,
    Curve,
    Frames,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolbarLayout {
    Compact,
    WithFrameRules,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AppTheme {
    Dark,
    Light,
}

struct ExportReport {
    path: PathBuf,
    result: Result<u64, String>,
    elapsed: Duration,
}

struct CurveCache {
    selection: Selection,
    bytes_per_row: usize,
    merge_columns: bool,
    series: Vec<CurveSeries>,
}

struct OpenDocument {
    id: u64,
    document: Arc<BinaryDocument>,
    view: DocumentViewState,
}

struct DocumentViewState {
    display_mode: DisplayMode,
    vertical_axis_mode: VerticalAxisMode,
    selection_mode: SelectionMode,
    selection: Option<Selection>,
    plot_merge_columns: bool,
    plot_renderer: PlotRenderer,
    plot_view: Option<PlotView>,
    bytes_per_row: usize,
    cursor: Option<u64>,
    jump_input: String,
    search_input: String,
    search_results: Vec<u64>,
    frame_sync_input: String,
    frames: Vec<FrameIndex>,
    frame_longest_len: u64,
    frame_truncated: bool,
}

impl DocumentViewState {
    fn new(document_len: u64) -> Self {
        Self {
            display_mode: DisplayMode::HexCharacterMapping,
            vertical_axis_mode: VerticalAxisMode::FrameNumber,
            selection_mode: SelectionMode::Continuous,
            selection: None,
            plot_merge_columns: false,
            plot_renderer: PlotRenderer::EguiPlot,
            plot_view: None,
            bytes_per_row: BesApp::DEFAULT_BYTES_PER_ROW,
            cursor: (!matches!(document_len, 0)).then_some(0),
            jump_input: String::new(),
            search_input: String::new(),
            search_results: Vec::new(),
            frame_sync_input: String::new(),
            frames: Vec::new(),
            frame_longest_len: 0,
            frame_truncated: false,
        }
    }
}

enum ExportEvent {
    Progress { written: u64, total: u64 },
    Finished(ExportReport),
}

pub struct BesApp {
    documents: Vec<OpenDocument>,
    active_document: Option<usize>,
    next_document_id: u64,
    document: Option<Arc<BinaryDocument>>,
    bitmap_texture: Option<egui::TextureHandle>,
    display_mode: DisplayMode,
    vertical_axis_mode: VerticalAxisMode,
    selection_mode: SelectionMode,
    selection: Option<Selection>,
    plot_merge_columns: bool,
    plot_renderer: PlotRenderer,
    plot_view: Option<PlotView>,
    curve_cache: Option<CurveCache>,
    theme: AppTheme,
    dock_state: DockState<WorkspaceTab>,
    pending_workspace_tabs: Vec<WorkspaceTab>,
    applied_toolbar_layout: Option<ToolbarLayout>,
    data_view_rect: Option<Rect>,
    selection_menu_open: bool,
    selection_menu_position: egui::Pos2,
    bytes_per_row: usize,
    cursor: Option<u64>,
    pending_navigation: Option<NavigationTarget>,
    jump_input: String,
    search_input: String,
    search_task: Option<SearchTask>,
    search_results: Vec<u64>,
    search_progress: Option<(u64, u64)>,
    frame_sync_input: String,
    frame_task: Option<FrameTask>,
    frames: Vec<FrameIndex>,
    frame_longest_len: u64,
    frame_progress: Option<(u64, u64)>,
    frame_truncated: bool,
    pending_frame_scroll: Option<usize>,
    export_inbox: UiInbox<ExportEvent>,
    export_progress: Option<(u64, u64)>,
    export_report: Option<ExportReport>,
    status: String,
}

impl BesApp {
    const DEFAULT_BYTES_PER_ROW: usize = 16;
    const MIN_BYTES_PER_ROW: usize = 4;
    const MAX_BYTES_PER_ROW: usize = 4194304;
    const BYTE_CELL_WIDTH: f32 = 27.0;
    const BIT_TEXT_CELL_WIDTH: f32 = 76.0;
    const BITMAP_PIXEL_SIZE: f32 = 1.0;
    const AXIS_WIDTH: f32 = 112.0;
    #[doc = "工具栏两行（不含帧规则）时预留的面板高度。"]
    const COMPACT_TOOLBAR_HEIGHT: f32 = 160.0;
    #[doc = "工具栏三行（含帧规则）时预留的面板高度。"]
    const FRAME_TOOLBAR_HEIGHT: f32 = 200.0;

    #[doc = "创建应用并初始化字体、视觉样式和默认界面状态。"]
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        load_fonts(&context.egui_ctx);
        configure_visuals(&context.egui_ctx, AppTheme::Dark);
        Self {
            documents: Vec::new(),
            active_document: None,
            next_document_id: 0,
            document: None,
            bitmap_texture: None,
            display_mode: DisplayMode::HexCharacterMapping,
            vertical_axis_mode: VerticalAxisMode::FrameNumber,
            selection_mode: SelectionMode::Continuous,
            selection: None,
            plot_merge_columns: false,
            plot_renderer: PlotRenderer::EguiPlot,
            plot_view: None,
            curve_cache: None,
            theme: AppTheme::Dark,
            dock_state: default_dock_state(),
            pending_workspace_tabs: Vec::new(),
            applied_toolbar_layout: None,
            data_view_rect: None,
            selection_menu_open: false,
            selection_menu_position: egui::Pos2::ZERO,
            bytes_per_row: Self::DEFAULT_BYTES_PER_ROW,
            cursor: None,
            pending_navigation: None,
            jump_input: String::new(),
            search_input: String::new(),
            search_task: None,
            search_results: Vec::new(),
            search_progress: None,
            frame_sync_input: String::new(),
            frame_task: None,
            frames: Vec::new(),
            frame_longest_len: 0,
            frame_progress: None,
            frame_truncated: false,
            pending_frame_scroll: None,
            export_inbox: UiInbox::new_with_ctx(&context.egui_ctx),
            export_progress: None,
            export_report: None,
            status: "请打开一个二进制文件或拖拽文件到此处".to_owned(),
        }
    }

    #[doc = "打开二进制文件，并重置与当前文档关联的交互状态。"]
    fn open(&mut self, path: &Path) {
        if let Some(index) = self
            .documents
            .iter()
            .position(|open| open.document.path() == path)
        {
            self.activate_document(index);
            self.open_workspace_tab(WorkspaceTab::Document(self.documents[index].id));
            return;
        }
        match BinaryDocument::open(path) {
            Ok(document) => {
                let document = Arc::new(document);
                let id = self.next_document_id;
                self.next_document_id += 1;
                self.documents.push(OpenDocument {
                    id,
                    view: DocumentViewState::new(document.len()),
                    document,
                });
                self.activate_document(self.documents.len() - 1);
                self.open_workspace_tab(WorkspaceTab::Document(id));
            }
            Err(error) => self.status = error.to_string(),
        }
    }

    fn activate_document(&mut self, index: usize) {
        let Some(open) = self.documents.get(index) else {
            return;
        };
        if self.active_document == Some(index) {
            return;
        }
        let document = Arc::clone(&open.document);
        self.cancel_document_tasks();
        self.store_active_view_state();
        let len = document.len();
        let path = document.path().to_path_buf();
        let view = std::mem::replace(
            &mut self.documents[index].view,
            DocumentViewState::new(len),
        );
        self.active_document = Some(index);
        self.document = Some(document);
        self.display_mode = view.display_mode;
        self.vertical_axis_mode = view.vertical_axis_mode;
        self.selection_mode = view.selection_mode;
        self.selection = view.selection;
        self.plot_merge_columns = view.plot_merge_columns;
        self.plot_renderer = view.plot_renderer;
        self.plot_view = view.plot_view;
        self.bytes_per_row = view.bytes_per_row;
        self.cursor = view.cursor;
        self.jump_input = view.jump_input;
        self.search_input = view.search_input;
        self.search_results = view.search_results;
        self.frame_sync_input = view.frame_sync_input;
        self.frames = view.frames;
        self.frame_longest_len = view.frame_longest_len;
        self.frame_truncated = view.frame_truncated;
        self.pending_navigation = None;
        self.curve_cache = None;
        self.search_progress = None;
        self.frame_progress = None;
        self.pending_frame_scroll = None;
        self.bitmap_texture = None;
        self.status = format!("已打开 {}（{} 字节）", path.display(), len);
    }

    fn close_document(&mut self, index: usize) {
        if index >= self.documents.len() {
            return;
        }
        let was_active = self.active_document == Some(index);
        if was_active {
            self.cancel_document_tasks();
            self.active_document = None;
        }
        self.documents.remove(index);
        if self.documents.is_empty() {
            self.open_workspace_tab(WorkspaceTab::Data);
            self.document = None;
            self.cursor = None;
            self.selection = None;
            self.curve_cache = None;
            self.bitmap_texture = None;
            self.search_results.clear();
            self.frames.clear();
            self.status = "请打开一个二进制文件或拖拽文件到此处".to_owned();
        } else if was_active {
            self.activate_document(index.min(self.documents.len() - 1));
        } else if self.active_document.is_some_and(|active| active > index) {
            self.active_document = self.active_document.map(|active| active - 1);
        }
    }

    fn cancel_document_tasks(&mut self) {
        if let Some(task) = self.search_task.take() {
            task.cancel();
        }
        if let Some(task) = self.frame_task.take() {
            task.cancel();
        }
        self.search_progress = None;
        self.frame_progress = None;
    }

    fn store_active_view_state(&mut self) {
        let Some(index) = self.active_document else {
            return;
        };
        self.documents[index].view = DocumentViewState {
            display_mode: self.display_mode,
            vertical_axis_mode: self.vertical_axis_mode,
            selection_mode: self.selection_mode,
            selection: self.selection,
            plot_merge_columns: self.plot_merge_columns,
            plot_renderer: self.plot_renderer,
            plot_view: self.plot_view,
            bytes_per_row: self.bytes_per_row,
            cursor: self.cursor,
            jump_input: std::mem::take(&mut self.jump_input),
            search_input: std::mem::take(&mut self.search_input),
            search_results: std::mem::take(&mut self.search_results),
            frame_sync_input: std::mem::take(&mut self.frame_sync_input),
            frames: std::mem::take(&mut self.frames),
            frame_longest_len: self.frame_longest_len,
            frame_truncated: self.frame_truncated,
        };
    }

    #[doc = "解析十六进制搜索条件并启动后台搜索任务。"]
    fn start_search(&mut self) {
        let Some(document) = self.document.as_ref() else {
            return;
        };
        match parse_hex_pattern(&self.search_input) {
            Ok(pattern) => {
                if let Some(task) = self.search_task.take() {
                    task.cancel();
                }
                self.search_results.clear();
                self.search_progress = Some((0, document.len()));
                self.search_task = Some(start_search(Arc::clone(document), pattern));
                self.status = "正在搜索…".to_owned();
            }
            Err(error) => self.status = error.to_string(),
        }
    }

    #[doc = "轮询搜索事件，更新结果、进度和状态信息。"]
    fn poll_search(&mut self, context: &egui::Context) {
        let Some(task) = self.search_task.as_ref() else {
            return;
        };
        let mut finished = false;
        for event in task.try_iter() {
            match event {
                SearchEvent::Progress { scanned, total } => {
                    self.search_progress = Some((scanned, total));
                }
                SearchEvent::Matches(offsets) => self.search_results.extend(offsets),
                SearchEvent::Finished {
                    total_matches,
                    truncated,
                } => {
                    self.status = if truncated {
                        format!("找到 {total_matches} 项（已达到结果上限）")
                    } else {
                        format!("找到 {total_matches} 项")
                    };
                    self.search_progress = None;
                    finished = true;
                }
                SearchEvent::Cancelled => {
                    self.status = "搜索已取消".to_owned();
                    self.search_progress = None;
                    finished = true;
                }
                SearchEvent::Failed(error) => {
                    self.status = error;
                    self.search_progress = None;
                    finished = true;
                }
            }
        }
        if finished {
            self.search_task = None;
        } else {
            context.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    #[doc = "解析帧规则并启动后台帧切分任务。"]
    fn start_frame_analysis(&mut self) {
        let Some(document) = self.document.as_ref() else {
            self.status = "请先打开一个二进制文件".to_owned();
            return;
        };
        let sync = match parse_mask_pattern(&self.frame_sync_input) {
            Ok(sync) => sync,
            Err(error) => {
                self.status = error.to_string();
                return;
            }
        };
        let rule = FrameRule { sync };
        if let Err(error) = rule.validate() {
            self.status = error.to_string();
            return;
        }
        if let Some(task) = self.frame_task.take() {
            task.cancel();
        }
        self.frames.clear();
        self.frame_longest_len = 0;
        self.frame_truncated = false;
        self.pending_frame_scroll = None;
        self.frame_progress = Some((0, document.len()));
        self.frame_task = Some(start_frame_analysis(
            Arc::clone(document),
            rule,
            ByteRange::new(0, document.len()),
        ));
        self.status = "正在切分帧…".to_owned();
    }

    #[doc = "轮询帧切分事件，累计帧索引并更新进度与状态。"]
    fn poll_frame_analysis(&mut self, context: &egui::Context) {
        let Some(task) = self.frame_task.as_ref() else {
            return;
        };
        let mut finished = false;
        for event in task.try_iter() {
            match event {
                FrameEvent::Progress { scanned, total } => {
                    self.frame_progress = Some((scanned, total));
                }
                FrameEvent::Frames(frames) => {
                    self.frame_longest_len = self
                        .frame_longest_len
                        .max(frames.iter().map(|frame| frame.len).max().unwrap_or(0));
                    self.frames.extend(frames);
                }
                FrameEvent::Finished {
                    frame_count,
                    truncated,
                } => {
                    self.frame_truncated = truncated;
                    self.status = if truncated {
                        format!("共 {frame_count} 帧（已达到上限，结果已截断）")
                    } else {
                        format!("共 {frame_count} 帧")
                    };
                    self.frame_progress = None;
                    finished = true;
                }
                FrameEvent::Cancelled => {
                    self.status = "帧切分已取消".to_owned();
                    self.frame_progress = None;
                    finished = true;
                }
                FrameEvent::Failed(error) => {
                    self.status = format!("帧切分失败：{error}");
                    self.frame_progress = None;
                    finished = true;
                }
            }
        }
        if finished {
            self.frame_task = None;
        } else {
            context.request_repaint_after(Duration::from_millis(50));
        }
    }

    #[doc = "绘制由主工具栏、导航、跳转、搜索和帧规则组成的顶部区域。"]
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| self.primary_toolbar(ui));
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.horizontal(|ui| self.navigation_toolbar(ui));
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);
            ui.horizontal(|ui| self.jump_toolbar(ui));
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);
            ui.horizontal(|ui| self.search_toolbar(ui));
        });
        if self.frame_toolbar_visible() {
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| self.frame_toolbar(ui));
        }
        ui.add_space(2.0);
    }

    #[doc = "绘制文件、显示模式、选取模式和面板控制项。"]
    fn primary_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("⬡ BES")
                    .strong()
                    .size(20.0)
                    .color(accent_text(ui)),
            );
            ui.label(
                RichText::new("二进制数据分析器")
                    .size(13.0)
                    .color(muted_text(ui)),
            );
            ui.add_space(12.0);
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("📂 打开文件…").strong().color(Color32::WHITE),
                    )
                    .fill(ui_palette(ui).accent_fill)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(6),
                )
                .clicked()
                && let Some(paths) = rfd::FileDialog::new().pick_files()
            {
                for path in paths {
                    self.open(&path);
                }
            }
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("显示")
                    .size(12.0)
                    .color(muted_text(ui)),
            );
            let previous_display_mode = self.display_mode;
            egui::ComboBox::from_id_salt("display_mode")
                .selected_text(self.display_mode.label())
                .show_ui(ui, |ui| {
                    for mode in DisplayMode::ALL {
                        ui.selectable_value(&mut self.display_mode, mode, mode.label());
                    }
                });
            egui::ComboBox::from_id_salt("vertical_axis_mode")
                .selected_text(format!("纵轴：{}", self.vertical_axis_mode.label()))
                .show_ui(ui, |ui| {
                    for mode in VerticalAxisMode::ALL {
                        ui.selectable_value(&mut self.vertical_axis_mode, mode, mode.label());
                    }
                });
            if self.display_mode != previous_display_mode {
                self.on_display_mode_changed();
            }
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("选取")
                    .size(12.0)
                    .color(muted_text(ui)),
            );
            let previous_selection_mode = self.selection_mode;
            ui.add_enabled_ui(self.display_mode != DisplayMode::VariableFrame, |ui| {
                egui::ComboBox::from_id_salt("selection_mode")
                    .selected_text(self.selection_mode.label())
                    .show_ui(ui, |ui| {
                        for mode in SelectionMode::ALL {
                            ui.selectable_value(&mut self.selection_mode, mode, mode.label());
                        }
                    });
            });
            if self.selection_mode != previous_selection_mode
                && let Some(selection) = &mut self.selection
            {
                selection.mode = self.selection_mode;
            }
            if ui
                .add(
                    egui::Button::new("清除选取")
                        .small()
                        .corner_radius(4),
                )
                .clicked()
            {
                self.selection = None;
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new("每行")
                    .size(12.0)
                    .color(muted_text(ui)),
            );
            ui.add(
                egui::DragValue::new(&mut self.bytes_per_row)
                    .range(Self::MIN_BYTES_PER_ROW..=Self::MAX_BYTES_PER_ROW)
                    .speed(1.0),
            );
            ui.label(
                RichText::new("字节")
                    .size(12.0)
                    .color(muted_text(ui)),
            );
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("面板")
                    .size(12.0)
                    .color(muted_text(ui)),
            );
            if ui
                .add(
                    egui::Button::new("数据检查器")
                        .small()
                        .corner_radius(4),
                )
                .clicked()
            {
                self.open_workspace_tab(WorkspaceTab::Inspector);
            }
            if ui
                .add_enabled(
                    curve_actions_enabled(self.selection_mode, self.selection),
                    egui::Button::new("曲线图")
                        .small()
                        .corner_radius(4),
                )
                .clicked()
            {
                self.open_workspace_tab(WorkspaceTab::Curve);
            }
            ui.separator();
            ui.label("主题");
            let previous = self.theme;
            ui.selectable_value(&mut self.theme, AppTheme::Dark, "深色");
            ui.selectable_value(&mut self.theme, AppTheme::Light, "浅色");
            if self.theme != previous {
                configure_visuals(ui.ctx(), self.theme);
            }
        });
    }

    #[doc = "切换显示模式后同步与视图相关的交互状态。"]
    fn on_display_mode_changed(&mut self) {
        if self.display_mode != DisplayMode::VariableFrame {
            return;
        }
        self.selection = None;
        self.curve_cache = None;
        self.plot_view = None;
        self.status = "已切换到不定帧长模式，选取功能暂不可用".to_owned();
    }

    #[doc = "帧规则行只在不定帧长模式下显示；切分进行中始终保留，以便取消任务。"]
    fn frame_toolbar_visible(&self) -> bool {
        self.display_mode == DisplayMode::VariableFrame || self.frame_task.is_some()
    }

    #[doc = "返回当前需要的工具栏布局，用于为停靠面板预留合适高度。"]
    fn toolbar_layout(&self) -> ToolbarLayout {
        if self.frame_toolbar_visible() {
            ToolbarLayout::WithFrameRules
        } else {
            ToolbarLayout::Compact
        }
    }

    #[doc = "绘制用于定位到数据边界的导航按钮。"]
    fn navigation_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("定位")
                .size(12.0)
                .color(muted_text(ui)),
        );
        let navigation_enabled = self
            .document
            .as_ref()
            .is_some_and(|document| !document.is_empty());
        ui.add_enabled_ui(navigation_enabled, |ui| {
            for (label, target) in [
                ("⏮ 首行", NavigationTarget::FirstRow),
                ("⏭ 末行", NavigationTarget::LastRow),
                ("⏪ 首列", NavigationTarget::FirstColumn),
                ("⏩ 末列", NavigationTarget::LastColumn),
            ] {
                if ui
                    .add(
                        egui::Button::new(label)
                            .small()
                            .corner_radius(4),
                    )
                    .clicked()
                {
                    self.navigate(target);
                }
            }
        });
    }

    #[doc = "绘制偏移输入框并处理跳转请求。"]
    fn jump_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("跳转")
                .size(12.0)
                .color(muted_text(ui)),
        );
        let jump = ui.add(
            egui::TextEdit::singleline(&mut self.jump_input)
                .desired_width(95.0),
        );
        if (jump.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)))
            || ui
                .add(
                    egui::Button::new("转到")
                        .small()
                        .corner_radius(4),
                )
                .clicked()
        {
            match parse_offset(&self.jump_input) {
                Some(offset) if self.document.as_ref().is_some_and(|doc| offset < doc.len()) => {
                    self.cursor = Some(offset);
                    self.scroll_to_frame_containing(offset);
                }
                _ => self.status = "无效或越界的偏移".to_owned(),
            }
        }
    }

    #[doc = "绘制十六进制搜索输入及搜索任务控制按钮。"]
    fn search_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Hex 搜索")
                .size(12.0)
                .color(muted_text(ui)),
        );
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.search_input)
                .desired_width(180.0),
        );
        if (search.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)))
            || ui
                .add(
                    egui::Button::new("搜索")
                        .small()
                        .corner_radius(4),
                )
                .clicked()
        {
            self.start_search();
        }
        if self.search_task.is_some()
            && ui
                .add(
                    egui::Button::new("取消")
                        .small()
                        .corner_radius(4),
                )
                .clicked()
            && let Some(task) = &self.search_task
        {
            task.cancel();
        }
    }

    #[doc = "绘制帧规则输入、切分控制与帧列表入口。"]
    fn frame_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("帧")
                .size(12.0)
                .color(muted_text(ui)),
        );
        ui.label(
            RichText::new("同步码")
                .size(12.0)
                .color(muted_text(ui)),
        );
        let sync = ui.add(
            egui::TextEdit::singleline(&mut self.frame_sync_input)
                .desired_width(140.0)
                .hint_text("AA 55 ?? 01"),
        );
        let submitted = sync.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let running = self.frame_task.is_some();
        if ui
            .add_enabled(
                !running,
                egui::Button::new("切分")
                    .small()
                    .corner_radius(4),
            )
            .clicked()
            || (submitted && !running)
        {
            self.start_frame_analysis();
        }
        if running
            && ui
                .add(
                    egui::Button::new("取消")
                        .small()
                        .corner_radius(4),
                )
                .clicked()
            && let Some(task) = &self.frame_task
        {
            task.cancel();
        }
        if ui
            .add_enabled(
                !self.frames.is_empty(),
                egui::Button::new("帧列表")
                    .small()
                    .corner_radius(4),
            )
            .clicked()
        {
            self.open_workspace_tab(WorkspaceTab::Frames);
        }
        if !self.frames.is_empty() {
            ui.label(
                RichText::new(format!("{} 帧", self.frames.len()))
                    .size(11.0)
                    .color(accent_text(ui)),
            );
        }
    }

    #[doc = "显示光标所在字节及其多种数值解释。"]
    fn inspector(&mut self, ui: &mut egui::Ui) {
        ui.heading(
            RichText::new("数据检查器")
                .size(16.0)
                .color(accent_text(ui)),
        );
        ui.separator();
        ui.add_space(6.0);
        let Some(document) = self.document.as_ref() else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("尚未打开文件")
                        .size(13.0)
                        .color(subtle_text(ui)),
                );
            });
            return;
        };
        let Some(offset) = self.cursor else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("请选择一个字节")
                        .size(13.0)
                        .color(subtle_text(ui)),
                );
            });
            return;
        };
        let len = (document.len() - offset).min(8);
        let Ok(bytes) = document.read(ByteRange::new(offset, len)) else {
            ui.colored_label(Color32::RED, "读取失败");
            return;
        };

        ui.add_space(4.0);
        inspector_rows(
            ui,
            "inspector_offset",
            &[
                ("偏移", format!("0x{offset:016X}")),
                ("十进制", offset.to_string()),
            ],
        );
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        if let Some(byte) = bytes.first() {
            inspector_rows(
                ui,
                "inspector_byte",
                &[
                    ("u8", byte.to_string()),
                    ("i8", (*byte as i8).to_string()),
                    ("bin", format!("{byte:08b}")),
                    ("oct", format!("{byte:03o}")),
                    ("hex", format!("{byte:02X}")),
                ],
            );
        }
        if bytes.len() >= 2 {
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            let value = [bytes[0], bytes[1]];
            inspector_rows(
                ui,
                "inspector_u16",
                &[
                    ("u16 LE", u16::from_le_bytes(value).to_string()),
                    ("u16 BE", u16::from_be_bytes(value).to_string()),
                ],
            );
        }
        if bytes.len() >= 4 {
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            let value = [bytes[0], bytes[1], bytes[2], bytes[3]];
            inspector_rows(
                ui,
                "inspector_u32",
                &[
                    ("u32 LE", u32::from_le_bytes(value).to_string()),
                    ("u32 BE", u32::from_be_bytes(value).to_string()),
                    ("f32 LE", format!("{:e}", f32::from_le_bytes(value))),
                ],
            );
        }
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("搜索结果")
                    .size(12.0)
                    .color(accent_text(ui)),
            );
            ui.label(
                RichText::new(format!("({})", self.search_results.len()))
                    .size(11.0)
                    .color(subtle_text(ui)),
            );
        });
        ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
            for (index, result) in self.search_results.iter().take(2_000).enumerate() {
                if ui
                    .selectable_label(
                        self.cursor == Some(*result),
                        RichText::new(format!("#{:<5} 0x{result:016X}", index + 1)).size(12.0),
                    )
                    .clicked()
                {
                    self.cursor = Some(*result);
                }
            }
        });
    }

    #[doc = "显示帧索引列表，支持点击定位到帧首字节。"]
    fn frame_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(
                RichText::new("帧列表")
                    .size(16.0)
                    .color(accent_text(ui)),
            );
            if !self.frames.is_empty() {
                ui.label(
                    RichText::new(format!(
                        "共 {} 帧{}",
                        self.frames.len(),
                        if self.frame_truncated {
                            "（已截断）"
                        } else {
                            ""
                        }
                    ))
                    .size(11.0)
                    .color(muted_text(ui)),
                );
            }
        });
        ui.separator();
        let Some(document) = self.document.as_ref().map(Arc::clone) else {
            ui.label(
                RichText::new("尚未打开文件")
                    .size(13.0)
                    .color(subtle_text(ui)),
            );
            return;
        };
        if self.frames.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("尚未切分帧")
                        .size(13.0)
                        .color(subtle_text(ui)),
                );
            });
            return;
        }
        let row_height = ui.text_style_height(&TextStyle::Monospace) + 4.0;
        let mut clicked = None;
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, row_height, self.frames.len(), |ui, rows| {
                for index in rows {
                    let frame = self.frames[index];
                    let selected = self.cursor.is_some_and(|offset| {
                        offset >= frame.start && offset < frame.start + frame.len
                    });
                    let text = format!(
                        "{:<7} 0x{:016X}  {:<9} {:<6} {}",
                        index + 1,
                        frame.start,
                        frame.len,
                        frame_status_label(frame.status),
                        frame_summary(&document, frame),
                    );
                    if ui
                        .selectable_label(selected, RichText::new(text).monospace().size(12.0))
                        .clicked()
                    {
                        clicked = Some(index);
                    }
                }
            });
        if let Some(index) = clicked {
            let frame = self.frames[index];
            self.cursor = Some(frame.start);
            self.pending_frame_scroll = Some(index);
        }
    }

    #[doc = "将光标移动到指定的数据边界，并安排视图滚动。"]
    fn navigate(&mut self, target: NavigationTarget) {
        let Some(document) = self.document.as_ref() else {
            return;
        };
        if self.display_mode == DisplayMode::VariableFrame && !self.frames.is_empty() {
            let current = self.cursor.unwrap_or(0);
            let index = match target {
                NavigationTarget::FirstRow => 0,
                NavigationTarget::LastRow => self.frames.len() - 1,
                NavigationTarget::FirstColumn | NavigationTarget::LastColumn => {
                    frame_index_at(&self.frames, current)
                }
            };
            let frame = self.frames[index];
            let column = match target {
                NavigationTarget::FirstColumn => 0,
                NavigationTarget::LastColumn => frame.len - 1,
                NavigationTarget::FirstRow | NavigationTarget::LastRow => {
                    current.saturating_sub(frame.start).min(frame.len - 1)
                }
            };
            self.cursor = Some(frame.start + column);
            match target {
                NavigationTarget::FirstColumn | NavigationTarget::LastColumn => {
                    self.pending_navigation = Some(target);
                }
                NavigationTarget::FirstRow | NavigationTarget::LastRow => {
                    self.pending_frame_scroll = Some(index);
                    self.pending_navigation = None;
                }
            }
            return;
        }
        self.cursor = navigation_target_offset(
            target,
            self.cursor,
            document.len(),
            self.bytes_per_row.max(1),
        );
        self.pending_navigation = Some(target);
    }

    #[doc = "在不定帧长模式下把包含指定偏移的帧滚动到视口。"]
    fn scroll_to_frame_containing(&mut self, offset: u64) {
        if self.display_mode != DisplayMode::VariableFrame || self.frames.is_empty() {
            return;
        }
        self.pending_frame_scroll = Some(frame_index_at(&self.frames, offset));
    }

    #[doc = "绘制选区的导航、导出和曲线图上下文操作。"]
    fn selection_context_menu(&mut self, ui: &mut egui::Ui) {
        ui.label("定位");
        let navigation_enabled = self
            .document
            .as_ref()
            .is_some_and(|document| !document.is_empty());
        let mut navigation = None;
        ui.add_enabled_ui(navigation_enabled, |ui| {
            for (label, target) in [
                ("转到首行", NavigationTarget::FirstRow),
                ("转到末行", NavigationTarget::LastRow),
                ("转到首列", NavigationTarget::FirstColumn),
                ("转到末列", NavigationTarget::LastColumn),
            ] {
                if ui.button(label).clicked() {
                    navigation = Some(target);
                }
            }
        });
        if let Some(target) = navigation {
            self.navigate(target);
            self.selection_menu_open = false;
            ui.close();
        }
        ui.separator();

        let export_enabled =
            self.selection.is_some() && self.document.is_some() && self.export_progress.is_none();
        ui.add_enabled_ui(export_enabled, |ui| {
            if ui.button("导出选取数据…").clicked() {
                self.export_selection();
                self.selection_menu_open = false;
                ui.close();
            }
        });
        ui.separator();
        ui.label("曲线图");
        let curve_enabled = curve_actions_enabled(self.selection_mode, self.selection);
        ui.add_enabled_ui(curve_enabled, |ui| {
            if ui.button("按列分别绘制").clicked() {
                self.plot_merge_columns = false;
                self.plot_view = None;
                self.open_workspace_tab(WorkspaceTab::Curve);
                self.selection_menu_open = false;
                ui.close();
            }
            if ui.button("多列合并绘制（大端）").clicked() {
                self.plot_merge_columns = true;
                self.plot_view = None;
                self.open_workspace_tab(WorkspaceTab::Curve);
                self.selection_menu_open = false;
                ui.close();
            }
        });
        if !curve_enabled {
            ui.weak("请先使用列选取或块选取选择数据");
        }
    }

    #[doc = "在记录的位置显示选区上下文菜单并处理关闭操作。"]
    fn show_selection_menu(&mut self, context: &egui::Context) {
        if !self.selection_menu_open {
            return;
        }
        let area = egui::Area::new(egui::Id::new("selection_context_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(self.selection_menu_position)
            .show(context, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    self.selection_context_menu(ui);
                });
            });
        let close_requested = context.input(|input| {
            input.key_pressed(egui::Key::Escape)
                || (input.pointer.primary_clicked()
                    && input
                        .pointer
                        .interact_pos()
                        .is_none_or(|position| !area.response.rect.contains(position)))
        });
        if close_requested {
            self.selection_menu_open = false;
        }
    }

    #[doc = "将当前选区导出到用户选择的二进制文件。"]
    fn export_selection(&mut self) {
        let (Some(selection), Some(document)) = (self.selection, self.document.as_ref()) else {
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("selection.bin")
            .save_file()
        else {
            return;
        };
        let document = Arc::clone(document);
        let bytes_per_row = self.bytes_per_row.max(1);
        let total = selection.export_len(document.len(), bytes_per_row);
        let sender = self.export_inbox.sender();
        self.export_progress = Some((0, total));
        self.status = "正在导出选取数据…".to_owned();
        let spawn = std::thread::Builder::new()
            .name("bes-selection-export".to_owned())
            .spawn(move || {
                let started = Instant::now();
                let result =
                    selection.export_with_progress(&document, bytes_per_row, &path, |written| {
                        let _ = sender.send(ExportEvent::Progress { written, total });
                    });
                let _ = sender.send(ExportEvent::Finished(ExportReport {
                    path,
                    result,
                    elapsed: started.elapsed(),
                }));
            });
        if let Err(error) = spawn {
            self.export_progress = None;
            self.status = format!("无法启动导出任务：{error}");
        }
    }

    #[doc = "接收后台导出任务的进度和完成事件，并更新界面状态。"]
    fn poll_export(&mut self, context: &egui::Context) {
        for event in self.export_inbox.read(context) {
            match event {
                ExportEvent::Progress { written, total } => {
                    self.export_progress = Some((written.min(total), total));
                }
                ExportEvent::Finished(report) => {
                    self.status = match &report.result {
                        Ok(written) => {
                            format!("已导出 {}（{written} 字节）", report.path.display())
                        }
                        Err(error) => format!("导出失败：{error}"),
                    };
                    self.export_progress = None;
                    self.export_report = Some(report);
                }
            }
        }
    }

    #[doc = "显示最近一次导出的结果、目标路径和耗时。"]
    fn show_export_report(&mut self, context: &egui::Context) {
        let Some(report) = self.export_report.as_ref() else {
            return;
        };
        let mut confirmed = false;
        let response = egui::Modal::new(egui::Id::new("export_report_modal")).show(context, |ui| {
            match &report.result {
                Ok(written) => {
                    ui.heading("导出完成");
                    ui.label(format!("导出大小：{written} 字节"));
                }
                Err(error) => {
                    ui.heading("导出失败");
                    ui.colored_label(Color32::RED, error);
                }
            }
            ui.label(format!("文件：{}", report.path.display()));
            ui.label(format!("耗时：{}", format_duration(report.elapsed)));
            ui.add_space(8.0);
            confirmed = ui.button("确定").clicked();
        });
        if confirmed || response.should_close() {
            self.export_report = None;
        }
    }
}

impl eframe::App for BesApp {
    #[doc = "处理每帧输入并绘制应用的完整界面。"]
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = ui.ctx().clone();
        let dropped_paths = context.input(|input| dropped_file_paths(&input.raw));
        let file_is_hovering = context.input(|input| !input.raw.hovered_files.is_empty());
        for path in dropped_paths {
            self.open(&path);
        }
        self.poll_search(&context);
        self.poll_export(&context);
        self.poll_frame_analysis(&context);
        self.data_view_rect = None;

        let colors = ui_palette(ui);
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(colors.faint_bg)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .stroke(egui::Stroke::new(1.0, colors.divider)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&self.status)
                            .small()
                            .color(ui.visuals().text_color()),
                    );
                    if let (Some(selection), Some(document)) =
                        (self.selection, self.document.as_ref())
                    {
                        ui.separator();
                        ui.label(
                            RichText::new(selection_status_text(
                                selection,
                                document.len(),
                                self.bytes_per_row.max(1),
                            ))
                            .small()
                            .color(ui.visuals().hyperlink_color),
                        );
                    }
                    if let Some((scanned, total)) = self.search_progress {
                        let fraction = if total == 0 {
                            1.0
                        } else {
                            scanned as f32 / total as f32
                        };
                        ui.add(
                            egui::ProgressBar::new(fraction)
                                .desired_width(180.0)
                                .show_percentage(),
                        );
                    }
                    if let Some((scanned, total)) = self.frame_progress {
                        let fraction = if total == 0 {
                            1.0
                        } else {
                            scanned as f32 / total as f32
                        };
                        ui.separator();
                        ui.add(
                            egui::ProgressBar::new(fraction)
                                .desired_width(220.0)
                                .desired_height(18.0)
                                .fill(Color32::from_rgb(120, 200, 140))
                                .text(format!(
                                    "帧切分 {:>3}%  ·  {} / {}",
                                    (fraction * 100.0).round() as u32,
                                    format_byte_count(scanned),
                                    format_byte_count(total)
                                )),
                        );
                    }
                    if let Some((written, total)) = self.export_progress {
                        let fraction = if total == 0 {
                            1.0
                        } else {
                            written as f32 / total as f32
                        };
                        ui.separator();
                        ui.add(
                            egui::ProgressBar::new(fraction)
                                .desired_width(240.0)
                                .desired_height(18.0)
                                .fill(Color32::from_rgb(53, 170, 217))
                                .text(format!(
                                    "导出 {:>3}%  ·  {} / {}",
                                    (fraction * 100.0).round() as u32,
                                    format_byte_count(written),
                                    format_byte_count(total)
                                )),
                        );
                    }
                });
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(colors.panel_fill)
                    .inner_margin(egui::Margin::same(4)),
            )
            .show(ui, |ui| self.show_workspace(ui));
        if let Some(position) = context.input(|input| {
            self.data_view_rect.and_then(|rect| {
                selection_menu_position(
                    input.pointer.secondary_clicked(),
                    input.pointer.interact_pos(),
                    rect,
                )
            })
        }) {
            self.selection_menu_position = position;
            self.selection_menu_open = true;
        }
        if file_is_hovering {
            let painter = context.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("file_drop_overlay"),
            ));
            let rect = context.content_rect();
            painter.rect_filled(rect, 0.0, colors.overlay);
            let card = Rect::from_center_size(rect.center(), Vec2::new(400.0, 180.0));
            painter.rect_filled(card, 16.0, colors.window_fill);
            painter.rect_stroke(
                card,
                16.0,
                egui::Stroke::new(1.5, colors.accent),
                egui::StrokeKind::Inside,
            );
            painter.text(
                card.center() - Vec2::new(0.0, 26.0),
                Align2::CENTER_CENTER,
                "📂",
                FontId::proportional(40.0),
                colors.accent,
            );
            painter.text(
                card.center() + Vec2::new(0.0, 26.0),
                Align2::CENTER_CENTER,
                "松开鼠标打开文件",
                FontId::proportional(20.0),
                colors.text,
            );
        }
        self.show_selection_menu(&context);
        self.show_export_report(&context);
    }
}

fn format_duration(duration: Duration) -> String {
    if duration.as_secs() >= 60 {
        let minutes = duration.as_secs() / 60;
        let seconds = duration.as_secs_f64() - minutes as f64 * 60.0;
        format!("{minutes} 分 {seconds:.1} 秒")
    } else if duration.as_secs() > 0 {
        format!("{:.2} 秒", duration.as_secs_f64())
    } else {
        format!("{} 毫秒", duration.as_millis())
    }
}

fn format_byte_count(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[doc = "以两列网格绘制数据检查器的标签与数值，使数值列左对齐。"]
fn inspector_rows(ui: &mut egui::Ui, id_salt: &str, rows: &[(&str, String)]) {
    let subtle = subtle_text(ui);
    egui::Grid::new(id_salt)
        .num_columns(2)
        .spacing(Vec2::new(18.0, 4.0))
        .show(ui, |ui| {
            for (label, value) in rows {
                ui.label(
                    RichText::new(*label)
                        .size(11.0)
                        .color(subtle)
                        .monospace(),
                );
                ui.monospace(RichText::new(value.as_str()).size(13.0));
                ui.end_row();
            }
        });
}

#[doc = "界面配色的语义槽位，深色与浅色主题各提供一套。"]
#[derive(Clone, Copy, Debug)]
struct Palette {
    #[doc = "面板与工作区背景。"]
    panel_fill: Color32,
    #[doc = "窗口、浮层与卡片背景。"]
    window_fill: Color32,
    #[doc = "输入框、进度条等凹陷背景。"]
    extreme_bg: Color32,
    #[doc = "状态栏等弱背景。"]
    faint_bg: Color32,
    #[doc = "代码与数据区域背景。"]
    code_bg: Color32,
    #[doc = "控件默认背景。"]
    widget_fill: Color32,
    #[doc = "控件悬停背景。"]
    widget_hovered: Color32,
    #[doc = "分隔线与控件描边。"]
    divider: Color32,
    #[doc = "正文文字。"]
    text: Color32,
    #[doc = "品牌与强调文字。"]
    accent: Color32,
    #[doc = "强调色填充，用于主按钮与选中态。"]
    accent_fill: Color32,
    #[doc = "选中项的文字与描边颜色。"]
    selection_stroke: Color32,
    #[doc = "搜索命中字节的高亮背景。"]
    match_fill: Color32,
    #[doc = "分组标签等次要文字。"]
    muted: Color32,
    #[doc = "辅助说明文字。"]
    subtle: Color32,
    #[doc = "模态遮罩背景。"]
    overlay: Color32,
}

#[doc = "返回指定主题的配色方案。"]
fn palette(theme: AppTheme) -> Palette {
    match theme {
        AppTheme::Dark => Palette {
            panel_fill: Color32::from_rgb(16, 20, 27),
            window_fill: Color32::from_rgb(22, 27, 35),
            extreme_bg: Color32::from_rgb(11, 15, 20),
            faint_bg: Color32::from_rgb(26, 32, 41),
            code_bg: Color32::from_rgb(12, 17, 23),
            widget_fill: Color32::from_rgb(30, 38, 49),
            widget_hovered: Color32::from_rgb(42, 54, 69),
            divider: Color32::from_rgb(45, 57, 73),
            text: Color32::from_rgb(229, 236, 245),
            accent: Color32::from_rgb(103, 197, 255),
            accent_fill: Color32::from_rgb(31, 111, 184),
            selection_stroke: Color32::from_rgb(190, 230, 255),
            match_fill: Color32::from_rgb(90, 70, 10),
            muted: Color32::from_rgb(143, 158, 178),
            subtle: Color32::from_rgb(118, 133, 153),
            overlay: Color32::from_rgba_unmultiplied(5, 9, 15, 200),
        },
        AppTheme::Light => Palette {
            panel_fill: Color32::from_rgb(244, 247, 250),
            window_fill: Color32::WHITE,
            extreme_bg: Color32::WHITE,
            faint_bg: Color32::from_rgb(235, 240, 246),
            code_bg: Color32::from_rgb(238, 242, 247),
            widget_fill: Color32::from_rgb(231, 237, 244),
            widget_hovered: Color32::from_rgb(212, 225, 239),
            divider: Color32::from_rgb(205, 215, 226),
            text: Color32::from_rgb(26, 32, 42),
            accent: Color32::from_rgb(11, 99, 168),
            accent_fill: Color32::from_rgb(38, 112, 184),
            // 选中项文字用白色，浅色主题下蓝底深字会看不清
            selection_stroke: Color32::WHITE,
            match_fill: Color32::from_rgb(255, 226, 130),
            muted: Color32::from_rgb(92, 107, 125),
            subtle: Color32::from_rgb(118, 133, 152),
            overlay: Color32::from_rgba_unmultiplied(15, 23, 42, 160),
        },
    }
}

#[doc = "按界面当前的明暗模式取配色方案。"]
fn ui_palette(ui: &egui::Ui) -> Palette {
    palette(if ui.visuals().dark_mode {
        AppTheme::Dark
    } else {
        AppTheme::Light
    })
}

#[doc = "分组标签等次要文字颜色。"]
fn muted_text(ui: &egui::Ui) -> Color32 {
    ui_palette(ui).muted
}

#[doc = "辅助说明文字颜色。"]
fn subtle_text(ui: &egui::Ui) -> Color32 {
    ui_palette(ui).subtle
}

#[doc = "品牌与强调文字颜色。"]
fn accent_text(ui: &egui::Ui) -> Color32 {
    ui_palette(ui).accent
}

fn configure_visuals(context: &egui::Context, theme: AppTheme) {
    let egui_theme = match theme {
        AppTheme::Dark => egui::Theme::Dark,
        AppTheme::Light => egui::Theme::Light,
    };
    context.set_theme(egui_theme);
    let mut style = (*context.style_of(egui_theme)).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.interact_size.y = 30.0;
    style.spacing.window_margin = egui::Margin::same(12);
    style.spacing.menu_margin = egui::Margin::same(8);
    style.spacing.scroll.bar_width = 10.0;
    style.spacing.scroll.bar_inner_margin = 4.0;
    style.spacing.scroll.bar_outer_margin = 2.0;
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(11.0, egui::FontFamily::Proportional),
    );

    let colors = palette(theme);
    let flat_radius = egui::CornerRadius::same(5);
    let visuals = &mut style.visuals;
    visuals.panel_fill = colors.panel_fill;
    visuals.window_fill = colors.window_fill;
    visuals.extreme_bg_color = colors.extreme_bg;
    visuals.faint_bg_color = colors.faint_bg;
    visuals.code_bg_color = colors.code_bg;
    visuals.override_text_color = Some(colors.text);
    visuals.hyperlink_color = colors.accent;
    visuals.selection.bg_fill = colors.accent_fill;
    visuals.selection.stroke = egui::Stroke::new(1.0, colors.selection_stroke);
    visuals.window_stroke = egui::Stroke::new(1.0, colors.divider);
    visuals.window_corner_radius = egui::CornerRadius::same(8);
    visuals.menu_corner_radius = egui::CornerRadius::same(6);
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, colors.divider);
    for (widget, fill) in [
        (&mut visuals.widgets.inactive, colors.widget_fill),
        (&mut visuals.widgets.hovered, colors.widget_hovered),
        (&mut visuals.widgets.active, colors.accent_fill),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = egui::Stroke::new(1.0, colors.divider);
        widget.corner_radius = flat_radius;
    }
    let shadow = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(if theme == AppTheme::Dark { 140 } else { 40 }),
    };
    visuals.window_shadow = shadow;
    visuals.popup_shadow = egui::epaint::Shadow { blur: 12, ..shadow };
    context.set_style_of(egui_theme, style);
}

fn selection_status_text(selection: Selection, document_len: u64, bytes_per_row: usize) -> String {
    let bytes_per_row = bytes_per_row.max(1);
    let document_rows = document_len.div_ceil(bytes_per_row as u64) as usize;
    match selection.mode {
        SelectionMode::Continuous => {
            let start = selection.anchor.offset.min(selection.focus.offset);
            let end = selection
                .anchor
                .offset
                .max(selection.focus.offset)
                .min(document_len.saturating_sub(1));
            let start = start.min(end);
            let byte_count = end.saturating_sub(start) + 1;
            let row_count = end / bytes_per_row as u64 - start / bytes_per_row as u64 + 1;
            format!("选区：{byte_count} 字节 · 跨 {row_count} 行")
        }
        SelectionMode::Block | SelectionMode::Row | SelectionMode::Column => {
            let selected_rows = selection.row_range();
            let row_count = if selection.mode == SelectionMode::Column {
                document_rows
            } else {
                let first = (*selected_rows.start()).min(document_rows);
                let end = selected_rows.end().saturating_add(1).min(document_rows);
                end.saturating_sub(first)
            };
            let column_count = if selection.mode == SelectionMode::Row {
                bytes_per_row.min(document_len as usize)
            } else {
                selection.column_range().count()
            };
            format!("选区：{row_count} 行 × {column_count} 列")
        }
    }
}

fn navigation_target_offset(
    target: NavigationTarget,
    cursor: Option<u64>,
    document_len: u64,
    bytes_per_row: usize,
) -> Option<u64> {
    if document_len == 0 {
        return None;
    }
    let bytes_per_row = bytes_per_row.max(1) as u64;
    let last_row = (document_len - 1) / bytes_per_row;
    let current = cursor.unwrap_or(0).min(document_len - 1);
    let current_row = (current / bytes_per_row).min(last_row);
    let current_column = current % bytes_per_row;
    let row = match target {
        NavigationTarget::FirstRow => 0,
        NavigationTarget::LastRow => last_row,
        NavigationTarget::FirstColumn | NavigationTarget::LastColumn => current_row,
    };
    let row_start = row * bytes_per_row;
    let row_len = (document_len - row_start).min(bytes_per_row);
    let column = match target {
        NavigationTarget::FirstColumn => 0,
        NavigationTarget::LastColumn => row_len - 1,
        NavigationTarget::FirstRow | NavigationTarget::LastRow => current_column.min(row_len - 1),
    };
    Some(row_start + column)
}

#[doc = "返回包含指定偏移的帧下标，偏移落在帧间空隙时返回其前面的帧。"]
fn frame_index_at(frames: &[FrameIndex], offset: u64) -> usize {
    frames
        .partition_point(|frame| frame.start <= offset)
        .saturating_sub(1)
        .min(frames.len().saturating_sub(1))
}

fn frame_status_label(status: FrameStatus) -> &'static str {
    match status {
        FrameStatus::Ok => "正常",
        FrameStatus::BadLength => "长度非法",
        FrameStatus::Truncated => "文件尾截断",
    }
}

fn frame_summary(document: &BinaryDocument, frame: FrameIndex) -> String {
    let len = frame.len.min(8);
    match document.read(ByteRange::new(frame.start, len)) {
        Ok(bytes) => bytes
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(" "),
        Err(_) => "读取失败".to_owned(),
    }
}

fn selection_menu_position(
    secondary_clicked: bool,
    pointer_position: Option<egui::Pos2>,
    content_rect: Rect,
) -> Option<egui::Pos2> {
    pointer_position.filter(|position| secondary_clicked && content_rect.contains(*position))
}

fn curve_actions_enabled(active_mode: SelectionMode, selection: Option<Selection>) -> bool {
    matches!(active_mode, SelectionMode::Column | SelectionMode::Block) && selection.is_some()
}

fn dropped_file_paths(input: &egui::RawInput) -> Vec<PathBuf> {
    input
        .dropped_files
        .iter()
        .map(|file| file.path().to_path_buf())
        .collect()
}

fn parse_offset(input: &str) -> Option<u64> {
    let input = input.trim();
    if let Some(hex) = input
        .strip_prefix("0x")
        .or_else(|| input.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).ok()
    } else {
        input.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        io::Write,
        path::{Path, PathBuf},
        sync::Arc,
        time::Duration,
    };

    use eframe::egui;

    use super::{
        AppTheme, BesApp, BinaryDocument, CurveSeries, DisplayMode, DocumentViewState, FrameIndex,
        FrameStatus, NavigationTarget, OpenDocument, PlotRenderer, Selection, SelectionMode, SelectionPoint,
        VerticalAxisMode, ToolbarLayout, WorkspaceTab, configure_visuals, curve_actions_enabled,
        default_dock_state,
        display_character, dropped_file_paths, edge_auto_scroll_delta, fixed_axis_rect,
        format_bit_text, format_byte_count, format_duration, format_nearest_curve_point,
        frame_click_offset, frame_index_at, frame_scroll_offset, last_row_scroll_offset,
        navigation_target_offset, nearest_point_at_x, open_workspace_tab, parse_offset,
        selection_background_color, selection_menu_position, selection_status_text,
        toolbar_fraction_for_height, virtual_scroll_row_count, visible_document_rows,
    };

    #[derive(Debug)]
    struct TestDroppedFile(PathBuf);

    impl egui::DroppedFile for TestDroppedFile {
        #[doc = "返回测试拖放文件的本地路径。"]
        fn path(&self) -> &Path {
            &self.0
        }

        #[doc = "从测试拖放文件路径读取全部字节。"]
        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|error| error.to_string())
        }
    }

    fn test_app(document: Arc<BinaryDocument>) -> BesApp {
        let document_len = document.len();
        BesApp {
            documents: vec![OpenDocument {
                id: 0,
                document: Arc::clone(&document),
                view: DocumentViewState::new(document_len),
            }],
            active_document: Some(0),
            next_document_id: 1,
            document: Some(document),
            bitmap_texture: None,
            display_mode: DisplayMode::HexCharacterMapping,
            vertical_axis_mode: VerticalAxisMode::FrameNumber,
            selection_mode: SelectionMode::Continuous,
            selection: None,
            plot_merge_columns: false,
            plot_renderer: PlotRenderer::EguiPlot,
            plot_view: None,
            curve_cache: None,
            theme: AppTheme::Dark,
            dock_state: default_dock_state(),
            pending_workspace_tabs: Vec::new(),
            applied_toolbar_layout: None,
            data_view_rect: None,
            selection_menu_open: false,
            selection_menu_position: egui::Pos2::ZERO,
            bytes_per_row: 16,
            cursor: Some(0),
            pending_navigation: None,
            jump_input: String::new(),
            search_input: String::new(),
            search_task: None,
            search_results: Vec::new(),
            search_progress: None,
            frame_sync_input: String::new(),
            frame_task: None,
            frames: Vec::new(),
            frame_longest_len: 0,
            frame_progress: None,
            frame_truncated: false,
            pending_frame_scroll: None,
            export_inbox: egui_inbox::UiInbox::new(),
            export_progress: None,
            export_report: None,
            status: String::new(),
        }
    }

    #[test]
    fn parses_decimal_and_hex_offsets() {
        assert_eq!(parse_offset("42"), Some(42));
        assert_eq!(parse_offset("0x2A"), Some(42));
        assert_eq!(parse_offset("xyz"), None);
    }

    #[test]
    fn formats_export_duration_for_dialog() {
        assert_eq!(format_duration(Duration::from_millis(875)), "875 毫秒");
        assert_eq!(format_duration(Duration::from_millis(1_250)), "1.25 秒");
        assert_eq!(
            format_duration(Duration::from_millis(62_500)),
            "1 分 2.5 秒"
        );
    }

    #[test]
    fn formats_export_progress_byte_counts_compactly() {
        assert_eq!(format_byte_count(512), "512 B");
        assert_eq!(format_byte_count(1536), "1.5 KiB");
        assert_eq!(format_byte_count(2 * 1024 * 1024), "2.0 MiB");
    }

    #[test]
    fn egui_plot_hover_uses_the_nearest_curve_data_point() {
        let points = [[0.0, 10.0], [10.0, 20.0], [20.0, 30.0]];
        assert_eq!(nearest_point_at_x(&points, 6.0), Some([10.0, 20.0]));

        let series = [CurveSeries {
            name: "列 3".to_owned(),
            points: points.to_vec(),
        }];
        let label =
            format_nearest_curve_point(&series, egui_plot::PlotPoint::new(10.1, 20.1), 20.0, 20.0);
        assert_eq!(label, "列 3\n行号: 10\n值: 20");
        assert!(
            format_nearest_curve_point(
                &series,
                egui_plot::PlotPoint::new(10.0, 100.0),
                20.0,
                20.0,
            )
            .is_empty()
        );
    }

    #[test]
    fn selection_status_reports_interactive_dimensions() {
        let block = Selection {
            mode: SelectionMode::Block,
            anchor: SelectionPoint {
                row: 4,
                column: 5,
                offset: 69,
            },
            focus: SelectionPoint {
                row: 2,
                column: 3,
                offset: 35,
            },
        };
        assert_eq!(selection_status_text(block, 200, 16), "选区：3 行 × 3 列");

        let continuous = Selection {
            mode: SelectionMode::Continuous,
            anchor: SelectionPoint {
                row: 2,
                column: 2,
                offset: 34,
            },
            focus: SelectionPoint {
                row: 0,
                column: 14,
                offset: 14,
            },
        };
        assert_eq!(
            selection_status_text(continuous, 200, 16),
            "选区：21 字节 · 跨 3 行"
        );

        let columns = Selection {
            mode: SelectionMode::Column,
            anchor: block.anchor,
            focus: block.focus,
        };
        assert_eq!(selection_status_text(columns, 34, 16), "选区：3 行 × 3 列");
    }

    #[test]
    fn navigation_targets_document_edges_and_clamps_partial_last_row() {
        let cursor = Some(21);
        assert_eq!(
            navigation_target_offset(NavigationTarget::FirstRow, cursor, 34, 16),
            Some(5)
        );
        assert_eq!(
            navigation_target_offset(NavigationTarget::LastRow, cursor, 34, 16),
            Some(33)
        );
        assert_eq!(
            navigation_target_offset(NavigationTarget::FirstColumn, cursor, 34, 16),
            Some(16)
        );
        assert_eq!(
            navigation_target_offset(NavigationTarget::LastColumn, cursor, 34, 16),
            Some(31)
        );
        assert_eq!(
            navigation_target_offset(NavigationTarget::LastRow, cursor, 0, 16),
            None
        );
    }

    #[test]
    fn last_row_navigation_renders_the_end_of_the_virtualized_document() {
        let context = egui::Context::default();
        let first_visible_row = Cell::new(0);
        let last_visible_row = Cell::new(0);
        let row_count = 19_094_568_960_usize.div_ceil(16);
        let scroll_row_count = virtual_scroll_row_count(row_count);
        let row_height = 16.0;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut output = context.run_ui(egui::RawInput::default(), |ui| {
                let scroll_offset = last_row_scroll_offset(
                    scroll_row_count,
                    row_height,
                    ui.spacing().item_spacing.y,
                );
                egui::ScrollArea::both()
                    .vertical_scroll_offset(scroll_offset)
                    .show_rows(ui, row_height, scroll_row_count, |_, rows| {
                        let rows = visible_document_rows(rows, scroll_row_count, row_count);
                        first_visible_row.set(rows.start);
                        last_visible_row.set(rows.end);
                    });
            });
            output.textures_delta.clear();
        }));

        assert!(result.is_ok(), "last-row navigation must not panic");
        assert!(first_visible_row.get() < last_visible_row.get());
        assert_eq!(last_visible_row.get(), row_count);

        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            egui::ScrollArea::both()
                .vertical_scroll_offset(0.0)
                .show_rows(ui, row_height, scroll_row_count, |_, rows| {
                    let rows = visible_document_rows(rows, scroll_row_count, row_count);
                    first_visible_row.set(rows.start);
                });
        });
        output.textures_delta.clear();
        assert_eq!(first_visible_row.get(), 0);
    }

    #[test]
    fn large_document_rows_have_distinct_scroll_coordinates() {
        let row_count = 19_094_568_960_usize.div_ceil(16);
        let scroll_row_count = virtual_scroll_row_count(row_count);
        let row_height = 16.0;

        assert!((scroll_row_count - 1) as f32 * row_height < scroll_row_count as f32 * row_height);
    }

    #[test]
    fn default_workspace_hides_curve_tab_until_requested() {
        let dock_state = default_dock_state();
        assert!(dock_state.find_tab(&WorkspaceTab::Toolbar).is_some());
        assert!(dock_state.find_tab(&WorkspaceTab::Data).is_some());
        assert!(dock_state.find_tab(&WorkspaceTab::Inspector).is_none());
        assert!(dock_state.find_tab(&WorkspaceTab::Curve).is_none());

        let egui_dock::Node::Vertical(root_split) = dock_state
            .main_surface()
            .root_node()
            .expect("default workspace should have a root node")
        else {
            panic!("toolbar should be split above the other workspace panels");
        };
        assert_eq!(root_split.fraction, 0.15);
    }

    #[test]
    fn document_tab_replaces_data_placeholder() {
        let mut dock_state = default_dock_state();

        open_workspace_tab(&mut dock_state, WorkspaceTab::Document(7));

        let document = dock_state.find_tab(&WorkspaceTab::Document(7)).unwrap();
        assert!(dock_state.find_tab(&WorkspaceTab::Data).is_none());
        assert!(dock_state.find_tab(&WorkspaceTab::Inspector).is_none());
        assert_eq!(
            dock_state
                .main_surface()[document.node]
                .tabs_count(),
            1
        );
    }

    #[test]
    fn inspector_can_be_opened_from_the_default_workspace() {
        let mut dock_state = default_dock_state();

        open_workspace_tab(&mut dock_state, WorkspaceTab::Inspector);

        let data = dock_state
            .find_tab(&WorkspaceTab::Data)
            .expect("workspace should retain the data tab");
        let inspector = dock_state
            .find_tab(&WorkspaceTab::Inspector)
            .expect("workspace should reopen the inspector");
        let content_node = data
            .node
            .parent()
            .expect("data and inspector should have a split parent");
        assert_eq!(data.node, content_node.left());
        assert_eq!(inspector.node, content_node.right());
        let egui_dock::Node::Horizontal(content_split) = &dock_state.main_surface()[content_node]
        else {
            panic!("inspector should reopen to the right of the data area");
        };
        assert_eq!(content_split.fraction, 0.78);
    }

    #[test]
    fn initial_toolbar_uses_a_bounded_pixel_height() {
        assert_eq!(toolbar_fraction_for_height(800.0, 120.0), 0.15);
        assert_eq!(toolbar_fraction_for_height(1600.0, 120.0), 0.075);
        assert_eq!(toolbar_fraction_for_height(200.0, 120.0), 0.4);
    }

    fn toolbar_height(app: &mut BesApp, context: &egui::Context) -> f32 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(1200.0, 760.0),
            )),
            ..Default::default()
        };
        let mut used_height = 0.0;
        let mut output = context.run_ui(input, |context| {
            egui::CentralPanel::default().show(context, |ui| {
                let top = ui.cursor().top();
                app.top_bar(ui);
                used_height = ui.cursor().top() - top;
            });
        });
        output.textures_delta.clear();
        used_height
    }

    #[test]
    fn toolbar_rows_fit_the_reserved_dock_height() {
        // dock 标签栏与面板内边距占用高度
        const DOCK_TAB_CHROME: f32 = 40.0;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let data = vec![0x41; 32];
        file.write_all(&data).unwrap();
        let document = Arc::new(BinaryDocument::open(file.path()).unwrap());
        let mut app = test_app(document);
        let context = egui::Context::default();
        configure_visuals(&context, AppTheme::Dark);

        let compact = toolbar_height(&mut app, &context);
        app.display_mode = DisplayMode::VariableFrame;
        let with_frame_rules = toolbar_height(&mut app, &context);

        assert!(
            compact + DOCK_TAB_CHROME <= BesApp::COMPACT_TOOLBAR_HEIGHT,
            "两行工具栏需要 {compact} px，加标签栏后超过预留高度 {}",
            BesApp::COMPACT_TOOLBAR_HEIGHT
        );
        assert!(
            with_frame_rules + DOCK_TAB_CHROME <= BesApp::FRAME_TOOLBAR_HEIGHT,
            "三行工具栏需要 {with_frame_rules} px，加标签栏后超过预留高度 {}",
            BesApp::FRAME_TOOLBAR_HEIGHT
        );
        assert!(
            compact + 30.0 <= with_frame_rules,
            "帧规则行应明显增加工具栏高度：{compact} → {with_frame_rules}"
        );
    }

    #[test]
    fn application_theme_switches_between_dark_and_light_visuals() {
        let context = egui::Context::default();

        configure_visuals(&context, AppTheme::Light);
        assert_eq!(context.theme(), egui::Theme::Light);
        assert!(!context.style_of(egui::Theme::Light).visuals.dark_mode);

        configure_visuals(&context, AppTheme::Dark);
        assert_eq!(context.theme(), egui::Theme::Dark);
        assert!(context.style_of(egui::Theme::Dark).visuals.dark_mode);
    }

    #[test]
    fn frame_toolbar_is_limited_to_the_variable_frame_mode() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let data = vec![0x41; 32];
        file.write_all(&data).unwrap();
        let document = Arc::new(BinaryDocument::open(file.path()).unwrap());
        let mut app = test_app(document);

        assert!(!app.frame_toolbar_visible());
        assert_eq!(app.toolbar_layout(), ToolbarLayout::Compact);
        for mode in DisplayMode::ALL {
            app.display_mode = mode;
            assert_eq!(
                app.frame_toolbar_visible(),
                mode == DisplayMode::VariableFrame,
                "{} 模式下的帧规则行可见性不正确",
                mode.label()
            );
        }
        assert_eq!(app.toolbar_layout(), ToolbarLayout::WithFrameRules);
    }

    #[test]
    fn vertical_axis_stays_at_viewport_left_when_content_scrolls_horizontally() {
        let viewport = egui::Rect::from_min_max(egui::pos2(24.0, 10.0), egui::pos2(500.0, 300.0));
        let scrolled_axis = egui::Rect::from_min_size(
            egui::pos2(-180.0, 42.0),
            egui::vec2(BesApp::AXIS_WIDTH, 20.0),
        );

        let fixed = fixed_axis_rect(viewport, scrolled_axis);

        assert_eq!(fixed.left(), viewport.left());
        assert_eq!(fixed.top(), scrolled_axis.top());
        assert_eq!(fixed.size(), scrolled_axis.size());
    }

    #[test]
    fn vertical_axis_uses_compact_width() {
        assert_eq!(BesApp::AXIS_WIDTH, 112.0);
    }

    #[test]
    fn extracts_all_native_dropped_file_paths() {
        let first = PathBuf::from(r"C:\data\first.bin");
        let second = PathBuf::from(r"C:\data\second.bin");
        let input = egui::RawInput {
            dropped_files: vec![
                Arc::new(TestDroppedFile(first.clone())),
                Arc::new(TestDroppedFile(second.clone())),
            ],
            ..Default::default()
        };

        assert_eq!(dropped_file_paths(&input), vec![first, second]);
    }

    #[test]
    fn opening_multiple_files_creates_switchable_documents_without_duplicates() {
        let mut first = tempfile::NamedTempFile::new().unwrap();
        first.write_all(&[1, 2]).unwrap();
        let first_document = Arc::new(BinaryDocument::open(first.path()).unwrap());
        let mut second = tempfile::NamedTempFile::new().unwrap();
        second.write_all(&[3, 4, 5]).unwrap();
        let mut app = test_app(first_document);

        app.open(second.path());
        assert_eq!(app.documents.len(), 2);
        assert_eq!(app.active_document, Some(1));
        assert_eq!(app.document.as_ref().unwrap().len(), 3);
        assert_eq!(app.pending_workspace_tabs, vec![WorkspaceTab::Document(1)]);

        app.open(first.path());
        assert_eq!(app.documents.len(), 2);
        assert_eq!(app.active_document, Some(0));
        assert_eq!(app.document.as_ref().unwrap().len(), 2);
        assert_eq!(
            app.pending_workspace_tabs,
            vec![WorkspaceTab::Document(1), WorkspaceTab::Document(0)]
        );
    }

    #[test]
    fn closing_active_document_selects_an_adjacent_tab() {
        let mut first = tempfile::NamedTempFile::new().unwrap();
        first.write_all(&[1]).unwrap();
        let first_document = Arc::new(BinaryDocument::open(first.path()).unwrap());
        let mut second = tempfile::NamedTempFile::new().unwrap();
        second.write_all(&[2, 3]).unwrap();
        let mut app = test_app(first_document);
        app.open(second.path());

        app.close_document(1);
        assert_eq!(app.documents.len(), 1);
        assert_eq!(app.active_document, Some(0));
        assert_eq!(app.document.as_ref().unwrap().len(), 1);

        app.close_document(0);
        assert!(app.documents.is_empty());
        assert_eq!(app.active_document, None);
        assert!(app.document.is_none());
    }

    #[test]
    fn each_document_restores_its_own_view_settings() {
        let mut first = tempfile::NamedTempFile::new().unwrap();
        first.write_all(&[1, 2]).unwrap();
        let first_document = Arc::new(BinaryDocument::open(first.path()).unwrap());
        let mut second = tempfile::NamedTempFile::new().unwrap();
        second.write_all(&[3, 4, 5]).unwrap();
        let mut app = test_app(first_document);
        app.display_mode = DisplayMode::FixedHex;
        app.bytes_per_row = 32;
        app.cursor = Some(1);

        app.open(second.path());
        app.display_mode = DisplayMode::Character;
        app.bytes_per_row = 8;
        app.cursor = Some(2);

        app.open(first.path());
        assert_eq!(app.display_mode, DisplayMode::FixedHex);
        assert_eq!(app.bytes_per_row, 32);
        assert_eq!(app.cursor, Some(1));

        app.open(second.path());
        assert_eq!(app.display_mode, DisplayMode::Character);
        assert_eq!(app.bytes_per_row, 8);
        assert_eq!(app.cursor, Some(2));
    }

    #[test]
    fn secondary_click_on_content_opens_selection_menu() {
        let context = egui::Context::default();
        let mut menu_requested = false;
        let pointer = egui::Pos2::new(100.0, 100.0);
        let mut run_frame = |pressed| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(400.0, 300.0),
                )),
                events: vec![
                    egui::Event::PointerMoved(pointer),
                    egui::Event::PointerButton {
                        pos: pointer,
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            };
            let mut output = context.run_ui(input, |ui| {
                let content_rect = ui.max_rect();
                ui.allocate_rect(content_rect, egui::Sense::click_and_drag());
                menu_requested = ui.ctx().input(|input| {
                    selection_menu_position(
                        input.pointer.secondary_clicked(),
                        input.pointer.interact_pos(),
                        content_rect,
                    )
                    .is_some()
                });
            });
            output.textures_delta.clear();
        };

        run_frame(true);
        run_frame(false);
        assert!(
            menu_requested,
            "right-click on byte content did not request the menu"
        );
    }

    #[test]
    fn selection_highlight_is_translucent_for_theme_independent_contrast() {
        let [red, green, blue, alpha] = selection_background_color().to_srgba_unmultiplied();
        assert!((69..=71).contains(&red));
        assert!((144..=147).contains(&green));
        assert!((219..=222).contains(&blue));
        assert!(alpha > 0 && alpha < 160);
    }

    #[test]
    fn selection_drag_scrolls_toward_viewport_edges() {
        let viewport = egui::Rect::from_min_max(egui::pos2(100.0, 50.0), egui::pos2(500.0, 350.0));

        assert_eq!(
            edge_auto_scroll_delta(egui::pos2(100.0, 200.0), viewport, 32.0, 24.0),
            egui::vec2(24.0, 0.0)
        );
        assert_eq!(
            edge_auto_scroll_delta(egui::pos2(500.0, 350.0), viewport, 32.0, 24.0),
            egui::vec2(-24.0, -24.0)
        );
        assert_eq!(
            edge_auto_scroll_delta(egui::pos2(300.0, 200.0), viewport, 32.0, 24.0),
            egui::Vec2::ZERO
        );
    }

    #[test]
    fn active_column_mode_enables_curve_actions_for_existing_selection() {
        let selection = crate::eframe_app::selection::Selection {
            mode: SelectionMode::Continuous,
            anchor: crate::eframe_app::selection::SelectionPoint {
                row: 0,
                column: 0,
                offset: 0,
            },
            focus: crate::eframe_app::selection::SelectionPoint {
                row: 2,
                column: 1,
                offset: 33,
            },
        };

        assert!(curve_actions_enabled(
            SelectionMode::Column,
            Some(selection)
        ));
        assert!(curve_actions_enabled(SelectionMode::Block, Some(selection)));
        assert!(!curve_actions_enabled(SelectionMode::Row, Some(selection)));
    }

    #[test]
    fn display_modes_have_expected_labels() {
        assert_eq!(
            DisplayMode::ALL.map(DisplayMode::label),
            [
                "比特位图",
                "等长帧比特字符",
                "等长帧十六进制",
                "字符方式",
                "十六进制字符对应",
                "不定帧长",
            ]
        );
    }

    #[test]
    fn vertical_axis_modes_format_row_metadata() {
        assert_eq!(
            VerticalAxisMode::ALL.map(VerticalAxisMode::label),
            ["帧号", "帧长", "起始比特"]
        );
        assert_eq!(VerticalAxisMode::FrameNumber.value(3, 48, 16), "3");
        assert_eq!(VerticalAxisMode::FrameLength.value(3, 48, 7), "7");
        assert_eq!(VerticalAxisMode::StartBit.value(3, 48, 16), "384");
    }

    #[test]
    fn bit_text_uses_most_significant_bit_first() {
        assert_eq!(format_bit_text(0x81), "10000001");
        assert_eq!(format_bit_text(0x42), "01000010");
    }

    #[test]
    fn character_mode_uses_single_byte_latin1_mapping() {
        assert_eq!(display_character(b'A'), 'A');
        assert_eq!(display_character(0xA3), '£');
        assert_eq!(display_character(0x1F), '·');
        assert_eq!(display_character(0x7F), '·');
        assert_eq!(display_character(0x80), '·');
    }

    #[test]
    fn wide_rows_keep_render_work_bounded() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&vec![0x41; 64 * 1024]).unwrap();
        let document = Arc::new(BinaryDocument::open(file.path()).unwrap());
        let mut app = test_app(document);
        app.bytes_per_row = 1024;
        let context = egui::Context::default();

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(1200.0, 760.0),
            )),
            ..Default::default()
        };
        for mode in DisplayMode::ALL {
            app.display_mode = mode;
            let mut output = context.run_ui(input.clone(), |context| {
                egui::CentralPanel::default().show(context, |ui| app.hex_view(ui));
            });
            output.textures_delta.clear();
            let shape_limit = 2_000;
            assert!(
                output.shapes.len() < shape_limit,
                "{} wide rows produced {} paint shapes",
                mode.label(),
                output.shapes.len()
            );
        }
    }

    #[test]
    fn variable_frame_view_keeps_render_work_bounded() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let data = vec![0x41; 64 * 1024];
        file.write_all(&data).unwrap();
        let document = Arc::new(BinaryDocument::open(file.path()).unwrap());
        let mut app = test_app(document);
        app.display_mode = DisplayMode::VariableFrame;
        app.bytes_per_row = 1;
        app.frames = (0..512)
            .map(|index| FrameIndex {
                start: index * 128,
                len: 128,
                status: FrameStatus::Ok,
            })
            .collect();
        app.frame_longest_len = 128;
        let context = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(1200.0, 760.0),
            )),
            ..Default::default()
        };

        let mut output = context.run_ui(input, |context| {
            egui::CentralPanel::default().show(context, |ui| app.hex_view(ui));
        });
        output.textures_delta.clear();

        assert!(
            output.shapes.len() > 200,
            "frame view still appears capped by bytes_per_row: {} paint shapes",
            output.shapes.len()
        );
        assert!(
            output.shapes.len() < 2_000,
            "frame view produced {} paint shapes",
            output.shapes.len()
        );
    }

    #[test]
    fn frame_navigation_maps_first_and_last_frame() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let data = vec![0x41; 64];
        file.write_all(&data).unwrap();
        let document = Arc::new(BinaryDocument::open(file.path()).unwrap());
        let mut app = test_app(document);
        app.display_mode = DisplayMode::VariableFrame;
        app.frames = vec![
            FrameIndex {
                start: 0,
                len: 8,
                status: FrameStatus::Ok,
            },
            FrameIndex {
                start: 12,
                len: 4,
                status: FrameStatus::Ok,
            },
            FrameIndex {
                start: 20,
                len: 20,
                status: FrameStatus::Ok,
            },
        ];
        app.cursor = Some(24);

        app.navigate(NavigationTarget::FirstRow);
        assert_eq!(app.cursor, Some(7));
        assert_eq!(app.pending_frame_scroll, Some(0));
        assert_eq!(app.pending_navigation, None);

        app.navigate(NavigationTarget::LastRow);
        assert_eq!(app.cursor, Some(20));
        assert_eq!(app.pending_frame_scroll, Some(2));

        app.navigate(NavigationTarget::FirstColumn);
        assert_eq!(app.cursor, Some(20));
        assert_eq!(app.pending_navigation, Some(NavigationTarget::FirstColumn));

        app.navigate(NavigationTarget::LastColumn);
        assert_eq!(app.cursor, Some(39));
        assert_eq!(app.pending_navigation, Some(NavigationTarget::LastColumn));
    }

    #[test]
    fn frame_mode_clears_selection() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let data = vec![0x41; 32];
        file.write_all(&data).unwrap();
        let document = Arc::new(BinaryDocument::open(file.path()).unwrap());
        let mut app = test_app(document);
        app.selection = Some(Selection {
            mode: SelectionMode::Continuous,
            anchor: SelectionPoint {
                row: 0,
                column: 0,
                offset: 0,
            },
            focus: SelectionPoint {
                row: 1,
                column: 0,
                offset: 16,
            },
        });

        app.display_mode = DisplayMode::VariableFrame;
        app.on_display_mode_changed();

        assert!(app.selection.is_none());
        assert!(app.curve_cache.is_none());
        assert!(app.plot_view.is_none());
        assert_eq!(app.status, "已切换到不定帧长模式，选取功能暂不可用");
    }

    #[test]
    fn clicks_past_frame_end_are_ignored() {
        let frame = FrameIndex {
            start: 100,
            len: 2,
            status: FrameStatus::Ok,
        };
        let hex_rect = egui::Rect::from_min_size(
            egui::pos2(300.0, 10.0),
            egui::vec2(16.0 * BesApp::BYTE_CELL_WIDTH, 20.0),
        );
        assert_eq!(
            frame_click_offset(hex_rect.left() + 0.5, hex_rect, frame, 16, 1024),
            Some(100)
        );
        assert_eq!(
            frame_click_offset(
                hex_rect.left() + 1.5 * BesApp::BYTE_CELL_WIDTH,
                hex_rect,
                frame,
                16,
                1024,
            ),
            Some(101)
        );
        assert_eq!(
            frame_click_offset(
                hex_rect.left() + 2.5 * BesApp::BYTE_CELL_WIDTH,
                hex_rect,
                frame,
                16,
                1024,
            ),
            None
        );
        assert_eq!(
            frame_click_offset(hex_rect.left() - 5.0, hex_rect, frame, 16, 1024),
            None
        );
    }

    #[test]
    fn frame_scroll_offset_targets_the_requested_frame_row() {
        assert_eq!(frame_scroll_offset(0, 20.0, 6.0), 0.0);
        assert_eq!(frame_scroll_offset(3, 20.0, 6.0), 78.0);
    }

    #[test]
    fn frame_index_locates_the_containing_frame() {
        let frames = [
            FrameIndex {
                start: 0,
                len: 8,
                status: FrameStatus::Ok,
            },
            FrameIndex {
                start: 12,
                len: 4,
                status: FrameStatus::Ok,
            },
            FrameIndex {
                start: 20,
                len: 20,
                status: FrameStatus::Truncated,
            },
        ];

        assert_eq!(frame_index_at(&frames, 0), 0);
        assert_eq!(frame_index_at(&frames, 7), 0);
        assert_eq!(frame_index_at(&frames, 10), 0);
        assert_eq!(frame_index_at(&frames, 12), 1);
        assert_eq!(frame_index_at(&frames, 24), 2);
        assert_eq!(frame_index_at(&frames, 4_096), 2);
    }
}
