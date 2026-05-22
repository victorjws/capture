use crate::constants::{defaults, gui as gui_const};
use eframe::egui;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Clone, Copy, PartialEq)]
enum ScrollKey {
    Space,
    Down,
    PageDown,
}

impl ScrollKey {
    fn as_str(&self) -> &str {
        match self {
            ScrollKey::Space => "space",
            ScrollKey::Down => "down",
            ScrollKey::PageDown => "pagedown",
        }
    }
}

#[derive(Clone)]
struct CaptureConfig {
    output_filename: String, // Filename without extension
    output_format: String,   // File format (png, jpg, etc.)
    overlap: u32,
    delay: u64,
    scroll_key: ScrollKey,

    // Screenshot mode settings
    max_scrolls: String, // Empty string means unlimited
    scroll_delay: u64,
    duplicate_threshold: usize,

    // Crop settings
    window_only: bool,
    crop_enabled: bool,
    use_preset: bool,
    selected_preset: String,
    crop_x: i32,
    crop_y: i32,
    crop_width: i32,
    crop_height: i32,

    // Font settings
    font_path: String,

    // UI settings
    status_color: [u8; 3], // RGB color values

    // Fix mode settings
    fix_input_path: String,
    fix_screen_height: u32,
    fix_output_filename: String,
    fix_folder_mode: bool,

    // Trim bottom settings (shared across capture and fix)
    trim_bottom: u32,
    half_seam: bool,
    best_overlap: bool,

    // Rename tab settings
    rename_folder_path: String,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            output_filename: "00".to_string(), // Just the filename without extension
            output_format: "png".to_string(),  // Default format
            overlap: defaults::OVERLAP,
            delay: defaults::DELAY,
            scroll_key: ScrollKey::Space,
            max_scrolls: defaults::MAX_SCROLLS_DEFAULT.to_string(),
            scroll_delay: defaults::SCROLL_DELAY,
            duplicate_threshold: defaults::DUPLICATE_THRESHOLD,
            window_only: false,
            crop_enabled: false,
            use_preset: true,
            selected_preset: "naver-series".to_string(),
            crop_x: 607,
            crop_y: 23,
            crop_width: 690,
            crop_height: 1007,
            font_path: String::new(),
            status_color: [255, 255, 0], // Yellow by default
            fix_input_path: String::new(),
            fix_screen_height: 1007,
            fix_output_filename: String::new(),
            fix_folder_mode: false,
            trim_bottom: 448,
            half_seam: false,
            best_overlap: false,
            rename_folder_path: String::new(),
        }
    }
}

#[derive(Clone)]
enum CaptureStatus {
    Idle,
    Running(String),   // Status message
    Completed(String), // Result message
    Error(String),
}

#[derive(Clone, Copy, PartialEq)]
enum TrimMode {
    BottomTrim,
    MiddleCut,
}

#[derive(Clone, Copy, PartialEq)]
enum TrimLine {
    BottomCut,
    MiddleTop,
    MiddleBottom,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Capture,
    Fix,
    Trim,
    Rename,
    Settings,
}

pub struct CaptureApp {
    current_tab: Tab,
    config: CaptureConfig,
    status: Arc<Mutex<CaptureStatus>>,
    is_running: Arc<Mutex<bool>>,
    should_stop: Arc<Mutex<bool>>,
    logs: Arc<Mutex<Vec<String>>>,
    presets: HashMap<String, String>,
    preset_names: Vec<String>,
    font_status: String,
    log_level: log::LevelFilter,

    // Trim tab state
    trim_input_path: String,
    trim_preview_texture: Option<egui::TextureHandle>,
    trim_img_width: u32,
    trim_img_height: u32,
    trim_cut_y: u32,
    trim_preview_rows: u32,
    trim_preview_start_y: u32,
    trim_output_path: String,
    trim_status: String,
    trim_zoom: f32,
    trim_mode: TrimMode,
    trim_middle_top: u32,
    trim_middle_bottom: u32,
    trim_dragging_line: Option<TrimLine>,
}

impl Default for CaptureApp {
    fn default() -> Self {
        let presets = crate::presets::get_all_presets().unwrap_or_default();
        let mut preset_names: Vec<String> = presets.keys().cloned().collect();
        preset_names.sort();

        Self {
            current_tab: Tab::Capture,
            config: CaptureConfig::default(),
            status: Arc::new(Mutex::new(CaptureStatus::Idle)),
            is_running: Arc::new(Mutex::new(false)),
            should_stop: Arc::new(Mutex::new(false)),
            logs: Arc::new(Mutex::new(Vec::new())),
            presets,
            preset_names,
            font_status: "Using default font".to_string(),
            log_level: log::LevelFilter::Info,

            trim_input_path: String::new(),
            trim_preview_texture: None,
            trim_img_width: 0,
            trim_img_height: 0,
            trim_cut_y: 0,
            trim_preview_rows: 800,
            trim_preview_start_y: 0,
            trim_output_path: String::new(),
            trim_status: String::new(),
            trim_zoom: 1.0,
            trim_mode: TrimMode::BottomTrim,
            trim_middle_top: 0,
            trim_middle_bottom: 0,
            trim_dragging_line: None,
        }
    }
}

