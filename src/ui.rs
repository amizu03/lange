use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use cccedict::cedict::CedictEntry;
use eframe::egui::{self, RichText};
use egui::{Color32, ComboBox, Margin, Visuals};
use egui_toast::{Toast, ToastKind, ToastOptions, Toasts};

use crate::anki::{CEDictEntry, Config};

const MAIN_TEXT_SIZE: f32 = 20.0;
const SMALL_TEXT_SIZE: f32 = 15.0;

pub struct CaptionApp {
    pub captions: Arc<Mutex<Vec<Vec<CedictEntry>>>>,
    pub current_entry: Option<(CedictEntry, String)>,
    pub config: Arc<Mutex<Config>>,
    pub toasts: Toasts,
    pub in_settings: bool,
    pub font: String,
    pub fonts: Vec<String>,
    pub model: String,
    pub models: Vec<String>,
}

const CONFIG_PATH: &str = "out/config.json";
const ANKI_PATH: &str = "out/deck.apkg";

fn format_pinyin(entry: &CedictEntry) -> String {
    match &entry.pinyin {
        Some(syllables) => prettify_pinyin::prettify(
            &syllables
                .iter()
                .map(|s| format!("{}{}", s.pronunciation, s.tone))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        None => String::new(),
    }
}

impl eframe::App for CaptionApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from_rgba_premultiplied(0.0, 0.0, 0.0, 0.7).to_array()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx().request_repaint_after(Duration::from_millis(100));

        let mut reset_ce = false;

        if self.in_settings {
            egui::Area::new("settings_toolbar".into())
                .anchor(egui::Align2::RIGHT_TOP, (-8.0, 8.0))
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    ui.horizontal_top(|ui| {
                        if ui
                            .button(RichText::new("x").size(SMALL_TEXT_SIZE).monospace())
                            .clicked()
                        {
                            self.in_settings = false;
                        }
                    });
                });

            egui::ScrollArea::vertical()
                .id_salt("entry_scroll")
                .auto_shrink(false)
                .content_margin(Margin::symmetric(10, 10))
                .show(ui, |ui| {
                    let mut should_save = false;

                    ComboBox::new(
                        "settings_font",
                        RichText::new("Font").size(SMALL_TEXT_SIZE).monospace(),
                    )
                    .selected_text(RichText::new(&self.font).size(SMALL_TEXT_SIZE).monospace())
                    .show_ui(ui, |ui| {
                        for font in &self.fonts {
                            if ui
                                .selectable_value(&mut self.font, font.clone(), font)
                                .clicked()
                            {
                                self.config.lock().unwrap().font = font.clone();
                                should_save = true;
                            }
                        }
                    });

                    ComboBox::new(
                        "settings_model",
                        RichText::new("Model").size(SMALL_TEXT_SIZE).monospace(),
                    )
                    .selected_text(RichText::new(&self.model).size(SMALL_TEXT_SIZE).monospace())
                    .show_ui(ui, |ui| {
                        for model in &self.models {
                            if ui
                                .selectable_value(&mut self.model, model.clone(), model)
                                .clicked()
                            {
                                self.config.lock().unwrap().model = model.clone();
                                should_save = true;
                            }
                        }
                    });

                    if should_save {
                        // save to deck
                        {
                            let config = self.config.lock().unwrap();

                            config.save(CONFIG_PATH);
                        }

                        self.toasts.add(Toast {
                            text: RichText::new("Changed config - restart to apply")
                                .size(SMALL_TEXT_SIZE)
                                .monospace()
                                .into(),
                            kind: ToastKind::Success,
                            options: ToastOptions::default()
                                .duration_in_seconds(5.0)
                                .show_progress(true),
                            ..Default::default()
                        });
                    }
                });
        } else if let Some((current_entry, pinyin)) = &self.current_entry {
            egui::Area::new("dict_toolbar".into())
                .anchor(egui::Align2::RIGHT_TOP, (-8.0, 8.0))
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    ui.horizontal_top(|ui| {
                        if ui
                            .button(RichText::new("+").size(SMALL_TEXT_SIZE).monospace())
                            .clicked()
                        {
                            let mut config = self.config.lock().unwrap();

                            if config.lookup(&current_entry.simplified).is_none() {
                                config.insert(CEDictEntry {
                                    zh: current_entry.simplified.clone(),
                                    pinyin: pinyin.clone(),
                                    definition: current_entry
                                        .definitions
                                        .as_ref()
                                        .map_or(String::new(), |s| s.join("\n")),
                                });

                                self.toasts.add(Toast {
                                    text: RichText::new("Added card")
                                        .size(SMALL_TEXT_SIZE)
                                        .monospace()
                                        .into(),
                                    kind: ToastKind::Success,
                                    options: ToastOptions::default()
                                        .duration_in_seconds(5.0)
                                        .show_progress(true),
                                    ..Default::default()
                                });
                            } else {
                                self.toasts.add(Toast {
                                    text: RichText::new("Already exists")
                                        .size(SMALL_TEXT_SIZE)
                                        .monospace()
                                        .into(),
                                    kind: ToastKind::Info,
                                    options: ToastOptions::default()
                                        .duration_in_seconds(5.0)
                                        .show_progress(true),
                                    ..Default::default()
                                });
                            }
                        }

                        if ui
                            .button(RichText::new("x").size(SMALL_TEXT_SIZE).monospace())
                            .clicked()
                        {
                            reset_ce = true;
                        }
                    });
                });

            egui::ScrollArea::vertical()
                .id_salt("entry_scroll")
                .auto_shrink(false)
                .content_margin(Margin::symmetric(10, 10))
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.label(
                            RichText::new(&current_entry.simplified)
                                .size(MAIN_TEXT_SIZE)
                                .monospace(),
                        );

                        ui.label(RichText::new(pinyin).size(SMALL_TEXT_SIZE).monospace());
                    });

                    ui.separator();

                    if let Some(defs) = &current_entry.definitions {
                        for def in defs {
                            ui.label(RichText::new(def).size(SMALL_TEXT_SIZE).monospace());
                        }
                    }
                });
        } else {
            egui::Area::new("toolbar".into())
                .anchor(egui::Align2::RIGHT_TOP, (-8.0, 8.0))
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .button(RichText::new("Settings").size(SMALL_TEXT_SIZE).monospace())
                            .clicked()
                        {
                            self.in_settings = true;
                        }

                        if ui
                            .button(RichText::new("Clear").size(SMALL_TEXT_SIZE).monospace())
                            .clicked()
                        {
                            self.captions.lock().unwrap().clear();
                        }

                        if ui
                            .button(RichText::new("Save").size(SMALL_TEXT_SIZE).monospace())
                            .clicked()
                        {
                            // save to deck
                            {
                                let anki = self.config.lock().unwrap();

                                anki.save(CONFIG_PATH);
                            }

                            self.toasts.add(Toast {
                                text: RichText::new("Saved dictionary")
                                    .size(SMALL_TEXT_SIZE)
                                    .monospace()
                                    .into(),
                                kind: ToastKind::Success,
                                options: ToastOptions::default()
                                    .duration_in_seconds(5.0)
                                    .show_progress(true),
                                ..Default::default()
                            });
                        }

                        if ui
                            .button(RichText::new("Export").size(SMALL_TEXT_SIZE).monospace())
                            .clicked()
                        {
                            // save to deck
                            {
                                let anki = self.config.lock().unwrap();

                                anki.export(ANKI_PATH);
                            }

                            self.toasts.add(Toast {
                                text: RichText::new("Exported to anki deck")
                                    .size(SMALL_TEXT_SIZE)
                                    .monospace()
                                    .into(),
                                kind: ToastKind::Success,
                                options: ToastOptions::default()
                                    .duration_in_seconds(5.0)
                                    .show_progress(true),
                                ..Default::default()
                            });
                        }

                        if ui
                            .button(RichText::new("x").size(SMALL_TEXT_SIZE).monospace())
                            .clicked()
                        {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });

            let lines = self.captions.lock().unwrap();

            egui::ScrollArea::vertical()
                .id_salt("captions_scroll")
                .stick_to_bottom(true)
                .auto_shrink(false)
                .content_margin(Margin::symmetric(10, 10))
                .show(ui, |ui| {
                    let button_padding = ui.style_mut().spacing.button_padding.x;

                    ui.style_mut().spacing.button_padding.x = -3.0;

                    for line in lines.iter() {
                        ui.horizontal(|ui| {
                            for entry in line.iter() {
                                let response = ui.selectable_label(
                                    false,
                                    RichText::new(&entry.simplified)
                                        .size(MAIN_TEXT_SIZE)
                                        .monospace(),
                                );

                                if response.secondary_clicked() {
                                    let mut config = self.config.lock().unwrap();

                                    if config.lookup(&entry.simplified).is_none() {
                                        config.insert(CEDictEntry {
                                            zh: entry.simplified.clone(),
                                            pinyin: format_pinyin(entry),
                                            definition: entry
                                                .definitions
                                                .as_ref()
                                                .map_or(String::new(), |s| s.join("\n")),
                                        });

                                        self.toasts.add(Toast {
                                            text: RichText::new("Added card")
                                                .size(SMALL_TEXT_SIZE)
                                                .monospace()
                                                .into(),
                                            kind: ToastKind::Success,
                                            options: ToastOptions::default()
                                                .duration_in_seconds(5.0)
                                                .show_progress(true),
                                            ..Default::default()
                                        });
                                    } else {
                                        self.toasts.add(Toast {
                                            text: RichText::new("Already exists")
                                                .size(SMALL_TEXT_SIZE)
                                                .monospace()
                                                .into(),
                                            kind: ToastKind::Info,
                                            options: ToastOptions::default()
                                                .duration_in_seconds(5.0)
                                                .show_progress(true),
                                            ..Default::default()
                                        });
                                    }
                                } else if response.clicked() {
                                    self.current_entry =
                                        Some((entry.clone(), format_pinyin(entry)));
                                }
                            }
                        });
                    }

                    ui.style_mut().spacing.button_padding.x = button_padding;
                });
        }

        if reset_ce {
            self.current_entry = None;
        }

        self.toasts.show(ui);
    }
}
