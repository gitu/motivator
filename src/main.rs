#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod app;
mod config;
mod photo;
mod theme;

fn main() -> eframe::Result {
    // `motivator --cutout <image> <friend-id>` runs the photo pipeline and
    // exits — useful for scripting/debugging avatar cut-outs
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "--cutout") {
        let (src, id) = (
            args.get(2).expect("usage: --cutout <image> <friend-id>"),
            args.get(3).expect("usage: --cutout <image> <friend-id>"),
        );
        match photo::process_and_store(std::path::Path::new(src), id) {
            Ok(p) => println!("ok {} split={:.3}", p.path.display(), p.split),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    let cfg = config::Config::load();
    // corner anchoring needs client-side positioning, which Wayland forbids —
    // prefer XWayland when both are available
    if cfg.prefer_x11 && std::env::var_os("DISPLAY").is_some() {
        std::env::remove_var("WAYLAND_DISPLAY");
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_resizable(false)
            .with_taskbar(false)
            .with_app_id("motivator")
            .with_title("motivator")
            .with_inner_size(app::initial_size(&cfg)),
        ..Default::default()
    };
    eframe::run_native(
        "motivator",
        options,
        Box::new(|cc| Ok(Box::new(app::MotivatorApp::new(cc, cfg)))),
    )
}
