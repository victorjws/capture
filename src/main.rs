use anyhow::Result;
use clap::Parser;
use capture::ScreenCapture;

#[derive(Parser, Debug)]
#[command(name = "capture")]
#[command(about = "Screen scroll capture tool", long_about = None)]
struct Args {
    #[arg(long, help = "Launch GUI mode")]
    gui: bool,

    #[arg(short, long, default_value = "scroll_capture.png")]
    output: String,

    #[arg(
        short = 'p',
        long,
        default_value_t = 125,
        help = "Overlap pixels for stitching"
    )]
    overlap: u32,

    #[arg(
        short,
        long,
        default_value_t = 3,
        help = "Delay in seconds before starting capture"
    )]
    delay: u64,

    #[arg(
        short = 'k',
        long,
        default_value = "space",
        help = "Key to use for scrolling: space, down, pagedown"
    )]
    key: String,

    // Video mode options
    #[arg(long, help = "Use video recording mode (recommended)")]
    video: bool,

    #[arg(
        long,
        default_value_t = 10,
        help = "Video recording duration in seconds"
    )]
    duration: u64,

    #[arg(
        long,
        default_value_t = 2,
        help = "Frames per second to extract from video"
    )]
    fps: u32,

    #[arg(long, help = "Capture only the focused window (not full screen)")]
    window_only: bool,

    #[arg(
        long,
        help = "Manual crop region as 'x,y,width,height' (e.g., '100,50,1920,1080')"
    )]
    crop: Option<String>,

    #[arg(
        long,
        help = "Use a crop preset (e.g., '1080p', 'vm-small', or custom preset name)"
    )]
    crop_preset: Option<String>,

    #[arg(long, help = "Interactive mode: select crop region with mouse")]
    select_region: bool,

    #[arg(long, help = "List available crop presets")]
    list_presets: bool,

    #[arg(
        long,
        help = "Save current crop region as a preset: 'name:x,y,width,height'"
    )]
    save_preset: Option<String>,

    // Old mode options
    #[arg(
        short,
        long,
        help = "Maximum number of scrolls (screenshot mode only, unlimited if not specified)"
    )]
    max_scrolls: Option<usize>,

    #[arg(
        long,
        default_value_t = 200,
        help = "Delay in milliseconds after scrolling before capturing (screenshot mode only)"
    )]
    scroll_delay: u64,
}

fn get_preset_file_path() -> Result<std::path::PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| anyhow::anyhow!("Could not find home directory"))?;
    Ok(std::path::PathBuf::from(home).join(".capture-presets.json"))
}

fn load_presets() -> Result<std::collections::HashMap<String, String>> {
    use std::collections::HashMap;

    let preset_file = get_preset_file_path()?;

    if !preset_file.exists() {
        return Ok(HashMap::new());
    }

    let content = std::fs::read_to_string(&preset_file)?;
    let presets: HashMap<String, String> =
        serde_json::from_str(&content).unwrap_or_else(|_| HashMap::new());

    Ok(presets)
}

fn save_presets(presets: &std::collections::HashMap<String, String>) -> Result<()> {
    let preset_file = get_preset_file_path()?;
    let content = serde_json::to_string_pretty(presets)?;
    std::fs::write(&preset_file, content)?;
    println!("✓ Presets saved to {}", preset_file.display());
    Ok(())
}

fn get_builtin_presets() -> std::collections::HashMap<String, String> {
    use std::collections::HashMap;
    let mut presets = HashMap::new();

    // Common screen resolutions
    presets.insert("1080p".to_string(), "0,0,1920,1080".to_string());
    presets.insert("720p".to_string(), "0,0,1280,720".to_string());
    presets.insert("4k".to_string(), "0,0,3840,2160".to_string());
    presets.insert("naver-series".to_string(), "607,23,690,1007".to_string());

    // VM window presets (common sizes)
    presets.insert("vm-small".to_string(), "100,100,1024,768".to_string());
    presets.insert("vm-medium".to_string(), "100,100,1280,800".to_string());
    presets.insert("vm-large".to_string(), "100,100,1920,1080".to_string());

    presets
}

