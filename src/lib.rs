pub mod constants;
pub mod gui;
pub mod presets;

use anyhow::Result;
use constants::CaptureTimings;
use constants::timing;
use crossterm::event::{Event, KeyCode, KeyEvent, poll, read};
use enigo::{Enigo, Key, Keyboard, Settings};
use image::{ImageBuffer, RgbaImage};
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

/// WebP stores width and height as 14-bit values, so neither may exceed this.
/// Scroll captures routinely run tens of thousands of pixels tall, so an
/// oversized capture is split across numbered files instead of failing to encode.
pub const WEBP_MAX_DIMENSION: u32 = 16383;

/// Splits `height` into `parts` runs that differ by at most one row, so a tall
/// capture does not end with a sliver of a final file.
fn split_heights(height: u32, parts: u32) -> Vec<u32> {
    let base = height / parts;
    let remainder = height % parts;
    (0..parts)
        .map(|i| if i < remainder { base + 1 } else { base })
        .collect()
}

/// Saves `img` to `output_path`, splitting it into numbered parts when the
/// target format cannot hold the whole image. Returns every path written, in
/// top-to-bottom order.
///
/// Only WebP has a size limit small enough to matter here, and its encoder in
/// the `image` crate is lossless-only, so splitting never costs any quality.
pub fn save_image(img: &RgbaImage, output_path: &str) -> Result<Vec<String>> {
    let path = std::path::Path::new(output_path);
    let is_webp = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("webp"))
        .unwrap_or(false);

    // Splitting is vertical only, so an over-wide image has no fallback.
    if is_webp && img.width() > WEBP_MAX_DIMENSION {
        return Err(anyhow::anyhow!(
            "Image is {}px wide, but WebP supports at most {}px. Use a narrower crop or save as PNG.",
            img.width(),
            WEBP_MAX_DIMENSION
        ));
    }

    if !is_webp || img.height() <= WEBP_MAX_DIMENSION {
        img.save(output_path)
            .map_err(|e| anyhow::anyhow!("Failed to save {}: {}", output_path, e))?;
        return Ok(vec![output_path.to_string()]);
    }

    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("Output filename cannot be empty"))?;
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("webp");

    let parts = img.height().div_ceil(WEBP_MAX_DIMENSION);
    let digits = parts.to_string().len();

    let mut written = Vec::with_capacity(parts as usize);
    let mut y = 0;
    for (i, part_height) in split_heights(img.height(), parts).into_iter().enumerate() {
        let name = format!("{}_{:0>width$}.{}", stem, i + 1, ext, width = digits);
        let part_path = path.with_file_name(&name);
        let part = image::imageops::crop_imm(img, 0, y, img.width(), part_height).to_image();
        part.save(&part_path)
            .map_err(|e| anyhow::anyhow!("Failed to save {}: {}", part_path.display(), e))?;
        written.push(part_path.to_string_lossy().into_owned());
        y += part_height;
    }

    Ok(written)
}

/// Extensions the folder-wide tools accept as input.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "tiff", "tif"];

/// Lists the image files directly inside `folder_path`, sorted by name.
/// Subfolders are left alone, so a folder-wide pass never reaches further than
/// the folder the user picked.
fn list_image_files(folder_path: &str) -> Result<Vec<std::path::PathBuf>> {
    let dir =
        std::fs::read_dir(folder_path).map_err(|e| anyhow::anyhow!("Cannot read folder: {}", e))?;

    let mut files: Vec<std::path::PathBuf> = dir
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| IMAGE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
                    .unwrap_or(false)
        })
        .collect();

    files.sort();
    Ok(files)
}

/// True for the `<name>_orig.<ext>` backups `fix_image` leaves behind, which a
/// folder-wide pass must not treat as a capture of its own.
fn is_orig_backup(path: &std::path::Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.ends_with("_orig"))
        .unwrap_or(false)
}

pub struct ScreenCapture {
    logs: Option<Arc<Mutex<Vec<String>>>>,
    timings: CaptureTimings,
}

impl ScreenCapture {
    pub fn new() -> Self {
        Self {
            logs: None,
            timings: CaptureTimings::default(),
        }
    }

