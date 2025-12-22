use eframe::egui;
use std::sync::{Arc, Mutex};
use std::thread;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq)]
enum CaptureMode {
    Screenshot,
    Video,
}

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
    capture_mode: CaptureMode,

    // Video mode settings
    video_duration: u64,
    video_fps: u32,

    // Screenshot mode settings
    max_scrolls: String,  // Empty string means unlimited
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
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            output_path: "scroll_capture.png".to_string(),
            overlap: 125,
            delay: 3,
            scroll_key: ScrollKey::Space,
            capture_mode: CaptureMode::Screenshot,
            video_duration: 10,
            video_fps: 2,
            max_scrolls: String::new(),
            scroll_delay: 200,
            window_only: false,
            crop_enabled: false,
            use_preset: false,
            selected_preset: String::new(),
            crop_x: 0,
            crop_y: 0,
            crop_width: 1920,
            crop_height: 1080,
        }
    }
}

#[derive(Clone)]
enum CaptureStatus {
    Idle,
    Running(String),  // Status message
    Completed(String), // Result message
    Error(String),
}

pub struct CaptureApp {
    config: CaptureConfig,
    status: Arc<Mutex<CaptureStatus>>,
    is_running: Arc<Mutex<bool>>,
    should_stop: Arc<Mutex<bool>>,
    presets: HashMap<String, String>,
    preset_names: Vec<String>,
}

impl Default for CaptureApp {
    fn default() -> Self {
        let presets = crate::presets::get_all_presets().unwrap_or_default();
        let mut preset_names: Vec<String> = presets.keys().cloned().collect();
        preset_names.sort();

        Self {
            config: CaptureConfig::default(),
            status: Arc::new(Mutex::new(CaptureStatus::Idle)),
            is_running: Arc::new(Mutex::new(false)),
            should_stop: Arc::new(Mutex::new(false)),
            presets,
            preset_names,
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
        let mut font_paths = vec![
            "assets/NotoSansKR-Regular.ttf".to_string(),
            "NotoSansKR-Regular.ttf".to_string(),
        ];

        // Add user config directory path if available
        if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
            font_paths.push(format!("{}/.config/capture/NotoSansKR-Regular.ttf", home));
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
            println!("    - assets/NotoSansKR-Regular.ttf");
            println!("    - NotoSansKR-Regular.ttf (current directory)");
            if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
                println!("    - {}/.config/capture/NotoSansKR-Regular.ttf", home);
            }
        }

