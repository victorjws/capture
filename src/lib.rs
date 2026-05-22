pub mod constants;
pub mod gui;
pub mod presets;

use anyhow::Result;
use constants::timing;
use crossterm::event::{Event, KeyCode, KeyEvent, poll, read};
use enigo::{Enigo, Key, Keyboard, Settings};
use image::{ImageBuffer, Rgba, RgbaImage};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{POINT, RECT};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};

pub const SUPPORTED_FORMATS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "tiff", "tif", "webp"];

/// Validates that the format is supported
pub fn validate_format(format: &str) -> Result<()> {
    let format_lower = format.to_lowercase();
    let format_clean = format_lower.trim_start_matches('.');

    if SUPPORTED_FORMATS.contains(&format_clean) {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Unsupported file format: '{}'\nSupported formats: {}",
            format,
            SUPPORTED_FORMATS.join(", ")
        ))
    }
}

/// Builds the full output path from filename and format
pub fn build_output_path(filename: &str, format: &str) -> String {
    let format_clean = format.trim_start_matches('.').to_lowercase();
    format!("{}.{}", filename, format_clean)
}

/// Validates that the output path is usable before capture starts
pub fn validate_output_path(output_path: &str) -> Result<()> {
    let path = std::path::Path::new(output_path);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if stem.is_empty() {
        return Err(anyhow::anyhow!("Output filename cannot be empty"));
    }
    let parent = path.parent().unwrap_or(std::path::Path::new("."));
    let parent = if parent == std::path::Path::new("") {
        std::path::Path::new(".")
    } else {
        parent
    };
    if !parent.exists() || !parent.is_dir() {
        return Err(anyhow::anyhow!(
            "Output directory does not exist: {}",
            parent.display()
        ));
    }
    Ok(())
}

pub struct ScreenCapture {
    logs: Option<Arc<Mutex<Vec<String>>>>,
}

impl ScreenCapture {
    pub fn new() -> Self {
        Self { logs: None }
    }

    pub fn new_with_logs(logs: Arc<Mutex<Vec<String>>>) -> Self {
        Self { logs: Some(logs) }
    }

    pub fn log(&self, level: log::Level, msg: &str) {
        if log::log_enabled!(level) {
            if let Some(logs) = &self.logs {
                let timestamp = chrono::Local::now().format("%H:%M:%S%.6f");
                logs.lock()
                    .unwrap()
                    .push(format!("[{}] {}", timestamp, msg));
            }
            log::log!(level, "{}", msg);
        }
    }

