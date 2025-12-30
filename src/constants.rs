// GUI-related constants
pub mod gui {
    // Window settings
    pub const WINDOW_WIDTH: f32 = 600.0;
    pub const WINDOW_HEIGHT: f32 = 800.0;
    pub const MIN_WINDOW_WIDTH: f32 = 500.0;
    pub const MIN_WINDOW_HEIGHT: f32 = 600.0;

    // Log display
    pub const LOG_HEIGHT_EMPTY: f32 = 50.0;
    pub const LOG_HEIGHT_WITH_CONTENT: f32 = 150.0;

    // Slider ranges
    pub const OVERLAP_MIN: u32 = 50;
    pub const OVERLAP_MAX: u32 = 500;
    pub const DELAY_MIN: u64 = 0;
    pub const DELAY_MAX: u64 = 10;
    pub const SCROLL_DELAY_MIN: u64 = 100;
    pub const SCROLL_DELAY_MAX: u64 = 1000;

    // Default font paths
    pub const DEFAULT_FONT_PATHS: &[&str] =
        &["assets/NotoSansKR-Regular.ttf", "NotoSansKR-Regular.ttf"];

    pub fn get_config_font_path() -> Option<String> {
        if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
            Some(format!("{}/.config/capture/NotoSansKR-Regular.ttf", home))
        } else {
            None
        }
    }
}

// Capture configuration defaults
pub mod defaults {
    pub const OUTPUT_PATH: &str = "00.png";
    pub const OVERLAP: u32 = 125;
    pub const DELAY: u64 = 3;
    pub const SCROLL_DELAY: u64 = 200;
    pub const MAX_SCROLLS_DEFAULT: &str = "";

    pub const CROP_X: i32 = 0;
    pub const CROP_Y: i32 = 0;
    pub const CROP_WIDTH: i32 = 1920;
    pub const CROP_HEIGHT: i32 = 1080;
}

// Capture timing constants
pub mod timing {
    pub const SCROLL_WAIT_MS: u64 = 500;
    pub const SMALL_DELAY_MS: u64 = 300;
    pub const MOUSE_POSITION_POLL_MS: u64 = 100;
    pub const ZOOM_ENABLE_DELAY_MS: u64 = 500;
    pub const KEYBOARD_POLL_MS: u64 = 500;
}
