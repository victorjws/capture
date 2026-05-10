use anyhow::Result;
use capture::presets;
use capture::{ScreenCapture, build_output_path, validate_format};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "capture")]
#[command(about = "Screen scroll capture tool", long_about = None)]
struct Args {
    #[arg(long, help = "Launch GUI mode")]
    gui: bool,

    #[arg(short, long, default_value = "00")]
    output: String,

    #[arg(
        short,
        long,
        default_value = "png",
        help = "Output format: png, jpg, jpeg, gif, bmp, tiff, tif, webp"
    )]
    format: String,

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

    #[arg(long, help = "Fix overlap artifacts in an existing stitched image")]
    fix: Option<String>,

    #[arg(
        long,
        help = "Height of each captured frame in pixels, required when using --fix"
    )]
    screen_height: Option<u32>,
}

fn list_presets() -> Result<()> {
    println!("\nAVAILABLE CROP PRESETS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    let builtin = presets::get_builtin_presets();
    let custom = presets::load_presets()?;

    println!("\nBuilt-in presets:");
    for (name, value) in builtin.iter() {
        if !custom.contains_key(name) {
            println!("  {} = {}", name, value);
        }
    }

    if !custom.is_empty() {
        println!("\nCustom presets:");
        for (name, value) in custom.iter() {
            println!("  {} = {}", name, value);
        }

        let preset_file = presets::get_preset_file_path()?;
        println!("\nCustom presets file: {}", preset_file.display());
    } else {
        println!("\nCustom presets: (none)");
        let preset_file = presets::get_preset_file_path()?;
        println!("   Save presets with: --save-preset name:x,y,w,h");
        println!("   File will be created at: {}", preset_file.display());
    }

    println!("\nUsage:");
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
    if presets::parse_crop_region(value).is_none() {
        return Err(anyhow::anyhow!(
            "Invalid crop region format: {}\nUse: x,y,width,height (e.g., '100,50,1920,1080')",
            value
        ));
    }

    let mut preset_map = presets::load_presets()?;
    preset_map.insert(name.to_string(), value.to_string());
    presets::save_presets(&preset_map)?;

    println!("Preset '{}' saved: {}", name, value);
    println!("\nUse with: --crop-preset {}", name);

    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Launch GUI mode if requested
    if args.gui {
        capture::gui::run_gui().map_err(|e| anyhow::anyhow!("GUI error: {:?}", e))?;
        return Ok(());
    }

    // Handle --fix
    if let Some(fix_path) = &args.fix {
        let screen_height = args.screen_height.ok_or_else(|| {
            anyhow::anyhow!("--screen-height is required when using --fix (e.g., --screen-height 1080)")
        })?;

        let img = image::open(fix_path)
            .map_err(|e| anyhow::anyhow!("Failed to open {}: {}", fix_path, e))?
            .to_rgba8();

        println!("Fixing: {} ({}x{})", fix_path, img.width(), img.height());

        if let Some((prev_frame, last_frame)) =
            ScreenCapture::extract_fix_debug_frames(&img, screen_height, args.overlap)
        {
            let stem = std::path::Path::new(fix_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("debug");
            let prev_path = build_output_path(&format!("{}_debug_prev", stem), &args.format);
            let last_path = build_output_path(&format!("{}_debug_last", stem), &args.format);
            prev_frame.save(&prev_path)?;
            last_frame.save(&last_path)?;
            println!("Debug frames: {} / {}", prev_path, last_path);
        }

        match ScreenCapture::fix_stitched_overlap(&img, screen_height, args.overlap) {
            Some(fixed) => {
                let output_path = if args.output == "00" {
                    let stem = std::path::Path::new(fix_path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("fixed");
                    build_output_path(&format!("{}_fixed", stem), &args.format)
                } else {
                    build_output_path(&args.output, &args.format)
                };
                fixed.save(&output_path)?;
                println!("Saved to {}", output_path);
            }
            None => {
                println!("No overlap detected — image looks correct.");
            }
        }
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

    // Validate format before starting capture
    validate_format(&args.format)?;

    // Build full output path
    let output_path = build_output_path(&args.output, &args.format);

    let capture = ScreenCapture::new();

    // Resolve crop value (preset takes precedence if both are specified)
    let crop_value = if let Some(preset_name) = &args.crop_preset {
        let all_presets = presets::get_all_presets()?;
        match all_presets.get(preset_name) {
            Some(value) => {
                println!("Using preset '{}': {}", preset_name, value);
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
            println!("\n📸 Starting capture with selected region...\n");
            let result_image = capture.capture_with_scroll(
                args.overlap,
                args.max_scrolls,
                args.delay,
                &args.key,
                false, // Don't use window_only
                Some(format!("{},{},{},{}", x, y, w, h)),
                args.scroll_delay,
            )?;

            result_image.save(&output_path)?;
            println!("\n💾 Saved to {}", output_path);
        }

        return Ok(());
    }

    // Screenshot mode
    println!("📸 SCREENSHOT MODE");
    println!("Configuration:");
    println!("  Output: {}", output_path);
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

    result_image.save(&output_path)?;
    println!("Saved to {}", output_path);

    Ok(())
}