impl CaptureApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Load fonts to support Unicode (including Korean, Japanese, Chinese, etc.)
        Self::setup_fonts(&cc.egui_ctx);
        log::set_max_level(log::LevelFilter::Info);
        Self::default()
    }

    fn setup_fonts(ctx: &egui::Context) {
        // Try to load font from user-specified or default location
        let mut font_paths: Vec<String> = gui_const::DEFAULT_FONT_PATHS
            .iter()
            .map(|s| s.to_string())
            .collect();

        // Add user config directory path if available
        if let Some(config_path) = gui_const::get_config_font_path() {
            font_paths.push(config_path);
        }

        let mut fonts = egui::FontDefinitions::default();
        let mut font_loaded = false;

        for path in font_paths.iter() {
            if let Ok(font_data) = std::fs::read(path) {
                fonts.font_data.insert(
                    "custom_font".to_owned(),
                    Arc::new(egui::FontData::from_owned(font_data)),
                );

                fonts
                    .families
                    .entry(egui::FontFamily::Proportional)
                    .or_default()
                    .insert(0, "custom_font".to_owned());

                fonts
                    .families
                    .entry(egui::FontFamily::Monospace)
                    .or_default()
                    .push("custom_font".to_owned());

                font_loaded = true;
                log::info!("Loaded font from: {}", path);
                break;
            }
        }

        if !font_loaded {
            log::warn!("No custom font found. Using default font.");
            log::warn!(
                "For Unicode support (Korean, Japanese, Chinese, etc.), place NotoSansKR-Regular.ttf in one of: {}{}",
                gui_const::DEFAULT_FONT_PATHS.join(", "),
                gui_const::get_config_font_path()
                    .map(|p| format!(", {}", p))
                    .unwrap_or_default()
            );
        }

        ctx.set_fonts(fonts);
    }

    fn load_font_from_path(&mut self, ctx: &egui::Context, path: &str) -> bool {
        if path.is_empty() {
            self.font_status = "No font path specified".to_string();
            return false;
        }

        match std::fs::read(path) {
            Ok(font_data) => {
                let mut fonts = egui::FontDefinitions::default();

                fonts.font_data.insert(
                    "custom_font".to_owned(),
                    std::sync::Arc::new(egui::FontData::from_owned(font_data)),
                );

                fonts
                    .families
                    .entry(egui::FontFamily::Proportional)
                    .or_default()
                    .insert(0, "custom_font".to_owned());

                fonts
                    .families
                    .entry(egui::FontFamily::Monospace)
                    .or_default()
                    .push("custom_font".to_owned());

                ctx.set_fonts(fonts);

                // Extract just the filename for display
                let filename = std::path::Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(path);

                self.font_status = format!("Loaded: {}", filename);
                true
            }
            Err(e) => {
                self.font_status = format!("Failed to load font: {}", e);
                false
            }
        }
    }

    fn start_capture(&mut self) {
        if let Err(e) = crate::validate_format(&self.config.output_format) {
            *self.status.lock().unwrap() = CaptureStatus::Error(format!("{}", e));
            return;
        }
        let output_path =
            crate::build_output_path(&self.config.output_filename, &self.config.output_format);
        if let Err(e) = crate::validate_output_path(&output_path) {
            *self.status.lock().unwrap() = CaptureStatus::Error(format!("{}", e));
            return;
        }

        let config = self.config.clone();
        let status = Arc::clone(&self.status);
        let is_running = Arc::clone(&self.is_running);
        let should_stop = Arc::clone(&self.should_stop);
        let logs = Arc::clone(&self.logs);

        // Set running state and reset stop flag
        *is_running.lock().unwrap() = true;
        *should_stop.lock().unwrap() = false;
        *status.lock().unwrap() = CaptureStatus::Running("Initializing capture...".to_string());

        // Clear previous logs
        logs.lock().unwrap().clear();

        // Spawn capture thread
        thread::spawn(move || {
            let result =
                Self::run_capture(config, status.clone(), should_stop.clone(), logs.clone());

            *is_running.lock().unwrap() = false;

            match result {
                Ok(output_path) => {
                    *status.lock().unwrap() =
                        CaptureStatus::Completed(format!("Successfully saved to: {}", output_path));
                }
                Err(e) => {
                    *status.lock().unwrap() =
                        CaptureStatus::Error(format!("Capture failed: {}", e));
                }
            }
        });
    }

    fn stop_capture(&mut self) {
        *self.should_stop.lock().unwrap() = true;
    }

    fn run_capture(
        config: CaptureConfig,
        status: Arc<Mutex<CaptureStatus>>,
        should_stop: Arc<Mutex<bool>>,
        logs: Arc<Mutex<Vec<String>>>,
    ) -> anyhow::Result<String> {
        let capture = crate::ScreenCapture::new_with_logs(logs);

        if config.delay > 0 {
            capture.log(
                log::Level::Info,
                &format!("Starting capture in {} seconds...", config.delay),
            );

            for remaining in (1..=config.delay).rev() {
                *status.lock().unwrap() = CaptureStatus::Running(format!(
                    "Starting in {} second{}...",
                    remaining,
                    if remaining > 1 { "s" } else { "" }
                ));

                if *should_stop.lock().unwrap() {
                    return Err(anyhow::anyhow!("Capture cancelled during countdown"));
                }

                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }

        let crop_option = if config.use_preset && !config.selected_preset.is_empty() {
            use crate::presets;
            if let Ok(all_presets) = presets::get_all_presets() {
                all_presets.get(&config.selected_preset).cloned()
            } else {
                None
            }
        } else if config.crop_enabled {
            Some(format!(
                "{},{},{},{}",
                config.crop_x, config.crop_y, config.crop_width, config.crop_height
            ))
        } else {
            None
        };

        capture.log(log::Level::Info, "Starting screenshot mode...");
        *status.lock().unwrap() = CaptureStatus::Running("Capturing screenshots...".to_string());
        let status_for_phase = Arc::clone(&status);
        let on_phase = move |msg: &str| {
            *status_for_phase.lock().unwrap() = CaptureStatus::Running(msg.to_string());
        };

        let max_scrolls = if config.max_scrolls.is_empty() {
            None
        } else {
            config.max_scrolls.parse().ok()
        };

        capture.log(
            log::Level::Info,
            &format!(
                "Max scrolls: {:?}, Scroll delay: {}ms, Overlap: {}px",
                max_scrolls
                    .map(|n: usize| n.to_string())
                    .unwrap_or("unlimited".to_string()),
                config.scroll_delay,
                config.overlap
            ),
        );

        let result_image = capture.capture_with_scroll_with_stop(
            config.overlap,
            max_scrolls,
            0,
            config.scroll_key.as_str(),
            config.window_only,
            crop_option,
            config.scroll_delay,
            should_stop.clone(),
            config.duplicate_threshold,
            config.trim_bottom,
            config.best_overlap,
            on_phase,
        )?;

        capture.log(log::Level::Info, "Saving image...");
        *status.lock().unwrap() = CaptureStatus::Running("Saving image...".to_string());

        let output_path = crate::build_output_path(&config.output_filename, &config.output_format);
        result_image.save(&output_path)?;
        Ok(output_path)
    }
}

impl eframe::App for CaptureApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.current_tab, Tab::Capture, "📷 Capture");
            ui.selectable_value(&mut self.current_tab, Tab::Fix, "🔧 Fix");
            ui.selectable_value(&mut self.current_tab, Tab::Trim, "✂ Trim");
            ui.selectable_value(&mut self.current_tab, Tab::Rename, "🔢 Rename");
            ui.selectable_value(&mut self.current_tab, Tab::Settings, "⚙ Settings");
        });

        ui.separator();
        ui.add_space(10.0);

        egui::ScrollArea::vertical().show(ui, |ui| match self.current_tab {
            Tab::Capture => self.render_capture_tab(ui, &ctx),
            Tab::Fix => self.render_fix_tab(ui, &ctx),
            Tab::Trim => self.render_trim_tab(ui, &ctx),
            Tab::Rename => self.render_rename_tab(ui, &ctx),
            Tab::Settings => self.render_settings_tab(ui, &ctx),
        });
    }
}

impl CaptureApp {
    fn render_capture_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // Configuration UI
        ui.heading("Screenshot Mode");
        ui.add_space(10.0);

