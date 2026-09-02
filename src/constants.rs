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
    pub const SCROLL_WAIT_MIN: u64 = 0;
    pub const SCROLL_WAIT_MAX: u64 = 2000;
    pub const POST_CAPTURE_MIN: u64 = 0;
    pub const POST_CAPTURE_MAX: u64 = 2000;
    pub const POLL_MIN: u64 = 0;
    pub const POLL_MAX: u64 = 2000;
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
    pub const OUTPUT_PATH: &str = "00.png";

    /// Linux scrolls a slightly different distance for the same keypress, so it
    /// needs its own stitching overlap.
    #[cfg(target_os = "linux")]
    pub const OVERLAP: u32 = 118;
    #[cfg(not(target_os = "linux"))]
    pub const OVERLAP: u32 = 125;

    pub const DELAY: u64 = 3;
    pub const SCROLL_DELAY: u64 = 200;
    pub const MAX_SCROLLS_DEFAULT: &str = "";
    pub const DUPLICATE_THRESHOLD: usize = 2;

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

/// Every delay in one scroll-and-capture cycle, in the order they happen.
///
/// One iteration is: press the scroll key, wait `scroll_wait_ms` for the content
/// to load, wait a further `scroll_delay_ms`, take the screenshot, compare it
/// against the previous one, wait `post_capture_ms`, then wait up to `poll_ms`
/// for a quit keypress before scrolling again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureTimings {
    /// After the scroll keypress, waiting for the page to render new content.
    pub scroll_wait_ms: u64,
    /// After `scroll_wait_ms`, immediately before the screenshot is taken.
    pub scroll_delay_ms: u64,
    /// After the screenshot has been compared against the previous frame.
    pub post_capture_ms: u64,
    /// How long each iteration waits for a quit keypress. In GUI mode nothing
    /// reads the keyboard, so this is a plain sleep.
    pub poll_ms: u64,
}

impl Default for CaptureTimings {
    fn default() -> Self {
        Self {
            scroll_wait_ms: timing::SCROLL_WAIT_MS,
            scroll_delay_ms: defaults::SCROLL_DELAY,
            post_capture_ms: timing::SMALL_DELAY_MS,
            poll_ms: timing::KEYBOARD_POLL_MS,
        }
    }
}
