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
- 🌏 **Unicode Support**: Korean font bundled into the binary, no setup required

## Installation

```bash
cargo build --release
```

This will create two executables:
- `capture` - Command-line interface (with console window)
- `capture-gui` - Graphical interface (no console window)

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
- Equivalent CLI command generator
- Copy settings to clipboard
- Bundled Unicode font (Korean works out of the box)

### CLI Mode

```bash
# Video mode (recommended)
./target/release/capture --video --duration 15 --output result.png

# Screenshot mode
./target/release/capture --max-scrolls 10 --output result.png

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
--overlap <PIXELS>       Overlap for stitching [default: 125]
--delay <SECONDS>        Delay before starting [default: 3]
--key <KEY>              Scroll key: space, down, pagedown [default: space]
--output <FILE>          Output file path [default: scroll_capture.png]
```

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
    libxcb-xfixes0-dev libxkbcommon-dev libdbus-1-dev libwayland-dev libxdo-dev
```

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
  --crop-preset 1080p --output webpage.png
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

MIT
