use anyhow::Result;
use std::collections::HashMap;

pub fn get_preset_file_path() -> Result<std::path::PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| anyhow::anyhow!("Could not find home directory"))?;
    Ok(std::path::PathBuf::from(home).join(".capture-presets.json"))
}

pub fn load_presets() -> Result<HashMap<String, String>> {
    let preset_file = get_preset_file_path()?;

    if !preset_file.exists() {
        return Ok(HashMap::new());
    }

    let content = std::fs::read_to_string(&preset_file)?;
    let presets: HashMap<String, String> =
        serde_json::from_str(&content).unwrap_or_else(|_| HashMap::new());

    Ok(presets)
}

pub fn save_presets(presets: &HashMap<String, String>) -> Result<()> {
    let preset_file = get_preset_file_path()?;
    let content = serde_json::to_string_pretty(presets)?;
    std::fs::write(&preset_file, content)?;
    Ok(())
}

pub fn get_builtin_presets() -> HashMap<String, String> {
    let mut presets = HashMap::new();

    // Common screen resolutions
    presets.insert("1080p".to_string(), "0,0,1920,1080".to_string());
    presets.insert("720p".to_string(), "0,0,1280,720".to_string());
    presets.insert("4k".to_string(), "0,0,3840,2160".to_string());
    presets.insert("naver-series".to_string(), "607,23,690,1007".to_string());
    presets.insert("naver-wide".to_string(), "412,23,1080,1007".to_string());

    // VM window presets (common sizes)
    presets.insert("vm-small".to_string(), "100,100,1024,768".to_string());
    presets.insert("vm-medium".to_string(), "100,100,1280,800".to_string());
    presets.insert("vm-large".to_string(), "100,100,1920,1080".to_string());

    presets
}

pub fn get_all_presets() -> Result<HashMap<String, String>> {
    let mut all_presets = get_builtin_presets();
    let custom_presets = load_presets()?;

    // Custom presets override built-in ones
    all_presets.extend(custom_presets);

    Ok(all_presets)
}

pub fn save_preset(name: &str, value: &str) -> Result<()> {
    if parse_crop_region(value).is_none() {
        return Err(anyhow::anyhow!(
            "Invalid crop region format: {}\nUse: x,y,width,height (e.g., '100,50,1920,1080')",
            value
        ));
    }
    let mut preset_map = load_presets()?;
    preset_map.insert(name.to_string(), value.to_string());
    save_presets(&preset_map)
}

pub fn parse_crop_region(crop_str: &str) -> Option<(i32, i32, i32, i32)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Built-in presets are never validated at startup, so a typo here would
    /// only surface as a silently ignored crop during a capture.
    #[test]
    fn builtin_presets_all_parse() {
        for (name, value) in get_builtin_presets() {
            assert!(
                parse_crop_region(&value).is_some(),
                "preset {name} has an unparseable region: {value}"
            );
        }
    }

    #[test]
    fn naver_wide_preset_is_valid() {
        let presets = get_builtin_presets();
        let value = presets
            .get("naver-wide")
            .expect("naver-wide preset missing");
        assert_eq!(parse_crop_region(value), Some((412, 23, 1080, 1007)));
    }
}
