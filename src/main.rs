use anyhow::Result;
use clap::Parser;
use enigo::{Enigo, Key, Keyboard, Settings};
use image::{ImageBuffer, Rgba, RgbaImage};
use std::thread;
use std::time::Duration;
use crossterm::event::{poll, read, Event, KeyCode, KeyEvent};

// macOS-specific imports
#[cfg(target_os = "macos")]
use core_graphics::display::{CGDisplay, CGMainDisplayID};
#[cfg(target_os = "macos")]
use core_graphics::image::CGImageRef;

// Windows-specific imports
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{RECT, HWND, POINT};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

#[derive(Parser, Debug)]
#[command(name = "capture")]
#[command(about = "Screen scroll capture tool using Core Graphics", long_about = None)]
struct Args {
    #[arg(short, long, default_value = "scroll_capture.png")]
    output: String,

    #[arg(short = 'p', long, default_value_t = 100, help = "Overlap pixels for stitching")]
    overlap: u32,

    #[arg(short, long, default_value_t = 3, help = "Delay in seconds before starting capture")]
    delay: u64,

    #[arg(short = 'k', long, default_value = "space", help = "Key to use for scrolling: space, down, pagedown")]
    key: String,

    // Video mode options
    #[arg(long, help = "Use video recording mode (recommended)")]
    video: bool,

    #[arg(long, default_value_t = 10, help = "Video recording duration in seconds")]
    duration: u64,

    #[arg(long, default_value_t = 2, help = "Frames per second to extract from video")]
    fps: u32,

    #[arg(long, help = "Capture only the focused window (not full screen)")]
    window_only: bool,

    #[arg(long, help = "Manual crop region as 'x,y,width,height' (e.g., '100,50,1920,1080')")]
    crop: Option<String>,

    #[arg(long, help = "Interactive mode: select crop region with mouse")]
    select_region: bool,

    // Old mode options
    #[arg(short, long, default_value_t = 50, help = "Maximum number of scrolls (screenshot mode only)")]
    max_scrolls: usize,
}

struct ScreenCapture {
    #[cfg(target_os = "macos")]
    display_id: u32,
    #[cfg(target_os = "windows")]
    _phantom: (),
}

