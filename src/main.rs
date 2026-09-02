use anyhow::Result;
use capture::constants::CaptureTimings;
use capture::presets;
use capture::{ScreenCapture, build_output_path, validate_format, validate_output_path};
use clap::Parser;
use log::info;

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
        default_value_t = capture::constants::defaults::OVERLAP,
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

    #[arg(
        short,
        long,
        help = "Maximum number of scrolls (screenshot mode only, unlimited if not specified)"
    )]
    max_scrolls: Option<usize>,

    #[arg(
        long,
        default_value_t = capture::constants::defaults::SCROLL_DELAY,
        help = "Delay in milliseconds between the scroll keypress and the screenshot, for content to load"
    )]
    scroll_delay: u64,

    #[arg(
        long,
        default_value_t = capture::constants::defaults::POST_CAPTURE_DELAY,
        help = "Delay in milliseconds between the screenshot and the next scroll; also the window for a Q keypress to stop"
    )]
    post_capture_delay: u64,

    #[arg(
        long,
        default_value_t = 2,
        help = "Number of consecutive identical frames required to stop capture (default: 2)"
    )]
    duplicate_threshold: usize,

    #[arg(long, help = "Fix overlap artifacts in an existing stitched image")]
    fix: Option<String>,

    #[arg(
        long,
        help = "Height of each captured frame in pixels, required when using --fix"
    )]
    screen_height: Option<u32>,

    #[arg(
        long,
        default_value_t = 448,
        help = "Pixels to trim from the bottom of the final image (e.g., remove webtoon footer UI)"
    )]
    trim_bottom: u32,

    #[arg(
        long,
        help = "Fix images captured with the old overlap/2 seam (use for captures made before the full-overlap stitching update)"
    )]
    half_seam: bool,

    #[arg(
        long,
        help = "Use best-match overlap detection for the last frame: picks the k with the highest match ratio instead of requiring 90%"
    )]
    best_overlap: bool,
}

fn list_presets() -> Result<()> {
    let builtin = presets::get_builtin_presets();
    let custom = presets::load_presets()?;

    info!("\nAVAILABLE CROP PRESETS");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    info!("\nBuilt-in presets:");
    for (name, value) in builtin.iter() {
        if !custom.contains_key(name) {
            info!("  {} = {}", name, value);
        }
    }

    if !custom.is_empty() {
        info!("\nCustom presets:");
        for (name, value) in custom.iter() {
            info!("  {} = {}", name, value);
        }
        let preset_file = presets::get_preset_file_path()?;
        info!("\nCustom presets file: {}", preset_file.display());
    } else {
        info!("\nCustom presets: (none)");
        let preset_file = presets::get_preset_file_path()?;
        info!("   Save presets with: --save-preset name:x,y,w,h");
        info!("   File will be created at: {}", preset_file.display());
    }

    info!("\nUsage:");
    info!("   --crop-preset <name>");
    info!("   Example: --crop-preset 1080p");

    Ok(())
}

fn save_preset_from_string(preset_str: &str) -> Result<()> {
    let parts: Vec<&str> = preset_str.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Err(anyhow::anyhow!(
            "Invalid preset format. Use: name:x,y,width,height\nExample: --save-preset mypreset:100,50,1920,1080"
        ));
    }
    let (name, value) = (parts[0].trim(), parts[1].trim());
    presets::save_preset(name, value)?;
    info!("Preset '{}' saved: {}", name, value);
    info!("Use with: --crop-preset {}", name);
    Ok(())
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(|buf, record| {
            use std::io::Write;
            let ts = chrono::Local::now().format("%H:%M:%S");
            writeln!(buf, "{} [{}] {}", ts, record.level(), record.args())
        })
        .init();

    let args = Args::parse();

    if args.gui {
        capture::gui::run_gui().map_err(|e| anyhow::anyhow!("GUI error: {:?}", e))?;
        return Ok(());
    }

    if let Some(fix_path) = &args.fix {
        let screen_height = args.screen_height.ok_or_else(|| {
            anyhow::anyhow!(
                "--screen-height is required when using --fix (e.g., --screen-height 1080)"
            )
        })?;
        let output_override = if args.output == "00" {
            None
        } else {
            Some(args.output.as_str())
        };
        if let Some(out) = output_override {
            validate_output_path(&build_output_path(out, &args.format))?;
        }
        let capture = ScreenCapture::new();
        match capture.fix_image(
            fix_path,
            &args.format,
            output_override,
            screen_height,
            args.overlap,
            args.trim_bottom,
            args.half_seam,
        )? {
            Some(path) => info!("Saved to {}", path),
            None => info!("No overlap detected — image looks correct."),
        }
        return Ok(());
    }

    if args.list_presets {
        return list_presets();
    }

    if let Some(preset_str) = &args.save_preset {
        return save_preset_from_string(preset_str);
    }

    validate_format(&args.format)?;

    let output_path = build_output_path(&args.output, &args.format);
    validate_output_path(&output_path)?;
    let capture = ScreenCapture::new().with_timings(CaptureTimings {
        scroll_delay_ms: args.scroll_delay,
        post_capture_ms: args.post_capture_delay,
    });

    let crop_value = if let Some(preset_name) = &args.crop_preset {
        let all_presets = presets::get_all_presets()?;
        match all_presets.get(preset_name) {
            Some(value) => {
                info!("Using preset '{}': {}", preset_name, value);
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

    if args.select_region {
        let (x, y, w, h) = ScreenCapture::select_region_interactive()?;

        use std::io::{self, Write};
        print!("Do you want to capture this region now? (y/N): ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if input.trim().to_lowercase() == "y" {
            info!("Starting capture with selected region...");
            let result_image = capture.capture_with_scroll(
                args.overlap,
                args.max_scrolls,
                args.delay,
                &args.key,
                false,
                Some(format!("{},{},{},{}", x, y, w, h)),
                args.duplicate_threshold,
                args.trim_bottom,
                args.best_overlap,
            )?;
            result_image.save(&output_path)?;
            info!("Saved to {}", output_path);
        }

        return Ok(());
    }

    info!("SCREENSHOT MODE");
    info!("Output: {}", output_path);
    info!("Overlap: {} pixels", args.overlap);
    match args.max_scrolls {
        Some(max) => info!("Max scrolls: {}", max),
        None => info!("Max scrolls: unlimited"),
    }
    info!("Scroll key: {}", args.key);

    let result_image = capture.capture_with_scroll(
        args.overlap,
        args.max_scrolls,
        args.delay,
        &args.key,
        args.window_only,
        crop_value,
        args.duplicate_threshold,
        args.trim_bottom,
        args.best_overlap,
    )?;

    result_image.save(&output_path)?;
    info!("Saved to {}", output_path);

    Ok(())
}
