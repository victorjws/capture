// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2025-2026 Jinwon Seo

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

pub const SUPPORTED_FORMATS: &[&str] = &[
    "jxl", "webp", "png", "jpg", "jpeg", "gif", "bmp", "tiff", "tif",
];

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

/// cjxl's own default. On a 690x85231 capture `-e 7` lands 30% under lossless
/// WebP in about 7 seconds; `-e 9` buys another 2% for 4.6x the time, which is
/// not worth paying on every capture.
const JXL_EFFORT: &str = "7";

/// Inputs cjxl reads itself, so converting one to JXL can hand over the file
/// instead of the pixels decoded out of it.
///
/// cjxl also reads PPM/PNM/PFM/PAM/PGX/EXR, but none of those are inputs this
/// program accepts (see [`IMAGE_EXTENSIONS`]). `.jxl` is left out on purpose:
/// a lone one is already in the target format and a numbered run of them is a
/// merge, and neither goes through here.
const CJXL_DIRECT_INPUT: &[&str] = &["jpg", "jpeg", "png", "gif"];

/// Shown wherever a missing cjxl or djxl is what actually went wrong, since the
/// bare "No such file or directory" from a failed spawn tells nobody anything.
const JXL_TOOLS_HINT: &str = "JPEG XL needs the libjxl command line tools (cjxl and djxl).\n\
     macOS: brew install jpeg-xl\n\
     Linux: apt install libjxl-tools";

/// True when `path` ends in `ext`, ignoring case.
fn has_extension(path: impl AsRef<std::path::Path>, ext: &str) -> bool {
    path.as_ref()
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case(ext))
        .unwrap_or(false)
}

/// True when `path` ends in any of `exts`, ignoring case.
fn has_any_extension(path: impl AsRef<std::path::Path>, exts: &[&str]) -> bool {
    exts.iter().any(|ext| has_extension(&path, ext))
}

/// True when the libjxl command line tools are installed. Probed once, since
/// otherwise every saved part would pay for two process spawns.
pub fn has_jxl_tools() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| tool_runs("cjxl") && tool_runs("djxl"))
}