    pub fn fix_image(
        &self,
        input_path: &str,
        output_format: &str,
        output_path_override: Option<&str>,
        screen_height: u32,
        overlap: u32,
        trim_bottom: u32,
        half_seam: bool,
    ) -> Result<Option<String>> {
        let img = image::open(input_path)
            .map_err(|e| anyhow::anyhow!("Failed to open image: {}", e))?
            .to_rgba8();

        self.log(
            log::Level::Info,
            &format!("Loaded: {} ({}x{})", input_path, img.width(), img.height()),
        );

        let fixed_opt =
            self.fix_stitched_overlap(&img, screen_height, overlap, trim_bottom, half_seam);

        let needs_save = fixed_opt.is_some() || trim_bottom > 0;
        if !needs_save {
            self.log(
                log::Level::Info,
                "No overlap detected — image looks correct.",
            );
            return Ok(None);
        }

        let mut result = fixed_opt.unwrap_or(img);
        if trim_bottom > 0 && result.height() > trim_bottom {
            let h = result.height() - trim_bottom;
            result = image::imageops::crop_imm(&result, 0, 0, result.width(), h).to_image();
            self.log(
                log::Level::Info,
                &format!(
                    "Trimmed {} pixels from bottom → new height: {}px",
                    trim_bottom, h
                ),
            );
        }

        let output_path = match output_path_override {
            Some(p) if !p.is_empty() => build_output_path(p, output_format),
            _ => {
                let stem = std::path::Path::new(input_path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("fixed");
                let dir = std::path::Path::new(input_path)
                    .parent()
                    .and_then(|p| p.to_str())
                    .unwrap_or(".");
                let orig_backup = format!(
                    "{}/{}",
                    dir,
                    build_output_path(&format!("{}_orig", stem), output_format)
                );
                std::fs::rename(input_path, &orig_backup)
                    .map_err(|e| anyhow::anyhow!("Failed to rename original: {}", e))?;
                input_path.to_string()
            }
        };
        result
            .save(&output_path)
            .map_err(|e| anyhow::anyhow!("Failed to save: {}", e))?;
        self.log(log::Level::Info, &format!("Saved: {}", output_path));
        Ok(Some(output_path))
    }

    pub fn fix_images_in_folder(
        &self,
        folder_path: &str,
        output_format: &str,
        screen_height: u32,
        overlap: u32,
        trim_bottom: u32,
        half_seam: bool,
        on_progress: impl Fn(usize, usize, &str),
    ) -> Result<(usize, usize)> {
        const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "tiff", "tif"];

        let dir = std::fs::read_dir(folder_path)
            .map_err(|e| anyhow::anyhow!("Cannot read folder: {}", e))?;

        let mut files: Vec<std::path::PathBuf> = dir
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .map(|e| IMAGE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
                        .unwrap_or(false)
                    && !p
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.ends_with("_orig"))
                        .unwrap_or(false)
            })
            .collect();

        files.sort();

        if files.is_empty() {
            return Ok((0, 0));
        }

        self.log(
            log::Level::Info,
            &format!("Found {} image(s) to process", files.len()),
        );

        let mut fixed_count = 0;
        let mut skipped_count = 0;
        let total = files.len();

        for (i, path) in files.iter().enumerate() {
            let file_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            on_progress(i + 1, total, &file_name);

            let path_str = path.to_string_lossy().into_owned();
            self.log(
                log::Level::Info,
                &format!("[{}/{}] {}", i + 1, total, path_str),
            );

            match self.fix_image(
                &path_str,
                output_format,
                None,
                screen_height,
                overlap,
                trim_bottom,
                half_seam,
            ) {
                Ok(Some(_)) => fixed_count += 1,
                Ok(None) => {
                    self.log(log::Level::Info, "  → No overlap detected, skipped");
                    skipped_count += 1;
                }
                Err(e) => {
                    self.log(log::Level::Warn, &format!("  → Error: {}", e));
                    skipped_count += 1;
                }
            }
        }

        Ok((fixed_count, skipped_count))
    }

    pub fn pad_numeric_filenames(
        &self,
        folder_path: &str,
        on_progress: impl Fn(usize, usize, &str),
    ) -> Result<usize> {
        const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "tiff", "tif"];

        let dir = std::fs::read_dir(folder_path)
            .map_err(|e| anyhow::anyhow!("Cannot read folder: {}", e))?;

        let mut files: Vec<std::path::PathBuf> = dir
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .map(|e| IMAGE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
                        .unwrap_or(false)
                    && p.file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.chars().all(|c| c.is_ascii_digit()))
                        .unwrap_or(false)
            })
            .collect();

        if files.is_empty() {
            self.log(log::Level::Info, "No numeric-named image files found.");
            return Ok(0);
        }

        let max_value: u64 = files
            .iter()
            .filter_map(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse::<u64>().ok())
            })
            .max()
            .unwrap_or(0);

        let max_digits = max_value.to_string().len();
        if max_digits < 2 {
            self.log(
                log::Level::Info,
                "All filenames already have 1 digit — nothing to pad.",
            );
            return Ok(0);
        }

        files.sort();
        let total = files.len();
        let mut renamed_count = 0;

        for (i, path) in files.iter().enumerate() {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            on_progress(i + 1, total, &format!("{}.{}", stem, ext));

            if stem.len() < max_digits {
                let new_stem = format!("{:0>width$}", stem, width = max_digits);
                let new_name = format!("{}.{}", new_stem, ext);
                let new_path = path.with_file_name(&new_name);
                self.log(
                    log::Level::Info,
                    &format!("[{}/{}] {} → {}", i + 1, total, stem, new_stem),
                );
                std::fs::rename(path, &new_path)
                    .map_err(|e| anyhow::anyhow!("Failed to rename {}: {}", stem, e))?;
                renamed_count += 1;
            } else {
                self.log(
                    log::Level::Info,
                    &format!(
                        "[{}/{}] {} (already {} digits, skip)",
                        i + 1,
                        total,
                        stem,
                        max_digits
                    ),
                );
            }
        }

        Ok(renamed_count)
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

        thread::spawn(move || {
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
                self.log(
                    log::Level::Warn,
                    "Crop region out of bounds, using full screen",
                );
            }
        }

        Ok(rgba_image)
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
        if img1.dimensions() != img2.dimensions() {
            self.log(
                log::Level::Debug,
                &format!(
                    "Size mismatch: {}x{} vs {}x{}",
                    img1.width(),
                    img1.height(),
                    img2.width(),
                    img2.height()
                ),
            );
            return false;
        }
        img1.as_raw() == img2.as_raw()
    }

    fn detect_actual_overlap(img_prev: &RgbaImage, img_last: &RgbaImage, min_overlap: u32) -> u32 {
        const MIN_MATCH_RATIO: f64 = 0.9;
        // A row counts as matching if this fraction of its pixels are identical.
        // Allows scrollbar or minor dynamic content to differ without breaking detection.
        const MIN_PIXEL_MATCH_RATIO: f64 = 0.98;

        let height = img_prev.height();
        let width = img_prev.width();
        let stride = (width * 4) as usize;
        let search_limit = height.saturating_sub(min_overlap);

        let mut best_ratio = -1.0_f64;
        let mut best_k = search_limit;

        for k in 0..=search_limit {
            let region_height = height - k;
            let matching = (0..region_height)
                .filter(|&j| {
                    let a = &img_prev.as_raw()
                        [(k + j) as usize * stride..(k + j + 1) as usize * stride];
                    let b = &img_last.as_raw()[j as usize * stride..(j + 1) as usize * stride];
                    if a == b {
                        return true;
                    }
                    let matching_pixels = a
                        .chunks_exact(4)
                        .zip(b.chunks_exact(4))
                        .filter(|(pa, pb)| pa == pb)
                        .count();
                    matching_pixels as f64 / width as f64 >= MIN_PIXEL_MATCH_RATIO
                })
                .count();

            let ratio = matching as f64 / region_height as f64;

            if ratio == 1.0 {
                return height - k;
            }

            if ratio > best_ratio {
                best_ratio = ratio;
                best_k = k;
            }
        }

        if best_ratio >= MIN_MATCH_RATIO {
            height - best_k
        } else {
            min_overlap
        }
    }

    // Detects the number of duplicate rows around the wrong 50/50 seam in a stitched image.
    // split_point is the seam position (last_start + overlap/2). Returns the smallest k≥1
    // such that the k rows immediately before the seam equal the k rows immediately after.
    fn detect_skip_amount(img: &RgbaImage, split_point: u32, max_k: u32) -> u32 {
        let width = img.width();
        let stride = (width * 4) as usize;
        let total_h = img.height();
        for k in 1..=max_k {
            if split_point < k || split_point + k > total_h {
                break;
            }
            if (0..k).all(|j| {
                let a = &img.as_raw()[(split_point - k + j) as usize * stride
                    ..(split_point - k + j + 1) as usize * stride];
                let b = &img.as_raw()
                    [(split_point + j) as usize * stride..(split_point + j + 1) as usize * stride];
                a == b
            }) {
                return k;
            }
        }
        0
    }

    fn stitch_images(&self, images: Vec<RgbaImage>, overlaps: &[u32]) -> RgbaImage {
        if images.is_empty() {
            return ImageBuffer::new(1, 1);
        }

        let width = images[0].width();
        let single_height = images[0].height();
        let stride = (width * 4) as usize;
        let step_sum: u32 = overlaps.iter().map(|&o| single_height - o).sum();
        let total_height = single_height + step_sum;

        let mut result = ImageBuffer::new(width, total_height);

        let mut y_offset = 0u32;
        for (i, img) in images.iter().enumerate() {
            let overlap = if i > 0 { overlaps[i - 1] } else { 0 };
            // Skip the overlapping rows entirely — the previous frame already covers them.
            let skip_rows = if i > 0 { overlap } else { 0 };

            for y in skip_rows..single_height {
                let target_y = (y_offset + y) as usize;
                let src = &img.as_raw()[(y as usize * stride)..(y as usize + 1) * stride];
                let dst = &mut result.as_mut()[target_y * stride..(target_y + 1) * stride];
                dst.copy_from_slice(src);
            }

            if i < overlaps.len() {
                y_offset += single_height - overlaps[i];
            }
        }

        result
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
        duplicate_threshold: usize,
        trim_bottom: u32,
    ) -> Result<RgbaImage> {
        self.capture_with_scroll_impl(
            overlap,
            max_scrolls,
            delay,
            key_type,
            window_only,
            crop,
            scroll_delay_ms,
            false,
            None,
            duplicate_threshold,
            trim_bottom,
            &|_| {},
        )
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
        duplicate_threshold: usize,
        trim_bottom: u32,
    ) -> Result<RgbaImage> {
        self.capture_with_scroll_impl(
            overlap,
            max_scrolls,
            delay,
            key_type,
            window_only,
            crop,
            scroll_delay_ms,
            true,
            None,
            duplicate_threshold,
            trim_bottom,
            &|_| {},
        )
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
        stop_flag: Arc<Mutex<bool>>,
        duplicate_threshold: usize,
        trim_bottom: u32,
        on_phase: impl Fn(&str),
    ) -> Result<RgbaImage> {
        self.capture_with_scroll_impl(
            overlap,
            max_scrolls,
            delay,
            key_type,
            window_only,
            crop,
            scroll_delay_ms,
            true,
            Some(stop_flag),
            duplicate_threshold,
            trim_bottom,
            &on_phase,
        )
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
        stop_flag: Option<Arc<Mutex<bool>>>,
        duplicate_threshold: usize,
        trim_bottom: u32,
        on_phase: &dyn Fn(&str),
    ) -> Result<RgbaImage> {
        self.log(
            log::Level::Info,
            &format!("Starting scroll capture in {} seconds...", delay),
        );
        self.log(
            log::Level::Info,
            "Please focus on the window you want to capture!",
        );
        self.log(
            log::Level::Info,
            "Make sure to grant Accessibility permission in System Settings > Privacy & Security",
        );
        self.log(
            log::Level::Info,
            &format!(
                "The program will press {} key once per capture",
                key_type.to_uppercase()
            ),
        );
        self.log(
            log::Level::Info,
            &format!("Scroll delay: {}ms", scroll_delay_ms),
        );
        if let Some(max) = max_scrolls {
            self.log(log::Level::Info, &format!("Max scrolls: {}", max));
        } else {
            self.log(log::Level::Info, "Max scrolls: unlimited (press Q to stop)");
        }
        thread::sleep(Duration::from_secs(delay));

        let crop_region: Option<(i32, i32, i32, i32)> = if let Some(crop_str) = crop {
            if let Some((x, y, w, h)) = presets::parse_crop_region(&crop_str) {
                self.log(
                    log::Level::Info,
                    &format!("Manual crop: {}x{} at ({}, {})", w, h, x, y),
                );
                Some((x, y, w, h))
            } else {
                self.log(
                    log::Level::Warn,
                    "Invalid crop format, capturing full screen",
                );
                self.log(
                    log::Level::Warn,
                    "   Use format: 'x,y,width,height' (e.g., '100,50,1920,1080')",
                );
                None
            }
        } else if window_only {
            if let Some((x, y, w, h)) = self.get_focused_window_bounds()? {
                self.log(
                    log::Level::Info,
                    &format!("Focused window: {}x{} at ({}, {})", w, h, x, y),
                );
                Some((x, y, w, h))
            } else {
                self.log(
                    log::Level::Warn,
                    "Could not detect focused window, capturing full screen",
                );
                None
            }
        } else {
            None
        };

        let mut images = Vec::new();
        let first_capture = self.capture_screen(crop_region)?;
        self.log(
            log::Level::Info,
            &format!(
                "Captured screen 1 ({}x{})",
                first_capture.width(),
                first_capture.height()
            ),
        );
        images.push(first_capture);

        let mut scroll_count = 0;
        let mut previous_capture = images[0].clone();
        let mut consecutive_identical: usize = 0;

        loop {
            if let Some(ref flag) = stop_flag {
                if *flag.lock().unwrap() {
                    self.log(log::Level::Info, "Stopped by user");
                    break;
                }
            }

            if let Some(max) = max_scrolls {
                if scroll_count >= max {
                    self.log(
                        log::Level::Info,
                        &format!("Reached maximum scroll limit ({})", max),
                    );
                    break;
                }
                self.log(
                    log::Level::Info,
                    &format!(
                        "[{}/{}] Pressing {}...",
                        scroll_count + 1,
                        max,
                        key_type.to_uppercase()
                    ),
                );
            } else {
                self.log(
                    log::Level::Info,
                    &format!(
                        "[{}] Pressing {}...",
                        scroll_count + 1,
                        key_type.to_uppercase()
                    ),
                );
            }

            self.scroll_down(key_type)?;
            thread::sleep(Duration::from_millis(scroll_delay_ms));

            let current_capture = self.capture_screen(crop_region)?;
            self.log(
                log::Level::Info,
                &format!(
                    "Captured screen {} ({}x{})",
                    scroll_count + 2,
                    current_capture.width(),
                    current_capture.height()
                ),
            );

            let is_identical = self.images_are_identical(&previous_capture, &current_capture);
            if is_identical {
                consecutive_identical += 1;
                if consecutive_identical >= duplicate_threshold {
                    self.log(
                        log::Level::Info,
                        &format!(
                            "Reached end of scrollable content ({} consecutive identical frames)",
                            consecutive_identical
                        ),
                    );
                    break;
                }
                previous_capture = current_capture;
            } else {
                consecutive_identical = 0;
                images.push(current_capture.clone());
                previous_capture = current_capture;
            }
            scroll_count += 1;

            thread::sleep(Duration::from_millis(timing::SMALL_DELAY_MS));

            if !skip_input {
                if poll(Duration::from_millis(timing::KEYBOARD_POLL_MS))? {
                    match read()? {
                        Event::Key(KeyEvent {
                            code: KeyCode::Char('q') | KeyCode::Char('Q'),
                            ..
                        }) => {
                            self.log(log::Level::Info, "Stopped by user");
                            break;
                        }
                        _ => {}
                    }
                }
            } else {
                thread::sleep(Duration::from_millis(timing::KEYBOARD_POLL_MS));
            }
        }

        if !skip_input {
            while poll(Duration::from_millis(0))? {
                let _ = read();
            }
        }

        let mut overlaps = vec![overlap; images.len().saturating_sub(1)];
        if let Some(last_i) = overlaps.len().checked_sub(1) {
            on_phase("Detecting last frame overlap...");
            let actual = Self::detect_actual_overlap(&images[last_i], &images[last_i + 1], overlap);
            overlaps[last_i] = actual;
            self.log(
                log::Level::Info,
                &format!(
                    "Frame {}->{} overlap: {}px (configured: {}px)",
                    last_i + 1,
                    last_i + 2,
                    actual,
                    overlap
                ),
            );
        }

        on_phase(&format!("Stitching... ({} frames)", images.len()));
        self.log(
            log::Level::Info,
            &format!("Stitching {} images...", images.len()),
        );
        let screen_height = images.first().map(|img| img.height()).unwrap_or(0);
        let image_count = images.len();
        let mut result = self.stitch_images(images, &overlaps);
        self.log(
            log::Level::Info,
            &format!("Done! Final image: {}x{}", result.width(), result.height()),
        );

        if image_count >= 2 {
            if let Some(fixed) =
                self.fix_stitched_overlap(&result, screen_height, overlap, 0, false)
            {
                result = fixed;
            }
        }

        if trim_bottom > 0 && result.height() > trim_bottom {
            let h = result.height() - trim_bottom;
            result = image::imageops::crop_imm(&result, 0, 0, result.width(), h).to_image();
            self.log(
                log::Level::Info,
                &format!(
                    "Trimmed {} pixels from bottom → new height: {}px",
                    trim_bottom, h
                ),
            );
        }

        Ok(result)
    }

    pub fn fix_stitched_overlap(
        &self,
        img: &RgbaImage,
        screen_height: u32,
        overlap: u32,
        trim_bottom: u32,
        half_seam: bool,
    ) -> Option<RgbaImage> {
        let total_h = img.height();
        let width = img.width();
        let effective_screen_height = screen_height.saturating_sub(trim_bottom);

        if total_h + overlap < 2 * effective_screen_height || effective_screen_height <= overlap {
            return None;
        }

        let split_point = if half_seam {
            total_h - effective_screen_height + overlap / 2
        } else {
            total_h - effective_screen_height + overlap
        };
        let max_k = effective_screen_height - overlap;

        let skip_amount = Self::detect_skip_amount(img, split_point, max_k);
        if skip_amount == 0 {
            self.log(
                log::Level::Info,
                "No overlap detected — image looks correct.",
            );
            return None;
        }

        let cut_row = if half_seam {
            let actual_overlap = overlap + skip_amount;
            let y_offset_correct = total_h - screen_height - skip_amount;
            y_offset_correct + actual_overlap / 2
        } else {
            split_point
        };
        let src_resume = cut_row + skip_amount;

        if src_resume > total_h {
            return None;
        }

        self.log(
            log::Level::Info,
            &format!(
                "Overlap detected: {}px extra beyond configured {}px — removing {} rows at row {}",
                skip_amount, overlap, skip_amount, cut_row
            ),
        );

        let new_total_h = total_h - skip_amount;
        let mut result: RgbaImage = ImageBuffer::new(width, new_total_h);
        let stride = (width * 4) as usize;

        let head = cut_row as usize * stride;
        result.as_mut()[..head].copy_from_slice(&img.as_raw()[..head]);

        let src_start = src_resume as usize * stride;
        let dst_start = (src_resume - skip_amount) as usize * stride;
        let tail = (total_h - src_resume) as usize * stride;
        result.as_mut()[dst_start..dst_start + tail]
            .copy_from_slice(&img.as_raw()[src_start..src_start + tail]);

        Some(result)
    }
}