        ctx.set_fonts(fonts);
    }

    fn start_capture(&mut self) {
        let config = self.config.clone();
        let status = Arc::clone(&self.status);
        let is_running = Arc::clone(&self.is_running);
        let should_stop = Arc::clone(&self.should_stop);

        // Set running state and reset stop flag
        *is_running.lock().unwrap() = true;
        *should_stop.lock().unwrap() = false;
        *status.lock().unwrap() = CaptureStatus::Running("Initializing capture...".to_string());

        // Spawn capture thread
        thread::spawn(move || {
            let result = Self::run_capture(config, status.clone(), should_stop.clone());

            *is_running.lock().unwrap() = false;

            match result {
                Ok(output_path) => {
                    *status.lock().unwrap() = CaptureStatus::Completed(
                        format!("Successfully saved to: {}", output_path)
                    );
                }
                Err(e) => {
                    *status.lock().unwrap() = CaptureStatus::Error(
                        format!("Capture failed: {}", e)
                    );
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
    ) -> anyhow::Result<String> {
        use crate::ScreenCapture;

        *status.lock().unwrap() = CaptureStatus::Running(
            format!("Starting in {} seconds...", config.delay)
        );

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
            Some(format!("{},{},{},{}",
                config.crop_x, config.crop_y,
                config.crop_width, config.crop_height))
        } else {
            None
        };

        let result_image = match config.capture_mode {
            CaptureMode::Video => {
                *status.lock().unwrap() = CaptureStatus::Running(
                    "Recording video...".to_string()
                );

                capture.capture_with_video_with_stop(
                    config.overlap,
                    config.video_duration,
                    config.delay,
                    config.scroll_key.as_str(),
                    config.video_fps,
                    config.window_only,
                    crop_option,
                    should_stop.clone(),
                )?
            }
            CaptureMode::Screenshot => {
                *status.lock().unwrap() = CaptureStatus::Running(
                    "Capturing screenshots...".to_string()
                );

                let max_scrolls = if config.max_scrolls.is_empty() {
                    None
                } else {
                    config.max_scrolls.parse().ok()
                };

                capture.capture_with_scroll_with_stop(
                    config.overlap,
                    max_scrolls,
                    config.delay,
                    config.scroll_key.as_str(),
                    config.window_only,
                    crop_option,
                    config.scroll_delay,
                    should_stop.clone(),
                )?
            }
        };

        *status.lock().unwrap() = CaptureStatus::Running(
            "Saving image...".to_string()
        );

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
                    ui.colored_label(egui::Color32::BLUE, format!("⏳ {}", msg));
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
            ui.separator();
            ui.add_space(10.0);

            // Configuration UI
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Capture Mode:");
                    ui.radio_value(&mut self.config.capture_mode, CaptureMode::Video, "Video");
                    ui.radio_value(&mut self.config.capture_mode, CaptureMode::Screenshot, "Screenshot");
                });

                ui.add_space(10.0);

                // Common settings
                ui.group(|ui| {
                    ui.label("Common Settings");

                    ui.horizontal(|ui| {
                        ui.label("Output file:");
                        ui.text_edit_singleline(&mut self.config.output_path);
                    });

                    ui.horizontal(|ui| {
                        ui.label("Overlap pixels:");
                        ui.add(egui::Slider::new(&mut self.config.overlap, 50..=500));
                    });

                    ui.horizontal(|ui| {
                        ui.label("Delay before start (seconds):");
                        ui.add(egui::Slider::new(&mut self.config.delay, 0..=10));
                    });

                    ui.horizontal(|ui| {
                        ui.label("Scroll key:");
                        ui.radio_value(&mut self.config.scroll_key, ScrollKey::Space, "Space");
                        ui.radio_value(&mut self.config.scroll_key, ScrollKey::Down, "Down Arrow");
                        ui.radio_value(&mut self.config.scroll_key, ScrollKey::PageDown, "Page Down");
                    });
                });

                ui.add_space(10.0);

                // Mode-specific settings
                match self.config.capture_mode {
                    CaptureMode::Video => {
                        ui.group(|ui| {
                            ui.label("Video Mode Settings");

                            ui.horizontal(|ui| {
                                ui.label("Recording duration (seconds):");
                                ui.add(egui::Slider::new(&mut self.config.video_duration, 5..=60));
                            });

                            ui.horizontal(|ui| {
                                ui.label("Frames per second:");
                                ui.add(egui::Slider::new(&mut self.config.video_fps, 1..=10));
                            });
                        });
                    }
                    CaptureMode::Screenshot => {
                        ui.group(|ui| {
                            ui.label("Screenshot Mode Settings");

                            ui.horizontal(|ui| {
                                ui.label("Max scrolls (leave empty for unlimited):");
                                ui.text_edit_singleline(&mut self.config.max_scrolls);
                            });

                            ui.horizontal(|ui| {
                                ui.label("Scroll delay (milliseconds):");
                                ui.add(egui::Slider::new(&mut self.config.scroll_delay, 100..=1000));
                            });
                        });
                    }
                }

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
                                .selected_text(
                                    if self.config.selected_preset.is_empty() {
                                        "Select preset..."
                                    } else {
                                        &self.config.selected_preset
                                    }
                                )
                                .show_ui(ui, |ui| {
                                    for preset_name in &self.preset_names {
                                        let label_text = if let Some(value) = self.presets.get(preset_name) {
                                            format!("{}: {}", preset_name, value)
                                        } else {
                                            preset_name.clone()
                                        };

                                        if ui.selectable_value(
                                            &mut self.config.selected_preset,
                                            preset_name.clone(),
                                            label_text
                                        ).clicked() {
                                            // Apply preset values to crop fields
                                            if let Some(crop_str) = self.presets.get(preset_name) {
                                                if let Some((x, y, w, h)) = crate::presets::parse_crop_region(crop_str) {
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

                // Action buttons
                let is_running = *self.is_running.lock().unwrap();

                ui.horizontal(|ui| {
                    if ui.add_enabled(!is_running, egui::Button::new("▶ Start Capture"))
                        .clicked()
                    {
                        self.start_capture();
                    }

                    if ui.add_enabled(is_running, egui::Button::new("⏹ Stop Capture"))
                        .clicked()
                    {
                        self.stop_capture();
                    }

                    if ui.button("📋 Copy Command").clicked() {
                        let cmd = self.generate_cli_command();
                        ui.ctx().copy_text(cmd);
                    }
                });

                ui.add_space(10.0);

                // Show equivalent CLI command
                ui.group(|ui| {
                    ui.label("Equivalent CLI command:");
                    ui.add_space(5.0);
                    let cmd = self.generate_cli_command();
                    ui.code(&cmd);
                });
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

        match self.config.capture_mode {
            CaptureMode::Video => {
                cmd.push("--video".to_string());
                cmd.push(format!("--duration {}", self.config.video_duration));
                cmd.push(format!("--fps {}", self.config.video_fps));
            }
            CaptureMode::Screenshot => {
                if !self.config.max_scrolls.is_empty() {
                    cmd.push(format!("--max-scrolls {}", self.config.max_scrolls));
                }
                cmd.push(format!("--scroll-delay {}", self.config.scroll_delay));
            }
        }

        if self.config.window_only {
            cmd.push("--window-only".to_string());
        }

        if self.config.use_preset && !self.config.selected_preset.is_empty() {
            cmd.push(format!("--crop-preset {}", self.config.selected_preset));
        } else if self.config.crop_enabled {
            cmd.push(format!("--crop \"{},{},{},{}\"",
                self.config.crop_x, self.config.crop_y,
                self.config.crop_width, self.config.crop_height));
        }

        cmd.join(" ")
    }
}

pub fn run_gui() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([600.0, 800.0])
            .with_min_inner_size([500.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Screen Scroll Capture",
        options,
        Box::new(|cc| Ok(Box::new(CaptureApp::new(cc)))),
    )
}