        // Status display
        let current_status = self.status.lock().unwrap().clone();
        match &current_status {
            CaptureStatus::Idle => {
                ui.label("Ready to capture");
            }
            CaptureStatus::Running(msg) => {
                let color = egui::Color32::from_rgb(
                    self.config.status_color[0],
                    self.config.status_color[1],
                    self.config.status_color[2],
                );
                ui.colored_label(color, format!("⏳ {}", msg));
                ctx.request_repaint(); // Keep updating while running
            }
            CaptureStatus::Completed(msg) => {
                ui.colored_label(egui::Color32::GREEN, format!("✓ {}", msg));
            }
            CaptureStatus::Error(msg) => {
                ui.colored_label(egui::Color32::RED, format!("✗ {}", msg));
            }
        }

        ui.add_space(10.0);

        // Action buttons
        let is_running = *self.is_running.lock().unwrap();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!is_running, egui::Button::new("▶ Start Capture"))
                .clicked()
            {
                self.start_capture();
            }

            if ui
                .add_enabled(is_running, egui::Button::new("⏹ Stop Capture"))
                .clicked()
            {
                self.stop_capture();
            }
        });

        ui.add_space(20.0);
        ui.separator();
        ui.add_space(10.0);

        // Common settings
        ui.group(|ui| {
            ui.label("Common Settings");

            ui.horizontal(|ui| {
                ui.label("Output filename:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.output_filename)
                        .hint_text("Enter filename without extension")
                        .desired_width(ui.available_width() * 0.6),
                );

                ui.label("Format:");
                egui::ComboBox::from_id_salt("format_selector")
                    .selected_text(&self.config.output_format)
                    .width(100.0)
                    .show_ui(ui, |ui| {
                        for format in crate::SUPPORTED_FORMATS {
                            ui.selectable_value(
                                &mut self.config.output_format,
                                format.to_string(),
                                *format,
                            );
                        }
                    });
            });

            ui.horizontal(|ui| {
                ui.label("Overlap pixels:");
                ui.add(egui::Slider::new(
                    &mut self.config.overlap,
                    gui_const::OVERLAP_MIN..=gui_const::OVERLAP_MAX,
                ));
            });

            ui.horizontal(|ui| {
                ui.checkbox(&mut self.config.best_overlap, "Best-match overlap detect");
                ui.label(egui::RichText::new("picks highest match ratio for last frame").weak());
            });

            ui.horizontal(|ui| {
                ui.label("Trim bottom (px):");
                ui.add(egui::DragValue::new(&mut self.config.trim_bottom).speed(1.0));
                ui.label(egui::RichText::new("trim bottom of last frame").weak());
            });

            ui.horizontal(|ui| {
                ui.label("Delay before start (seconds):");
                ui.add(egui::Slider::new(
                    &mut self.config.delay,
                    gui_const::DELAY_MIN..=gui_const::DELAY_MAX,
                ));
            });

            ui.horizontal(|ui| {
                ui.label("Scroll key:");
                ui.radio_value(&mut self.config.scroll_key, ScrollKey::Space, "Space");
                ui.radio_value(&mut self.config.scroll_key, ScrollKey::Down, "Down Arrow");
                ui.radio_value(
                    &mut self.config.scroll_key,
                    ScrollKey::PageDown,
                    "Page Down",
                );
            });
        });

        ui.add_space(10.0);

        // Screenshot settings
        ui.group(|ui| {
            ui.label("Screenshot Settings");

            ui.horizontal(|ui| {
                ui.label("Max scrolls (leave empty for unlimited):");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.max_scrolls)
                        .desired_width(ui.available_width()),
                );
            });

            ui.horizontal(|ui| {
                ui.label("Scroll delay (milliseconds):");
                ui.add(egui::Slider::new(
                    &mut self.config.scroll_delay,
                    gui_const::SCROLL_DELAY_MIN..=gui_const::SCROLL_DELAY_MAX,
                ));
            });

            ui.horizontal(|ui| {
                ui.label("Duplicate threshold:");
                ui.add(
                    egui::DragValue::new(&mut self.config.duplicate_threshold).range(
                        gui_const::DUPLICATE_THRESHOLD_MIN..=gui_const::DUPLICATE_THRESHOLD_MAX,
                    ),
                );
            });
        });

        ui.add_space(10.0);

        // Crop settings
        ui.group(|ui| {
            ui.label("Crop Settings");

            ui.checkbox(&mut self.config.window_only, "Capture focused window only");

            ui.separator();

            ui.checkbox(&mut self.config.use_preset, "Use crop preset");

            if self.config.use_preset {
                ui.horizontal(|ui| {
                    ui.label("Preset:");
                    egui::ComboBox::from_id_salt("preset_selector")
                        .selected_text(if self.config.selected_preset.is_empty() {
                            "Select preset..."
                        } else {
                            &self.config.selected_preset
                        })
                        .show_ui(ui, |ui| {
                            for preset_name in &self.preset_names {
                                let label_text = if let Some(value) = self.presets.get(preset_name)
                                {
                                    format!("{}: {}", preset_name, value)
                                } else {
                                    preset_name.clone()
                                };

                                if ui
                                    .selectable_value(
                                        &mut self.config.selected_preset,
                                        preset_name.clone(),
                                        label_text,
                                    )
                                    .clicked()
                                {
                                    // Apply preset values to crop fields
                                    if let Some(crop_str) = self.presets.get(preset_name) {
                                        if let Some((x, y, w, h)) =
                                            crate::presets::parse_crop_region(crop_str)
                                        {
                                            self.config.crop_x = x;
                                            self.config.crop_y = y;
                                            self.config.crop_width = w;
                                            self.config.crop_height = h;
                                        }
                                    }
                                }
                            }
                        });
                });

                if !self.config.selected_preset.is_empty() {
                    if let Some(value) = self.presets.get(&self.config.selected_preset) {
                        ui.label(format!("Region: {}", value));
                    }
                }
            }

            ui.separator();

            ui.checkbox(&mut self.config.crop_enabled, "Custom crop region");

            if self.config.crop_enabled {
                ui.horizontal(|ui| {
                    ui.label("X:");
                    ui.add(egui::DragValue::new(&mut self.config.crop_x).speed(1.0));
                    ui.label("Y:");
                    ui.add(egui::DragValue::new(&mut self.config.crop_y).speed(1.0));
                });

                ui.horizontal(|ui| {
                    ui.label("Width:");
                    ui.add(egui::DragValue::new(&mut self.config.crop_width).speed(1.0));
                    ui.label("Height:");
                    ui.add(egui::DragValue::new(&mut self.config.crop_height).speed(1.0));
                });
            }
        });

        ui.add_space(20.0);

        // Show equivalent CLI command
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label("Equivalent CLI command:");
                if ui.button("📋 Copy").clicked() {
                    let cmd = self.generate_cli_command();
                    ui.ctx().copy_text(cmd);
                }
            });

            ui.add_space(5.0);
            let cmd = self.generate_cli_command();

            ui.add(
                egui::TextEdit::multiline(&mut cmd.as_str())
                    .code_editor()
                    .desired_width(f32::INFINITY),
            );
        });

        ui.add_space(20.0);

        // Capture Log (at the bottom)
        ui.group(|ui| {
            ui.label("Capture Log");
            ui.add_space(5.0);

            let logs = self.logs.lock().unwrap();

            // Use remaining available height, with a reasonable minimum
            let available_height = ui.available_height();
            let scroll_height = available_height.max(200.0);

            egui::ScrollArea::vertical()
                .max_height(scroll_height)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if logs.is_empty() {
                        ui.label("No logs yet...");
                    } else {
                        for log in logs.iter() {
                            ui.label(egui::RichText::new(log).font(egui::FontId::monospace(12.0)));
                        }
                    }
                });
        });
    }

    fn render_settings_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Settings");
        ui.add_space(10.0);

        // Font settings
        ui.group(|ui| {
            ui.label("Font Settings");

            let font_row = ui.horizontal(|ui| {
                ui.label("Font file:");
                ui.add(egui::TextEdit::singleline(&mut self.config.font_path)
                    .desired_width(ui.available_width() - 80.0));

                if ui.button("Browse...").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Font files", &["ttf", "otf", "ttc"])
                        .pick_file()
                    {
                        if let Some(path_str) = path.to_str() {
                            self.config.font_path = path_str.to_string();
                        }
                    }
                }
            });
            let hover_pos = ctx.input(|i| i.pointer.hover_pos());
            if !ctx.input(|i| i.raw.hovered_files.is_empty())
                && hover_pos.map_or(false, |p| font_row.response.rect.contains(p))
            {
                ui.painter().rect_stroke(
                    font_row.response.rect,
                    4.0,
                    egui::Stroke::new(2.0, egui::Color32::LIGHT_BLUE),
                    egui::StrokeKind::Outside,
                );
            }
            let dropped = ctx.input(|i| i.raw.dropped_files.clone());
            if !dropped.is_empty()
                && hover_pos.map_or(false, |p| font_row.response.rect.contains(p))
            {
                if let Some(path) = dropped[0].path.as_ref().and_then(|p| p.to_str()) {
                    self.config.font_path = path.to_string();
                }
            }

            ui.horizontal(|ui| {
                if ui.button("Load Font").clicked() {
                    let font_path = self.config.font_path.clone();
                    self.load_font_from_path(ctx, &font_path);
                }

                ui.label(&self.font_status);
            });

            ui.label("Tip: Load a font file to support different languages (Korean, Japanese, Chinese, etc.)");
        });

        ui.add_space(10.0);

        // UI settings
        ui.group(|ui| {
            ui.label("UI Settings");

            ui.label("Status message color:");
            ui.add_space(5.0);

            // RGB Sliders
            ui.horizontal(|ui| {
                ui.label("R:");
                ui.add(
                    egui::Slider::new(&mut self.config.status_color[0], 0..=255).fixed_decimals(0),
                );
            });

            ui.horizontal(|ui| {
                ui.label("G:");
                ui.add(
                    egui::Slider::new(&mut self.config.status_color[1], 0..=255).fixed_decimals(0),
                );
            });

            ui.horizontal(|ui| {
                ui.label("B:");
                ui.add(
                    egui::Slider::new(&mut self.config.status_color[2], 0..=255).fixed_decimals(0),
                );
            });

            // Color preview
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.label("Preview:");
                let color = egui::Color32::from_rgb(
                    self.config.status_color[0],
                    self.config.status_color[1],
                    self.config.status_color[2],
                );
                ui.colored_label(color, "⏳ Sample status message");
            });

            // Quick presets
            ui.add_space(10.0);
            ui.label("Quick presets:");
            ui.horizontal_wrapped(|ui| {
                if ui.button("Yellow").clicked() {
                    self.config.status_color = [255, 255, 0];
                }
                if ui.button("Cyan").clicked() {
                    self.config.status_color = [0, 255, 255];
                }
                if ui.button("Light Blue").clicked() {
                    self.config.status_color = [100, 200, 255];
                }
                if ui.button("White").clicked() {
                    self.config.status_color = [255, 255, 255];
                }
                if ui.button("Green").clicked() {
                    self.config.status_color = [0, 255, 0];
                }
            });
        });

        ui.add_space(10.0);

        // Log level settings
        ui.group(|ui| {
            ui.label("Log Level");
            ui.add_space(5.0);

            let levels = [
                (log::LevelFilter::Error, "Error"),
                (log::LevelFilter::Warn, "Warn"),
                (log::LevelFilter::Info, "Info"),
                (log::LevelFilter::Debug, "Debug"),
                (log::LevelFilter::Trace, "Trace"),
            ];

            let mut changed = false;
            ui.horizontal(|ui| {
                for (level, label) in &levels {
                    if ui.radio(self.log_level == *level, *label).clicked() {
                        self.log_level = *level;
                        changed = true;
                    }
                }
            });

            if changed {
                log::set_max_level(self.log_level);
            }

            ui.add_space(5.0);
            ui.label(
                egui::RichText::new(match self.log_level {
                    log::LevelFilter::Error => "Only errors",
                    log::LevelFilter::Warn => "Errors + warnings",
                    log::LevelFilter::Info => "Normal operation logs (default)",
                    log::LevelFilter::Debug => "Detailed debug output (image comparison, etc.)",
                    log::LevelFilter::Trace => "All internal events",
                    log::LevelFilter::Off => "",
                })
                .weak(),
            );
        });

        ui.add_space(10.0);

        ui.group(|ui| {
            ui.label("Reset Configuration");
            ui.add_space(5.0);
            if ui.button("Reset to Defaults").clicked() {
                self.config = CaptureConfig::default();
            }
            ui.label(egui::RichText::new("Resets all settings to their default values.").weak());
        });
    }
}

