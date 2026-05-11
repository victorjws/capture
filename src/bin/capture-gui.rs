#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    capture::gui::run_gui()?;
    Ok(())
}
