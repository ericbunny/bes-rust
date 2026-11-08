//! cargo run --bin bes_egui --release
//! cargo build --bin bes_egui --release
//!
#![cfg_attr(windows, windows_subsystem = "windows")]

fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("BES 二进制数据分析器")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 560.0])
            .with_drag_and_drop(true)
            // 与可执行文件内嵌的图标同源，任务栏与资源管理器显示一致
            .with_icon(bes_rust::utils::egui_helper::load_app_icon()),
        // 窗口首次出现时居中，避免停在屏幕左上角
        centered: true,
        ..Default::default()
    }
}

fn main() -> eframe::Result {
    eframe::run_native(
        "bes-rust",
        native_options(),
        Box::new(|context| Ok(Box::new(bes_rust::BesApp::new(context)))),
    )
}

#[cfg(test)]
mod tests {
    use super::native_options;

    #[test]
    fn native_window_enables_file_drag_and_drop() {
        assert_eq!(native_options().viewport.drag_and_drop, Some(true));
    }

    #[test]
    fn native_window_starts_centered() {
        assert!(native_options().centered);
    }

    #[test]
    fn native_window_uses_the_embedded_app_icon() {
        let icon = native_options()
            .viewport
            .icon
            .expect("窗口必须设置应用图标，否则任务栏会退回默认图标");
        assert_eq!((icon.width, icon.height), (256, 256));
    }
}
