use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Proxy {
    pub remote: String,
    pub bind: String,
    pub protocol: String,
}

pub fn load_config(path: &Path) -> Result<Vec<Proxy>, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    // Strip UTF-8 BOM if present
    let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
    let proxies: Vec<Proxy> = serde_json::from_str(content)?;
    Ok(proxies)
}

pub fn save_config(path: &Path, proxies: &[Proxy]) -> Result<(), Box<dyn std::error::Error>> {
    let json = serde_json::to_string_pretty(proxies)?;
    std::fs::write(path, json)?;
    Ok(())
}
