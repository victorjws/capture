# Screen Scroll Capture Tool

A powerful screen capture tool that automatically scrolls and stitches screenshots together to create long captures of scrollable content.

## Features

- 🎬 **Video Mode**: Records video while auto-scrolling, then extracts and stitches frames
- 📸 **Screenshot Mode** (Default): Takes individual screenshots while scrolling
- 🖱️ **Interactive Region Selection**: Visually select capture area with mouse
- ✂️ **Flexible Cropping**: Crop specific regions or capture focused windows
- 🎯 **Preset Support**: Save and reuse crop configurations
- 🎨 **GUI & CLI**: Use graphical interface or command line
- ⏹️ **Stop Anytime**: Cancel capture in progress from GUI
- 🗜️ **Lossless JPEG XL**: default output, about 30% smaller than lossless WebP
- 🌏 **Unicode Support**: Korean font bundled into the binary, no setup required

## Installation

```bash
cargo build --release
```

This will create two executables:
- `capture` - Command-line interface (with console window)
- `capture-gui` - Graphical interface (no console window)

### libjxl (required for the default format)

Captures are saved as JPEG XL, which is encoded by the libjxl command line
tools. Install them once:

```bash
brew install jpeg-xl        # macOS
apt install libjxl-tools    # Debian/Ubuntu
```

Without them, `--format jxl` refuses to start rather than failing after a long
scroll. Every other format works with no extra setup, so `--format webp` is
always available as a fallback.

## Usage

### GUI Mode (No Console Window)

Simply run the GUI executable:

```bash
./target/release/capture-gui
```

Or from CLI:

```bash
cargo run --bin capture-gui
```

**GUI Features:**
- Visual configuration of all capture settings
- Real-time status updates during capture
- **Stop capture anytime** with Stop button
- Crop preset selector with dropdown
- **Convert tab**: re-encode existing captures (e.g. old PNGs or WebPs) to JXL
- Equivalent CLI command generator
- Copy settings to clipboard
- Bundled Unicode font (Korean works out of the box)

### CLI Mode

```bash
# Video mode (recommended)
./target/release/capture --video --duration 15 --output result

# Screenshot mode
./target/release/capture --max-scrolls 10 --output result

# Save as WebP instead of the default JXL
./target/release/capture --max-scrolls 10 --output result --format webp

# Interactive region selection
./target/release/capture --select-region

# Use preset crop regions
./target/release/capture --crop-preset 1080p --video

# Custom crop region
./target/release/capture --crop "100,100,1920,1080" --video

# Capture focused window only
./target/release/capture --window-only --video
```

### Common Options

```
--gui                    Launch GUI mode (from capture binary)
--video                  Use video recording mode (recommended)
--duration <SECONDS>     Video recording duration [default: 10]
--fps <FPS>              Frames to extract per second [default: 2]
--overlap <PIXELS>       Overlap for stitching [default: 125, Linux: 118]
--delay <SECONDS>        Delay before starting [default: 3]
--key <KEY>              Scroll key: space, down, pagedown [default: space]
--output <NAME>          Output filename without extension [default: 00]
--format <FORMAT>        jxl, webp, png, jpg, jpeg, gif, bmp, tiff, tif [default: jxl]
--convert <PATH>         Convert an existing image or folder to --format
--delete-original        Delete the source after --convert proves it lossless
```

### Output Format

The default is **JPEG XL**, encoded mathematically lossless (`cjxl -d 0`). On a
690x85231 capture it comes out around **30% smaller than lossless WebP** and
half the size of PNG, with pixels identical to the source.

WebP is the best-supported fallback and is also always lossless here — the
`image` crate's encoder has no lossy mode. Both are written the same way, so you
can move captures between the two formats at any time without losing anything.

Captures are split into evenly sized, numbered parts at **16383px** tall:

```
00_1.jxl  00_2.jxl  00_3.jxl  ...
```

For WebP that is a hard format limit: it stores dimensions in 14 bits. JXL has
no such limit, but it is cut at the same rows on purpose — a CBZ page tens of
thousands of pixels tall is unreadable in every viewer, and matching cut points
keep a JXL archive and a WebP copy made from it part for part identical.

Part numbers are zero-padded when there are ten or more, so they stay in order.
A WebP wider than 16383px cannot be split vertically and is rejected — use a
narrower crop or `--format jxl`. PNG and JXL are never split by width.

If a JXL encode fails mid-capture, the image is saved as lossless WebP instead
with a warning, rather than losing a scroll that took minutes. Conversions do
not fall back, since their source file is still on disk.

### Building a CBZ

A CBZ is just a ZIP of page images, so the parts go straight in:

```bash
zip -0 chapter.cbz 00_*.jxl
```

Viewer support for JXL is still uneven. Mihon reads it natively, as do macOS
Preview and other desktop viewers; Kavita does not yet. Keep the archive in JXL
and generate a reading copy when you need one — it costs nothing, both formats
being lossless:

```bash
./target/release/capture --convert ./chapter --format webp
```

### Converting Existing Captures

Captures saved before JXL became the default can be re-encoded with the exact
same rules — lossless, and split at the same 16383px boundary:

```bash
# One file, original kept
./target/release/capture --convert old.png

# Every image in a folder (subfolders untouched), originals deleted after
./target/release/capture --convert ./captures --delete-original
```

The new file is written next to the original under the same name, using
`--format` (JXL by default). Files already in the target format are skipped,
`<name>_orig.*` backups from `--fix` are left alone, and an existing file with
the target name is never overwritten.

