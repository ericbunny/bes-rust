use eframe::egui;
use egui_dock::{
    DockArea, DockState, Node, NodeIndex, Split, Style as DockStyle, TabViewer,
};
use egui_dock::tab_viewer::OnCloseResponse;

use super::{BesApp, ToolbarLayout, WorkspaceTab};

impl BesApp {
    #[doc = "打开或聚焦指定的工作区标签页。"]
    pub(super) fn open_workspace_tab(&mut self, tab: WorkspaceTab) {
        self.pending_workspace_tabs.push(tab);
    }

    #[doc = "初始化停靠布局并绘制工作区。"]
    pub(super) fn show_workspace(&mut self, ui: &mut egui::Ui) {
        let layout = self.toolbar_layout();
        if self.applied_toolbar_layout != Some(layout) {
            let reserved_height = match layout {
                ToolbarLayout::Compact => Self::COMPACT_TOOLBAR_HEIGHT,
                ToolbarLayout::WithFrameRules => Self::FRAME_TOOLBAR_HEIGHT,
            };
            let toolbar_fraction =
                toolbar_fraction_for_height(ui.available_height(), reserved_height);
            if let Some(egui_dock::Node::Vertical(root_split)) =
                self.dock_state.main_surface_mut().root_node_mut()
            {
                root_split.fraction = toolbar_fraction;
            }
            self.applied_toolbar_layout = Some(layout);
        }

        let mut dock_style = DockStyle::from_egui(ui.style().as_ref());
        dock_style.separator.extra = 40.0;
        dock_style.separator.extra_interact_width = 8.0;
        dock_style.tab.active.bg_fill = ui.visuals().selection.bg_fill;
        // 选中标签使用白字，保证浅色主题下蓝底深字也能看清
        dock_style.tab.active.text_color = egui::Color32::WHITE;
        dock_style.tab.active.corner_radius = egui::CornerRadius::same(6);
        dock_style.tab.inactive.bg_fill = ui.visuals().window_fill;
        dock_style.tab.inactive.text_color = ui.visuals().text_color();
        dock_style.tab.inactive.corner_radius = egui::CornerRadius::same(6);
        dock_style.tab.hovered.bg_fill = ui.visuals().widgets.hovered.weak_bg_fill;
        dock_style.tab.hovered.corner_radius = egui::CornerRadius::same(6);
        let mut dock_state = std::mem::replace(&mut self.dock_state, DockState::new(Vec::new()));
        DockArea::new(&mut dock_state)
            .style(dock_style)
            .show_close_buttons(true)
            .show_leaf_collapse_buttons(false) // 隐藏倒三角
            .show_leaf_close_all_buttons(false) // 隐藏 ×
            .show_inside(ui, &mut WorkspaceTabViewer { app: self });
        self.dock_state = dock_state;
        for tab in std::mem::take(&mut self.pending_workspace_tabs) {
            open_workspace_tab(&mut self.dock_state, tab);
        }
    }
}

pub(super) fn open_workspace_tab(dock_state: &mut DockState<WorkspaceTab>, tab: WorkspaceTab) {
    if tab == WorkspaceTab::Data && content_tab_path(dock_state).is_none() {
        *dock_state = default_dock_state();
        return;
    }
    if let Some(path) = dock_state.find_tab(&tab) {
        let _ = dock_state.set_active_tab(path);
    } else if tab == WorkspaceTab::Inspector
        && let Some(data_path) = content_tab_path(dock_state)
    {
        let mut content_path = data_path.node_path();
        while content_path
            .node
            .parent()
            .is_some_and(|parent| parent != NodeIndex::root())
        {
            content_path.node = content_path.node.parent().expect("node has a parent");
        }
        dock_state.split(
            content_path,
            Split::Right,
            0.78,
            Node::leaf(WorkspaceTab::Inspector),
        );
    } else if matches!(tab, WorkspaceTab::Curve | WorkspaceTab::Frames)
        && let Some(data_path) = content_tab_path(dock_state)
    {
        dock_state.split(data_path.node_path(), Split::Below, 0.7, Node::leaf(tab));
    } else if matches!(tab, WorkspaceTab::Document(_))
        && let Some(content_path) = content_tab_path(dock_state)
    {
        let _ = dock_state.set_active_tab(content_path);
        dock_state.set_focused_node_and_surface(content_path.node_path());
        dock_state.push_to_focused_leaf(tab);
        if let Some(data_path) = dock_state.find_tab(&WorkspaceTab::Data) {
            dock_state.remove_tab(data_path);
        }
    } else {
        dock_state.push_to_focused_leaf(tab);
    }
}