impl ScreenCapture {
    fn new() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            display_id: unsafe { CGMainDisplayID() },
            #[cfg(target_os = "windows")]
            _phantom: (),
        }
    }

    fn parse_crop_region(crop_str: &str) -> Option<(i32, i32, i32, i32)> {
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
            let coords: Vec<i32> = result.trim()
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
        let _ = std::process::Command::new("magnify.exe")
            .spawn();
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

            thread::sleep(Duration::from_millis(100));
        }
    }

    fn select_region_interactive() -> Result<(i32, i32, i32, i32)> {
        use std::io::{self, Write};

        println!("\n🖱️  INTERACTIVE REGION SELECTION");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!();

        #[cfg(target_os = "macos")]
        {
            println!("💡 TIP: Press ⌥⌘8 to toggle macOS Zoom (magnifier)");
            println!("        ⌥⌘= to zoom in, ⌥⌘- to zoom out");
        }

        #[cfg(target_os = "windows")]
        {
            println!("💡 TIP: Press Win+Plus to open Windows Magnifier");
            println!("        Win+Plus/Minus to zoom in/out");
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
            thread::sleep(Duration::from_millis(500));
        }

        println!();
        println!("Step 1/2: Position mouse at TOP-LEFT corner");

        let (x1, y1) = Self::show_live_coordinates()?;
        println!("✓ Top-left corner: ({}, {})", x1, y1);
        println!();

        println!("Step 2/2: Position mouse at BOTTOM-RIGHT corner");

        let (x2, y2) = Self::show_live_coordinates()?;
        println!("✓ Bottom-right corner: ({}, {})", x2, y2);
        println!();

        // Calculate region
        let x = x1.min(x2);
        let y = y1.min(y2);
        let width = (x2 - x1).abs();
        let height = (y2 - y1).abs();

        if width <= 0 || height <= 0 {
            return Err(anyhow::anyhow!("Invalid region: width and height must be positive"));
        }

        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("✅ Region selected:");
        println!("   Position: ({}, {})", x, y);
        println!("   Size: {}x{}", width, height);
        println!();
        println!("📋 Use this command:");
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
                let parts: Vec<i32> = result.trim().split(',')
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
            if hwnd.0 == 0 {
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

    fn capture_screen(&self) -> Result<RgbaImage> {
        // Try screenshots crate first (more compatible)
        let screen = screenshots::Screen::all()
            .map_err(|e| anyhow::anyhow!("Failed to get screens: {}", e))?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No screen found"))?;

        let captured_image = screen.capture()
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
        thread::sleep(Duration::from_millis(500)); // Wait for content to load
        Ok(())
    }

    fn images_are_similar(&self, img1: &RgbaImage, img2: &RgbaImage, overlap_height: u32) -> (bool, f32) {
        if img1.width() != img2.width() || img1.height() != img2.height() {
            return (false, 100.0);
        }

        let height = img1.height();
        let width = img1.width();

        if overlap_height >= height {
            return (false, 100.0);
        }

        let start_y1 = height - overlap_height;

        let mut diff_count = 0;
        let total_pixels = (overlap_height * width) as usize;
        let threshold = (total_pixels as f32 * 0.05) as usize;

        for y in 0..overlap_height {
            for x in 0..width {
                let pixel1 = img1.get_pixel(x, start_y1 + y);
                let pixel2 = img2.get_pixel(x, y);

                if pixel1 != pixel2 {
                    diff_count += 1;
                    if diff_count > threshold {
                        let diff_percentage = (diff_count as f32 / total_pixels as f32) * 100.0;
                        return (false, diff_percentage);
                    }
                }
            }
        }

        let diff_percentage = (diff_count as f32 / total_pixels as f32) * 100.0;
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
                            continue;
                        }
                        let pixel = img.get_pixel(x, y);
                        result.put_pixel(x, target_y, *pixel);
                    }
                }
            }
        }

        result
    }

    pub fn capture_with_video(&self, overlap: u32, duration: u64, delay: u64, key_type: &str, fps: u32, window_only: bool, crop: Option<String>) -> Result<RgbaImage> {
        println!("Starting video-based scroll capture in {} seconds...", delay);
        println!("Please focus on the window you want to capture!");
        println!("Video will be recorded for {} seconds", duration);
        thread::sleep(Duration::from_secs(delay));

        #[cfg(target_os = "macos")]
        let video_file = "/tmp/scroll_capture_video.mov";
        #[cfg(target_os = "macos")]
        let frames_dir = "/tmp/scroll_capture_frames";

        #[cfg(target_os = "windows")]
        let video_file = std::env::temp_dir().join("scroll_capture_video.mp4");
        #[cfg(target_os = "windows")]
        let frames_dir = std::env::temp_dir().join("scroll_capture_frames");

        #[cfg(target_os = "windows")]
        let video_file = video_file.to_str().unwrap();
        #[cfg(target_os = "windows")]
        let frames_dir = frames_dir.to_str().unwrap();

        // Clean up old files
        let _ = std::fs::remove_file(video_file);
        let _ = std::fs::remove_dir_all(frames_dir);
        std::fs::create_dir_all(frames_dir)?;

        // Determine crop filter (manual crop takes precedence)
        let crop_filter = if let Some(crop_str) = crop {
            // Manual crop region
            if let Some((x, y, w, h)) = Self::parse_crop_region(&crop_str) {
                println!("✂️  Manual crop: {}x{} at ({}, {})", w, h, x, y);
                Some(format!("crop={}:{}:{}:{}", w, h, x, y))
            } else {
                println!("⚠ Invalid crop format, capturing full screen");
                println!("   Use format: 'x,y,width,height' (e.g., '100,50,1920,1080')");
                None
            }
        } else if window_only {
            // Auto-detect focused window
            if let Some((x, y, w, h)) = self.get_focused_window_bounds()? {
                println!("📐 Focused window: {}x{} at ({}, {})", w, h, x, y);
                Some(format!("crop={}:{}:{}:{}", w, h, x, y))
            } else {
                println!("⚠ Could not detect focused window, capturing full screen");
                None
            }
        } else {
            None
        };

        println!("\n🎥 Starting video recording...");

        // Start ffmpeg video recording in background with stdin for control
        let mut cmd = std::process::Command::new("ffmpeg");

        #[cfg(target_os = "macos")]
        {
            cmd.arg("-f").arg("avfoundation")
                .arg("-i").arg("1:none"); // Screen 1, no audio
        }

        #[cfg(target_os = "windows")]
        {
            cmd.arg("-f").arg("gdigrab")
                .arg("-framerate").arg("30")
                .arg("-i").arg("desktop"); // Capture desktop
        }

        // Add crop filter if capturing window only
        if let Some(ref filter) = crop_filter {
            cmd.arg("-vf").arg(filter);
        }

        let mut ffmpeg_process = cmd
            .arg("-t").arg(duration.to_string())
            .arg("-y") // Overwrite output file
            .arg(video_file)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to start ffmpeg: {}", e))?;

        // Wait a bit for ffmpeg to start
        thread::sleep(Duration::from_secs(2));

        println!("✓ Recording started, performing auto-scroll...");
        println!("  Press 'Q' at any time to stop recording\n");

        // Perform auto-scrolling with Q key detection
        let scroll_interval = Duration::from_millis(500);
        let end_time = std::time::Instant::now() + Duration::from_secs(duration - 2);
        let mut user_stopped = false;

        while std::time::Instant::now() < end_time {
            self.scroll_down(key_type)?;

            // Check for Q key to stop early
            if poll(Duration::from_millis(500))? {
                match read()? {
                    Event::Key(KeyEvent { code: KeyCode::Char('q') | KeyCode::Char('Q'), .. }) => {
                        println!("\n✓ Stopped by user");
                        user_stopped = true;
                        break;
                    }
                    _ => {} // Ignore other keys
                }
            }
        }

        // Clear any remaining events
        while poll(Duration::from_millis(0))? {
            let _ = read();
        }

        println!("\n⏸ Stopping recording...");

        // Send 'q' to ffmpeg stdin to stop gracefully
        if let Some(mut stdin) = ffmpeg_process.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(b"q");
            let _ = stdin.flush();
        }

        let _ = ffmpeg_process.wait();

        println!("✓ Video recorded to {}", video_file);
        println!("\n🎞 Extracting frames from video...");

        // Extract frames using ffmpeg
        let output = std::process::Command::new("ffmpeg")
            .arg("-i").arg(video_file)
            .arg("-vf").arg(format!("fps={}", fps))
            .arg(format!("{}/frame_%04d.png", frames_dir))
            .output()
            .map_err(|e| anyhow::anyhow!("Failed to extract frames: {}", e))?;

        if !output.status.success() {
            return Err(anyhow::anyhow!("Frame extraction failed"));
        }

        println!("✓ Frames extracted to {}", frames_dir);
        println!("\n🔗 Loading and stitching frames...");

        // Load all frames
        let mut frame_paths: Vec<_> = std::fs::read_dir(frames_dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("png"))
            .collect();

        frame_paths.sort();

        if frame_paths.is_empty() {
            return Err(anyhow::anyhow!("No frames extracted"));
        }

        println!("  Found {} frames", frame_paths.len());

        let mut images = Vec::new();
        for (i, path) in frame_paths.iter().enumerate() {
            let img = image::open(path)?.to_rgba8();

            // Skip similar frames
            if i > 0 {
                let (is_similar, diff) = self.images_are_similar(&images.last().unwrap(), &img, overlap);
                println!("  Frame {} - diff: {:.1}%", i + 1, diff);
                if !is_similar {
                    images.push(img);
                }
            } else {
                println!("  Frame {} (first frame)", i + 1);
                images.push(img);
            }
        }

        println!("\n✓ Selected {} unique frames for stitching", images.len());

        let result = self.stitch_images(images, overlap);
        println!("✓ Done! Final image size: {}x{}", result.width(), result.height());

        // Clean up
        let _ = std::fs::remove_file(video_file);
        let _ = std::fs::remove_dir_all(frames_dir);

        Ok(result)
    }

    pub fn capture_with_scroll(&self, overlap: u32, max_scrolls: usize, delay: u64, key_type: &str) -> Result<RgbaImage> {
        println!("Starting scroll capture in {} seconds...", delay);
        println!("Please focus on the window you want to capture!");
        println!("Make sure to grant Accessibility permission in System Settings > Privacy & Security");
        println!("The program will press {} key once per capture", key_type.to_uppercase());
        thread::sleep(Duration::from_secs(delay));

        let mut images = Vec::new();
        let first_capture = self.capture_screen()?;
        println!("✓ Captured screen 1 ({}x{})", first_capture.width(), first_capture.height());
        images.push(first_capture.clone());

        let mut previous_capture = first_capture;
        let mut scroll_count = 0;

        while scroll_count < max_scrolls {
            println!("\n[{}/{}] Pressing {}...", scroll_count + 1, max_scrolls, key_type.to_uppercase());
            self.scroll_down(key_type)?;

            let current_capture = self.capture_screen()?;
            println!("✓ Captured screen {}", scroll_count + 2);

            let (is_similar, diff_percentage) = self.images_are_similar(&previous_capture, &current_capture, overlap);
            println!("  Image difference: {:.2}%", diff_percentage);

            if is_similar {
                println!("\n⚠ Reached end of scrollable content (images are {:.2}% similar)", 100.0 - diff_percentage);
                break;
            }

            images.push(current_capture.clone());
            previous_capture = current_capture;
            scroll_count += 1;

            // Check for user input to stop early
            println!("  Press 'Q' to stop early (waiting 1s)...");
            if poll(Duration::from_millis(1000))? {
                match read()? {
                    Event::Key(KeyEvent { code: KeyCode::Char('q') | KeyCode::Char('Q'), .. }) => {
                        println!("\n✓ Stopped by user");
                        break;
                    }
                    _ => {} // Ignore other keys
                }
            }
        }

        // Clear any remaining events before finishing
        while poll(Duration::from_millis(0))? {
            let _ = read();
        }

        println!("\nStitching {} images together...", images.len());
        let result = self.stitch_images(images, overlap);
        println!("✓ Done! Final image size: {}x{}", result.width(), result.height());

        Ok(result)
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let capture = ScreenCapture::new();

    // Handle region selection mode
    if args.select_region {
        let (x, y, w, h) = ScreenCapture::select_region_interactive()?;

        // Offer to run capture immediately
        use std::io::{self, Write};
        print!("Do you want to capture this region now? (y/N): ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if input.trim().to_lowercase() == "y" {
            println!("\n🎬 Starting capture with selected region...\n");
            let result_image = capture.capture_with_video(
                args.overlap,
                args.duration,
                args.delay,
                &args.key,
                args.fps,
                false, // Don't use window_only
                Some(format!("{},{},{},{}", x, y, w, h)),
            )?;

            result_image.save(&args.output)?;
            println!("\n💾 Saved to {}", args.output);
        }

        return Ok(());
    }

    if args.video {
        // Video recording mode
        println!("🎬 VIDEO RECORDING MODE");
        println!("Configuration:");
        println!("  Output: {}", args.output);
        println!("  Overlap: {} pixels", args.overlap);
        println!("  Duration: {} seconds", args.duration);
        println!("  FPS: {}", args.fps);
        println!("  Scroll key: {}", args.key);
        println!();

        let result_image = capture.capture_with_video(
            args.overlap,
            args.duration,
            args.delay,
            &args.key,
            args.fps,
            args.window_only,
            args.crop.clone(),
        )?;

        result_image.save(&args.output)?;
        println!("\n💾 Saved to {}", args.output);
    } else {
        // Screenshot mode
        println!("📸 SCREENSHOT MODE");
        println!("Configuration:");
        println!("  Output: {}", args.output);
        println!("  Overlap: {} pixels", args.overlap);
        println!("  Max scrolls: {}", args.max_scrolls);
        println!("  Scroll key: {}", args.key);
        println!();

        let result_image = capture.capture_with_scroll(
            args.overlap,
            args.max_scrolls,
            args.delay,
            &args.key,
        )?;

        result_image.save(&args.output)?;
        println!("Saved to {}", args.output);
    }

    Ok(())
}
