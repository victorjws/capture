pub mod gui;
pub mod presets;
pub mod constants;

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, poll, read};
use enigo::{Enigo, Key, Keyboard, Settings};
use image::{ImageBuffer, Rgba, RgbaImage};
use std::thread;
use std::time::Duration;
use constants::{timing, similarity};

#[cfg(target_os = "macos")]
use core_graphics::display::CGMainDisplayID;
#[cfg(target_os = "macos")]
use core_graphics::image::CGImageRef;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{HWND, POINT, RECT};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};

pub struct ScreenCapture {
    #[cfg(target_os = "macos")]
    display_id: u32,
    #[cfg(target_os = "windows")]
    _phantom: (),
}

impl ScreenCapture {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            display_id: unsafe { CGMainDisplayID() },
            #[cfg(target_os = "windows")]
            _phantom: (),
        }
    }

    pub fn parse_crop_region(crop_str: &str) -> Option<(i32, i32, i32, i32)> {
        let parts: Vec<i32> = crop_str
            .split(|c| c == ',' || c == ':' || c == ' ')
            .filter_map(|s| s.trim().parse().ok())
            .collect();

        if parts.len() == 4 && parts[2] > 0 && parts[3] > 0 {
            Some((parts[0], parts[1], parts[2], parts[3]))
        } else {
            None
        }
    }

    #[cfg(target_os = "macos")]
    fn get_mouse_position() -> Result<(i32, i32)> {
        let script = r#"
tell application "System Events"
    set mousePos to position of mouse
    return (item 1 of mousePos) & "," & (item 2 of mousePos)
end tell
"#;

        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()?;

        if output.status.success() {
            let result = String::from_utf8_lossy(&output.stdout);
            let coords: Vec<i32> = result
                .trim()
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();

            if coords.len() == 2 {
                return Ok((coords[0], coords[1]));
            }
        }

        Err(anyhow::anyhow!("Failed to get mouse position"))
    }

    #[cfg(target_os = "windows")]
    fn get_mouse_position() -> Result<(i32, i32)> {
        unsafe {
            let mut point = POINT { x: 0, y: 0 };
            if GetCursorPos(&mut point).is_ok() {
                Ok((point.x, point.y))
            } else {
                Err(anyhow::anyhow!("Failed to get cursor position"))
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn enable_zoom() -> Result<()> {
        let script = r#"
tell application "System Events"
    key code 28 using {command down, option down}
end tell
"#;
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output();
        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn enable_zoom() -> Result<()> {
        // Launch Windows Magnifier
        let _ = std::process::Command::new("magnify.exe").spawn();
        Ok(())
    }

    fn show_live_coordinates() -> Result<(i32, i32)> {
        use std::io::{self, Write};

        println!("   Live coordinates (move mouse, press ENTER to select):");
        println!("   ┌─────────────────────────────────────────┐");

        // Show live coordinates until Enter is pressed
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let mut input = String::new();
            let _ = io::stdin().read_line(&mut input);
            let _ = tx.send(());
        });

        loop {
            if let Ok((x, y)) = Self::get_mouse_position() {
                print!("\r   │ Current position: ({:4}, {:4})          │", x, y);
                io::stdout().flush()?;
            }

            // Check if Enter was pressed
            if rx.try_recv().is_ok() {
                let (x, y) = Self::get_mouse_position()?;
                println!("\r   └─────────────────────────────────────────┘");
                return Ok((x, y));
            }

            thread::sleep(Duration::from_millis(timing::MOUSE_POSITION_POLL_MS));
        }
    }

    pub fn select_region_interactive() -> Result<(i32, i32, i32, i32)> {
        use std::io::{self, Write};

        println!("\nINTERACTIVE REGION SELECTION");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!();

        #[cfg(target_os = "macos")]
        {
            println!("TIP: Press Option+Command+8 to toggle macOS Zoom (magnifier)");
            println!("     Option+Command+= to zoom in, Option+Command+- to zoom out");
        }

        #[cfg(target_os = "windows")]
        {
            println!("TIP: Press Win+Plus to open Windows Magnifier");
            println!("     Win+Plus/Minus to zoom in/out");
        }

        println!();

        // Offer to enable zoom automatically
        print!("Do you want to enable Magnifier now? (y/N): ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if input.trim().to_lowercase() == "y" {
            println!("Enabling Magnifier...");
            Self::enable_zoom()?;
            thread::sleep(Duration::from_millis(timing::ZOOM_ENABLE_DELAY_MS));
        }

        println!();
        println!("Step 1/2: Position mouse at TOP-LEFT corner");

        let (x1, y1) = Self::show_live_coordinates()?;
        println!("Top-left corner: ({}, {})", x1, y1);
        println!();

        println!("Step 2/2: Position mouse at BOTTOM-RIGHT corner");

        let (x2, y2) = Self::show_live_coordinates()?;
        println!("Bottom-right corner: ({}, {})", x2, y2);
        println!();

        // Calculate region
        let x = x1.min(x2);
        let y = y1.min(y2);
        let width = (x2 - x1).abs();
        let height = (y2 - y1).abs();

        if width <= 0 || height <= 0 {
            return Err(anyhow::anyhow!(
                "Invalid region: width and height must be positive"
            ));
        }

        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("Region selected:");
        println!("   Position: ({}, {})", x, y);
        println!("   Size: {}x{}", width, height);
        println!();
        println!("Use this command:");
        println!("   --crop \"{},{},{},{}\"", x, y, width, height);
        println!();

        Ok((x, y, width, height))
    }

    #[cfg(target_os = "macos")]
    fn get_focused_window_bounds(&self) -> Result<Option<(i32, i32, i32, i32)>> {
        let script = r#"
tell application "System Events"
    set frontApp to first application process whose frontmost is true
    set frontWindow to front window of frontApp
    set windowPosition to position of frontWindow
    set windowSize to size of frontWindow
    return (item 1 of windowPosition) & "," & (item 2 of windowPosition) & "," & (item 1 of windowSize) & "," & (item 2 of windowSize)
end tell
"#;

        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let result = String::from_utf8_lossy(&output.stdout);
                let parts: Vec<i32> = result
                    .trim()
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();

                if parts.len() == 4 {
                    return Ok(Some((parts[0], parts[1], parts[2], parts[3])));
                }
            }
        }

        Ok(None)
    }

    #[cfg(target_os = "windows")]
    fn get_focused_window_bounds(&self) -> Result<Option<(i32, i32, i32, i32)>> {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0 == std::ptr::null_mut() {
                return Ok(None);
            }

            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_ok() {
                let x = rect.left;
                let y = rect.top;
                let width = rect.right - rect.left;
                let height = rect.bottom - rect.top;
                return Ok(Some((x, y, width, height)));
            }
        }
        Ok(None)
    }

    fn capture_screen(&self, crop_region: Option<(i32, i32, i32, i32)>) -> Result<RgbaImage> {
        // Try screenshots crate first (more compatible)
        let screen = screenshots::Screen::all()
            .map_err(|e| anyhow::anyhow!("Failed to get screens: {}", e))?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No screen found"))?;

        let captured_image = screen
            .capture()
            .map_err(|e| anyhow::anyhow!("Failed to capture screen: {}", e))?;

        // screenshots crate uses image 0.24, we use 0.25
        // Convert pixel data manually to avoid version conflict
        let width = captured_image.width();
        let height = captured_image.height();

        let mut rgba_image = RgbaImage::new(width, height);
        for (x, y, pixel) in captured_image.enumerate_pixels() {
            // Manually copy RGBA values
            let rgba = Rgba([pixel[0], pixel[1], pixel[2], pixel[3]]);
            rgba_image.put_pixel(x, y, rgba);
        }

        // Apply crop if specified
        if let Some((crop_x, crop_y, crop_w, crop_h)) = crop_region {
            // Ensure crop region is within bounds
            let crop_x = crop_x.max(0) as u32;
            let crop_y = crop_y.max(0) as u32;
            let crop_w = crop_w.max(0) as u32;
            let crop_h = crop_h.max(0) as u32;

            if crop_x + crop_w <= width && crop_y + crop_h <= height {
                let mut cropped = RgbaImage::new(crop_w, crop_h);
                for y in 0..crop_h {
                    for x in 0..crop_w {
                        let pixel = rgba_image.get_pixel(crop_x + x, crop_y + y);
                        cropped.put_pixel(x, y, *pixel);
                    }
                }
                return Ok(cropped);
            } else {
                println!("Crop region out of bounds, using full screen");
            }
        }

        Ok(rgba_image)
    }

    #[cfg(target_os = "macos")]
    fn cgimage_to_rgba(cg_image: &CGImageRef) -> RgbaImage {
        let width = cg_image.width() as u32;
        let height = cg_image.height() as u32;
        let bytes_per_row = cg_image.bytes_per_row();
        let data = cg_image.data();
        let bytes = data.bytes();

        let mut img_buffer = ImageBuffer::new(width, height);

        for y in 0..height {
            for x in 0..width {
                let offset = (y as usize * bytes_per_row) + (x as usize * 4);
                if offset + 3 < bytes.len() {
                    let pixel = Rgba([
                        bytes[offset + 2],
                        bytes[offset + 1],
                        bytes[offset],
                        bytes[offset + 3],
                    ]);
                    img_buffer.put_pixel(x, y, pixel);
                }
            }
        }

        img_buffer
    }

    fn scroll_down(&self, key_type: &str) -> Result<()> {
        let mut enigo = Enigo::new(&Settings::default())?;

        // Select key based on user input
        let key = match key_type.to_lowercase().as_str() {
            "down" => Key::DownArrow,
            "pagedown" => Key::PageDown,
            _ => Key::Space, // default to Space
        };

        enigo.key(key, enigo::Direction::Click)?;
        thread::sleep(Duration::from_millis(timing::SCROLL_WAIT_MS)); // Wait for content to load
        Ok(())
    }

    fn images_are_identical(&self, img1: &RgbaImage, img2: &RgbaImage) -> bool {
        // Check if images have the same dimensions
        if img1.width() != img2.width() || img1.height() != img2.height() {
            println!("    [DEBUG] Size mismatch: {}x{} vs {}x{}",
                     img1.width(), img1.height(), img2.width(), img2.height());
            return false;
        }

        let width = img1.width();
        let height = img1.height();
        let total_pixels = (width * height) as usize;

        println!("    [DEBUG] Comparing entire images: {}x{} ({} pixels)",
                 width, height, total_pixels);

        // Compare every pixel
        let mut diff_count = 0;
        for y in 0..height {
            for x in 0..width {
                if img1.get_pixel(x, y) != img2.get_pixel(x, y) {
                    diff_count += 1;
                    // Early exit if we find any difference
                    if diff_count > 0 {
                        let diff_percentage = (diff_count as f32 / total_pixels as f32) * 100.0;
                        println!("    [DEBUG] Found {} different pixels ({:.6}%)",
                                 diff_count, diff_percentage);
                        return false;
                    }
                }
            }
        }

        println!("    [DEBUG] Images are completely identical");
        true
    }

    fn images_are_similar(
        &self,
        img1: &RgbaImage,
        img2: &RgbaImage,
        overlap_height: u32,
    ) -> (bool, f32) {
        if img1.width() != img2.width() || img1.height() != img2.height() {
            println!("    [DEBUG] Size mismatch: {}x{} vs {}x{}",
                     img1.width(), img1.height(), img2.width(), img2.height());
            return (false, 100.0);
        }

        let height = img1.height();
        let width = img1.width();

        if overlap_height >= height {
            println!("    [DEBUG] Overlap too large: {} >= {}", overlap_height, height);
            return (false, 100.0);
        }

        let start_y1 = height - overlap_height;
        println!("    [DEBUG] Comparing bottom {}px of img1 (y={}-{}) with top {}px of img2 (y=0-{})",
                 overlap_height, start_y1, height, overlap_height, overlap_height);

        let mut diff_count = 0;
        let total_pixels = (overlap_height * width) as usize;
        let threshold = (total_pixels as f32 * similarity::DIFF_THRESHOLD_PERCENTAGE) as usize;
        println!("    [DEBUG] Total pixels to compare: {}, threshold: {}", total_pixels, threshold);

        for y in 0..overlap_height {
            for x in 0..width {
                let pixel1 = img1.get_pixel(x, start_y1 + y);
                let pixel2 = img2.get_pixel(x, y);

                if pixel1 != pixel2 {
                    diff_count += 1;
                    if diff_count > threshold {
                        let diff_percentage = (diff_count as f32 / total_pixels as f32) * 100.0;
                        println!("    [DEBUG] Early exit: {} different pixels found ({}%)",
                                 diff_count, diff_percentage);
                        return (false, diff_percentage);
                    }
                }
            }
        }

        let diff_percentage = (diff_count as f32 / total_pixels as f32) * 100.0;
        println!("    [DEBUG] Full scan complete: {} different pixels ({}%)",
                 diff_count, diff_percentage);
        (true, diff_percentage)
    }

    fn stitch_images(&self, images: Vec<RgbaImage>, overlap: u32) -> RgbaImage {
        if images.is_empty() {
            return ImageBuffer::new(1, 1);
        }

        let width = images[0].width();
        let single_height = images[0].height();
        let total_height = single_height + (images.len() as u32 - 1) * (single_height - overlap);

        let mut result = ImageBuffer::new(width, total_height);

        for (i, img) in images.iter().enumerate() {
            let y_offset = i as u32 * (single_height - overlap);

            for y in 0..single_height {
                for x in 0..width {
                    let target_y = y_offset + y;
                    if target_y < total_height {
                        if i > 0 && y < overlap {
                            // Use middle of overlap as boundary
                            if y >= overlap / 2 {
                                // Bottom half of overlap: use current image
                                let pixel = img.get_pixel(x, y);
                                result.put_pixel(x, target_y, *pixel);
                            }
                            // Top half: skip (previous image already there)
                        } else {
                            // Copy pixel normally (outside overlap region)
                            let pixel = img.get_pixel(x, y);
                            result.put_pixel(x, target_y, *pixel);
                        }
                    }
                }
            }
        }

        result
    }

    fn log_msg(logs: &Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>, msg: &str) {
        if let Some(logs) = logs {
            let timestamp = chrono::Local::now().format("%H:%M:%S");
            logs.lock().unwrap().push(format!("[{}] {}", timestamp, msg));
        }
        println!("{}", msg);
    }

    pub fn capture_with_scroll(
        &self,
        overlap: u32,
        max_scrolls: Option<usize>,
        delay: u64,
        key_type: &str,
        window_only: bool,
        crop: Option<String>,
        scroll_delay_ms: u64,
    ) -> Result<RgbaImage> {
        self.capture_with_scroll_impl(overlap, max_scrolls, delay, key_type, window_only, crop, scroll_delay_ms, false, None, None)
    }

    pub fn capture_with_scroll_no_input(
        &self,
        overlap: u32,
        max_scrolls: Option<usize>,
        delay: u64,
        key_type: &str,
        window_only: bool,
        crop: Option<String>,
        scroll_delay_ms: u64,
    ) -> Result<RgbaImage> {
        self.capture_with_scroll_impl(overlap, max_scrolls, delay, key_type, window_only, crop, scroll_delay_ms, true, None, None)
    }

    pub fn capture_with_scroll_with_stop(
        &self,
        overlap: u32,
        max_scrolls: Option<usize>,
        delay: u64,
        key_type: &str,
        window_only: bool,
        crop: Option<String>,
        scroll_delay_ms: u64,
        stop_flag: std::sync::Arc<std::sync::Mutex<bool>>,
        logs: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> Result<RgbaImage> {
        self.capture_with_scroll_impl(overlap, max_scrolls, delay, key_type, window_only, crop, scroll_delay_ms, true, Some(stop_flag), Some(logs))
    }

    fn capture_with_scroll_impl(
        &self,
        overlap: u32,
        max_scrolls: Option<usize>,
        delay: u64,
        key_type: &str,
        window_only: bool,
        crop: Option<String>,
        scroll_delay_ms: u64,
        skip_input: bool,
        stop_flag: Option<std::sync::Arc<std::sync::Mutex<bool>>>,
        logs: Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
    ) -> Result<RgbaImage> {
        Self::log_msg(&logs, &format!("Starting scroll capture in {} seconds...", delay));
        Self::log_msg(&logs, "Please focus on the window you want to capture!");
        Self::log_msg(&logs, "Make sure to grant Accessibility permission in System Settings > Privacy & Security");
        Self::log_msg(&logs, &format!("The program will press {} key once per capture", key_type.to_uppercase()));
        Self::log_msg(&logs, &format!("Scroll delay: {}ms", scroll_delay_ms));
        if let Some(max) = max_scrolls {
            Self::log_msg(&logs, &format!("Max scrolls: {}", max));
        } else {
            Self::log_msg(&logs, "Max scrolls: unlimited (press Q to stop)");
        }
        thread::sleep(Duration::from_secs(delay));

        // Determine crop region (manual crop takes precedence)
        let crop_region: Option<(i32, i32, i32, i32)> = if let Some(crop_str) = crop {
            // Manual crop region
            if let Some((x, y, w, h)) = Self::parse_crop_region(&crop_str) {
                Self::log_msg(&logs, &format!("Manual crop: {}x{} at ({}, {})", w, h, x, y));
                Some((x, y, w, h))
            } else {
                Self::log_msg(&logs, "Invalid crop format, capturing full screen");
                Self::log_msg(&logs, "   Use format: 'x,y,width,height' (e.g., '100,50,1920,1080')");
                None
            }
        } else if window_only {
            // Auto-detect focused window
            if let Some((x, y, w, h)) = self.get_focused_window_bounds()? {
                Self::log_msg(&logs, &format!("Focused window: {}x{} at ({}, {})", w, h, x, y));
                Some((x, y, w, h))
            } else {
                Self::log_msg(&logs, "Could not detect focused window, capturing full screen");
                None
            }
        } else {
            None
        };

        let mut images = Vec::new();
        let first_capture = self.capture_screen(crop_region)?;
        Self::log_msg(&logs, &format!("Captured screen 1 ({}x{})",
            first_capture.width(), first_capture.height()));
        images.push(first_capture.clone());

        let mut previous_capture = first_capture;
        let mut scroll_count = 0;

        loop {
            // Check stop flag
            if let Some(ref flag) = stop_flag {
                if *flag.lock().unwrap() {
                    Self::log_msg(&logs, "Stopped by user");
                    break;
                }
            }

            // Check if we've reached max_scrolls limit
            if let Some(max) = max_scrolls {
                if scroll_count >= max {
                    Self::log_msg(&logs, &format!("Reached maximum scroll limit ({})", max));
                    break;
                }
                Self::log_msg(&logs, &format!("[{}/{}] Pressing {}...", scroll_count + 1, max, key_type.to_uppercase()));
            } else {
                Self::log_msg(&logs, &format!("[{}] Pressing {}...", scroll_count + 1, key_type.to_uppercase()));
            }

            self.scroll_down(key_type)?;

            // Wait for content to settle after scrolling
            thread::sleep(Duration::from_millis(scroll_delay_ms));

            let current_capture = self.capture_screen(crop_region)?;
            Self::log_msg(&logs, &format!("Captured screen {} ({}x{})",
                scroll_count + 2, current_capture.width(), current_capture.height()));

            // Check if entire images are identical (no scrolling happened)
            let is_identical = self.images_are_identical(&previous_capture, &current_capture);

            if is_identical {
                Self::log_msg(&logs, "Reached end of scrollable content (images are completely identical)");
                break;
            }

            images.push(current_capture.clone());
            previous_capture = current_capture;
            scroll_count += 1;

            // Small delay before next scroll
            thread::sleep(Duration::from_millis(timing::SMALL_DELAY_MS));

            // Check for user input to stop early (only in terminal mode)
            if !skip_input {
                if poll(Duration::from_millis(timing::KEYBOARD_POLL_MS))? {
                    match read()? {
                        Event::Key(KeyEvent {
                            code: KeyCode::Char('q') | KeyCode::Char('Q'),
                            ..
                        }) => {
                            Self::log_msg(&logs, "Stopped by user");
                            break;
                        }
                        _ => {} // Ignore other keys
                    }
                }
            } else {
                // In GUI mode, just sleep
                thread::sleep(Duration::from_millis(timing::KEYBOARD_POLL_MS));
            }
        }

        // Clear any remaining events before finishing (only in terminal mode)
        if !skip_input {
            while poll(Duration::from_millis(0))? {
                let _ = read();
            }
        }

        Self::log_msg(&logs, &format!("Stitching {} images...", images.len()));
        let result = self.stitch_images(images, overlap);
        Self::log_msg(&logs, &format!("Done! Final image: {}x{}",
            result.width(), result.height()));

        Ok(result)
    }
}
