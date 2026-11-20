//! Entry point. Everything lives in the library so tests can drive it.

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([1024.0, 640.0])
            .with_title("m241"),
        ..Default::default()
    };
    eframe::run_native(
        "m241",
        options,
        Box::new(|cc| {
            m241_gui::theme::install(&cc.egui_ctx);
            Ok(Box::new(m241_gui::App::default()))
        }),
    )
}