fn tool_runs(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A scratch file that deletes itself on drop, so a failed cjxl run never
/// leaves a few hundred megabytes of raw pixels behind.
struct TempFile(std::path::PathBuf);

impl TempFile {
    fn new(ext: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "capture-{}-{}.{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            ext
        );
        Self(std::env::temp_dir().join(name))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Writes `img` somewhere cjxl can read it. cjxl takes files rather than raw
/// pixels on stdin, and PNG-encoding a part costs more than the JXL encode
/// itself, so this writes a PNM: a short header followed by the raw samples.
///
/// Captures are opaque, so the alpha channel is dropped when it carries no
/// information, which makes the scratch file a quarter smaller.
fn write_pnm_temp(img: &RgbaImage) -> Result<TempFile> {
    let opaque = img.pixels().all(|p| p[3] == u8::MAX);
    let temp = TempFile::new(if opaque { "ppm" } else { "pam" });
    let file = std::fs::File::create(temp.path())
        .map_err(|e| anyhow::anyhow!("Failed to create {}: {}", temp.path().display(), e))?;
    let mut out = std::io::BufWriter::new(file);

    write_pnm(&mut out, img, opaque)
        .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", temp.path().display(), e))?;

    Ok(temp)
}

/// Writes a PNM whose samples are already laid out the way the format wants, so
/// the header is the only work.
fn write_raw_pnm_temp(
    ext: &str,
    magic: &str,
    width: u32,
    height: u32,
    samples: &[u8],
) -> Result<TempFile> {
    use std::io::Write;

    let temp = TempFile::new(ext);
    let file = std::fs::File::create(temp.path())
        .map_err(|e| anyhow::anyhow!("Failed to create {}: {}", temp.path().display(), e))?;
    let mut out = std::io::BufWriter::new(file);
    let write = |out: &mut std::io::BufWriter<std::fs::File>| -> std::io::Result<()> {
        write!(out, "{}\n{} {}\n255\n", magic, width, height)?;
        out.write_all(samples)?;
        out.flush()
    };
    write(&mut out)
        .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", temp.path().display(), e))?;
    Ok(temp)
}

fn write_pnm(out: &mut impl std::io::Write, img: &RgbaImage, opaque: bool) -> std::io::Result<()> {
    if opaque {
        write!(out, "P6\n{} {}\n255\n", img.width(), img.height())?;
        let mut row = Vec::with_capacity(img.width() as usize * 3);
        for y in 0..img.height() {
            row.clear();
            for x in 0..img.width() {
                row.extend_from_slice(&img.get_pixel(x, y).0[..3]);
            }
            out.write_all(&row)?;
        }
    } else {
        write!(
            out,
            "P7\nWIDTH {}\nHEIGHT {}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n",
            img.width(),
            img.height()
        )?;
        out.write_all(img.as_raw())?;
    }
    out.flush()
}

/// Encodes `img` as mathematically lossless JPEG XL via cjxl.
fn encode_jxl(img: &RgbaImage, output_path: &str) -> Result<()> {
    let input = write_pnm_temp(img)?;
    let result = std::process::Command::new("cjxl")
        .arg(input.path())
        .arg(output_path)
        .args(["-d", "0", "-e", JXL_EFFORT, "--quiet"])
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run cjxl: {}\n{}", e, JXL_TOOLS_HINT))?;

    if !result.status.success() {
        return Err(anyhow::anyhow!(
            "cjxl failed for {}: {}",
            output_path,
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    Ok(())
}

/// Encodes a file cjxl can read by handing over the file itself, never the
/// pixels decoded out of it.
///
/// Decoding first is what made a JPEG grow: the pixels a JPEG decodes to carry
/// every ringing and blocking artifact it baked in, and reproducing that noise
/// bit for bit costs several times what the JPEG spent throwing it away. Given
/// the file, cjxl instead transcodes — it lifts the quantized DCT coefficients
/// out without ever running an inverse DCT and repacks them with its own
/// entropy coder, so the image is unchanged and the file lands 16-22% smaller.
/// A `jbrd` box carries the JPEG's headers, so `djxl out.jxl out.jpg` returns
/// the original file byte for byte.
///
/// The same applies to everything else cjxl reads: bit depth, grayscale, the
/// ICC profile, Exif, XMP and every frame of an animation come along because
/// they never pass through us.
///
/// `--allow_jpeg_reconstruction=0` and `-x strip=` must never be added here.
/// Both discard what makes the result reconstructible.
fn encode_jxl_from_file(input: &std::path::Path, output_path: &str) -> Result<()> {
    // -d 0 is redundant for JPEG and GIF, whose default distance is already 0,
    // and lossless JPEG transcoding takes precedence over it either way. It is
    // what makes PNG and the rest lossless, so it stays.
    let result = std::process::Command::new("cjxl")
        .arg(input)
        .arg(output_path)
        .args(["-d", "0", "-e", JXL_EFFORT, "--quiet"])
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run cjxl: {}\n{}", e, JXL_TOOLS_HINT))?;

    if !result.status.success() {
        return Err(anyhow::anyhow!(
            "cjxl failed for {}: {}",
            output_path,
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    Ok(())
}

/// A scratch file cjxl can read, by the cheapest route that keeps everything.
///
/// Raw PNM samples where the pixels are already 8 bits and there is no profile to
/// embed: PNG-encoding a tall capture costs more than the JXL encode itself, and
/// this is the path the 690x85231 archive conversion takes. Anything deeper, or
/// carrying an ICC profile, goes out as a PNG, which holds both and a PNM holds
/// neither.
fn write_scratch_for_cjxl(source: &SourceImage) -> Result<TempFile> {
    if source.icc.is_none() {
        let (w, h) = (source.img.width(), source.img.height());
        match &source.img {
            image::DynamicImage::ImageRgba8(img) => return write_pnm_temp(img),
            image::DynamicImage::ImageRgb8(img) => {
                return write_raw_pnm_temp("ppm", "P6", w, h, img.as_raw());
            }
            image::DynamicImage::ImageLuma8(img) => {
                return write_raw_pnm_temp("pgm", "P5", w, h, img.as_raw());
            }
            _ => {}
        }
    }

    let temp = TempFile::new("png");
    write_dynamic(
        &source.img,
        &temp.path().to_string_lossy(),
        Metadata {
            icc: source.icc.as_deref(),
            exif: None,
        },
    )?;
    Ok(temp)
}

/// Encodes a source cjxl cannot read itself, keeping what the pixels alone
/// would not carry.
///
/// Only WebP, BMP and TIFF get here; cjxl reads every other input this program
/// accepts, and those go through [`encode_jxl_from_file`]. The scratch file is a
/// PNG rather than the PNM the capture path writes because [`write_pnm`] only
/// speaks 8-bit RGB and RGBA, while a PNG holds 16-bit samples, a grayscale
/// channel count and the ICC profile in one go. Exif is handed over separately
/// since cjxl reads it from a flag but not from a PNG chunk.
fn encode_jxl_from_source(source: &SourceImage, output_path: &str) -> Result<()> {
    let temp = write_scratch_for_cjxl(source)?;

    let exif = match &source.exif {
        Some(bytes) => {
            let file = TempFile::new("exif");
            std::fs::write(file.path(), bytes)
                .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", file.path().display(), e))?;
            Some(file)
        }
        None => None,
    };

    let mut command = std::process::Command::new("cjxl");
    command
        .arg(temp.path())
        .arg(output_path)
        .args(["-d", "0", "-e", JXL_EFFORT, "--quiet"]);
    if let Some(exif) = &exif {
        command
            .arg("-x")
            .arg(format!("exif={}", exif.path().display()));
    }

    let result = command
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run cjxl: {}\n{}", e, JXL_TOOLS_HINT))?;
    if !result.status.success() {
        return Err(anyhow::anyhow!(
            "cjxl failed for {}: {}",
            output_path,
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    Ok(())
}

/// Decodes a JPEG XL file via djxl. The `image` crate has no JXL support, so
/// the round trip goes through a PNG the decoder does understand.
fn decode_jxl(input_path: &str) -> Result<RgbaImage> {
    let temp = TempFile::new("png");
    let result = std::process::Command::new("djxl")
        .arg(input_path)
        .arg(temp.path())
        .arg("--quiet")
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run djxl: {}\n{}", e, JXL_TOOLS_HINT))?;

    if !result.status.success() {
        return Err(anyhow::anyhow!(
            "djxl failed for {}: {}",
            input_path,
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }

    Ok(image::open(temp.path())
        .map_err(|e| anyhow::anyhow!("Failed to open image: {}", e))?
        .to_rgba8())
}

/// Loads any supported image as RGBA, routing `.jxl` through libjxl.
pub fn open_image(input_path: &str) -> Result<RgbaImage> {
    if has_extension(input_path, "jxl") {
        decode_jxl(input_path)
    } else {
        Ok(image::open(input_path)
            .map_err(|e| anyhow::anyhow!("Failed to open image: {}", e))?
            .to_rgba8())
    }
}

/// A file a conversion opened: the pixels at the depth they are actually
/// stored, plus what the source carried beside them.
///
/// [`open_image`] flattens everything to 8-bit RGBA, which is right for a
/// screenshot and wrong for a conversion. `xcap` hands over an `RgbaImage` and
/// the capture pipeline is built on that type, so the convert path reused it and
/// coerced whatever it read to match — halving a 16-bit PNG, dropping the ICC
/// profile that says what the colors mean, and keeping only the first frame of
/// an animation. This keeps all of it so the writer can decide what survives.
struct SourceImage {
    img: image::DynamicImage,
    icc: Option<Vec<u8>>,
    exif: Option<Vec<u8>>,
    orientation: image::metadata::Orientation,
    /// Frames past the first. No target reachable through the pixel path can
    /// hold them, so their presence is a refusal rather than a warning.
    extra_frames: usize,
}

impl SourceImage {
    fn bits_per_channel(&self) -> u16 {
        bits_per_channel(self.img.color())
    }

    /// What `target_ext` cannot carry across, in words fit for an error message.
    ///
    /// Only worth asking before deleting an original. A reading copy is welcome
    /// to be 8-bit sRGB; an archive that replaces the original is not.
    fn losses_for(&self, target_ext: &str) -> Vec<String> {
        let mut lost = Vec::new();
        let bits = self.bits_per_channel();
        if self.extra_frames > 0 {
            // JXL can hold an animation, but only when cjxl reads the file
            // itself. Reaching here means the pixels are being handed over one
            // image at a time, so the rest of the frames are gone either way.
            lost.push(format!(
                "only the first of {} frames is kept, since the pixels are handed to {} one image at a time",
                self.extra_frames + 1,
                target_ext
            ));
        }
        if bits > 8 && !format_holds_deep_pixels(target_ext) {
            lost.push(format!(
                "{} holds 8 bits per channel, not the {} this image has",
                target_ext, bits
            ));
        }
        if self.exif.is_some() && !format_holds_exif(target_ext) {
            lost.push(format!("{} cannot store the Exif metadata", target_ext));
        }
        if self.icc.is_some() && !format_holds_icc(target_ext) {
            lost.push(format!(
                "{} cannot store the ICC colour profile",
                target_ext
            ));
        }
        lost
    }

    fn metadata(&self) -> Metadata<'_> {
        Metadata {
            icc: self.icc.as_deref(),
            exif: self.exif.as_deref(),
        }
    }

    /// Turns the Exif orientation into actual rotated pixels, for targets that
    /// cannot carry the tag.
    ///
    /// Without this a photo taken sideways comes out sideways: the `image` crate
    /// does not apply the tag on read, so the pixels are stored rotated and only
    /// the tag says so. Targets that keep Exif are left alone, where passing the
    /// tag through means the result reads exactly like the source.
    fn bake_orientation(&mut self) -> bool {
        if self.orientation == image::metadata::Orientation::NoTransforms {
            return false;
        }
        self.img.apply_orientation(self.orientation);
        self.orientation = image::metadata::Orientation::NoTransforms;
        true
    }
}

/// Writes a converted source, carrying across whatever `target_ext` can hold.
fn write_converted(
    source: &SourceImage,
    output_path: &str,
    target_ext: &str,
) -> Result<Vec<String>> {
    if target_ext == "jxl" {
        encode_jxl_from_source(source, output_path)?;
        return Ok(vec![output_path.to_string()]);
    }
    if source.bits_per_channel() > 8 && format_holds_deep_pixels(target_ext) {
        write_dynamic(&source.img, output_path, source.metadata())?;
        return Ok(vec![output_path.to_string()]);
    }
    // Borrowed where it already is 8-bit RGBA, since `to_rgba8` would clone the
    // whole buffer, and a scroll capture's buffer runs to hundreds of megabytes.
    match &source.img {
        image::DynamicImage::ImageRgba8(img) => {
            save_image_with(img, output_path, source.metadata())
        }
        other => save_image_with(&other.to_rgba8(), output_path, source.metadata()),
    }
}

/// True for targets that can store Exif. TIFF is missing because the encoder
/// here implements `set_icc_profile` but not `set_exif_metadata`.
fn format_holds_exif(format: &str) -> bool {
    matches!(format, "jxl" | "png" | "webp" | "jpg" | "jpeg")
}

/// True for targets that keep more than 8 bits per channel. WebP is 8-bit in
/// this encoder (`L8`/`La8`/`Rgb8`/`Rgba8` only) and baseline JPEG is 8-bit by
/// definition.
fn format_holds_deep_pixels(format: &str) -> bool {
    matches!(format, "jxl" | "png" | "tiff" | "tif")
}

/// True for targets whose encoder here can embed an ICC profile.
fn format_holds_icc(format: &str) -> bool {
    matches!(
        format,
        "jxl" | "png" | "tiff" | "tif" | "webp" | "jpg" | "jpeg"
    )
}

/// Loads a file for conversion without flattening it.
///
/// Refuses an animated WebP outright: the frames can be read but there is no
/// lossless way to hand them on — cjxl does not read WebP, `PngEncoder` does not
/// write APNG, and a GIF would quantise to 256 colours. Silently keeping frame
/// one is worse than saying so.
fn open_source_image(input_path: &str) -> Result<SourceImage> {
    use image::ImageDecoder;

    if has_extension(input_path, "jxl") {
        return Ok(SourceImage {
            img: image::DynamicImage::ImageRgba8(decode_jxl(input_path)?),
            icc: None,
            exif: None,
            orientation: image::metadata::Orientation::NoTransforms,
            extra_frames: 0,
        });
    }

    let path = std::path::Path::new(input_path);
    let mut decoder = image::ImageReader::open(path)
        .map_err(|e| anyhow::anyhow!("Failed to open {}: {}", input_path, e))?
        .with_guessed_format()
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", input_path, e))?
        .into_decoder()
        .map_err(|e| anyhow::anyhow!("Failed to decode {}: {}", input_path, e))?;

    // Read the metadata off the decoder before the pixels consume it.
    let icc = decoder.icc_profile().unwrap_or(None);
    let exif = decoder.exif_metadata().unwrap_or(None);
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);

    let extra_frames = count_extra_frames(path)?;
    if extra_frames > 0 && has_extension(path, "webp") {
        return Err(anyhow::anyhow!(
            "{} is an animated WebP with {} frames, and none of the formats reachable from here \
             can hold them losslessly.\nConvert it with a tool that keeps the animation, or use \
             --format png to keep the first frame on purpose.",
            input_path,
            extra_frames + 1
        ));
    }

    let img = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| anyhow::anyhow!("Failed to decode {}: {}", input_path, e))?;

    Ok(SourceImage {
        img,
        icc,
        exif,
        orientation,
        extra_frames,
    })
}

/// Frames past the first in `path`, and 0 for anything not animated.
///
/// The cheap header check comes first, so an ordinary single-frame PNG is never
/// decoded here only to be decoded again for its pixels.
fn count_extra_frames(path: &std::path::Path) -> Result<usize> {
    let animated = if has_extension(path, "gif") {
        true
    } else if has_extension(path, "png") {
        let file = std::io::BufReader::new(
            std::fs::File::open(path)
                .map_err(|e| anyhow::anyhow!("Failed to open {}: {}", path.display(), e))?,
        );
        image::codecs::png::PngDecoder::new(file)
            .and_then(|d| d.is_apng())
            .unwrap_or(false)
    } else if has_extension(path, "webp") {
        let file = std::io::BufReader::new(
            std::fs::File::open(path)
                .map_err(|e| anyhow::anyhow!("Failed to open {}: {}", path.display(), e))?,
        );
        image::codecs::webp::WebPDecoder::new(file)
            .map(|d| d.has_animation())
            .unwrap_or(false)
    } else {
        false
    };

    if !animated {
        return Ok(0);
    }
    match read_pixels(path)? {
        Pixels::Frames(frames) => Ok(frames.len().saturating_sub(1)),
        Pixels::Still(_) => Ok(0),
    }
}

/// Fails before a capture starts when the chosen format needs a tool that is
/// not installed, so a long scroll is never lost at the save step.
pub fn validate_encoder(format: &str) -> Result<()> {
    let format_clean = format.trim_start_matches('.').to_lowercase();
    if format_clean == "jxl" && !has_jxl_tools() {
        return Err(anyhow::anyhow!(
            "{}\nOr pick another format, for example --format webp.",
            JXL_TOOLS_HINT
        ));
    }
    Ok(())
}

/// The tallest part [`save_image`] will write for this output path, or `None`
/// when the format takes the image whole.
///
/// For WebP this is a hard format limit. JPEG XL keeps a chapter in one file:
/// the archive is one image per chapter, and the WebP reading copy made from it
/// is where the 16383px page split happens.
///
/// JXL's own limit is far enough away to leave unguarded. Level 5, the level
/// decoders support most widely, allows 2^18 = 262144 px per side and 2^28
/// pixels in total, so a capture up to 1024px wide can be 262144px tall — three
/// times the tallest one measured here. Beyond that `cjxl` does not fail: its
/// default `--codestream_level=-1` quietly writes a level 10 file (2^30 per
/// side), which some viewers refuse.
fn max_part_height(path: &std::path::Path) -> Option<u32> {
    if has_extension(path, "webp") {
        Some(WEBP_MAX_DIMENSION)
    } else {
        None
    }
}

/// Splits `height` into `parts` runs that differ by at most one row, so a tall
/// capture does not end with a sliver of a final file.
fn split_heights(height: u32, parts: u32) -> Vec<u32> {
    let base = height / parts;
    let remainder = height % parts;
    (0..parts)
        .map(|i| if i < remainder { base + 1 } else { base })
        .collect()
}

/// What a source carried beside its pixels, to be embedded wherever the target
/// format can hold it.
#[derive(Default, Clone, Copy)]
struct Metadata<'a> {
    icc: Option<&'a [u8]>,
    exif: Option<&'a [u8]>,
}

impl Metadata<'_> {
    fn is_empty(&self) -> bool {
        self.icc.is_none() && self.exif.is_none()
    }
}

/// Writes raw samples through the encoder for `path`, embedding what `meta`
/// carries and the format accepts.
///
/// `set_icc_profile` and `set_exif_metadata` are only reachable on an encoder
/// built by hand, which is why this exists at all: `DynamicImage::save` builds
/// one internally and gives no way in. A format that cannot hold a piece of
/// metadata is not an error here — [`SourceImage::losses_for`] is what reports
/// that, before anything gets deleted.
fn encode_with_metadata(
    path: &str,
    bytes: &[u8],
    width: u32,
    height: u32,
    color: image::ExtendedColorType,
    meta: Metadata,
) -> Result<()> {
    use image::ImageEncoder;

    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if !format_holds_metadata(path) {
        return Err(anyhow::anyhow!(
            "{} cannot carry metadata, and should not have been asked to",
            path
        ));
    }

    // Created only once the format is known, so an unsupported one leaves no
    // empty file behind. TiffEncoder wants Seek, which BufWriter<File> has.
    let file = std::fs::File::create(path)
        .map_err(|e| anyhow::anyhow!("Failed to create {}: {}", path, e))?;
    let out = std::io::BufWriter::new(file);
    let fail = |e: image::ImageError| anyhow::anyhow!("Failed to save {}: {}", path, e);

    // Each encoder takes `self` by value in `write_image`, so this cannot be a
    // trait object and every arm has to finish the write itself. A format that
    // cannot hold one of these is a no-op, not an error: refusing is
    // [`SourceImage::losses_for`]'s job, before anything is deleted.
    macro_rules! encode {
        ($encoder:expr) => {{
            let mut encoder = $encoder;
            if let Some(icc) = meta.icc {
                let _ = encoder.set_icc_profile(icc.to_vec());
            }
            if let Some(exif) = meta.exif {
                let _ = encoder.set_exif_metadata(exif.to_vec());
            }
            encoder
                .write_image(bytes, width, height, color)
                .map_err(fail)
        }};
    }

    match ext.as_str() {
        "png" => encode!(image::codecs::png::PngEncoder::new(out)),
        "webp" => encode!(image::codecs::webp::WebPEncoder::new_lossless(out)),
        "jpg" | "jpeg" => encode!(image::codecs::jpeg::JpegEncoder::new(out)),
        "tiff" | "tif" => encode!(image::codecs::tiff::TiffEncoder::new(out)),
        // Unreachable: format_holds_metadata covers exactly the arms above.
        _ => Err(anyhow::anyhow!("No encoder for {}", path)),
    }
}

/// Writes a single image, routing `.jxl` through libjxl and everything else
/// through the `image` crate.
fn write_one(img: &RgbaImage, path: &str, meta: Metadata) -> Result<()> {
    if has_extension(path, "jxl") {
        return encode_jxl(img, path);
    }
    if meta.is_empty() || !format_holds_metadata(path) {
        return img
            .save(path)
            .map_err(|e| anyhow::anyhow!("Failed to save {}: {}", path, e));
    }

    // JPEG has no alpha channel, and an encoder built by hand does not get the
    // conversion `save` would have done for us.
    if has_extension(path, "jpg") || has_extension(path, "jpeg") {
        let rgb: image::RgbImage = ImageBuffer::from_fn(img.width(), img.height(), |x, y| {
            let p = img.get_pixel(x, y).0;
            image::Rgb([p[0], p[1], p[2]])
        });
        return encode_with_metadata(
            path,
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
            meta,
        );
    }

    encode_with_metadata(
        path,
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::Rgba8,
        meta,
    )
}

/// Writes an image at the depth it already has, for the targets that can hold
/// more than 8 bits per channel.
///
/// Never splits, and does not need to: the formats that reach it (JXL through
/// its own encoder, PNG and TIFF here) all take an image of any height, and the
/// one format that does split cannot hold deep pixels anyway.
fn write_dynamic(img: &image::DynamicImage, path: &str, meta: Metadata) -> Result<()> {
    if meta.is_empty() || !format_holds_metadata(path) {
        return img
            .save(path)
            .map_err(|e| anyhow::anyhow!("Failed to save {}: {}", path, e));
    }
    encode_with_metadata(
        path,
        img.as_bytes(),
        img.width(),
        img.height(),
        img.color().into(),
        meta,
    )
}

/// True when [`encode_with_metadata`] has an encoder for `path`'s format.
fn format_holds_metadata(path: &str) -> bool {
    has_any_extension(path, &["png", "webp", "jpg", "jpeg", "tiff", "tif"])
}

/// Saves `img` to `output_path`, splitting it into numbered parts when the
/// target format cannot hold the whole image. Returns every path written, in
/// top-to-bottom order.
///
/// Both split formats are encoded losslessly, so splitting never costs any
/// quality. A failed write takes its own partial output with it, leaving no
/// half-converted set of parts on disk.
pub fn save_image(img: &RgbaImage, output_path: &str) -> Result<Vec<String>> {
    save_image_with(img, output_path, Metadata::default())
}

/// [`save_image`], embedding what a converted source carried beside its pixels.
///
/// A fresh capture has no metadata to carry, which is why [`save_image`] is the
/// whole story for the capture path.
fn save_image_with(img: &RgbaImage, output_path: &str, meta: Metadata) -> Result<Vec<String>> {
    let path = std::path::Path::new(output_path);

    // Splitting is vertical only, so an over-wide image has no fallback.
    if has_extension(path, "webp") && img.width() > WEBP_MAX_DIMENSION {
        return Err(anyhow::anyhow!(
            "Image is {}px wide, but WebP supports at most {}px. Use a narrower crop or save as JXL.",
            img.width(),
            WEBP_MAX_DIMENSION
        ));
    }

    let Some(max_height) = max_part_height(path).filter(|max| img.height() > *max) else {
        write_one(img, output_path, meta).inspect_err(|_| {
            let _ = std::fs::remove_file(output_path);
        })?;
        return Ok(vec![output_path.to_string()]);
    };

    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("Output filename cannot be empty"))?;
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("webp");

    let parts = img.height().div_ceil(max_height);
    let digits = parts.to_string().len();

    let mut written: Vec<String> = Vec::with_capacity(parts as usize);
    let mut y = 0;
    for (i, part_height) in split_heights(img.height(), parts).into_iter().enumerate() {
        let name = format!("{}_{:0>width$}.{}", stem, i + 1, ext, width = digits);
        let part_path = path.with_file_name(&name).to_string_lossy().into_owned();
        let part = image::imageops::crop_imm(img, 0, y, img.width(), part_height).to_image();
        if let Err(e) = write_one(&part, &part_path, meta) {
            let _ = std::fs::remove_file(&part_path);
            for done in &written {
                let _ = std::fs::remove_file(done);
            }
            return Err(e);
        }
        written.push(part_path);
        y += part_height;
    }

    Ok(written)
}

/// Reads back what [`save_image`] just wrote and checks it against `source`
/// pixel for pixel, stacking the parts top to bottom.
///
/// Only worth its cost before deleting an original that cannot be recaptured.
/// Parts are compared one at a time so a tall capture is never held twice.
fn verify_written_matches(source: &RgbaImage, written: &[String]) -> Result<()> {
    let stride = source.width() as usize * 4;
    let src = source.as_raw();
    let mut y = 0u32;

    for path in written {
        let part = open_image(path)?;
        if part.width() != source.width() {
            return Err(anyhow::anyhow!(
                "{} is {}px wide, but the source is {}px",
                path,
                part.width(),
                source.width()
            ));
        }
        if y + part.height() > source.height() {
            return Err(anyhow::anyhow!(
                "{} runs past the bottom of the source",
                path
            ));
        }

        let part_raw = part.as_raw();
        for row in 0..part.height() as usize {
            let at = (y as usize + row) * stride;
            let from = row * stride;
            if src[at..at + stride] != part_raw[from..from + stride] {
                return Err(anyhow::anyhow!(
                    "{} differs from the source at row {}",
                    path,
                    y as usize + row
                ));
            }
        }
        y += part.height();
    }

    if y != source.height() {
        return Err(anyhow::anyhow!(
            "Parts cover {}px, but the source is {}px tall",
            y,
            source.height()
        ));
    }
    Ok(())
}

/// What a conversion actually proved, so the log cannot claim more than was
/// checked.
///
/// Lossless here means lossless against the **original file**, which is a higher
/// bar than matching pixels and not one every route can clear. Only a transcoded
/// JPEG can be turned back into the file it came from; for a source that was
/// lossless to begin with, the file's bytes hold no image information the pixels
/// and metadata do not, so matching those is the whole of it.
#[derive(Clone, Copy)]
enum Proof {
    /// The original file can be rebuilt from the result, byte for byte.
    FileBytes,
    /// Pixels and carried metadata match. The file's own bytes are not
    /// reproducible, since re-encoding picks its own filters and layout.
    Pixels,
}

impl Proof {
    fn describe(self) -> &'static str {
        match self {
            Proof::FileBytes => "Verified byte for byte against",
            Proof::Pixels => "Verified pixels and carried metadata against",
        }
    }
}

/// Bits per channel, which is what "did this conversion drop half the data"
/// turns on. `ColorType` spells depth and channel count together, so a 16-bit
/// grayscale and a 16-bit RGBA have to compare equal here.
fn bits_per_channel(color: image::ColorType) -> u16 {
    color.bits_per_pixel() / u16::from(color.channel_count())
}

/// What an image file holds, kept apart because an animation has to be compared
/// frame by frame and a still image has to keep its own bit depth.
enum Pixels {
    Still(image::DynamicImage),
    /// RGBA8 because that is all `AnimationDecoder` hands back. Nothing this
    /// program reads stores an animation deeper than 8 bits.
    Frames(Vec<RgbaImage>),
}

/// Reads `path` as pixels, picking up every frame when it is animated.
///
/// `image::open` silently keeps only the first frame, which is exactly the loss
/// this has to be able to see.
fn read_pixels(path: &std::path::Path) -> Result<Pixels> {
    use image::AnimationDecoder;

    let open = |p: &std::path::Path| -> Result<std::io::BufReader<std::fs::File>> {
        Ok(std::io::BufReader::new(std::fs::File::open(p).map_err(
            |e| anyhow::anyhow!("Failed to open {}: {}", p.display(), e),
        )?))
    };
    let frames = |f: image::Frames| -> Result<Vec<RgbaImage>> {
        Ok(f.collect_frames()
            .map_err(|e| anyhow::anyhow!("Failed to read frames of {}: {}", path.display(), e))?
            .into_iter()
            .map(|frame| frame.into_buffer())
            .collect())
    };
    let bad = |e: image::ImageError| anyhow::anyhow!("Failed to read {}: {}", path.display(), e);

    if has_extension(path, "gif") {
        let decoder = image::codecs::gif::GifDecoder::new(open(path)?).map_err(bad)?;
        return Ok(Pixels::Frames(frames(decoder.into_frames())?));
    }
    if has_extension(path, "png") {
        let decoder = image::codecs::png::PngDecoder::new(open(path)?).map_err(bad)?;
        if decoder.is_apng().map_err(bad)? {
            let apng = decoder.apng().map_err(bad)?;
            return Ok(Pixels::Frames(frames(apng.into_frames())?));
        }
        return Ok(Pixels::Still(
            image::DynamicImage::from_decoder(decoder).map_err(bad)?,
        ));
    }
    if has_extension(path, "webp") {
        let decoder = image::codecs::webp::WebPDecoder::new(open(path)?).map_err(bad)?;
        if decoder.has_animation() {
            return Ok(Pixels::Frames(frames(decoder.into_frames())?));
        }
        return Ok(Pixels::Still(
            image::DynamicImage::from_decoder(decoder).map_err(bad)?,
        ));
    }
    if has_extension(path, "jxl") {
        return Ok(Pixels::Still(image::DynamicImage::ImageRgba8(decode_jxl(
            &path.to_string_lossy(),
        )?)));
    }

    Ok(Pixels::Still(image::open(path).map_err(bad)?))
}

/// Checks two images hold the same pixels at the same depth.
///
/// Channel layout is allowed to differ — a grayscale image that comes back as
/// gray written into three channels lost nothing, and a decoder is free to make
/// that choice. Bit depth is not, since dropping from 16 bits to 8 is the silent
/// loss worth failing over.
fn dynamic_match(
    a: &image::DynamicImage,
    b: &image::DynamicImage,
    a_name: &str,
    b_name: &str,
) -> Result<()> {
    let (a_bits, b_bits) = (bits_per_channel(a.color()), bits_per_channel(b.color()));
    if a_bits != b_bits {
        return Err(anyhow::anyhow!(
            "{} holds {} bits per channel, but {} came back with {}",
            a_name,
            a_bits,
            b_name,
            b_bits
        ));
    }
    if (a.width(), a.height()) != (b.width(), b.height()) {
        return Err(anyhow::anyhow!(
            "{} is {}x{}, but {} came back {}x{}",
            a_name,
            a.width(),
            a.height(),
            b_name,
            b.width(),
            b.height()
        ));
    }
    let same = if a_bits > 8 {
        a.to_rgba16() == b.to_rgba16()
    } else {
        a.to_rgba8() == b.to_rgba8()
    };
    if !same {
        return Err(anyhow::anyhow!(
            "{} does not hold the same pixels as {}",
            b_name,
            a_name
        ));
    }
    Ok(())
}

/// Checks that `converted` holds every pixel `source` did: at the same bit
/// depth, and frame for frame when either is animated.
///
/// Channel layout is allowed to differ — a grayscale source that comes back as
/// gray written into three channels lost nothing, and a decoder is free to make
/// that choice. Bit depth is not allowed to differ, since dropping from 16 bits
/// to 8 is the silent loss worth failing over.
fn pixels_match(source: &std::path::Path, converted: &std::path::Path) -> Result<()> {
    match (read_pixels(source)?, read_pixels(converted)?) {
        (Pixels::Still(a), Pixels::Still(b)) => dynamic_match(
            &a,
            &b,
            &source.display().to_string(),
            &converted.display().to_string(),
        ),
        (Pixels::Frames(a), Pixels::Frames(b)) => {
            if a.len() != b.len() {
                return Err(anyhow::anyhow!(
                    "{} has {} frames, but {} came back with {}",
                    source.display(),
                    a.len(),
                    converted.display(),
                    b.len()
                ));
            }
            for (i, (x, y)) in a.iter().zip(&b).enumerate() {
                if x != y {
                    return Err(anyhow::anyhow!(
                        "{} differs from {} at frame {}",
                        converted.display(),
                        source.display(),
                        i + 1
                    ));
                }
            }
            Ok(())
        }
        (Pixels::Frames(a), Pixels::Still(_)) => Err(anyhow::anyhow!(
            "{} has {} frames, but {} came back as a single image",
            source.display(),
            a.len(),
            converted.display()
        )),
        (Pixels::Still(_), Pixels::Frames(b)) => Err(anyhow::anyhow!(
            "{} is a single image, but {} came back with {} frames",
            source.display(),
            converted.display(),
            b.len()
        )),
    }
}

/// Proves that the JXL cjxl wrote straight from `source` gives `source` back.
///
/// A transcoded JPEG can be reconstructed as the original file, so that is
/// compared byte for byte — a stronger claim than any pixel check, and the one
/// worth making before deleting a JPEG. Everything else is compared as pixels
/// at its own depth, animation included.
fn verify_jxl_matches_source(source: &std::path::Path, jxl: &str) -> Result<Proof> {
    let jpeg_source = has_extension(source, "jpg") || has_extension(source, "jpeg");
    let temp = TempFile::new(if jpeg_source { "jpg" } else { "png" });

    let result = std::process::Command::new("djxl")
        .arg(jxl)
        .arg(temp.path())
        .arg("--quiet")
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run djxl: {}\n{}", e, JXL_TOOLS_HINT))?;
    if !result.status.success() {
        return Err(anyhow::anyhow!(
            "djxl could not read {} back: {}",
            jxl,
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }

    if !jpeg_source {
        pixels_match(source, temp.path())?;
        return Ok(Proof::Pixels);
    }

    let original = std::fs::read(source)
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", source.display(), e))?;
    let restored = std::fs::read(temp.path())
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", temp.path().display(), e))?;
    if original != restored {
        return Err(anyhow::anyhow!(
            "{} does not reconstruct {} byte for byte",
            jxl,
            source.display()
        ));
    }
    Ok(Proof::FileBytes)
}

/// Proves a converted file holds the pixels the conversion started from.
///
/// The reference is the image in hand, not the file on disk: the pixels may have
/// been rotated to bake in an Exif orientation the target cannot carry, in which
/// case the source file no longer says what the output should look like.
fn verify_converted(
    source: &SourceImage,
    written: &[String],
    target_ext: &str,
    source_name: &str,
) -> Result<Proof> {
    if target_ext == "jxl" {
        let temp = TempFile::new("png");
        let result = std::process::Command::new("djxl")
            .arg(&written[0])
            .arg(temp.path())
            .arg("--quiet")
            .output()
            .map_err(|e| anyhow::anyhow!("Failed to run djxl: {}\n{}", e, JXL_TOOLS_HINT))?;
        if !result.status.success() {
            return Err(anyhow::anyhow!(
                "djxl could not read {} back: {}",
                written[0],
                String::from_utf8_lossy(&result.stderr).trim()
            ));
        }
        let got = image::open(temp.path())
            .map_err(|e| anyhow::anyhow!("Failed to read {} back: {}", written[0], e))?;
        dynamic_match(&source.img, &got, source_name, &written[0])?;
        return Ok(Proof::Pixels);
    }

    if written.len() == 1 {
        let got = image::open(&written[0])
            .map_err(|e| anyhow::anyhow!("Failed to read {} back: {}", written[0], e))?;
        dynamic_match(&source.img, &got, source_name, &written[0])?;
        return Ok(Proof::Pixels);
    }

    // Only WebP splits, and it is 8-bit, so the stacked comparison loses nothing.
    match &source.img {
        image::DynamicImage::ImageRgba8(img) => verify_written_matches(img, written),
        other => verify_written_matches(&other.to_rgba8(), written),
    }?;
    Ok(Proof::Pixels)
}

/// [`save_image`], but a failed JXL encode falls back to lossless WebP rather
/// than losing the image.
///
/// Only for saving a fresh capture: redoing a long scroll is expensive, while
/// every other caller still has its source file on disk and is better served by
/// a plain error. Converting `a.webp` to JXL through this would overwrite the
/// source with its own fallback.
pub fn save_image_or_webp(img: &RgbaImage, output_path: &str) -> Result<Vec<String>> {
    let err = match save_image(img, output_path) {
        Ok(written) => return Ok(written),
        Err(e) if has_extension(output_path, "jxl") => e,
        Err(e) => return Err(e),
    };

    let fallback = std::path::Path::new(output_path).with_extension("webp");
    if fallback.exists() {
        return Err(anyhow::anyhow!(
            "{}\nCannot fall back to WebP: {} already exists.",
            err,
            fallback.display()
        ));
    }

    log::warn!("{}", err);
    log::warn!("Falling back to lossless WebP: {}", fallback.display());
    save_image(img, &fallback.to_string_lossy())
}

/// Extensions the folder-wide tools accept as input. GIF is here because cjxl
/// takes a GIF file whole, animation included, so a folder pass no longer has a
/// reason to walk past one.
const IMAGE_EXTENSIONS: &[&str] = &[
    "jxl", "png", "jpg", "jpeg", "webp", "gif", "bmp", "tiff", "tif",
];

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

/// The stem and part number of a `<stem>_<n>.<ext>` file, named the way
/// [`save_image`] numbers the parts of one split image. `None` for every other
/// name.
fn part_number(path: &std::path::Path) -> Option<(String, u32)> {
    let (stem, number) = path.file_stem()?.to_str()?.rsplit_once('_')?;
    if stem.is_empty() || number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((stem.to_string(), number.parse().ok()?))
}

/// True when `format` holds an image of any height, so parts that only exist
/// because of a shorter format's limit are worth stacking back together.
fn format_takes_whole_image(format: &str) -> bool {
    max_part_height(std::path::Path::new(&build_output_path("x", format))).is_none()
}

/// Every part of the split image `path` belongs to, top to bottom, along with
/// the stem they share.
///
/// `None` unless the folder holds a gapless `<stem>_1` .. `<stem>_N` run of two
/// or more files with the same extension, which is exactly what [`save_image`]
/// writes. A lone `photo_7.png`, or a `photo_1.png` with no `photo_2.png`, is
/// an ordinary file that happens to end in a number.
fn sibling_parts(path: &std::path::Path) -> Option<(String, Vec<std::path::PathBuf>)> {
    let (stem, _) = part_number(path)?;
    let ext = path.extension()?.to_str()?.to_lowercase();
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => std::path::Path::new("."),
    };

    let mut parts: Vec<(u32, std::path::PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && has_extension(p, &ext))
        .filter_map(|p| {
            part_number(&p)
                .filter(|(s, _)| *s == stem)
                .map(|(_, n)| (n, p))
        })
        .collect();

    parts.sort();
    if parts.len() < 2 || parts.iter().map(|(n, _)| *n).ne(1..=parts.len() as u32) {
        return None;
    }
    Some((stem, parts.into_iter().map(|(_, p)| p).collect()))
}

/// Stacks `parts` back into the one image they were cut from.
///
/// `Ok(None)` when their widths disagree: a split never changes the width, so
/// those files were never one image and each belongs in its own output. The
/// parts are appended row by row and dropped as they go, so only the merged
/// image and the part being read are ever held at once.
fn merge_parts(parts: &[std::path::PathBuf]) -> Result<Option<RgbaImage>> {
    let mut width: Option<u32> = None;
    let mut height: u32 = 0;
    let mut rows: Vec<u8> = Vec::new();

    for path in parts {
        let part = open_image(&path.to_string_lossy())?;
        match width {
            None => {
                width = Some(part.width());
                rows.reserve(part.as_raw().len() * parts.len());
            }
            Some(w) if w != part.width() => return Ok(None),
            Some(_) => {}
        }
        height = height
            .checked_add(part.height())
            .ok_or_else(|| anyhow::anyhow!("Merged image would be too tall to hold"))?;
        rows.extend_from_slice(part.as_raw());
    }

    let width = width.ok_or_else(|| anyhow::anyhow!("No parts to merge"))?;
    RgbaImage::from_raw(width, height, rows)
        .map(Some)
        .ok_or_else(|| anyhow::anyhow!("Merged parts do not add up to a {}px wide image", width))
}

/// Comma-separated paths, for a log line about a whole set of inputs at once.
fn join_paths(paths: &[std::path::PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>()
        .join(", ")
}

/// What one pass of [`ScreenCapture::convert_group`] did.
enum Converted {
    /// Every path written, in top-to-bottom order.
    Written(Vec<String>),
    /// Already in the target format, so there was nothing to do.
    AlreadyTarget,
    /// The inputs turned out not to be parts of one image after all, and are
    /// better converted one by one.
    NotOneImage,
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
        let img = open_image(input_path)?;

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
    /// When `input_path` is one part of a `<stem>_1` .. `<stem>_N` run and the
    /// target format has no height limit, the whole run is stacked back into a
    /// single `<stem>.<format>` instead — including a run already in the target
    /// format, since `ch01_1.jxl` plus `ch01_2.jxl` is still one chapter in two
    /// files.
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

        if format_takes_whole_image(&target_ext) {
            if let Some((stem, parts)) = sibling_parts(path) {
                if let Converted::Written(written) =
                    self.convert_group(&parts, Some(&stem), &target_ext, delete_original)?
                {
                    return Ok(Some(written));
                }
            }
        }

        match self.convert_group(&[path.to_path_buf()], None, &target_ext, delete_original)? {
            Converted::Written(written) => Ok(Some(written)),
            _ => Ok(None),
        }
    }

    /// Converts one image, or the parts of one split image, to `target_ext`.
    ///
    /// `merged_stem` is the name the parts share, and is what makes this a
    /// merge: with it the inputs are stacked into a single image and the
    /// already-in-target-format skip does not apply, since merging N files into
    /// one is work even when the format stays the same.
    fn convert_group(
        &self,
        inputs: &[std::path::PathBuf],
        merged_stem: Option<&str>,
        target_ext: &str,
        delete_original: bool,
    ) -> Result<Converted> {
        let first = inputs
            .first()
            .ok_or_else(|| anyhow::anyhow!("Nothing to convert"))?;
        let first_path = first.to_string_lossy().into_owned();

        if merged_stem.is_none() && has_extension(first, target_ext) {
            self.log(
                log::Level::Info,
                &format!("Already {}: {} — skipped", target_ext, first_path),
            );
            return Ok(Converted::AlreadyTarget);
        }

        let stem = match merged_stem {
            Some(stem) => stem,
            None => first
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| anyhow::anyhow!("Input filename cannot be empty"))?,
        };
        let output_path = first
            .with_file_name(build_output_path(stem, target_ext))
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

        // Anything cjxl reads itself is handed over as a file. Decoding it first
        // is what makes a JPEG grow, and what drops bit depth, metadata and
        // frames on everything else. Merged parts cannot go this way: stacking
        // them needs the pixels.
        if merged_stem.is_none()
            && target_ext == "jxl"
            && has_any_extension(first, CJXL_DIRECT_INPUT)
        {
            let size = match image::image_dimensions(first) {
                Ok((w, h)) => format!(" ({}x{})", w, h),
                Err(_) => String::new(),
            };
            self.log(
                log::Level::Info,
                &format!("Handing {}{} to cjxl as it is", first_path, size),
            );
            match encode_jxl_from_file(first, &output_path) {
                Ok(()) => {
                    self.log(log::Level::Info, &format!("Saved: {}", output_path));
                    if delete_original {
                        let proof = verify_jxl_matches_source(first, &output_path);
                        self.delete_originals(inputs, std::slice::from_ref(&output_path), proof)?;
                    }
                    return Ok(Converted::Written(vec![output_path]));
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&output_path);
                    self.log(
                        log::Level::Warn,
                        &format!(
                            "cjxl would not take {} as it is, so its pixels are being re-encoded instead, which may make the result larger than the source: {}",
                            first_path, e
                        ),
                    );
                }
            }
        }

        if merged_stem.is_some() {
            self.log(
                log::Level::Info,
                &format!(
                    "Merging {} parts of {} into {}",
                    inputs.len(),
                    stem,
                    output_path
                ),
            );
            let Some(img) = merge_parts(inputs)? else {
                self.log(
                    log::Level::Info,
                    &format!(
                        "{}_1..{} differ in width, so they are not one split image — converting them separately",
                        stem,
                        inputs.len()
                    ),
                );
                return Ok(Converted::NotOneImage);
            };
            self.log(
                log::Level::Info,
                &format!("Merged: {}x{}", img.width(), img.height()),
            );

            let written = self.save_and_log(save_image(&img, &output_path)?, img.height());
            if delete_original {
                // The parts were written from 8-bit RGBA by this program, so
                // comparing the stacked pixels is the whole of what there is.
                let proof = verify_written_matches(&img, &written).map(|()| Proof::Pixels);
                self.delete_originals(inputs, &written, proof)?;
            }
            return Ok(Converted::Written(written));
        }

        let mut source = open_source_image(&first_path)?;
        self.log(
            log::Level::Info,
            &format!(
                "Loaded: {} ({}x{}, {} bits per channel)",
                first_path,
                source.img.width(),
                source.img.height(),
                source.bits_per_channel()
            ),
        );

        if !format_holds_exif(target_ext) && source.bake_orientation() {
            self.log(
                log::Level::Info,
                &format!(
                    "Rotated the pixels to match the Exif orientation, since {} cannot carry the tag",
                    target_ext
                ),
            );
        }

        let lost = source.losses_for(target_ext);
        for reason in &lost {
            self.log(log::Level::Warn, reason);
        }

        let height = source.img.height();
        let written =
            self.save_and_log(write_converted(&source, &output_path, target_ext)?, height);

        if delete_original {
            let proof = if lost.is_empty() {
                verify_converted(&source, &written, target_ext, &first_path)
            } else {
                Err(anyhow::anyhow!(
                    "This conversion could not carry everything the source holds:\n  {}",
                    lost.join("\n  ")
                ))
            };
            self.delete_originals(inputs, &written, proof)?;
        }

        Ok(Converted::Written(written))
    }

    /// Logs what a save produced and hands the paths back unchanged.
    fn save_and_log(&self, written: Vec<String>, height: u32) -> Vec<String> {
        if written.len() > 1 {
            self.log(
                log::Level::Info,
                &format!(
                    "Image is {}px tall, past the {}px page limit — split into {} parts",
                    height,
                    WEBP_MAX_DIMENSION,
                    written.len()
                ),
            );
        }
        self.log(log::Level::Info, &format!("Saved: {}", written.join(", ")));
        written
    }

    /// Deletes the inputs a conversion has replaced, but only if `proof` says
    /// the result really kept everything.
    ///
    /// An original capture cannot be redone, so "the encoder returned Ok" is not
    /// good enough. Each conversion path brings its own proof, because the
    /// strongest claim available differs: a transcoded JPEG can be reconstructed
    /// byte for byte, while a re-encode can only be compared as pixels.
    fn delete_originals(
        &self,
        inputs: &[std::path::PathBuf],
        written: &[String],
        proof: Result<Proof>,
    ) -> Result<()> {
        let proof = proof.map_err(|e| {
            anyhow::anyhow!(
                "{}\nKept the original. The converted file(s) are at: {}",
                e,
                written.join(", ")
            )
        })?;
        self.log(
            log::Level::Info,
            &format!("{} {}", proof.describe(), join_paths(inputs)),
        );
        for input in inputs {
            let path = input.to_string_lossy().into_owned();
            match std::fs::remove_file(input) {
                Ok(()) => self.log(log::Level::Info, &format!("Deleted original: {}", path)),
                Err(e) => self.log(
                    log::Level::Warn,
                    &format!("Converted, but failed to delete {}: {}", path, e),
                ),
            }
        }
        Ok(())
    }

    /// Converts every image directly inside `folder_path`. Returns
    /// (converted, skipped, failed), and nothing stops the run.
    ///
    /// A `<stem>_1` .. `<stem>_N` run counts as one image, and one conversion:
    /// its parts are stacked back together whenever the target format can hold
    /// them whole. Each merge is attempted at its first part, so parts that turn
    /// out not to belong together are still converted one by one further down
    /// the list.
    ///
    /// Failures are counted apart from skips because a bulk migration run with
    /// `delete_original` set needs to make a broken file obvious, not bury it
    /// alongside the files that were already in the target format.
    pub fn convert_images_in_folder(
        &self,
        folder_path: &str,
        format: &str,
        delete_original: bool,
        on_progress: impl Fn(usize, usize, &str),
    ) -> Result<(usize, usize, usize)> {
        let mut files = list_image_files(folder_path)?;
        files.retain(|p| !is_orig_backup(p));

        if files.is_empty() {
            return Ok((0, 0, 0));
        }

        self.log(
            log::Level::Info,
            &format!("Found {} image(s) to convert", files.len()),
        );

        let target_ext = format.trim_start_matches('.').to_lowercase();
        let merges = format_takes_whole_image(&target_ext);

        let mut converted_count = 0;
        let mut skipped_count = 0;
        let mut failed_count = 0;
        let mut merged: std::collections::HashSet<std::path::PathBuf> =
            std::collections::HashSet::new();
        // Runs that looked numbered but are not one image, so the rest of the
        // run does not pay for the same failed merge again.
        let mut unmergeable: std::collections::HashSet<std::path::PathBuf> =
            std::collections::HashSet::new();
        let total = files.len();

        for (i, path) in files.iter().enumerate() {
            let file_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            on_progress(i + 1, total, &file_name);

            // Already folded into an earlier part's merge.
            if merged.contains(path) {
                continue;
            }

            let path_str = path.to_string_lossy().into_owned();
            self.log(
                log::Level::Info,
                &format!("[{}/{}] {}", i + 1, total, path_str),
            );

            let group = if merges && !unmergeable.contains(path) {
                sibling_parts(path)
            } else {
                None
            };
            let result = match &group {
                Some((stem, parts)) => {
                    self.convert_group(parts, Some(stem), &target_ext, delete_original)
                }
                None => self.convert_group(
                    std::slice::from_ref(path),
                    None,
                    &target_ext,
                    delete_original,
                ),
            };

            match result {
                Ok(Converted::Written(_)) => {
                    converted_count += 1;
                    if let Some((_, parts)) = group {
                        merged.extend(parts);
                    }
                }
                Ok(Converted::AlreadyTarget) => skipped_count += 1,
                // Not one image after all, so this part converts on its own and
                // the rest of the run is left for their own turns.
                Ok(Converted::NotOneImage) => {
                    if let Some((_, parts)) = group {
                        unmergeable.extend(parts);
                    }
                    match self.convert_group(
                        std::slice::from_ref(path),
                        None,
                        &target_ext,
                        delete_original,
                    ) {
                        Ok(Converted::Written(_)) => converted_count += 1,
                        Ok(_) => skipped_count += 1,
                        Err(e) => {
                            self.log(log::Level::Warn, &format!("  → Error: {}", e));
                            failed_count += 1;
                        }
                    }
                }
                Err(e) => {
                    self.log(log::Level::Warn, &format!("  → Error: {}", e));
                    failed_count += 1;
                }
            }
        }

        Ok((converted_count, skipped_count, failed_count))
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
        let counts = ScreenCapture::new()
            .convert_images_in_folder(&folder, "webp", false, |_, _, _| {})
            .unwrap();

        assert_eq!(counts, (2, 1, 0));
        assert!(std::path::Path::new(&dir.path("a.webp")).exists());
        assert!(std::path::Path::new(&dir.path("b.webp")).exists());
    }

    /// A file that cannot be converted must not hide among the files that were
    /// simply already in the target format, or a bulk migration silently loses
    /// pages.
    #[test]
    fn convert_images_in_folder_counts_failures_separately() {
        let dir = TempDir::new("convert-folder-fail");
        noisy_image(8, 100).save(dir.path("a.png")).unwrap();
        noisy_image(8, 100).save(dir.path("b.webp")).unwrap();
        std::fs::write(dir.path("broken.png"), b"not a png at all").unwrap();

        let folder = dir.0.to_string_lossy().into_owned();
        let counts = ScreenCapture::new()
            .convert_images_in_folder(&folder, "webp", false, |_, _, _| {})
            .unwrap();

        assert_eq!(counts, (1, 1, 1));
    }

    #[test]
    fn part_number_reads_only_save_image_part_names() {
        let part = |name: &str| part_number(std::path::Path::new(name));

        assert_eq!(part("ch01_1.webp"), Some(("ch01".into(), 1)));
        assert_eq!(part("ch01_07.webp"), Some(("ch01".into(), 7)));
        assert_eq!(part("a_b_2.webp"), Some(("a_b".into(), 2)));
        assert_eq!(part("cover.webp"), None);
        assert_eq!(part("ch01_.webp"), None);
        assert_eq!(part("_1.webp"), None);
        assert_eq!(part("ch01_2b.webp"), None);
    }

    /// Two files whose names happen to end in numbers are not a split image.
    #[test]
    fn sibling_parts_needs_a_gapless_run() {
        let dir = TempDir::new("siblings");
        let img = noisy_image(4, 10);
        img.save(dir.path("ch01_1.webp")).unwrap();
        img.save(dir.path("ch01_3.webp")).unwrap();
        img.save(dir.path("solo_1.webp")).unwrap();
        img.save(dir.path("mixed_2.png")).unwrap();
        img.save(dir.path("mixed_1.webp")).unwrap();

        for name in ["ch01_1.webp", "solo_1.webp", "mixed_1.webp"] {
            let path = dir.path(name);
            assert!(
                sibling_parts(std::path::Path::new(&path)).is_none(),
                "{name} is not part of a complete run"
            );
        }
    }

    /// Parts exist only because WebP cannot hold a whole chapter, so converting
    /// them to a format that can puts the chapter back into one file.
    #[test]
    fn convert_image_merges_parts_into_one_file() {
        let dir = TempDir::new("merge-parts");
        let top = noisy_image(8, 120);
        let bottom = noisy_image(8, 80);
        top.save(dir.path("ch01_1.webp")).unwrap();
        bottom.save(dir.path("ch01_2.webp")).unwrap();

        let written = ScreenCapture::new()
            .convert_image(&dir.path("ch01_1.webp"), "png", false)
            .unwrap()
            .unwrap();

        assert_eq!(written, vec![dir.path("ch01.png")]);
        let merged = image::open(&written[0]).unwrap().to_rgba8();
        assert_eq!(merged.dimensions(), (8, 200));
        let expected: Vec<u8> = top
            .as_raw()
            .iter()
            .chain(bottom.as_raw())
            .copied()
            .collect();
        assert_eq!(merged.as_raw(), &expected, "parts must stack in order");
    }

    /// The run is found from the folder, not from which part was pointed at.
    #[test]
    fn convert_image_merges_when_pointed_at_a_later_part() {
        let dir = TempDir::new("merge-later-part");
        noisy_image(8, 40).save(dir.path("ch01_1.webp")).unwrap();
        noisy_image(8, 40).save(dir.path("ch01_2.webp")).unwrap();

        let written = ScreenCapture::new()
            .convert_image(&dir.path("ch01_2.webp"), "png", false)
            .unwrap()
            .unwrap();

        assert_eq!(written, vec![dir.path("ch01.png")]);
    }

    /// Two parts in the target format are still two files, so the merge is the
    /// work being asked for even though the extension does not change.
    #[test]
    fn convert_image_merges_parts_already_in_the_target_format() {
        let dir = TempDir::new("merge-same-format");
        noisy_image(8, 40).save(dir.path("ch01_1.png")).unwrap();
        noisy_image(8, 40).save(dir.path("ch01_2.png")).unwrap();

        let written = ScreenCapture::new()
            .convert_image(&dir.path("ch01_1.png"), "png", false)
            .unwrap()
            .unwrap();

        assert_eq!(written, vec![dir.path("ch01.png")]);
        assert_eq!(height_of(&written[0]), 80);
    }

    /// Merging into WebP would only be undone by the split that follows it.
    #[test]
    fn convert_image_does_not_merge_into_a_format_with_a_height_limit() {
        let dir = TempDir::new("merge-webp-target");
        noisy_image(8, 40).save(dir.path("ch01_1.png")).unwrap();
        noisy_image(8, 40).save(dir.path("ch01_2.png")).unwrap();

        let written = ScreenCapture::new()
            .convert_image(&dir.path("ch01_1.png"), "webp", false)
            .unwrap()
            .unwrap();

        assert_eq!(written, vec![dir.path("ch01_1.webp")]);
    }

    #[test]
    fn convert_image_leaves_a_lone_numbered_file_alone() {
        let dir = TempDir::new("merge-lone");
        noisy_image(8, 40).save(dir.path("photo_7.webp")).unwrap();

        let written = ScreenCapture::new()
            .convert_image(&dir.path("photo_7.webp"), "png", false)
            .unwrap()
            .unwrap();

        assert_eq!(written, vec![dir.path("photo_7.png")]);
    }

    /// A split keeps the width, so numbered files of different widths are an
    /// ordinary numbered set and must convert one by one.
    #[test]
    fn convert_image_falls_back_when_parts_differ_in_width() {
        let dir = TempDir::new("merge-mismatch");
        noisy_image(8, 40).save(dir.path("photo_1.webp")).unwrap();
        noisy_image(12, 40).save(dir.path("photo_2.webp")).unwrap();

        let written = ScreenCapture::new()
            .convert_image(&dir.path("photo_1.webp"), "png", false)
            .unwrap()
            .unwrap();

        assert_eq!(written, vec![dir.path("photo_1.png")]);
        assert!(!std::path::Path::new(&dir.path("photo.png")).exists());
    }

    /// The merged file stands in for every part, so none of them may survive a
    /// verified delete.
    #[test]
    fn convert_image_deletes_every_merged_part() {
        let dir = TempDir::new("merge-delete");
        noisy_image(8, 40).save(dir.path("ch01_1.webp")).unwrap();
        noisy_image(8, 60).save(dir.path("ch01_2.webp")).unwrap();

        ScreenCapture::new()
            .convert_image(&dir.path("ch01_1.webp"), "png", true)
            .unwrap()
            .unwrap();

        assert!(std::path::Path::new(&dir.path("ch01.png")).exists());
        assert!(!std::path::Path::new(&dir.path("ch01_1.webp")).exists());
        assert!(!std::path::Path::new(&dir.path("ch01_2.webp")).exists());
    }

    #[test]
    fn convert_images_in_folder_counts_a_merged_run_once() {
        let dir = TempDir::new("merge-folder");
        for part in 1..=3 {
            noisy_image(8, 40)
                .save(dir.path(&format!("ch01_{}.webp", part)))
                .unwrap();
        }
        noisy_image(8, 40).save(dir.path("cover.webp")).unwrap();

        let folder = dir.0.to_string_lossy().into_owned();
        let counts = ScreenCapture::new()
            .convert_images_in_folder(&folder, "png", false, |_, _, _| {})
            .unwrap();

        assert_eq!(counts, (2, 0, 0), "the run counts as one conversion");
        assert_eq!(height_of(&dir.path("ch01.png")), 120);
        assert!(std::path::Path::new(&dir.path("cover.png")).exists());
        assert!(!std::path::Path::new(&dir.path("ch01_1.png")).exists());
    }

    /// A failed merge must not swallow the rest of the run.
    #[test]
    fn convert_images_in_folder_converts_mismatched_parts_one_by_one() {
        let dir = TempDir::new("merge-folder-mismatch");
        noisy_image(8, 40).save(dir.path("photo_1.webp")).unwrap();
        noisy_image(12, 40).save(dir.path("photo_2.webp")).unwrap();

        let folder = dir.0.to_string_lossy().into_owned();
        let counts = ScreenCapture::new()
            .convert_images_in_folder(&folder, "png", false, |_, _, _| {})
            .unwrap();

        assert_eq!(counts, (2, 0, 0));
        assert!(std::path::Path::new(&dir.path("photo_1.png")).exists());
        assert!(std::path::Path::new(&dir.path("photo_2.png")).exists());
        assert!(!std::path::Path::new(&dir.path("photo.png")).exists());
    }

    #[test]
    fn save_image_rejects_too_wide_webp() {
        let dir = TempDir::new("wide-webp");
        let path = dir.path("out.webp");
        let img = noisy_image(WEBP_MAX_DIMENSION + 1, 4);

        let err = save_image(&img, &path).unwrap_err().to_string();

        assert!(err.contains("wide"), "unexpected error: {err}");
    }

    /// JXL goes through cjxl/djxl, so every JXL test is a no-op where those are
    /// not installed rather than a failure.
    macro_rules! needs_jxl {
        () => {
            if !has_jxl_tools() {
                eprintln!("skipped: cjxl/djxl not installed");
                return;
            }
        };
    }

    #[test]
    fn save_image_writes_short_jxl_as_one_file() {
        needs_jxl!();
        let dir = TempDir::new("short-jxl");
        let path = dir.path("out.jxl");
        let img = noisy_image(8, 500);

        let written = save_image(&img, &path).unwrap();

        assert_eq!(written, vec![path.clone()]);
        assert_eq!(open_image(&path).unwrap().dimensions(), img.dimensions());
    }

    /// JXL allows 2^30 rows, so a chapter stays one file however tall it is.
    /// Only the WebP reading copy is cut into pages.
    #[test]
    fn save_image_keeps_tall_jxl_as_one_file() {
        needs_jxl!();
        let dir = TempDir::new("tall-jxl");
        let jxl = dir.path("out.jxl");
        let webp = dir.path("out.webp");
        let img = noisy_image(8, 40_000);

        let jxl_parts = save_image(&img, &jxl).unwrap();
        let webp_parts = save_image(&img, &webp).unwrap();

        assert_eq!(jxl_parts, vec![jxl.clone()]);
        assert_eq!(open_image(&jxl).unwrap().dimensions(), img.dimensions());
        assert_eq!(webp_parts.len(), 3, "WebP still pages at its own limit");
    }

    /// The entire reason for choosing JXL here is that `-d 0` is mathematically
    /// lossless, so a tall capture must decode back to the exact source rows.
    #[test]
    fn jxl_roundtrip_is_lossless() {
        needs_jxl!();
        let dir = TempDir::new("lossless-jxl");
        let path = dir.path("out.jxl");
        let img = noisy_image(16, 20_000);

        let written = save_image(&img, &path).unwrap();
        assert_eq!(written.len(), 1);

        verify_written_matches(&img, &written).unwrap();
    }

    /// Screenshots are opaque and take the PPM path, but a capture carrying
    /// real alpha must survive too.
    #[test]
    fn jxl_preserves_alpha() {
        needs_jxl!();
        let dir = TempDir::new("alpha-jxl");
        let path = dir.path("out.jxl");
        let img = RgbaImage::from_fn(32, 32, |x, y| {
            image::Rgba([x as u8, y as u8, 0, (x * 8 % 256) as u8])
        });

        save_image(&img, &path).unwrap();

        assert_eq!(open_image(&path).unwrap().as_raw(), img.as_raw());
    }

    #[test]
    fn convert_image_png_to_jxl_is_lossless() {
        needs_jxl!();
        let dir = TempDir::new("convert-png-jxl");
        let png = dir.path("shot.png");
        let img = noisy_image(8, 200);
        img.save(&png).unwrap();

        let written = ScreenCapture::new()
            .convert_image(&png, "jxl", false)
            .unwrap()
            .unwrap();

        assert_eq!(written, vec![dir.path("shot.jxl")]);
        assert_eq!(open_image(&written[0]).unwrap().as_raw(), img.as_raw());
        assert!(std::path::Path::new(&png).exists());
    }

    /// The migration path for captures already saved as WebP, and the one that
    /// would destroy its own source if a fallback ever wrote over it.
    #[test]
    fn convert_image_webp_to_jxl_keeps_source_intact() {
        needs_jxl!();
        let dir = TempDir::new("convert-webp-jxl");
        let webp = dir.path("shot.webp");
        let img = noisy_image(8, 200);
        img.save(&webp).unwrap();
        let before = std::fs::read(&webp).unwrap();

        let written = ScreenCapture::new()
            .convert_image(&webp, "jxl", false)
            .unwrap()
            .unwrap();

        assert_eq!(written, vec![dir.path("shot.jxl")]);
        assert_eq!(open_image(&written[0]).unwrap().as_raw(), img.as_raw());
        assert_eq!(
            std::fs::read(&webp).unwrap(),
            before,
            "the source must never be rewritten by its own conversion"
        );
    }

    /// Reading copies for viewers that cannot open JXL yet.
    #[test]
    fn convert_image_jxl_to_webp_is_lossless() {
        needs_jxl!();
        let dir = TempDir::new("convert-jxl-webp");
        let jxl = dir.path("shot.jxl");
        let img = noisy_image(8, 200);
        save_image(&img, &jxl).unwrap();

        let written = ScreenCapture::new()
            .convert_image(&jxl, "webp", false)
            .unwrap()
            .unwrap();

        assert_eq!(written, vec![dir.path("shot.webp")]);
        assert_eq!(
            image::open(&written[0]).unwrap().to_rgba8().as_raw(),
            img.as_raw()
        );
    }

    #[test]
    fn convert_image_to_jxl_deletes_verified_original() {
        needs_jxl!();
        let dir = TempDir::new("convert-jxl-delete");
        let png = dir.path("shot.png");
        noisy_image(8, 200).save(&png).unwrap();

        let written = ScreenCapture::new()
            .convert_image(&png, "jxl", true)
            .unwrap()
            .unwrap();

        assert!(std::path::Path::new(&written[0]).exists());
        assert!(!std::path::Path::new(&png).exists());
    }

    /// The check that stands between a bad encode and a deleted original.
    #[test]
    fn verify_written_matches_rejects_altered_output() {
        let dir = TempDir::new("verify-mismatch");
        let path = dir.path("out.png");
        let img = noisy_image(8, 100);
        let mut altered = img.clone();
        altered.put_pixel(3, 40, image::Rgba([1, 2, 3, 255]));
        altered.save(&path).unwrap();

        let err = verify_written_matches(&img, &[path])
            .unwrap_err()
            .to_string();

        assert!(err.contains("row 40"), "unexpected error: {err}");
    }

    #[test]
    fn verify_written_matches_rejects_short_output() {
        let dir = TempDir::new("verify-short");
        let path = dir.path("out.png");
        let img = noisy_image(8, 100);
        image::imageops::crop_imm(&img, 0, 0, 8, 60)
            .to_image()
            .save(&path)
            .unwrap();

        let err = verify_written_matches(&img, &[path])
            .unwrap_err()
            .to_string();

        assert!(err.contains("60px"), "unexpected error: {err}");
    }

    /// A capture is expensive to redo, so a JXL failure must still land on disk
    /// as WebP.
    #[test]
    fn save_image_or_webp_falls_back_for_jxl() {
        let dir = TempDir::new("fallback-jxl");
        let path = dir.path("out.jxl");
        let img = noisy_image(8, 100);

        // A directory in the way makes cjxl fail without needing it uninstalled.
        std::fs::create_dir_all(&path).unwrap();

        let written = save_image_or_webp(&img, &path).unwrap();

        assert_eq!(written, vec![dir.path("out.webp")]);
        assert_eq!(
            image::open(&written[0]).unwrap().to_rgba8().as_raw(),
            img.as_raw()
        );
    }

    /// Falling back must never claim a path that already holds something.
    #[test]
    fn save_image_or_webp_refuses_to_clobber_the_fallback_path() {
        let dir = TempDir::new("fallback-clash");
        let path = dir.path("out.jxl");
        let webp = dir.path("out.webp");
        let existing = noisy_image(4, 50);
        existing.save(&webp).unwrap();
        std::fs::create_dir_all(&path).unwrap();

        let err = save_image_or_webp(&noisy_image(8, 100), &path)
            .unwrap_err()
            .to_string();

        assert!(err.contains("already exists"), "unexpected error: {err}");
        assert_eq!(
            image::open(&webp).unwrap().to_rgba8().as_raw(),
            existing.as_raw()
        );
    }

    /// Other formats must not get a silent WebP substitution.
    #[test]
    fn save_image_or_webp_does_not_fall_back_for_png() {
        let dir = TempDir::new("fallback-png");
        let path = dir.path("out.png");
        std::fs::create_dir_all(&path).unwrap();

        assert!(save_image_or_webp(&noisy_image(8, 100), &path).is_err());
        assert!(!std::path::Path::new(&dir.path("out.webp")).exists());
    }

    /// A half-written set of parts would look like a complete conversion to the
    /// next run, so a failed save takes its own output with it.
    #[test]
    fn save_image_cleans_up_partial_parts() {
        let dir = TempDir::new("partial-parts");
        let path = dir.path("out.webp");
        // Part 2 cannot be written, so part 1 must not survive either.
        std::fs::create_dir_all(dir.path("out_2.webp")).unwrap();

        assert!(save_image(&noisy_image(8, 40_000), &path).is_err());
        assert!(!std::path::Path::new(&dir.path("out_1.webp")).exists());
    }

    #[test]
    fn validate_encoder_accepts_formats_without_external_tools() {
        assert!(validate_encoder("webp").is_ok());
        assert!(validate_encoder("png").is_ok());
        assert!(validate_encoder(".PNG").is_ok());
    }

    #[test]
    fn validate_encoder_matches_tool_availability_for_jxl() {
        let result = validate_encoder("jxl");
        if has_jxl_tools() {
            assert!(result.is_ok());
        } else {
            let err = result.unwrap_err().to_string();
            assert!(err.contains("cjxl"), "unexpected error: {err}");
        }
    }

    /// A smooth image, which is what a JPEG is good at. [`noisy_image`] is the
    /// worst case for DCT compression, so a JPEG of it would be larger than the
    /// lossless encode and prove nothing about transcoding.
    fn smooth_image(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            let r = (x * 255 / width.max(1)) as u8;
            let g = (y * 255 / height.max(1)) as u8;
            image::Rgba([r, g, 128u8.wrapping_add(((x + y) / 4) as u8), 255])
        })
    }

    /// Writes a JPEG at a quality that leaves real artifacts to carry across.
    fn write_jpeg(path: &str, img: &RgbaImage, exif: Option<&[u8]>) -> RgbaImage {
        use image::{ExtendedColorType, ImageEncoder};

        let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
        let file = std::fs::File::create(path).unwrap();
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), 80);
        if let Some(exif) = exif {
            encoder.set_exif_metadata(exif.to_vec()).unwrap();
        }
        encoder
            .write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                ExtendedColorType::Rgb8,
            )
            .unwrap();
        image::open(path).unwrap().to_rgba8()
    }

    /// A minimal little-endian Exif block holding just an orientation tag.
    fn exif_orientation(value: u16) -> Vec<u8> {
        let mut exif = vec![b'I', b'I', 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00];
        exif.extend_from_slice(&1u16.to_le_bytes()); // one entry
        exif.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation
        exif.extend_from_slice(&3u16.to_le_bytes()); // SHORT
        exif.extend_from_slice(&1u32.to_le_bytes()); // one value
        exif.extend_from_slice(&value.to_le_bytes());
        exif.extend_from_slice(&[0, 0]); // padded to four bytes
        exif.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
        exif
    }

    /// Writes an animated GIF, one flat frame per shade.
    ///
    /// The frames carry a real delay on purpose: with the zero delay
    /// `Frame::new` gives, djxl folds the animation back into a single image, so
    /// a zero-delay fixture would look like frame loss that is not there.
    fn write_animated_gif(path: &str, shades: &[u8]) {
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = image::codecs::gif::GifEncoder::new(file);
        for &shade in shades {
            let frame = RgbaImage::from_pixel(32, 24, image::Rgba([shade, shade, shade, 255]));
            encoder
                .encode_frame(image::Frame::from_parts(
                    frame,
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(100, 1),
                ))
                .expect("frame");
        }
    }

    /// An ICC profile that libjxl will actually accept, which rules out one
    /// invented here. Tests needing one skip where the system has none.
    fn system_icc_profile() -> Option<Vec<u8>> {
        [
            "/System/Library/ColorSync/Profiles/Display P3.icc",
            "/System/Library/ColorSync/Profiles/AdobeRGB1998.icc",
            "/usr/share/color/icc/colord/sRGB.icc",
        ]
        .iter()
        .find_map(|path| std::fs::read(path).ok())
    }

    /// Converting a JPEG must not make it bigger. The pixel path had to
    /// re-encode every artifact the JPEG baked in, which cost several times what
    /// the JPEG spent throwing it away.
    #[test]
    fn convert_jpeg_to_jxl_beats_the_source_size() {
        needs_jxl!();
        let dir = TempDir::new("jpeg-smaller");
        let jpg = dir.path("photo.jpg");
        write_jpeg(&jpg, &smooth_image(600, 800), None);

        let written = ScreenCapture::new()
            .convert_image(&jpg, "jxl", false)
            .unwrap()
            .unwrap();

        let source = std::fs::metadata(&jpg).unwrap().len();
        let converted = std::fs::metadata(&written[0]).unwrap().len();
        assert!(
            converted < source,
            "{converted} bytes is not smaller than the {source}-byte source"
        );
    }

    /// The point of handing cjxl the file: the original JPEG comes back whole,
    /// which is a stronger claim than matching pixels.
    #[test]
    fn convert_jpeg_to_jxl_reconstructs_the_original_bytes() {
        needs_jxl!();
        let dir = TempDir::new("jpeg-bytes");
        let jpg = dir.path("photo.jpg");
        write_jpeg(&jpg, &smooth_image(320, 240), None);
        let original = std::fs::read(&jpg).unwrap();

        let written = ScreenCapture::new()
            .convert_image(&jpg, "jxl", false)
            .unwrap()
            .unwrap();

        let back = dir.path("back.jpg");
        assert!(
            std::process::Command::new("djxl")
                .args([&written[0], &back])
                .arg("--quiet")
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(std::fs::read(&back).unwrap(), original);
    }

    #[test]
    fn convert_jpeg_to_jxl_deletes_original_after_verification() {
        needs_jxl!();
        let dir = TempDir::new("jpeg-delete");
        let jpg = dir.path("photo.jpg");
        write_jpeg(&jpg, &smooth_image(200, 150), None);

        let written = ScreenCapture::new()
            .convert_image(&jpg, "jxl", true)
            .unwrap()
            .unwrap();

        assert!(!std::path::Path::new(&jpg).exists());
        assert!(std::path::Path::new(&written[0]).exists());
    }

    /// `to_rgba8` used to halve this, and the old verification compared the
    /// halved buffer, so `--delete-original` removed the only deep copy.
    #[test]
    fn convert_16bit_png_keeps_16_bits() {
        needs_jxl!();
        let dir = TempDir::new("deep-png");
        let png = dir.path("deep.png");
        let deep: image::ImageBuffer<image::Rgb<u16>, Vec<u16>> =
            image::ImageBuffer::from_fn(64, 48, |x, y| {
                image::Rgb([x as u16 * 1000, y as u16 * 1200, 40_000])
            });
        image::DynamicImage::ImageRgb16(deep).save(&png).unwrap();

        let written = ScreenCapture::new()
            .convert_image(&png, "jxl", true)
            .unwrap()
            .unwrap();

        assert!(!std::path::Path::new(&png).exists(), "original survived");
        let back = dir.path("back.png");
        assert!(
            std::process::Command::new("djxl")
                .args([&written[0], &back])
                .arg("--quiet")
                .status()
                .unwrap()
                .success()
        );
        let bits = bits_per_channel(image::open(&back).unwrap().color());
        assert_eq!(bits, 16, "came back with {bits} bits per channel");
    }

    #[test]
    fn convert_grayscale_png_stays_grayscale() {
        needs_jxl!();
        let dir = TempDir::new("gray-png");
        let png = dir.path("gray.png");
        let gray: image::GrayImage =
            image::ImageBuffer::from_fn(80, 60, |x, y| image::Luma([((x + y) % 256) as u8]));
        image::DynamicImage::ImageLuma8(gray).save(&png).unwrap();

        let written = ScreenCapture::new()
            .convert_image(&png, "jxl", false)
            .unwrap()
            .unwrap();

        let back = dir.path("back.png");
        assert!(
            std::process::Command::new("djxl")
                .args([&written[0], &back])
                .arg("--quiet")
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            image::open(&back).unwrap().color().channel_count(),
            1,
            "a single channel went in, so a single channel should come out"
        );
    }

    /// `image::open` keeps only the first frame. cjxl takes the GIF whole, so
    /// none of them are lost any more.
    #[test]
    fn convert_animated_gif_keeps_every_frame() {
        needs_jxl!();
        let dir = TempDir::new("anim-gif");
        let gif = dir.path("anim.gif");
        write_animated_gif(&gif, &[40, 120, 200]);

        let written = ScreenCapture::new()
            .convert_image(&gif, "jxl", false)
            .unwrap()
            .unwrap();

        let back = dir.path("back.png");
        assert!(
            std::process::Command::new("djxl")
                .args([&written[0], &back])
                .arg("--quiet")
                .status()
                .unwrap()
                .success()
        );
        match read_pixels(std::path::Path::new(&back)).unwrap() {
            Pixels::Frames(frames) => assert_eq!(frames.len(), 3),
            Pixels::Still(_) => panic!("the animation came back as one image"),
        }
    }

    /// A target that holds one image is told to say so rather than quietly
    /// keeping frame one.
    #[test]
    fn convert_animated_gif_to_png_refuses_to_delete() {
        let dir = TempDir::new("anim-to-png");
        let gif = dir.path("anim.gif");
        write_animated_gif(&gif, &[10, 250]);

        let err = ScreenCapture::new()
            .convert_image(&gif, "png", true)
            .unwrap_err()
            .to_string();

        assert!(err.contains("frames"), "unexpected error: {err}");
        assert!(std::path::Path::new(&gif).exists(), "original was deleted");
    }

    /// WebP is 8-bit here, so a deep source cannot survive the trip and the
    /// original has to stay.
    #[test]
    fn convert_refuses_to_delete_when_bit_depth_drops() {
        let dir = TempDir::new("depth-drop");
        let png = dir.path("deep.png");
        let deep: image::ImageBuffer<image::Rgb<u16>, Vec<u16>> =
            image::ImageBuffer::from_fn(32, 32, |x, y| {
                image::Rgb([x as u16 * 2000, y as u16 * 2000, 5])
            });
        image::DynamicImage::ImageRgb16(deep).save(&png).unwrap();

        let err = ScreenCapture::new()
            .convert_image(&png, "webp", true)
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("8 bits per channel"),
            "unexpected error: {err}"
        );
        assert!(std::path::Path::new(&png).exists(), "original was deleted");
    }

    #[test]
    fn convert_carries_icc_profile_to_jxl() {
        needs_jxl!();
        let Some(profile) = system_icc_profile() else {
            eprintln!("skipped: no system ICC profile to test with");
            return;
        };
        let dir = TempDir::new("icc-jxl");
        let png = dir.path("tagged.png");
        write_dynamic(
            &image::DynamicImage::ImageRgba8(smooth_image(48, 48)),
            &png,
            Metadata {
                icc: Some(&profile),
                exif: None,
            },
        )
        .unwrap();

        let written = ScreenCapture::new()
            .convert_image(&png, "jxl", false)
            .unwrap()
            .unwrap();

        let back = dir.path("back.png");
        assert!(
            std::process::Command::new("djxl")
                .args([&written[0], &back])
                .arg("--quiet")
                .status()
                .unwrap()
                .success()
        );
        let mut decoder = image::ImageReader::open(&back)
            .unwrap()
            .into_decoder()
            .unwrap();
        assert!(
            image::ImageDecoder::icc_profile(&mut decoder)
                .unwrap()
                .is_some(),
            "the colour profile did not survive"
        );
    }

    /// A JPEG's Exif has to reach the JXL, or the archive forgets which way up
    /// the photo goes and what its colours meant.
    #[test]
    fn convert_carries_exif_to_jxl() {
        needs_jxl!();
        let dir = TempDir::new("exif-jxl");
        let jpg = dir.path("photo.jpg");
        write_jpeg(&jpg, &smooth_image(64, 96), Some(&exif_orientation(6)));

        let written = ScreenCapture::new()
            .convert_image(&jpg, "jxl", false)
            .unwrap()
            .unwrap();

        let exif = dir.path("back.exif");
        assert!(
            std::process::Command::new("djxl")
                .args([&written[0], &exif])
                .args(["--output_format", "exif"])
                .arg("--quiet")
                .status()
                .unwrap()
                .success()
        );
        let carried = std::fs::read(&exif).unwrap();
        assert!(
            carried.windows(2).any(|w| w == 0x0112u16.to_le_bytes()),
            "the orientation tag is not in the {} bytes that came back",
            carried.len()
        );
    }

    /// TIFF cannot carry Exif through this encoder, so the orientation has to go
    /// into the pixels or the image comes out sideways.
    #[test]
    fn convert_bakes_orientation_for_formats_without_exif() {
        let dir = TempDir::new("orientation");
        let jpg = dir.path("upright.jpg");
        write_jpeg(&jpg, &smooth_image(40, 80), Some(&exif_orientation(6)));

        let written = ScreenCapture::new()
            .convert_image(&jpg, "tiff", false)
            .unwrap()
            .unwrap();

        let out = image::open(&written[0]).unwrap();
        assert_eq!(
            (out.width(), out.height()),
            (80, 40),
            "Rotate90 should have swapped the sides"
        );
    }

    /// Frames that cannot be carried anywhere are worth an error, not a silent
    /// loss. Needs a tool that writes animated WebP, since `image` cannot.
    #[test]
    fn convert_refuses_animated_webp() {
        if std::process::Command::new("img2webp")
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| !s.success())
            .unwrap_or(true)
        {
            eprintln!("skipped: img2webp not installed");
            return;
        }
        let dir = TempDir::new("anim-webp");
        let mut frames = Vec::new();
        for (i, shade) in [30u8, 200].into_iter().enumerate() {
            let path = dir.path(&format!("f{i}.png"));
            RgbaImage::from_pixel(16, 16, image::Rgba([shade, shade, shade, 255]))
                .save(&path)
                .unwrap();
            frames.push(path);
        }
        let webp = dir.path("anim.webp");
        assert!(
            std::process::Command::new("img2webp")
                .args(["-lossless", "-o", &webp])
                .args(&frames)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        );

        let err = ScreenCapture::new()
            .convert_image(&webp, "jxl", false)
            .unwrap_err()
            .to_string();

        assert!(err.contains("animated WebP"), "unexpected error: {err}");
    }
}