impl CaptureApp {
    fn start_fix(&mut self) {
        if self.config.fix_input_path.is_empty() {
            *self.status.lock().unwrap() =
                CaptureStatus::Error("No input path specified.".to_string());
            return;
        }

        let input_path = self.config.fix_input_path.clone();
        let screen_height = self.config.fix_screen_height;
        let overlap = self.config.overlap;
        let trim_bottom = self.config.trim_bottom;
        let half_seam = self.config.half_seam;
        let output_format = self.config.output_format.clone();
        let fix_output = self.config.fix_output_filename.clone();
        let folder_mode = self.config.fix_folder_mode;

        let status = Arc::clone(&self.status);
        let is_running = Arc::clone(&self.is_running);
        let logs = Arc::clone(&self.logs);

        *is_running.lock().unwrap() = true;
        *status.lock().unwrap() = CaptureStatus::Running("Starting...".to_string());
        logs.lock().unwrap().clear();

        thread::spawn(move || {
            let capture = crate::ScreenCapture::new_with_logs(logs);

            if folder_mode {
                Self::run_fix_folder(
                    &capture,
                    &input_path,
                    &output_format,
                    screen_height,
                    overlap,
                    trim_bottom,
                    half_seam,
                    &status,
                );
            } else {
                Self::run_fix_single(
                    &capture,
                    &input_path,
                    &output_format,
                    &fix_output,
                    screen_height,
                    overlap,
                    trim_bottom,
                    half_seam,
                    &status,
                );
            }

            *is_running.lock().unwrap() = false;
        });
    }

