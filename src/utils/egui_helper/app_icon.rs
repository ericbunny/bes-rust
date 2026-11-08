use eframe::egui;

#[doc = "内置应用图标（PNG），与 assets/bes.ico 由同一份绘制脚本生成。"]
const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/bes.png");

#[doc = "加载窗口图标，使标题栏、任务栏与可执行文件图标保持一致。"]
pub fn load_app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(APP_ICON_PNG).expect("内置应用图标必须是有效的 PNG")
}

#[cfg(test)]
mod tests {
    use super::load_app_icon;

    #[test]
    fn window_icon_is_a_square_opaque_bitmap() {
        let icon = load_app_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
        assert!(
            icon.rgba.chunks_exact(4).any(|pixel| pixel[3] == 255),
            "窗口图标不能是全透明的空图"
        );
    }

    #[test]
    fn icon_file_contains_the_sizes_windows_shell_expects() {
        const ICON_FILE: &[u8] = include_bytes!("../../../assets/bes.ico");
        assert_eq!(u16::from_le_bytes([ICON_FILE[0], ICON_FILE[1]]), 0);
        assert_eq!(u16::from_le_bytes([ICON_FILE[2], ICON_FILE[3]]), 1);
        let count = u16::from_le_bytes([ICON_FILE[4], ICON_FILE[5]]) as usize;
        assert_eq!(count, 6, "图标应包含 6 个尺寸");
        let sizes: Vec<u8> = (0..count).map(|index| ICON_FILE[6 + index * 16]).collect();
        assert_eq!(sizes, vec![16, 32, 48, 64, 128, 0], "0 表示 256×256 条目");
        for index in 0..count {
            let entry = 6 + index * 16;
            let offset =
                u32::from_le_bytes(ICON_FILE[entry + 12..entry + 16].try_into().unwrap()) as usize;
            let len =
                u32::from_le_bytes(ICON_FILE[entry + 8..entry + 12].try_into().unwrap()) as usize;
            assert!(
                offset + len <= ICON_FILE.len(),
                "第 {index} 个图标条目的数据越界"
            );
        }
        let png_entry = 6 + (count - 1) * 16;
        let png_offset =
            u32::from_le_bytes(ICON_FILE[png_entry + 12..png_entry + 16].try_into().unwrap())
                as usize;
        assert!(
            eframe::icon_data::from_png_bytes(&ICON_FILE[png_offset..]).is_ok(),
            "256×256 条目应是可解析的 PNG"
        );
    }
}
