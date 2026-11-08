use eframe::egui::{self, Align2, Color32, FontId, Rect, RichText, Sense, Vec2};
use egui_plot::{CoordinatesFormatter, Corner, Legend, Line, Plot, PlotPoint, PlotPoints};

use super::{BesApp, CurveCache, PlotRenderer, PlotView, muted_text, subtle_text};
use crate::eframe_app::selection::CurveSeries;

impl BesApp {
    const MAX_CURVE_POINTS: usize = 4_096;

    #[doc = "根据当前列选区或块选区绘制数据曲线。"]
    pub(super) fn curve_plot(&mut self, ui: &mut egui::Ui) {
        let (Some(selection), Some(document)) = (self.selection, self.document.as_ref()) else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("没有可绘制的列选区或块选区")
                        .size(14.0)
                        .color(subtle_text(ui)),
                );
            });
            return;
        };
        let mut reset = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("绘图实现：")
                    .size(12.0)
                    .color(muted_text(ui)),
            );
            ui.selectable_value(&mut self.plot_renderer, PlotRenderer::EguiPlot, "egui_plot");
            ui.selectable_value(&mut self.plot_renderer, PlotRenderer::Legacy, "旧版手绘");
            ui.separator();
            ui.label(
                RichText::new("滚轮缩放 · 左键拖动平移 · 悬停查看数据 · 双击重置")
                    .size(11.0)
                    .color(subtle_text(ui)),
            );
            reset = ui
                .add(
                    egui::Button::new("重置视图")
                        .small()
                        .corner_radius(4),
                )
                .clicked();
        });
        let bytes_per_row = self.bytes_per_row.max(1);
        let cache_matches = self.curve_cache.as_ref().is_some_and(|cache| {
            cache.selection == selection
                && cache.bytes_per_row == bytes_per_row
                && cache.merge_columns == self.plot_merge_columns
        });
        if !cache_matches {
            let series = match selection.curve_series_limited(
                document,
                bytes_per_row,
                self.plot_merge_columns,
                Self::MAX_CURVE_POINTS,
            ) {
                Ok(series) => series,
                Err(error) => {
                    ui.colored_label(Color32::RED, error);
                    return;
                }
            };
            self.curve_cache = Some(CurveCache {
                selection,
                bytes_per_row,
                merge_columns: self.plot_merge_columns,
                series,
            });
        }
        let series = &self
            .curve_cache
            .as_ref()
            .expect("curve cache was initialized")
            .series;
        match self.plot_renderer {
            PlotRenderer::EguiPlot => paint_curve_series_egui_plot(ui, series, reset),
            PlotRenderer::Legacy => {
                if reset {
                    self.plot_view = None;
                }
                paint_curve_series(ui, series, &mut self.plot_view);
            }
        }
    }
}

fn paint_curve_series_egui_plot(ui: &mut egui::Ui, series: &[CurveSeries], reset: bool) {
    let colors = [
        Color32::from_rgb(80, 160, 255),
        Color32::from_rgb(255, 120, 90),
        Color32::from_rgb(90, 200, 130),
        Color32::from_rgb(220, 170, 60),
        Color32::from_rgb(190, 110, 240),
        Color32::from_rgb(70, 210, 210),
    ];
    let mut plot = Plot::new("selection_curve_egui_plot")
        .height(ui.available_height().max(320.0))
        .legend(Legend::default())
        .x_axis_label("行号")
        .y_axis_label("元素值")
        .coordinates_formatter(
            Corner::LeftTop,
            CoordinatesFormatter::new(|pointer, bounds| {
                format_nearest_curve_point(series, *pointer, bounds.width(), bounds.height())
            }),
        )
        .allow_double_click_reset(true);
    if reset {
        plot = plot.reset();
    }
    plot.show(ui, |plot_ui| {
        for (index, curve) in series.iter().enumerate() {
            let points = PlotPoints::from_iter(curve.points.iter().copied());
            plot_ui.line(
                Line::new(curve.name.clone(), points)
                    .color(colors[index % colors.len()])
                    .width(1.2),
            );
        }
    });
}