    fn run_fix_single(
        capture: &crate::ScreenCapture,
        input_path: &str,
        output_format: &str,
        fix_output: &str,
        screen_height: u32,
        overlap: u32,
        trim_bottom: u32,
        half_seam: bool,
        status: &Arc<Mutex<CaptureStatus>>,
    ) {
        let output_override = if fix_output.is_empty() {
            None
        } else {
            Some(fix_output)
        };
        match capture.fix_image(
            input_path,
            output_format,
            output_override,
            screen_height,
            overlap,
            trim_bottom,
            half_seam,
        ) {
            Ok(Some(path)) => {
                *status.lock().unwrap() = CaptureStatus::Completed(format!("Saved to: {}", path));
            }
            Ok(None) => {
                *status.lock().unwrap() = CaptureStatus::Completed(
                    "No overlap detected — image looks correct.".to_string(),
                );
            }
            Err(e) => {
                *status.lock().unwrap() = CaptureStatus::Error(format!("{}", e));
            }
        }
    }

    fn run_fix_folder(
        capture: &crate::ScreenCapture,
        folder_path: &str,
        output_format: &str,
        screen_height: u32,
        overlap: u32,
        trim_bottom: u32,
        half_seam: bool,
        status: &Arc<Mutex<CaptureStatus>>,
    ) {
        let result = capture.fix_images_in_folder(
            folder_path,
            output_format,
            screen_height,
            overlap,
            trim_bottom,
            half_seam,
            |cur, total, name| {
                *status.lock().unwrap() =
                    CaptureStatus::Running(format!("[{}/{}] {}", cur, total, name));
            },
        );

        match result {
            Ok((fixed, skipped)) if fixed + skipped == 0 => {
                *status.lock().unwrap() =
                    CaptureStatus::Completed("No images found in folder.".to_string());
            }
            Ok((fixed, skipped)) => {
                *status.lock().unwrap() = CaptureStatus::Completed(format!(
                    "Done: {} fixed, {} skipped (total {})",
                    fixed,
                    skipped,
                    fixed + skipped
                ));
            }
            Err(e) => {
                *status.lock().unwrap() = CaptureStatus::Error(format!("{}", e));
            }
        }
    }

    fn render_fix_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Fix Overlap");
        ui.add_space(10.0);

        // Status display (reuse capture status)
        let current_status = self.status.lock().unwrap().clone();
        match &current_status {
            CaptureStatus::Idle => {
                ui.label("Select a stitched image to fix.");
            }
            CaptureStatus::Running(msg) => {
                let color = egui::Color32::from_rgb(
                    self.config.status_color[0],
                    self.config.status_color[1],
                    self.config.status_color[2],
                );
                ui.colored_label(color, format!("⏳ {}", msg));
                ctx.request_repaint();
            }
            CaptureStatus::Completed(msg) => {
                ui.colored_label(egui::Color32::GREEN, format!("✓ {}", msg));
            }
            CaptureStatus::Error(msg) => {
                ui.colored_label(egui::Color32::RED, format!("✗ {}", msg));
            }
        }

        ui.add_space(10.0);

        let is_running = *self.is_running.lock().unwrap();

