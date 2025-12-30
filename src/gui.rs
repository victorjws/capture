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
    output_path: String,
    overlap: u32,
    delay: u64,
    scroll_key: ScrollKey,

    // Screenshot mode settings
    max_scrolls: String, // Empty string means unlimited
    scroll_delay: u64,

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
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            output_path: defaults::OUTPUT_PATH.to_string(),
            overlap: defaults::OVERLAP,
            delay: defaults::DELAY,
            scroll_key: ScrollKey::Space,
            max_scrolls: defaults::MAX_SCROLLS_DEFAULT.to_string(),
            scroll_delay: defaults::SCROLL_DELAY,
            window_only: false,
            crop_enabled: false,
            use_preset: false,
            selected_preset: String::new(),
            crop_x: defaults::CROP_X,
            crop_y: defaults::CROP_Y,
            crop_width: defaults::CROP_WIDTH,
            crop_height: defaults::CROP_HEIGHT,
            font_path: String::new(),
            status_color: [255, 255, 0], // Yellow by default
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
enum Tab {
    Capture,
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
        }
    }
}

impl CaptureApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Load fonts to support Unicode (including Korean, Japanese, Chinese, etc.)
        Self::setup_fonts(&cc.egui_ctx);
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

                font_loaded = true;
                println!("Loaded font from: {}", path);
                break;
            }
        }

        if !font_loaded {
            println!("No custom font found. Using default font.");
            println!("For Unicode support (Korean, Japanese, Chinese, etc.):");
            println!("  Place NotoSansKR-Regular.ttf in one of these locations:");
            for path in gui_const::DEFAULT_FONT_PATHS {
                println!("    - {}", path);
            }
            if let Some(config_path) = gui_const::get_config_font_path() {
                println!("    - {}", config_path);
            }
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

    fn log(logs: &Arc<Mutex<Vec<String>>>, message: String) {
        let timestamp = chrono::Local::now().format("%H:%M:%S%.6f");
        let log_entry = format!("[{}] {}", timestamp, message);
        logs.lock().unwrap().push(log_entry);
    }

    fn run_capture(
        config: CaptureConfig,
        status: Arc<Mutex<CaptureStatus>>,
        should_stop: Arc<Mutex<bool>>,
        logs: Arc<Mutex<Vec<String>>>,
    ) -> anyhow::Result<String> {
        use crate::ScreenCapture;

        // Countdown display
        if config.delay > 0 {
            Self::log(
                &logs,
                format!("Starting capture in {} seconds...", config.delay),
            );

            for remaining in (1..=config.delay).rev() {
                *status.lock().unwrap() = CaptureStatus::Running(format!(
                    "Starting in {} second{}...",
                    remaining,
                    if remaining > 1 { "s" } else { "" }
                ));

                // Check if stop was requested during countdown
                if *should_stop.lock().unwrap() {
                    return Err(anyhow::anyhow!("Capture cancelled during countdown"));
                }

                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }

        let capture = ScreenCapture::new();

        // Prepare crop option
        let crop_option = if config.use_preset && !config.selected_preset.is_empty() {
            // Use preset value directly
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

        Self::log(&logs, "Starting screenshot mode...".to_string());
        *status.lock().unwrap() = CaptureStatus::Running("Capturing screenshots...".to_string());

        let max_scrolls = if config.max_scrolls.is_empty() {
            None
        } else {
            config.max_scrolls.parse().ok()
        };

        Self::log(
            &logs,
            format!(
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
            0, // Delay already handled in GUI countdown
            config.scroll_key.as_str(),
            config.window_only,
            crop_option,
            config.scroll_delay,
            should_stop.clone(),
            logs.clone(),
        )?;

        Self::log(&logs, "Saving image...".to_string());

        *status.lock().unwrap() = CaptureStatus::Running("Saving image...".to_string());

        result_image.save(&config.output_path)?;
        Ok(config.output_path.clone())
    }
}

impl eframe::App for CaptureApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Screen Scroll Capture Tool");
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

            // Tab buttons
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.current_tab, Tab::Capture, "📷 Capture");
                ui.selectable_value(&mut self.current_tab, Tab::Settings, "⚙ Settings");
            });

            ui.separator();
            ui.add_space(10.0);

            // Tab content
            egui::ScrollArea::vertical().show(ui, |ui| match self.current_tab {
                Tab::Capture => self.render_capture_tab(ui, ctx),
                Tab::Settings => self.render_settings_tab(ui, ctx),
            });
        });
    }
}

impl CaptureApp {
    fn render_capture_tab(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        // Configuration UI
        ui.heading("Screenshot Mode");
        ui.add_space(10.0);

        // Action buttons at the top
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
                ui.label("Output file:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.output_path)
                        .desired_width(ui.available_width()),
                );
            });

            ui.horizontal(|ui| {
                ui.label("Overlap pixels:");
                ui.add(egui::Slider::new(
                    &mut self.config.overlap,
                    gui_const::OVERLAP_MIN..=gui_const::OVERLAP_MAX,
                ));
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
                    .desired_width(f32::INFINITY)
            );
        });

        ui.add_space(20.0);

        // Capture Log (at the bottom)
        ui.group(|ui| {
            ui.label("Capture Log");
            ui.add_space(5.0);

            let logs = self.logs.lock().unwrap();
            let scroll_height = if logs.is_empty() {
                gui_const::LOG_HEIGHT_EMPTY
            } else {
                gui_const::LOG_HEIGHT_WITH_CONTENT
            };

            egui::ScrollArea::vertical()
                .max_height(scroll_height)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if logs.is_empty() {
                        ui.label("No logs yet...");
                    } else {
                        for log in logs.iter() {
                            ui.label(log);
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

            ui.horizontal(|ui| {
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
    }
}

impl CaptureApp {
    fn generate_cli_command(&self) -> String {
        let mut cmd = vec!["capture".to_string()];

        cmd.push(format!("--output {}", self.config.output_path));
        cmd.push(format!("--overlap {}", self.config.overlap));
        cmd.push(format!("--delay {}", self.config.delay));
        cmd.push(format!("--key {}", self.config.scroll_key.as_str()));

        if !self.config.max_scrolls.is_empty() {
            cmd.push(format!("--max-scrolls {}", self.config.max_scrolls));
        }
        cmd.push(format!("--scroll-delay {}", self.config.scroll_delay));

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
