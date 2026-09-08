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
    pub const SCROLL_DELAY_MIN: u64 = 0;
    pub const SCROLL_DELAY_MAX: u64 = 3000;
    pub const POST_CAPTURE_MIN: u64 = 0;
    pub const POST_CAPTURE_MAX: u64 = 3000;
    pub const DUPLICATE_THRESHOLD_MIN: usize = 1;
    pub const DUPLICATE_THRESHOLD_MAX: usize = 10;

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
    pub const OUTPUT_PATH: &str = "00.webp";

    /// Linux scrolls a slightly different distance for the same keypress, so it
    /// needs its own stitching overlap.
    #[cfg(target_os = "linux")]
    pub const OVERLAP: u32 = 118;
    #[cfg(not(target_os = "linux"))]
    pub const OVERLAP: u32 = 125;

    pub const DELAY: u64 = 3;
    pub const SCROLL_DELAY: u64 = 1200;
    pub const POST_CAPTURE_DELAY: u64 = 0;
    pub const MAX_SCROLLS_DEFAULT: &str = "";
    pub const DUPLICATE_THRESHOLD: usize = 2;

    pub const CROP_X: i32 = 0;
    pub const CROP_Y: i32 = 0;
    pub const CROP_WIDTH: i32 = 1920;
    pub const CROP_HEIGHT: i32 = 1080;
}

// Capture timing constants
pub mod timing {
    pub const MOUSE_POSITION_POLL_MS: u64 = 100;
    pub const ZOOM_ENABLE_DELAY_MS: u64 = 500;
}

/// Every delay in one scroll-and-capture cycle, in the order they happen.
///
/// One iteration is: press the scroll key, wait `scroll_delay_ms` for the new
/// content to load, take the screenshot, compare it against the previous one,
/// then wait `post_capture_ms` before scrolling again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureTimings {
    /// Between the scroll keypress and the screenshot, giving the page time to
    /// render the new content.
    pub scroll_delay_ms: u64,
    /// Between the comparison and the next scroll keypress. In CLI mode this
    /// doubles as the window for a Q keypress to stop the capture; in GUI mode
    /// nothing reads the keyboard, so it is a plain sleep.
    pub post_capture_ms: u64,
}

impl Default for CaptureTimings {
    fn default() -> Self {
        Self {
            scroll_delay_ms: defaults::SCROLL_DELAY,
            post_capture_ms: defaults::POST_CAPTURE_DELAY,
        }
    }
}