        ui.group(|ui| {
            // Mode toggle
            ui.horizontal(|ui| {
                ui.label("Mode:");
                ui.radio_value(&mut self.config.fix_folder_mode, false, "Single file");
                ui.radio_value(
                    &mut self.config.fix_folder_mode,
                    true,
                    "Folder (all images)",
                );
            });

            ui.separator();

            // Input path
            let fix_input_row = ui.horizontal(|ui| {
                ui.label(if self.config.fix_folder_mode {
                    "Folder:"
                } else {
                    "Input image:"
                });
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.fix_input_path)
                        .hint_text(if self.config.fix_folder_mode {
                            "Path to folder containing images"
                        } else {
                            "Path to stitched image"
                        })
                        .desired_width(ui.available_width() - 90.0),
                );
                if ui.button("Browse...").clicked() {
                    if self.config.fix_folder_mode {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            if let Some(s) = path.to_str() {
                                self.config.fix_input_path = s.to_string();
                            }
                        }
                    } else if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp", "tiff"])
                        .pick_file()
                    {
                        if let Some(s) = path.to_str() {
                            self.config.fix_input_path = s.to_string();
                        }
                    }
                }
            });
            let hover_pos = ctx.input(|i| i.pointer.hover_pos());
            if !ctx.input(|i| i.raw.hovered_files.is_empty())
                && hover_pos.map_or(false, |p| fix_input_row.response.rect.contains(p))
            {
                ui.painter().rect_stroke(
                    fix_input_row.response.rect,
                    4.0,
                    egui::Stroke::new(2.0, egui::Color32::LIGHT_BLUE),
                    egui::StrokeKind::Outside,
                );
            }
            let dropped = ctx.input(|i| i.raw.dropped_files.clone());
            if !dropped.is_empty()
                && hover_pos.map_or(false, |p| fix_input_row.response.rect.contains(p))
            {
                if let Some(path) = dropped[0].path.as_ref().and_then(|p| p.to_str()) {
                    self.config.fix_input_path = path.to_string();
                }
            }

            // Output filename (single file mode only)
            if !self.config.fix_folder_mode {
                ui.horizontal(|ui| {
                    ui.label("Output filename:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.config.fix_output_filename)
                            .hint_text(
                                "Leave empty to overwrite original (backup saved as <name>_orig)",
                            )
                            .desired_width(ui.available_width()),
                    );
                });
            } else {
                ui.label(
                    egui::RichText::new(
                        "Output: original overwritten, backup saved as <name>_orig.<format>",
                    )
                    .weak(),
                );
            }

            ui.add_space(5.0);

            // Screen height + overlap
            ui.horizontal(|ui| {
                ui.label("Screen height (px):");
                ui.add(egui::DragValue::new(&mut self.config.fix_screen_height).speed(1.0));

                ui.add_space(20.0);
                ui.label("Overlap (px):");
                ui.add(egui::DragValue::new(&mut self.config.overlap).speed(1.0));
            });

            ui.horizontal(|ui| {
                ui.label("Trim bottom (px):");
                ui.add(egui::DragValue::new(&mut self.config.trim_bottom).speed(1.0));
                ui.label(egui::RichText::new("trim bottom of last frame").weak());
            });

            ui.horizontal(|ui| {
                ui.checkbox(&mut self.config.half_seam, "Legacy seam (overlap/2)");
                ui.label(egui::RichText::new("for old-version captures").weak());
            });
        });

        ui.add_space(10.0);

        let btn_label = if self.config.fix_folder_mode {
            "🔧 Fix All Images"
        } else {
            "🔧 Fix Image"
        };
        if ui
            .add_enabled(!is_running, egui::Button::new(btn_label))
            .clicked()
        {
            self.start_fix();
        }

        ui.add_space(20.0);

        // Log
        ui.group(|ui| {
            ui.label("Log");
            ui.add_space(5.0);
            let logs = self.logs.lock().unwrap();
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if logs.is_empty() {
                        ui.label("No logs yet...");
                    } else {
                        for log in logs.iter() {
                            ui.label(egui::RichText::new(log).font(egui::FontId::monospace(12.0)));
                        }
                    }
                });
        });
    }

    fn load_trim_preview(&mut self, ctx: &egui::Context) {
        if self.trim_input_path.is_empty() {
            self.trim_status = "No input file specified.".to_string();
            return;
        }
        let img = match image::open(&self.trim_input_path) {
            Ok(i) => i.to_rgba8(),
            Err(e) => {
                self.trim_status = format!("Failed to open image: {}", e);
                return;
            }
        };
        let (w, h) = img.dimensions();

        // First-time load: reset state for this image
        if self.trim_img_width != w || self.trim_img_height != h {
            self.trim_img_width = w;
            self.trim_img_height = h;
            self.trim_cut_y = h;
            self.trim_preview_start_y = h.saturating_sub(self.trim_preview_rows);
            self.trim_middle_top = h / 3;
            self.trim_middle_bottom = h * 2 / 3;
        }

        let start_y = self.trim_preview_start_y.min(h.saturating_sub(1));
        let preview_h = (h - start_y).min(self.trim_preview_rows);

        let sub = image::imageops::crop_imm(&img, 0, start_y, w, preview_h).to_image();
        let color_img = egui::ColorImage::from_rgba_unmultiplied(
            [w as usize, preview_h as usize],
            sub.as_raw(),
        );
        self.trim_preview_texture =
            Some(ctx.load_texture("trim_preview", color_img, egui::TextureOptions::NEAREST));
        self.trim_status = format!(
            "Loaded: {}×{}  (showing {}px from Y={})",
            w, h, preview_h, start_y
        );
    }

    fn apply_trim(&mut self) {
        if self.trim_input_path.is_empty() {
            self.trim_status = "No input file specified.".to_string();
            return;
        }
        let img = match image::open(&self.trim_input_path) {
            Ok(i) => i.to_rgba8(),
            Err(e) => {
                self.trim_status = format!("Failed to open image: {}", e);
                return;
            }
        };
        let (w, h) = img.dimensions();
        let cut = self.trim_cut_y.min(h);
        if cut == 0 {
            self.trim_status = "Cut Y is 0 — nothing to save.".to_string();
            return;
        }
        let cropped = image::imageops::crop_imm(&img, 0, 0, w, cut).to_image();
        let out = if self.trim_output_path.is_empty() {
            self.trim_input_path.clone()
        } else {
            self.trim_output_path.clone()
        };
        match cropped.save(&out) {
            Ok(_) => self.trim_status = format!("Saved {}×{} → {}", w, cut, out),
            Err(e) => self.trim_status = format!("Error saving: {}", e),
        }
    }

    fn apply_middle_cut(&mut self) {
        if self.trim_input_path.is_empty() {
            self.trim_status = "No input file specified.".to_string();
            return;
        }
        let img = match image::open(&self.trim_input_path) {
            Ok(i) => i.to_rgba8(),
            Err(e) => {
                self.trim_status = format!("Failed to open image: {}", e);
                return;
            }
        };
        let (w, h) = img.dimensions();
        let top_cut = self.trim_middle_top.min(h);
        let bot_cut = self.trim_middle_bottom.min(h);
        if top_cut >= bot_cut {
            self.trim_status = "Top cut Y must be less than bottom cut Y.".to_string();
            return;
        }
        let top_h = top_cut;
        let bot_h = h - bot_cut;
        if top_h + bot_h == 0 {
            self.trim_status = "Nothing would remain after cut.".to_string();
            return;
        }
        let top_section = image::imageops::crop_imm(&img, 0, 0, w, top_h).to_image();
        let bot_section = image::imageops::crop_imm(&img, 0, bot_cut, w, bot_h).to_image();
        let mut result = image::RgbaImage::new(w, top_h + bot_h);
        image::imageops::replace(&mut result, &top_section, 0i64, 0i64);
        image::imageops::replace(&mut result, &bot_section, 0i64, top_h as i64);
        let out = if self.trim_output_path.is_empty() {
            self.trim_input_path.clone()
        } else {
            self.trim_output_path.clone()
        };
        match result.save(&out) {
            Ok(_) => {
                self.trim_status = format!(
                    "Saved {}×{} (removed {}px from Y={}..{}) → {}",
                    w,
                    top_h + bot_h,
                    bot_cut - top_cut,
                    top_cut,
                    bot_cut,
                    out
                )
            }
            Err(e) => self.trim_status = format!("Error saving: {}", e),
        }
    }

    fn render_trim_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Trim / Cut");
        ui.add_space(10.0);

        ui.group(|ui| {
            // Mode
            ui.horizontal(|ui| {
                ui.label("Mode:");
                ui.radio_value(&mut self.trim_mode, TrimMode::BottomTrim, "Bottom Trim");
                ui.radio_value(&mut self.trim_mode, TrimMode::MiddleCut, "Middle Cut");
            });
            ui.separator();

            // Input file
            let trim_input_row = ui.horizontal(|ui| {
                ui.label("Input:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.trim_input_path)
                        .hint_text("Path to image file")
                        .desired_width(ui.available_width() - 90.0),
                );
                if ui.button("Browse...").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp", "tiff"])
                        .pick_file()
                    {
                        if let Some(s) = path.to_str() {
                            self.trim_input_path = s.to_string();
                            self.trim_preview_texture = None;
                            self.trim_status = String::new();
                            self.trim_img_width = 0;
                            self.trim_img_height = 0;
                        }
                    }
                }
            });
            let hover_pos = ctx.input(|i| i.pointer.hover_pos());
            if !ctx.input(|i| i.raw.hovered_files.is_empty())
                && hover_pos.map_or(false, |p| trim_input_row.response.rect.contains(p))
            {
                ui.painter().rect_stroke(
                    trim_input_row.response.rect,
                    4.0,
                    egui::Stroke::new(2.0, egui::Color32::LIGHT_BLUE),
                    egui::StrokeKind::Outside,
                );
            }
            let dropped = ctx.input(|i| i.raw.dropped_files.clone());
            if !dropped.is_empty()
                && hover_pos.map_or(false, |p| trim_input_row.response.rect.contains(p))
            {
                if let Some(path) = dropped[0].path.as_ref().and_then(|p| p.to_str()) {
                    self.trim_input_path = path.to_string();
                    self.trim_preview_texture = None;
                    self.trim_status = String::new();
                    self.trim_img_width = 0;
                    self.trim_img_height = 0;
                }
            }

            // View controls
            ui.horizontal(|ui| {
                ui.label("Preview rows:");
                ui.add(
                    egui::DragValue::new(&mut self.trim_preview_rows)
                        .range(50..=10000)
                        .speed(10.0),
                );
                ui.add_space(8.0);
                ui.label("Start Y:");
                let max_start = self.trim_img_height.saturating_sub(1);
                ui.add(
                    egui::DragValue::new(&mut self.trim_preview_start_y)
                        .range(0..=max_start)
                        .speed(10.0),
                );
            });

            // Zoom controls
            ui.horizontal(|ui| {
                ui.label("Zoom:");
                ui.add(egui::Slider::new(&mut self.trim_zoom, 0.25..=16.0).logarithmic(true));
                if ui.small_button("1:1").clicked() {
                    self.trim_zoom = 1.0;
                }
                if ui.small_button("Fit").clicked() && self.trim_img_width > 0 {
                    let avail = ui.available_width() - 20.0;
                    self.trim_zoom = (avail / self.trim_img_width as f32).max(0.1);
                }
                ui.label(format!("{:.2}×", self.trim_zoom));
            });

            if ui.button("Load Preview").clicked() {
                self.load_trim_preview(ctx);
            }
        });

        if !self.trim_status.is_empty() {
            ui.add_space(4.0);
            let color = egui::Color32::from_rgb(
                self.config.status_color[0],
                self.config.status_color[1],
                self.config.status_color[2],
            );
            ui.colored_label(color, self.trim_status.clone());
        }

        // ---- Preview area (only when texture loaded) ----
        if self.trim_preview_texture.is_some() {
            let texture_id = self.trim_preview_texture.as_ref().unwrap().id();
            let img_w = self.trim_img_width;
            let img_h = self.trim_img_height;
            let preview_start = self.trim_preview_start_y;
            let preview_rows = self.trim_preview_rows;
            let preview_h = (img_h - preview_start.min(img_h)).min(preview_rows);
            let zoom = self.trim_zoom;

            ui.add_space(6.0);

            // Info + numeric controls
            match self.trim_mode {
                TrimMode::BottomTrim => {
                    ui.label(format!(
                        "{}×{}  |  Cut Y: {}  |  Keep: {}px  |  Remove: {}px",
                        img_w,
                        img_h,
                        self.trim_cut_y,
                        self.trim_cut_y,
                        img_h.saturating_sub(self.trim_cut_y)
                    ));
                    ui.horizontal(|ui| {
                        ui.label("Cut Y:");
                        ui.add(
                            egui::DragValue::new(&mut self.trim_cut_y)
                                .range(0..=img_h)
                                .speed(1.0),
                        );
                    });
                }
                TrimMode::MiddleCut => {
                    ui.label(format!(
                        "{}×{}  |  Remove Y {}..{} ({}px)  |  Result: {}px",
                        img_w,
                        img_h,
                        self.trim_middle_top,
                        self.trim_middle_bottom,
                        self.trim_middle_bottom.saturating_sub(self.trim_middle_top),
                        img_h.saturating_sub(
                            self.trim_middle_bottom.saturating_sub(self.trim_middle_top)
                        )
                    ));
                    ui.horizontal(|ui| {
                        ui.label("Top Y:");
                        let bot = self.trim_middle_bottom;
                        ui.add(
                            egui::DragValue::new(&mut self.trim_middle_top)
                                .range(0..=bot)
                                .speed(1.0),
                        );
                        ui.add_space(10.0);
                        ui.label("Bottom Y:");
                        let top = self.trim_middle_top;
                        ui.add(
                            egui::DragValue::new(&mut self.trim_middle_bottom)
                                .range(top..=img_h)
                                .speed(1.0),
                        );
                    });
                }
            }

            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Drag lines to set cut points. Scroll to pan when zoomed.")
                    .weak()
                    .small(),
            );
            ui.add_space(4.0);

            // Scrollable image view
            let display_w = img_w as f32 * zoom;
            let display_h = preview_h as f32 * zoom;

            egui::ScrollArea::both().max_height(550.0).show(ui, |ui| {
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(display_w, display_h), egui::Sense::drag());

                if ui.is_rect_visible(rect) {
                    // Draw texture
                    ui.painter().image(
                        texture_id,
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );

                    // abs image Y → screen Y
                    let to_screen_y = |abs_y: u32| -> f32 {
                        let in_preview = abs_y.saturating_sub(preview_start).min(preview_h) as f32;
                        rect.top() + in_preview * zoom
                    };

                    match self.trim_mode {
                        TrimMode::BottomTrim => {
                            let line_y = to_screen_y(self.trim_cut_y);
                            ui.painter().hline(
                                rect.x_range(),
                                line_y,
                                egui::Stroke::new(2.0, egui::Color32::RED),
                            );
                            ui.painter().text(
                                egui::pos2(rect.left() + 4.0, line_y - 15.0),
                                egui::Align2::LEFT_TOP,
                                format!("Y={}", self.trim_cut_y),
                                egui::FontId::monospace(11.0),
                                egui::Color32::RED,
                            );
                        }
                        TrimMode::MiddleCut => {
                            let top_y = to_screen_y(self.trim_middle_top);
                            let bot_y = to_screen_y(self.trim_middle_bottom);

                            // Shade removed zone
                            if bot_y > top_y {
                                let shade =
                                    egui::Rect::from_x_y_ranges(rect.x_range(), top_y..=bot_y);
                                ui.painter().rect_filled(
                                    shade,
                                    0.0,
                                    egui::Color32::from_black_alpha(110),
                                );
                            }

                            // Top line (yellow)
                            ui.painter().hline(
                                rect.x_range(),
                                top_y,
                                egui::Stroke::new(2.0, egui::Color32::YELLOW),
                            );
                            ui.painter().text(
                                egui::pos2(rect.left() + 4.0, top_y + 2.0),
                                egui::Align2::LEFT_TOP,
                                format!("Top Y={}", self.trim_middle_top),
                                egui::FontId::monospace(11.0),
                                egui::Color32::YELLOW,
                            );

                            // Bottom line (red)
                            ui.painter().hline(
                                rect.x_range(),
                                bot_y,
                                egui::Stroke::new(2.0, egui::Color32::RED),
                            );
                            ui.painter().text(
                                egui::pos2(rect.left() + 4.0, bot_y - 15.0),
                                egui::Align2::LEFT_TOP,
                                format!("Bot Y={}", self.trim_middle_bottom),
                                egui::FontId::monospace(11.0),
                                egui::Color32::RED,
                            );
                        }
                    }
                }

                // Drag handling: pick nearest line on drag start
                if resp.drag_started() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let rel_y = (pos.y - rect.top()) / zoom;
                        let abs_y = preview_start + rel_y as u32;
                        self.trim_dragging_line = Some(match self.trim_mode {
                            TrimMode::BottomTrim => TrimLine::BottomCut,
                            TrimMode::MiddleCut => {
                                let d_top = (abs_y as f32 - self.trim_middle_top as f32).abs();
                                let d_bot = (abs_y as f32 - self.trim_middle_bottom as f32).abs();
                                if d_top <= d_bot {
                                    TrimLine::MiddleTop
                                } else {
                                    TrimLine::MiddleBottom
                                }
                            }
                        });
                    }
                }

                if resp.dragged() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let rel_y = (pos.y - rect.top()).clamp(0.0, rect.height()) / zoom;
                        let abs_y = (preview_start + rel_y as u32).min(img_h);
                        match self.trim_dragging_line {
                            Some(TrimLine::BottomCut) => self.trim_cut_y = abs_y,
                            Some(TrimLine::MiddleTop) => {
                                self.trim_middle_top = abs_y.min(self.trim_middle_bottom);
                            }
                            Some(TrimLine::MiddleBottom) => {
                                self.trim_middle_bottom = abs_y.max(self.trim_middle_top);
                            }
                            None => {}
                        }
                    }
                }

                if !resp.dragged() && !resp.drag_started() {
                    self.trim_dragging_line = None;
                }
            });

            ui.add_space(8.0);

            // Output path
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Output:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.trim_output_path)
                            .hint_text("Leave empty to overwrite original")
                            .desired_width(ui.available_width() - 90.0),
                    );
                    if ui.button("Browse...").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp", "tiff"])
                            .save_file()
                        {
                            if let Some(s) = path.to_str() {
                                self.trim_output_path = s.to_string();
                            }
                        }
                    }
                });
            });

            ui.add_space(8.0);

            match self.trim_mode {
                TrimMode::BottomTrim => {
                    if ui.button("✂ Apply Bottom Trim").clicked() {
                        self.apply_trim();
                    }
                }
                TrimMode::MiddleCut => {
                    if ui.button("✂ Apply Middle Cut").clicked() {
                        self.apply_middle_cut();
                    }
                }
            }
        }
    }

    fn render_rename_tab(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        ui.heading("Rename: Pad Filenames");
        ui.add_space(10.0);

        ui.label("Pads numeric filenames with leading zeros to match the maximum digit count.");
        ui.label(
            egui::RichText::new("e.g. 1.png → 001.png, 12.png → 012.png (when max is 3 digits)")
                .weak(),
        );
        ui.add_space(10.0);

        // Status display
        let current_status = self.status.lock().unwrap().clone();
        match &current_status {
            CaptureStatus::Idle => {}
            CaptureStatus::Running(msg) => {
                let color = egui::Color32::from_rgb(
                    self.config.status_color[0],
                    self.config.status_color[1],
                    self.config.status_color[2],
                );
                ui.colored_label(color, format!("⏳ {}", msg));
            }
            CaptureStatus::Completed(msg) => {
                ui.colored_label(egui::Color32::GREEN, format!("✓ {}", msg));
            }
            CaptureStatus::Error(msg) => {
                ui.colored_label(egui::Color32::RED, format!("✗ {}", msg));
            }
        }

        ui.add_space(10.0);

        let is_running = *self.is_running.lock().unwrap();

        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label("Folder:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.rename_folder_path)
                        .hint_text("Folder path containing image files")
                        .desired_width(ui.available_width() - 90.0),
                );
                if ui.button("Browse...").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        if let Some(s) = path.to_str() {
                            self.config.rename_folder_path = s.to_string();
                        }
                    }
                }
            });
        });

        ui.add_space(10.0);

        if ui
            .add_enabled(!is_running, egui::Button::new("🔢 Pad Filenames"))
            .clicked()
        {
            self.start_pad_filenames();
        }

        ui.add_space(20.0);

        ui.group(|ui| {
            ui.label("Log");
            ui.add_space(5.0);
            let logs = self.logs.lock().unwrap();
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if logs.is_empty() {
                        ui.label("No logs yet...");
                    } else {
                        for log in logs.iter() {
                            ui.label(egui::RichText::new(log).font(egui::FontId::monospace(12.0)));
                        }
                    }
                });
        });
    }

    fn start_pad_filenames(&mut self) {
        let folder_path = self.config.rename_folder_path.clone();
        if folder_path.is_empty() {
            *self.status.lock().unwrap() =
                CaptureStatus::Error("Please select a folder.".to_string());
            return;
        }

        let status = Arc::clone(&self.status);
        let is_running = Arc::clone(&self.is_running);
        let logs = Arc::clone(&self.logs);

        *is_running.lock().unwrap() = true;
        *status.lock().unwrap() = CaptureStatus::Running("Processing...".to_string());
        logs.lock().unwrap().clear();

        thread::spawn(move || {
            let capture = crate::ScreenCapture::new_with_logs(logs);
            match capture.pad_numeric_filenames(&folder_path, |cur, total, name| {
                *status.lock().unwrap() =
                    CaptureStatus::Running(format!("[{}/{}] {}", cur, total, name));
            }) {
                Ok(0) => {
                    *status.lock().unwrap() =
                        CaptureStatus::Completed("No files to pad.".to_string());
                }
                Ok(n) => {
                    *status.lock().unwrap() =
                        CaptureStatus::Completed(format!("Done: {} files renamed", n));
                }
                Err(e) => {
                    *status.lock().unwrap() = CaptureStatus::Error(format!("{}", e));
                }
            }
            *is_running.lock().unwrap() = false;
        });
    }
}

