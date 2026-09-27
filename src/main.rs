mod anki;
mod audio;
mod capture;
mod transcribe;
mod ui;
mod window;

use std::{
    io::Cursor,
    path::PathBuf,
    str::FromStr,
    sync::{mpsc, Arc, Mutex},
    thread::sleep,
    time::Duration,
};

use cccedict::cedict::{Cedict, CedictEntry};
use eframe::egui::{self};
use egui::Align2;
use egui_toast::Toasts;
use window::{focused_monitor_rect, WindowInfo};

use crate::anki::Config;

const ASSET_LINKS: [(&str, &str); 7] = [
    ("fonts/noto_sans.ttf", "https://github.com/life888888/cjk-fonts-ttf/releases/download/v0.1.0/NotoSansMonoCJKsc-Regular.ttf"),
    ("fonts/zpix.ttf", "https://github.com/SolidZORO/zpix-pixel-font/releases/download/v3.3.0/zpix.ttf"),
    ("fonts/unifont.ttf", "https://github.com/multitheftauto/unifont/releases/download/v16.0.04/unifont-16.0.04.ttf"),
    ("models/medium.bin", "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium-q8_0.bin"),
    ("models/small.bin", "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q8_0.bin"),
    ("models/tiny.bin", "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny-q8_0.bin"),
    ("dicts/cedict.zip", "https://www.mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.zip"),
];

fn list_files(dir: &str) -> Option<Vec<String>> {
    Some(
        std::fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect(),
    )
}

fn download_assets() {
    // Create assets folder holding runtime data
    let _ = std::fs::create_dir("assets/");
    let _ = std::fs::create_dir("assets/fonts/");
    let _ = std::fs::create_dir("assets/models/");
    let _ = std::fs::create_dir("assets/dicts/");

    // Download assets
    for (asset_name, asset_link) in ASSET_LINKS {
        let path = format!("assets/{asset_name}");

        if !std::fs::exists(&path).map_or(false, |x| x) {
            let font = reqwest::blocking::get(asset_link)
                .expect("Failed to download asset")
                .bytes()
                .expect("Failed to get asset bytes");
            std::fs::write(path, font).expect("Failed to write asset to disk");
        }
    }

    // Extract ce dictionary
    zip_extract::extract(
        Cursor::new(std::fs::read("assets/dicts/cedict.zip").expect("Failed to extract cedict")),
        &PathBuf::from("assets/dicts/"),
        false,
    )
    .expect("Failed to extract CEDict");
}

fn main() -> eframe::Result {
    download_assets();

    let (node_id, pid) = loop {
        sleep(Duration::from_secs(1));

        if let Some(w) = WindowInfo::current() {
            if w.class == "firefox"
                || w.class == "firefox-nightly"
                || w.class == "MozillaWindowClass"
            {
                #[cfg(windows)]
                break (0, w.pid);

                #[cfg(unix)]
                if let Some(id) = w.pipewire_node() {
                    break (id, w.pid);
                }
            }
        }
    };

    let _ = std::fs::create_dir("out/");
    let config = Arc::new(Mutex::new(
        Config::from_path("out/config.json").unwrap_or_else(|| Config::new()),
    ));
    let cedict = Arc::new(
        Cedict::from_path("assets/dicts/cedict_ts.u8").expect("Failed to open CE dictionary"),
    );
    let captions: Arc<Mutex<Vec<Vec<CedictEntry>>>> = Arc::new(Mutex::new(Vec::new()));
    let captions_ui = Arc::clone(&captions);

    let (tx, rx) = mpsc::sync_channel::<audio::AudioChunk>(1);

    transcribe::spawn_transcribe_thread(
        rx,
        format!("assets/models/{}", config.lock().unwrap().model),
        captions,
        cedict,
    );
    capture::spawn_capture_thread(tx, node_id, pid);

    const WIN_W: f32 = 600.0;
    const WIN_H: f32 = 60.0 * 2.0;
    const MARGIN: f32 = 80.0;

    let pos = focused_monitor_rect()
        .map(|(mx, my, mw, mh)| egui::Pos2::new(mx + (mw - WIN_W) / 2.0, my + mh - WIN_H - MARGIN));

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([WIN_W, WIN_H])
        .with_title("lange")
        .with_resizable(false)
        .with_maximized(false)
        .with_active(false)
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top();

    if let Some(p) = pos {
        #[cfg(unix)]
        window::apply_compositor_position(p);

        viewport = viewport.with_position(p);
    }

    let options = eframe::NativeOptions {
        viewport,
        stencil_buffer: 0,
        depth_buffer: 0,
        multisampling: 0,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        "lange",
        options,
        Box::new(|cc| {
            #[cfg(windows)]
            window::enable_dwm_per_pixel_alpha(cc);

            let mut fonts = egui::FontDefinitions::default();

            if let Ok(bytes) =
                std::fs::read(format!("assets/fonts/{}", config.lock().unwrap().font))
            {
                let data = egui::FontData::from_owned(bytes);

                fonts.font_data.insert("font".into(), data.into());

                for family in [
                    &egui::FontFamily::Proportional,
                    &egui::FontFamily::Monospace,
                ] {
                    fonts
                        .families
                        .entry(family.clone())
                        .or_default()
                        .insert(0, "font".into());
                }
            }

            cc.egui_ctx.set_fonts(fonts);
            cc.egui_ctx.options_mut(|o| {
                o.tessellation_options.feathering = false;
                o.tessellation_options.round_text_to_pixels = true;
            });

            let font = config.lock().unwrap().font.clone();
            let model = config.lock().unwrap().model.clone();

            Ok(Box::new(ui::CaptionApp {
                captions: captions_ui,
                current_entry: None,
                toasts: Toasts::new()
                    .anchor(Align2::RIGHT_BOTTOM, (-10.0, -10.0)) // 10 units from the bottom right corner
                    .direction(egui::Direction::BottomUp),
                in_settings: false,
                font,
                fonts: list_files("assets/fonts/").expect("Failed to list fonts"),
                model,
                models: list_files("assets/models/").expect("Failed to list models"),
                config,
            }))
        }),
    )
}