pub(super) fn format_nearest_curve_point(
    series: &[CurveSeries],
    pointer: PlotPoint,
    x_span: f64,
    y_span: f64,
) -> String {
    let x_span = x_span.abs().max(f64::EPSILON);
    let y_span = y_span.abs().max(f64::EPSILON);
    let nearest = series
        .iter()
        .filter_map(|curve| {
            nearest_point_at_x(&curve.points, pointer.x).map(|point| {
                let dx = (point[0] - pointer.x) / x_span;
                let dy = (point[1] - pointer.y) / y_span;
                (curve, point, dx.hypot(dy))
            })
        })
        .min_by(|left, right| left.2.total_cmp(&right.2));

    match nearest.filter(|(_, _, distance)| *distance <= 0.04) {
        Some((curve, point, _)) => format!(
            "{}\n行号: {:.0}\n值: {}",
            curve.name,
            point[0],
            format_curve_value(point[1])
        ),
        None => String::new(),
    }
}

pub(super) fn nearest_point_at_x(points: &[[f64; 2]], x: f64) -> Option<[f64; 2]> {
    let insertion = points.partition_point(|point| point[0] < x);
    [
        insertion.checked_sub(1),
        (insertion < points.len()).then_some(insertion),
    ]
    .into_iter()
    .flatten()
    .map(|index| points[index])
    .min_by(|left, right| (left[0] - x).abs().total_cmp(&(right[0] - x).abs()))
}

fn format_curve_value(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1.0e16 {
        format!("{value:.0}")
    } else {
        format!("{value:.6}")
    }
}