    pub fn new_with_logs(logs: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            logs: Some(logs),
            timings: CaptureTimings::default(),
        }
    }

    /// Override the delays used by the scroll-and-capture loop.
    pub fn with_timings(mut self, timings: CaptureTimings) -> Self {
        self.timings = timings;
        self
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
    ) -> Result<Option<Vec<String>>> {
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
                let input = std::path::Path::new(input_path);
                let stem = input
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("fixed");
                let dir = input.parent().and_then(|p| p.to_str()).unwrap_or(".");
                // The backup is a plain rename, not a transcode, so it must keep
                // the input's own extension even when writing a different format.
                let input_ext = input.extension().and_then(|s| s.to_str()).unwrap_or("png");
                let orig_backup = format!("{}/{}_orig.{}", dir, stem, input_ext);
                std::fs::rename(input_path, &orig_backup)
                    .map_err(|e| anyhow::anyhow!("Failed to rename original: {}", e))?;
                format!("{}/{}", dir, build_output_path(stem, output_format))
            }
        };
        let written = save_image(&result, &output_path)?;
        self.log(log::Level::Info, &format!("Saved: {}", written.join(", ")));
        Ok(Some(written))
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
        let mut files = list_image_files(folder_path)?;
        files.retain(|p| !is_orig_backup(p));

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

    /// Re-encodes an existing image as `format`, writing it next to the original
    /// under the same name. Saving goes through [`save_image`], so a capture past
    /// the WebP height limit is split into numbered parts exactly like a fresh
    /// capture is.
    ///
    /// Returns `None` when the file is already in the target format, and every
    /// path written otherwise.
    pub fn convert_image(
        &self,
        input_path: &str,
        format: &str,
        delete_original: bool,
    ) -> Result<Option<Vec<String>>> {
        let path = std::path::Path::new(input_path);
        let target_ext = format.trim_start_matches('.').to_lowercase();

        if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case(&target_ext))
            .unwrap_or(false)
        {
            self.log(
                log::Level::Info,
                &format!("Already {}: {} — skipped", target_ext, input_path),
            );
            return Ok(None);
        }

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("Input filename cannot be empty"))?;
        let output_path = path
            .with_file_name(build_output_path(stem, &target_ext))
            .to_string_lossy()
            .into_owned();

        // Captures are expensive to redo, so a conversion never writes over a
        // file that is already sitting there.
        if std::path::Path::new(&output_path).exists() {
            return Err(anyhow::anyhow!(
                "{} already exists — remove it or rename the source first",
                output_path
            ));
        }

        let img = image::open(input_path)
            .map_err(|e| anyhow::anyhow!("Failed to open image: {}", e))?
            .to_rgba8();
        self.log(
            log::Level::Info,
            &format!("Loaded: {} ({}x{})", input_path, img.width(), img.height()),
        );

        let written = save_image(&img, &output_path)?;
        if written.len() > 1 {
            self.log(
                log::Level::Info,
                &format!(
                    "Image is {}px tall, past the {} limit — split into {} parts",
                    img.height(),
                    target_ext.to_uppercase(),
                    written.len()
                ),
            );
        }
        self.log(log::Level::Info, &format!("Saved: {}", written.join(", ")));

        // Only after the new file is safely on disk.
        if delete_original {
            match std::fs::remove_file(input_path) {
                Ok(()) => self.log(
                    log::Level::Info,
                    &format!("Deleted original: {}", input_path),
                ),
                Err(e) => self.log(
                    log::Level::Warn,
                    &format!("Converted, but failed to delete {}: {}", input_path, e),
                ),
            }
        }

        Ok(Some(written))
    }

    /// Converts every image directly inside `folder_path`. Returns
    /// (converted, skipped); files already in the target format and files that
    /// fail to convert both count as skipped, and neither stops the run.
    pub fn convert_images_in_folder(
        &self,
        folder_path: &str,
        format: &str,
        delete_original: bool,
        on_progress: impl Fn(usize, usize, &str),
    ) -> Result<(usize, usize)> {
        let mut files = list_image_files(folder_path)?;
        files.retain(|p| !is_orig_backup(p));

        if files.is_empty() {
            return Ok((0, 0));
        }

        self.log(
            log::Level::Info,
            &format!("Found {} image(s) to convert", files.len()),
        );

        let mut converted_count = 0;
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

            match self.convert_image(&path_str, format, delete_original) {
                Ok(Some(_)) => converted_count += 1,
                Ok(None) => skipped_count += 1,
                Err(e) => {
                    self.log(log::Level::Warn, &format!("  → Error: {}", e));
                    skipped_count += 1;
                }
            }
        }

        Ok((converted_count, skipped_count))
    }

    pub fn pad_numeric_filenames(
        &self,
        folder_path: &str,
        on_progress: impl Fn(usize, usize, &str),
    ) -> Result<usize> {
        let mut files = list_image_files(folder_path)?;
        files.retain(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.chars().all(|c| c.is_ascii_digit()))
                .unwrap_or(false)
        });

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

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn get_mouse_position() -> Result<(i32, i32)> {
        Err(anyhow::anyhow!(
            "Reading the mouse position is not supported on this platform. Pass the region explicitly with --crop 'x,y,width,height'."
        ))
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

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn enable_zoom() -> Result<()> {
        // No portable magnifier to launch; region selection still works without it.
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

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn get_focused_window_bounds(&self) -> Result<Option<(i32, i32, i32, i32)>> {
        Err(anyhow::anyhow!(
            "--window-only is not supported on this platform. Pass the region explicitly with --crop 'x,y,width,height'."
        ))
    }

    fn capture_screen(&self, crop_region: Option<(i32, i32, i32, i32)>) -> Result<RgbaImage> {
        let monitor = xcap::Monitor::all()
            .map_err(|e| anyhow::anyhow!("Failed to get monitors: {}", e))?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No monitor found"))?;

        // xcap builds on the same image version we do, so this needs no conversion.
        let rgba_image = monitor
            .capture_image()
            .map_err(|e| anyhow::anyhow!("Failed to capture screen: {}", e))?;

        let width = rgba_image.width();
        let height = rgba_image.height();

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

    fn detect_actual_overlap(
        img_prev: &RgbaImage,
        img_last: &RgbaImage,
        min_overlap: u32,
        use_best_match: bool,
    ) -> u32 {
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

        if use_best_match {
            // best_k==0 means the fixed bottom region (e.g. nav bar) dominated the match
            // rather than actual content overlap — fall back to configured overlap.
            if best_k > 0 {
                height - best_k
            } else {
                min_overlap
            }
        } else {
            if best_ratio >= MIN_MATCH_RATIO {
                height - best_k
            } else {
                min_overlap
            }
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
        duplicate_threshold: usize,
        trim_bottom: u32,
        best_overlap: bool,
    ) -> Result<RgbaImage> {
        self.capture_with_scroll_impl(
            overlap,
            max_scrolls,
            delay,
            key_type,
            window_only,
            crop,
            false,
            None,
            duplicate_threshold,
            trim_bottom,
            best_overlap,
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
        duplicate_threshold: usize,
        trim_bottom: u32,
        best_overlap: bool,
    ) -> Result<RgbaImage> {
        self.capture_with_scroll_impl(
            overlap,
            max_scrolls,
            delay,
            key_type,
            window_only,
            crop,
            true,
            None,
            duplicate_threshold,
            trim_bottom,
            best_overlap,
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
        stop_flag: Arc<Mutex<bool>>,
        duplicate_threshold: usize,
        trim_bottom: u32,
        best_overlap: bool,
        on_phase: impl Fn(&str),
    ) -> Result<RgbaImage> {
        self.capture_with_scroll_impl(
            overlap,
            max_scrolls,
            delay,
            key_type,
            window_only,
            crop,
            true,
            Some(stop_flag),
            duplicate_threshold,
            trim_bottom,
            best_overlap,
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
        skip_input: bool,
        stop_flag: Option<Arc<Mutex<bool>>>,
        duplicate_threshold: usize,
        trim_bottom: u32,
        best_overlap: bool,
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
            &format!(
                "Delays: scroll delay {}ms, post capture {}ms",
                self.timings.scroll_delay_ms, self.timings.post_capture_ms
            ),
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
            // Give the page time to render the newly scrolled-in content.
            thread::sleep(Duration::from_millis(self.timings.scroll_delay_ms));

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

            // Wait before the next scroll. In CLI mode the wait doubles as the
            // window for a Q keypress, so it is a poll rather than a sleep.
            if !skip_input {
                if poll(Duration::from_millis(self.timings.post_capture_ms))? {
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
                thread::sleep(Duration::from_millis(self.timings.post_capture_ms));
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
            let actual = Self::detect_actual_overlap(
                &images[last_i],
                &images[last_i + 1],
                overlap,
                best_overlap,
            );
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory that removes itself when the test ends, so a failing
    /// assertion cannot leave multi-megabyte images behind.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("capture-test-{}", name));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self, file: &str) -> String {
            self.0.join(file).to_string_lossy().into_owned()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Fills the image with a cheap deterministic pattern. Flat colors compress
    /// to almost nothing, which would let a broken split still round-trip.
    fn noisy_image(width: u32, height: u32) -> RgbaImage {
        let mut state: u32 = 0x1234_5678;
        RgbaImage::from_fn(width, height, |_, _| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let b = state.to_le_bytes();
            image::Rgba([b[0], b[1], b[2], 255])
        })
    }

    fn height_of(path: &str) -> u32 {
        image::open(path).unwrap().height()
    }

    #[test]
    fn save_image_keeps_tall_png_as_one_file() {
        let dir = TempDir::new("tall-png");
        let path = dir.path("out.png");
        let img = noisy_image(8, WEBP_MAX_DIMENSION + 5000);

        let written = save_image(&img, &path).unwrap();

        assert_eq!(written, vec![path.clone()]);
        assert_eq!(height_of(&path), img.height());
    }

    #[test]
    fn save_image_writes_short_webp_as_one_file() {
        let dir = TempDir::new("short-webp");
        let path = dir.path("out.webp");
        let img = noisy_image(8, WEBP_MAX_DIMENSION);

        let written = save_image(&img, &path).unwrap();

        assert_eq!(written, vec![path.clone()]);
        assert_eq!(height_of(&path), WEBP_MAX_DIMENSION);
    }

    #[test]
    fn save_image_splits_tall_webp() {
        let dir = TempDir::new("split-webp");
        let path = dir.path("out.webp");
        let img = noisy_image(8, 40_000);

        let written = save_image(&img, &path).unwrap();

        assert_eq!(written.len(), 3);
        assert_eq!(written[0], dir.path("out_1.webp"));
        assert_eq!(written[2], dir.path("out_3.webp"));

        let heights: Vec<u32> = written.iter().map(|p| height_of(p)).collect();
        assert_eq!(heights.iter().sum::<u32>(), img.height());
        assert!(heights.iter().all(|h| *h <= WEBP_MAX_DIMENSION));
        let (min, max) = (heights.iter().min().unwrap(), heights.iter().max().unwrap());
        assert!(max - min <= 1, "parts should be even, got {heights:?}");
    }

    /// Ten or more parts must stay zero-padded, otherwise `_10` sorts before
    /// `_2` everywhere the parts are listed or re-stitched.
    #[test]
    fn save_image_zero_pads_part_numbers() {
        let dir = TempDir::new("pad-webp");
        let path = dir.path("out.webp");
        let img = noisy_image(4, WEBP_MAX_DIMENSION * 9 + 1);

        let written = save_image(&img, &path).unwrap();

        assert_eq!(written.len(), 10);
        assert_eq!(written[0], dir.path("out_01.webp"));
        assert_eq!(written[9], dir.path("out_10.webp"));
    }

    /// The whole point of WebP here is that it is lossless, so every split part
    /// must decode back to the exact source rows.
    #[test]
    fn webp_split_roundtrip_is_lossless() {
        let dir = TempDir::new("lossless-webp");
        let path = dir.path("out.webp");
        let img = noisy_image(16, 20_000);

        let written = save_image(&img, &path).unwrap();
        assert_eq!(written.len(), 2);

        let mut y = 0;
        for part_path in &written {
            let part = image::open(part_path).unwrap().to_rgba8();
            let expected =
                image::imageops::crop_imm(&img, 0, y, img.width(), part.height()).to_image();
            assert_eq!(
                part.as_raw(),
                expected.as_raw(),
                "part {part_path} differs from source rows at y={y}"
            );
            y += part.height();
        }
        assert_eq!(y, img.height());
    }

    #[test]
    fn convert_image_writes_webp_next_to_png() {
        let dir = TempDir::new("convert-png");
        let png = dir.path("shot.png");
        let img = noisy_image(8, 200);
        img.save(&png).unwrap();

        let written = ScreenCapture::new()
            .convert_image(&png, "webp", false)
            .unwrap()
            .unwrap();

        assert_eq!(written, vec![dir.path("shot.webp")]);
        assert!(
            std::path::Path::new(&png).exists(),
            "original must be kept by default"
        );
        let converted = image::open(&written[0]).unwrap().to_rgba8();
        assert_eq!(
            converted.as_raw(),
            img.as_raw(),
            "conversion must be lossless"
        );
    }

    #[test]
    fn convert_image_deletes_original_when_requested() {
        let dir = TempDir::new("convert-delete");
        let png = dir.path("shot.png");
        noisy_image(8, 200).save(&png).unwrap();

        let written = ScreenCapture::new()
            .convert_image(&png, "webp", true)
            .unwrap()
            .unwrap();

        assert!(std::path::Path::new(&written[0]).exists());
        assert!(!std::path::Path::new(&png).exists());
    }

    /// The whole point of converting through save_image: an old PNG taller than
    /// WebP allows must split just like a fresh capture does.
    #[test]
    fn convert_image_splits_tall_png() {
        let dir = TempDir::new("convert-tall");
        let png = dir.path("tall.png");
        noisy_image(4, 40_000).save(&png).unwrap();

        let written = ScreenCapture::new()
            .convert_image(&png, "webp", false)
            .unwrap()
            .unwrap();

        assert_eq!(written.len(), 3);
        assert_eq!(written[0], dir.path("tall_1.webp"));
        let heights: Vec<u32> = written.iter().map(|p| height_of(p)).collect();
        assert_eq!(heights.iter().sum::<u32>(), 40_000);
        assert!(heights.iter().all(|h| *h <= WEBP_MAX_DIMENSION));
    }

    #[test]
    fn convert_image_skips_same_format() {
        let dir = TempDir::new("convert-same");
        let webp = dir.path("shot.webp");
        noisy_image(8, 100).save(&webp).unwrap();
        let before = std::fs::read(&webp).unwrap();

        let result = ScreenCapture::new()
            .convert_image(&webp, "webp", true)
            .unwrap();

        assert!(result.is_none());
        assert_eq!(std::fs::read(&webp).unwrap(), before);
    }

    #[test]
    fn convert_image_refuses_to_overwrite_existing() {
        let dir = TempDir::new("convert-clash");
        let png = dir.path("shot.png");
        let webp = dir.path("shot.webp");
        noisy_image(8, 100).save(&png).unwrap();
        let existing = noisy_image(4, 50);
        existing.save(&webp).unwrap();

        let err = ScreenCapture::new()
            .convert_image(&png, "webp", true)
            .unwrap_err()
            .to_string();

        assert!(err.contains("already exists"), "unexpected error: {err}");
        assert_eq!(
            image::open(&webp).unwrap().to_rgba8().as_raw(),
            existing.as_raw(),
            "existing file must be untouched"
        );
        assert!(
            std::path::Path::new(&png).exists(),
            "source must survive a failed conversion"
        );
    }

    #[test]
    fn convert_images_in_folder_reports_counts() {
        let dir = TempDir::new("convert-folder");
        noisy_image(8, 100).save(dir.path("a.png")).unwrap();
        noisy_image(8, 100).save(dir.path("b.png")).unwrap();
        noisy_image(8, 100).save(dir.path("c.webp")).unwrap();

        let folder = dir.0.to_string_lossy().into_owned();
        let (converted, skipped) = ScreenCapture::new()
            .convert_images_in_folder(&folder, "webp", false, |_, _, _| {})
            .unwrap();

        assert_eq!((converted, skipped), (2, 1));
        assert!(std::path::Path::new(&dir.path("a.webp")).exists());
        assert!(std::path::Path::new(&dir.path("b.webp")).exists());
    }

    #[test]
    fn save_image_rejects_too_wide_webp() {
        let dir = TempDir::new("wide-webp");
        let path = dir.path("out.webp");
        let img = noisy_image(WEBP_MAX_DIMENSION + 1, 4);

        let err = save_image(&img, &path).unwrap_err().to_string();

        assert!(err.contains("wide"), "unexpected error: {err}");
    }
}
