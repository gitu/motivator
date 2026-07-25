#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod app;
mod autostart;
mod config;
mod photo;
mod schedule;
mod share;
mod theme;

fn main() -> eframe::Result {
    // `motivator --cutout <image> <friend-id> [auto|precut|raw]` runs the
    // photo pipeline and exits — useful for scripting/debugging avatar
    // cut-outs
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "--cutout") {
        let usage = "usage: --cutout <image> <friend-id> [auto|precut|raw]";
        let (src, id) = (args.get(2).expect(usage), args.get(3).expect(usage));
        let mode = match args.get(4).map(String::as_str) {
            None | Some("auto") => config::PhotoMode::Auto,
            Some("precut") => config::PhotoMode::Precut,
            Some("raw") => config::PhotoMode::Raw,
            Some(_) => panic!("{usage}"),
        };
        match photo::process_and_store(std::path::Path::new(src), id, mode) {
            Ok(p) => {
                let face = p.face.map_or("face=none".to_string(), |f| {
                    format!(
                        "split={:.3} chin={:.3} eyes={}",
                        f.split,
                        f.chin,
                        f.eyes
                            .map_or("none".to_string(), |(y, h)| format!("{y:.3}h{h:.3}"))
                    )
                });
                println!("ok {} {face} frames={}", p.path.display(), p.frames.len());
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    // `motivator --talkframe <cutout.png> <friend-id>` asks the configured
    // image API for a mouth-open frame and stores it as {id}.talk — the
    // headless twin of the "✨ generate with ai" button
    if args.get(1).is_some_and(|a| a == "--talkframe") {
        let usage = "usage: --talkframe <cutout.png> <friend-id>";
        let (src, id) = (args.get(2).expect(usage), args.get(3).expect(usage));
        let cfg = config::Config::load();
        let result = photo::png_bytes_of(std::path::Path::new(src))
            .and_then(|png| api::talk_frame(&cfg.api, &png))
            .and_then(|png| {
                photo::process_and_store_bytes(
                    &png,
                    Some("png"),
                    &format!("{id}.talk"),
                    config::PhotoMode::Auto,
                )
            });
        match result {
            Ok(p) => println!("ok {}", p.path.display()),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    // `motivator --share <friend-id> <out.png>` / `--import <card.png>` run the
    // friend-card codec headless — useful for scripting/debugging shares
    if args.get(1).is_some_and(|a| a == "--share") {
        let (id, out) = (
            args.get(2).expect("usage: --share <friend-id> <out.png>"),
            args.get(3).expect("usage: --share <friend-id> <out.png>"),
        );
        let cfg = config::Config::load();
        let Some(f) = cfg.friends.iter().find(|f| &f.id == id) else {
            eprintln!("error: no friend with id '{id}'");
            std::process::exit(1);
        };
        let sys = theme::system_theme().unwrap_or(egui::Theme::Dark);
        let accent = theme::palette(sys).accent_color(f.accent);
        let result = share::encode_card(f, [accent.r(), accent.g(), accent.b()]).and_then(|card| {
            card.save_with_format(out, image::ImageFormat::Png)
                .map_err(|e| e.to_string())
        });
        match result {
            Ok(()) => println!("ok {out}"),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    if args.get(1).is_some_and(|a| a == "--import") {
        let src = args.get(2).expect("usage: --import <card.png>");
        let mut cfg = config::Config::load();
        let result = image::open(src)
            .map_err(|e| format!("could not read image: {e}"))
            .and_then(|img| share::decode_card(&img.to_rgba8()))
            .and_then(|(s, photo)| share::import_into(&mut cfg, s, photo));
        match result {
            Ok(id) => {
                cfg.save();
                println!("ok imported as {id}");
            }
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