fn paint_curve_series(ui: &mut egui::Ui, series: &[CurveSeries], view: &mut Option<PlotView>) {
    let colors = [
        Color32::from_rgb(80, 160, 255),
        Color32::from_rgb(255, 120, 90),
        Color32::from_rgb(90, 200, 130),
        Color32::from_rgb(220, 170, 60),
        Color32::from_rgb(190, 110, 240),
        Color32::from_rgb(70, 210, 210),
    ];
    ui.horizontal_wrapped(|ui| {
        for (index, curve) in series.iter().enumerate() {
            ui.colored_label(colors[index % colors.len()], &curve.name);
        }
    });
    let size = Vec2::new(
        ui.available_width().max(320.0),
        ui.available_height().max(320.0),
    );
    let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
    let plot = Rect::from_min_max(
        response.rect.min + Vec2::new(60.0, 12.0),
        response.rect.max - Vec2::new(12.0, 38.0),
    );
    painter.rect_filled(plot, 0.0, ui.visuals().extreme_bg_color);
    painter.rect_stroke(
        plot,
        0.0,
        ui.visuals().window_stroke,
        egui::StrokeKind::Inside,
    );
    let points = series.iter().flat_map(|series| series.points.iter());
    let Some(first) = points.clone().next() else {
        painter.text(
            plot.center(),
            Align2::CENTER_CENTER,
            "选区没有完整数据行",
            FontId::proportional(18.0),
            ui.visuals().weak_text_color(),
        );
        return;
    };
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (first[0], first[0], first[1], first[1]);
    for point in points {
        min_x = min_x.min(point[0]);
        max_x = max_x.max(point[0]);
        min_y = min_y.min(point[1]);
        max_y = max_y.max(point[1]);
    }
    let natural_view = PlotView {
        min_x,
        max_x: max_x.max(min_x + 1.0),
        min_y,
        max_y: max_y.max(min_y + 1.0),
    };
    if view.is_none() || response.double_clicked() {
        *view = Some(natural_view);
    }
    let current = view.as_mut().expect("plot view was initialized");
    if response.dragged() {
        let delta = ui.input(|input| input.pointer.delta());
        let x_span = current.max_x - current.min_x;
        let y_span = current.max_y - current.min_y;
        let shift_x = -f64::from(delta.x / plot.width()) * x_span;
        let shift_y = f64::from(delta.y / plot.height()) * y_span;
        current.min_x += shift_x;
        current.max_x += shift_x;
        current.min_y += shift_y;
        current.max_y += shift_y;
    }
    if let Some(pointer) = response
        .hover_pos()
        .filter(|pointer| plot.contains(*pointer))
    {
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if scroll.abs() > f32::EPSILON {
            let factor = f64::from((scroll * 0.003).exp()).clamp(0.2, 5.0);
            let x_ratio = f64::from((pointer.x - plot.left()) / plot.width());
            let y_ratio = f64::from((plot.bottom() - pointer.y) / plot.height());
            let center_x = current.min_x + x_ratio * (current.max_x - current.min_x);
            let center_y = current.min_y + y_ratio * (current.max_y - current.min_y);
            let x_span = ((current.max_x - current.min_x) / factor).max(1.0e-6);
            let y_span = ((current.max_y - current.min_y) / factor).max(1.0e-6);
            current.min_x = center_x - x_ratio * x_span;
            current.max_x = current.min_x + x_span;
            current.min_y = center_y - y_ratio * y_span;
            current.max_y = current.min_y + y_span;
        }
    }
    let (min_x, max_x, min_y, max_y) = (current.min_x, current.max_x, current.min_y, current.max_y);
    let x_span = (max_x - min_x).max(1.0e-6);
    let y_span = (max_y - min_y).max(1.0e-6);
    for (index, curve) in series.iter().enumerate() {
        let screen_points = curve.points.iter().map(|point| {
            egui::Pos2::new(
                plot.left() + ((point[0] - min_x) / x_span) as f32 * plot.width(),
                plot.bottom() - ((point[1] - min_y) / y_span) as f32 * plot.height(),
            )
        });
        painter.add(egui::Shape::line(
            screen_points.collect(),
            egui::Stroke::new(1.2, colors[index % colors.len()]),
        ));
    }
    if let Some(pointer) = response
        .hover_pos()
        .filter(|pointer| plot.contains(*pointer))
    {
        let mut nearest: Option<(usize, [f64; 2], egui::Pos2, f32)> = None;
        for (series_index, curve) in series.iter().enumerate() {
            for point in &curve.points {
                let screen = egui::Pos2::new(
                    plot.left() + ((point[0] - min_x) / x_span) as f32 * plot.width(),
                    plot.bottom() - ((point[1] - min_y) / y_span) as f32 * plot.height(),
                );
                let distance = screen.distance(pointer);
                if nearest.is_none_or(|nearest| distance < nearest.3) {
                    nearest = Some((series_index, *point, screen, distance));
                }
            }
        }
        if let Some((series_index, point, screen, _distance)) =
            nearest.filter(|item| item.3 <= 16.0)
        {
            painter.line_segment(
                [
                    egui::Pos2::new(screen.x, plot.top()),
                    egui::Pos2::new(screen.x, plot.bottom()),
                ],
                egui::Stroke::new(0.7, ui.visuals().weak_text_color()),
            );
            painter.circle_filled(screen, 4.0, colors[series_index % colors.len()]);
            let tooltip = format!(
                "{}\n行号: {:.0}\n值: {:.6}",
                series[series_index].name, point[0], point[1]
            );
            painter.text(
                pointer + Vec2::new(12.0, 12.0),
                Align2::LEFT_TOP,
                tooltip,
                FontId::monospace(12.0),
                ui.visuals().text_color(),
            );
        }
    }
    painter.text(
        egui::Pos2::new(plot.center().x, response.rect.bottom() - 8.0),
        Align2::CENTER_BOTTOM,
        format!("行号  {min_x:.0} .. {max_x:.0}"),
        FontId::monospace(12.0),
        ui.visuals().text_color(),
    );
    painter.text(
        egui::Pos2::new(response.rect.left() + 4.0, plot.top()),
        Align2::LEFT_TOP,
        format!("要素值\n{max_y:.3}\n..\n{min_y:.3}"),
        FontId::monospace(12.0),
        ui.visuals().text_color(),
    );
}