fn content_tab_path(dock_state: &DockState<WorkspaceTab>) -> Option<egui_dock::TabPath> {
    dock_state.iter_all_tabs().find_map(|(path, tab)| {
        matches!(tab, WorkspaceTab::Data | WorkspaceTab::Document(_)).then_some(path)
    })
}

pub(super) fn default_dock_state() -> DockState<WorkspaceTab> {
    let mut dock_state = DockState::new(vec![WorkspaceTab::Data]);
    dock_state
        .main_surface_mut()
        .split_above(NodeIndex::root(), 0.15, vec![WorkspaceTab::Toolbar]);
    dock_state
}

pub(super) fn toolbar_fraction_for_height(available_height: f32, toolbar_height: f32) -> f32 {
    if available_height <= 0.0 {
        return 0.12;
    }
    (toolbar_height / available_height).clamp(0.06, 0.4)
}

struct WorkspaceTabViewer<'a> {
    app: &'a mut BesApp,
}

impl TabViewer for WorkspaceTabViewer<'_> {
    type Tab = WorkspaceTab;

    #[doc = "返回工作区标签页的稳定界面标识。"]
    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::new(*tab)
    }

    #[doc = "返回工作区标签页显示的标题。"]
    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        match tab {
            WorkspaceTab::Toolbar => "工具栏",
            WorkspaceTab::Data => "二进制数据",
            WorkspaceTab::Document(id) => {
                return self
                    .app
                    .documents
                    .iter()
                    .find(|open| open.id == *id)
                    .and_then(|open| open.document.path().file_name())
                    .map_or_else(|| "文件".into(), |name| name.to_string_lossy().into_owned().into());
            }
            WorkspaceTab::Inspector => "数据检查器",
            WorkspaceTab::Curve => "曲线图",
            WorkspaceTab::Frames => "帧列表",
        }
        .into()
    }

    #[doc = "将指定工作区标签页分派给对应的界面绘制函数。"]
    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        match tab {
            WorkspaceTab::Toolbar => self.app.top_bar(ui),
            WorkspaceTab::Data => {
                self.app.data_view_rect = Some(ui.max_rect());
                self.app.hex_view(ui);
            }
            WorkspaceTab::Document(id) => {
                if let Some(index) = self.app.documents.iter().position(|open| open.id == *id) {
                    self.app.activate_document(index);
                    self.app.data_view_rect = Some(ui.max_rect());
                    self.app.hex_view(ui);
                }
            }
            WorkspaceTab::Inspector => self.app.inspector(ui),
            WorkspaceTab::Curve => self.app.curve_plot(ui),
            WorkspaceTab::Frames => self.app.frame_list(ui),
        }
    }

    fn on_tab_button(&mut self, tab: &mut Self::Tab, response: &egui::Response) {
        if response.clicked()
            && let WorkspaceTab::Document(id) = *tab
            && let Some(index) = self.app.documents.iter().position(|open| open.id == id)
        {
            self.app.activate_document(index);
        }
    }

    #[doc = "判断指定工作区标签页是否允许关闭。"]
    fn is_closeable(&self, tab: &Self::Tab) -> bool {
        !matches!(*tab, WorkspaceTab::Toolbar | WorkspaceTab::Data)
    }

    fn on_close(&mut self, tab: &mut Self::Tab) -> OnCloseResponse {
        if let WorkspaceTab::Document(id) = *tab
            && let Some(index) = self.app.documents.iter().position(|open| open.id == id)
        {
            self.app.close_document(index);
        }
        OnCloseResponse::Close
    }

    #[doc = "返回指定工作区标签页的水平和垂直滚动条设置。"]
    fn scroll_bars(&self, tab: &Self::Tab) -> [bool; 2] {
        match tab {
            WorkspaceTab::Toolbar | WorkspaceTab::Inspector => [false, true],
            WorkspaceTab::Data
            | WorkspaceTab::Document(_)
            | WorkspaceTab::Curve
            | WorkspaceTab::Frames => [false, false],
        }
    }
}
