use std::sync::Arc;

use eframe::egui;

pub fn load_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "simhei".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "fonts/simhei.ttf"
        ))),
    );
    fonts
        .families
        .get_mut(&egui::FontFamily::Proportional)
        .unwrap()
        .insert(0, "simhei".to_owned());
    fonts
        .families
        .get_mut(&egui::FontFamily::Monospace)
        .unwrap()
        .push("simhei".to_owned());
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, FontFamily, FontId};

    use super::load_fonts;

    #[test]
    fn installs_chinese_glyphs_for_both_ui_font_families() {
        let context = egui::Context::default();
        load_fonts(&context);
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.fonts_mut(|fonts| {
                assert!(fonts.has_glyph(&FontId::new(16.0, FontFamily::Proportional), '中'));
                assert!(fonts.has_glyph(&FontId::new(16.0, FontFamily::Monospace), '文'));
            });
        });
        output.textures_delta.clear();
    }
}