fn get_all_presets() -> Result<std::collections::HashMap<String, String>> {
    let mut all_presets = get_builtin_presets();
    let custom_presets = load_presets()?;

    // Custom presets override built-in ones
    all_presets.extend(custom_presets);

    Ok(all_presets)
}

fn list_presets() -> Result<()> {
    println!("\n📋 AVAILABLE CROP PRESETS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    let builtin = get_builtin_presets();
    let custom = load_presets()?;

    println!("\n🔧 Built-in presets:");
    for (name, value) in builtin.iter() {
        if !custom.contains_key(name) {
            println!("  {} = {}", name, value);
        }
    }

    if !custom.is_empty() {
        println!("\n⭐ Custom presets:");
        for (name, value) in custom.iter() {
            println!("  {} = {}", name, value);
        }

        let preset_file = get_preset_file_path()?;
        println!("\n📁 Custom presets file: {}", preset_file.display());
    } else {
        println!("\n⭐ Custom presets: (none)");
        let preset_file = get_preset_file_path()?;
        println!("   Save presets with: --save-preset name:x,y,w,h");
        println!("   File will be created at: {}", preset_file.display());
    }

    println!("\n💡 Usage:");
    println!("   --crop-preset <name>");
    println!("   Example: --crop-preset 1080p");
    println!();

    Ok(())
}

fn save_preset_from_string(preset_str: &str) -> Result<()> {
    let parts: Vec<&str> = preset_str.splitn(2, ':').collect();

    if parts.len() != 2 {
        return Err(anyhow::anyhow!(
            "Invalid preset format. Use: name:x,y,width,height\nExample: --save-preset mypreset:100,50,1920,1080"
        ));
    }

    let name = parts[0].trim();
    let value = parts[1].trim();

    // Validate the crop region format
    if ScreenCapture::parse_crop_region(value).is_none() {
        return Err(anyhow::anyhow!(
            "Invalid crop region format: {}\nUse: x,y,width,height (e.g., '100,50,1920,1080')",
            value
        ));
    }

    let mut presets = load_presets()?;
    presets.insert(name.to_string(), value.to_string());
    save_presets(&presets)?;

    println!("✓ Preset '{}' saved: {}", name, value);
    println!("\n💡 Use with: --crop-preset {}", name);

    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Launch GUI mode if requested
    if args.gui {
        capture::gui::run_gui().map_err(|e| anyhow::anyhow!("GUI error: {:?}", e))?;
        return Ok(());
    }

    // Handle --list-presets
    if args.list_presets {
        return list_presets();
    }

    // Handle --save-preset
    if let Some(preset_str) = &args.save_preset {
        return save_preset_from_string(preset_str);
    }

    let capture = ScreenCapture::new();

    // Resolve crop value (preset takes precedence if both are specified)
    let crop_value = if let Some(preset_name) = &args.crop_preset {
        let all_presets = get_all_presets()?;
        match all_presets.get(preset_name) {
            Some(value) => {
                println!("📌 Using preset '{}': {}", preset_name, value);
                Some(value.clone())
            }
            None => {
                return Err(anyhow::anyhow!(
                    "Preset '{}' not found. Use --list-presets to see available presets.",
                    preset_name
                ));
            }
        }
    } else {
        args.crop.clone()
    };

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
            crop_value.clone(),
        )?;

        result_image.save(&args.output)?;
        println!("\n💾 Saved to {}", args.output);
    } else {
        // Screenshot mode
        println!("📸 SCREENSHOT MODE");
        println!("Configuration:");
        println!("  Output: {}", args.output);
        println!("  Overlap: {} pixels", args.overlap);
        if let Some(max) = args.max_scrolls {
            println!("  Max scrolls: {}", max);
        } else {
            println!("  Max scrolls: unlimited");
        }
        println!("  Scroll key: {}", args.key);
        println!();

        let result_image = capture.capture_with_scroll(
            args.overlap,
            args.max_scrolls,
            args.delay,
            &args.key,
            args.window_only,
            crop_value.clone(),
            args.scroll_delay,
        )?;

        result_image.save(&args.output)?;
        println!("Saved to {}", args.output);
    }

    Ok(())
}
