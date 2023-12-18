struct ProfileRenderData {}

use crate::Notecrumbs;

struct NoteRenderData {
    content: String,
    profile: ProfileRenderData,
}

enum RenderData {
    Note(NoteRenderData),
}

fn note_ui(app: &Notecrumbs, ctx: &egui::Context, content: &str) {
    use egui::{FontId, RichText};

    egui::CentralPanel::default().show(&ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("✏").font(FontId::proportional(120.0)));
            ui.vertical(|ui| {
                ui.label(RichText::new(content).font(FontId::proportional(40.0)));
            });
        })
    });
}

pub fn render_note(app: &Notecrumbs, content: &str) -> Vec<u8> {
    use egui_skia::{rasterize, RasterizeOptions};
    use skia_safe::EncodedImageFormat;

    let options = RasterizeOptions {
        pixels_per_point: 1.0,
        frames_before_screenshot: 1,
    };

    let mut surface = rasterize((1200, 630), |ctx| note_ui(app, ctx, content), Some(options));

    surface
        .image_snapshot()
        .encode_to_data(EncodedImageFormat::PNG)
        .expect("expected image")
        .as_bytes()
        .into()
}