`--delete-original` reads the new file back and compares it to the source pixel
for pixel first; anything that does not match keeps its original and is reported
as a failure. A folder run counts converted, skipped and failed files
separately, so a broken page cannot hide among the ones that were already in the
target format. The GUI offers the same thing in the **Convert** tab.

### Capture Loop Delays

A scroll-and-capture cycle has two waits, one either side of the screenshot.
Both are in milliseconds and both are tunable from the CLI and the GUI:

```
--scroll-delay <MS>         Between the scroll keypress and the screenshot,
                            giving the page time to render [default: 700]
--post-capture-delay <MS>   Between the screenshot and the next scroll
                            keypress [default: 800]
```

With the defaults a cycle costs 1500ms plus the time to take and compare the
screenshot. Raise `--scroll-delay` for pages that load slowly; lower
`--post-capture-delay` to speed up a capture.

In CLI mode `--post-capture-delay` also serves as the window in which pressing
Q stops the capture, so dropping it to 0 makes the run harder to interrupt. GUI
mode has a Stop button and does not read the keyboard, so it is a plain sleep
there.

### Crop Presets

List available presets:
```bash
./target/release/capture --list-presets
```

Save a custom preset:
```bash
./target/release/capture --save-preset mypreset:100,50,1920,1080
```

Use a preset:
```bash
./target/release/capture --crop-preset mypreset --video
```

**Built-in presets:**
- `1080p` - 1920x1080 full HD
- `720p` - 1280x720 HD
- `4k` - 3840x2160 ultra HD
- `naver-series` - 690x1007 Naver Series reader pane
- `naver-wide` - 1080x1007 wide Naver reader pane (same rows, wider column)
- `vm-small`, `vm-medium`, `vm-large` - Common VM window sizes

## Unicode Font Support

Two fonts are compiled into the binary, so Korean text and symbols render on any machine
with no font installation:

- **NanumGothic** - Hangul
- **Noto Sans Symbols** - symbols the default fonts lack

Both are registered as fallbacks: the default fonts still handle Latin first.

### Using a different font

To override the primary font, either:

- Load it at runtime from the **Settings** tab (Browse, or drag a `.ttf`/`.otf`/`.ttc` onto
  the field, then click **Load Font**), or
- Place a file named `NotoSansKR-Regular.ttf` in one of these locations, which is picked up
  automatically at startup:
  - `assets/NotoSansKR-Regular.ttf`
  - `NotoSansKR-Regular.ttf` (current directory)
  - `~/.config/capture/NotoSansKR-Regular.ttf` (Linux/macOS)
  - `%USERPROFILE%\.config\capture\NotoSansKR-Regular.ttf` (Windows)

An override takes top priority; the bundled fonts stay behind it as fallbacks, so Korean
keeps rendering even if the override lacks Hangul.

### Font licenses

NanumGothic and Noto Sans Symbols are both distributed under the
[SIL Open Font License 1.1](https://scripts.sil.org/OFL).

## Platform Support

### macOS
- Requires Accessibility permissions for keyboard simulation
- Uses AVFoundation for video recording
- Interactive region selection with AppleScript

### Windows
- Uses GDI for screen capture
- Uses Windows Magnifier for region selection
- Supports focused window detection

### Linux
- Full-screen capture and `--crop 'x,y,width,height'` work
- Interactive region selection and `--window-only` are not implemented; both
  report an error telling you to pass `--crop` instead

Cross-compile with [`cross`](https://github.com/cross-rs/cross), which installs the
required X11/Wayland development packages per `Cross.toml`:

```bash
cross build --release --target x86_64-unknown-linux-gnu
```

Building natively on Linux needs those same packages:

```bash
sudo apt-get install pkg-config libxcb1-dev libxcb-render0-dev libxcb-shape0-dev \
    libxcb-xfixes0-dev libxkbcommon-dev libdbus-1-dev libwayland-dev libxdo-dev \
    libegl1-mesa-dev libgbm-dev libdrm-dev libpipewire-0.3-dev clang
```

The EGL, GBM, DRM and PipeWire packages are what `xcap` links against for
Wayland screen capture.

## Requirements

- Rust 1.70+
- ffmpeg (for video mode)
- System permissions:
  - macOS: Accessibility, Screen Recording
  - Windows: No special permissions needed

## Tips

1. **Video mode is recommended** - More reliable and faster than screenshot mode
2. **Adjust overlap** if you see artifacts in the stitched image
3. **Use interactive region selection** (`--select-region`) to find exact coordinates
4. **Save frequently used regions** as presets for quick access
5. **GUI mode** is perfect for occasional use and experimenting with settings
6. **CLI mode** is ideal for automation and scripts

## Examples

### Capture a long webpage
```bash
./target/release/capture-gui
# Then configure in GUI and click "Start Capture"
```

### Automated capture with specific settings
```bash
./target/release/capture --video --duration 20 --fps 3 \
  --crop-preset 1080p --output webpage
```

### Capture focused window
```bash
./target/release/capture --window-only --video --duration 15
```

## Troubleshooting

### macOS: Permission errors
- Go to System Settings > Privacy & Security > Accessibility
- Add Terminal or your terminal app to the list

### Video mode not working
- Make sure `ffmpeg` is installed: `brew install ffmpeg` (macOS) or download from ffmpeg.org

### Stitching artifacts
- Increase `--overlap` value (try 150-200)
- Reduce `--fps` in video mode (try 1-2)
- Use slower scroll key (try `pagedown` instead of `space`)

## License

[GNU Affero General Public License v3.0](LICENSE) (`AGPL-3.0-only`).

The bundled fonts under `assets/` are not covered by the AGPL; see
[Font licenses](#font-licenses).