impl CaptureApp {
    fn generate_cli_command(&self) -> String {
        let mut cmd = vec!["capture".to_string()];

        cmd.push(format!("--output {}", self.config.output_filename));
        cmd.push(format!("--format {}", self.config.output_format));
        cmd.push(format!("--overlap {}", self.config.overlap));
        cmd.push(format!("--delay {}", self.config.delay));
        cmd.push(format!("--key {}", self.config.scroll_key.as_str()));

        if !self.config.max_scrolls.is_empty() {
            cmd.push(format!("--max-scrolls {}", self.config.max_scrolls));
        }
        cmd.push(format!("--scroll-delay {}", self.config.scroll_delay));
        cmd.push(format!(
            "--duplicate-threshold {}",
            self.config.duplicate_threshold
        ));

        if self.config.window_only {
            cmd.push("--window-only".to_string());
        }

        if self.config.use_preset && !self.config.selected_preset.is_empty() {
            cmd.push(format!("--crop-preset {}", self.config.selected_preset));
        } else if self.config.crop_enabled {
            cmd.push(format!(
                "--crop \"{},{},{},{}\"",
                self.config.crop_x,
                self.config.crop_y,
                self.config.crop_width,
                self.config.crop_height
            ));
        }

        cmd.join(" ")
    }
}

pub fn run_gui() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([gui_const::WINDOW_WIDTH, gui_const::WINDOW_HEIGHT])
            .with_min_inner_size([gui_const::MIN_WINDOW_WIDTH, gui_const::MIN_WINDOW_HEIGHT]),
        ..Default::default()
    };

    eframe::run_native(
        "Screen Scroll Capture",
        options,
        Box::new(|cc| Ok(Box::new(CaptureApp::new(cc)))),
    )
}
