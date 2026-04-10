use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Proxy {
    pub remote: String,
    pub bind: String,
    pub protocol: String,
}

fn default_max_connections() -> usize { 1024 }
fn default_max_client_tunnels() -> usize { 1024 }
fn default_keepalive_idle() -> u64 { 60 }
fn default_keepalive_interval() -> u64 { 30 }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
    #[serde(default = "default_max_client_tunnels")]
    pub max_client_tunnels: usize,
    #[serde(default = "default_keepalive_idle")]
    pub keepalive_idle: u64,
    #[serde(default = "default_keepalive_interval")]
    pub keepalive_interval: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            max_connections: default_max_connections(),
            max_client_tunnels: default_max_client_tunnels(),
            keepalive_idle: default_keepalive_idle(),
            keepalive_interval: default_keepalive_interval(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    #[serde(default)]
    pub settings: Settings,
    pub proxies: Vec<Proxy>,
}

pub fn load_config(path: &Path) -> Result<Config, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    // Strip UTF-8 BOM if present
    let content = content.strip_prefix('\u{feff}').unwrap_or(&content);

    // Try new object format first, fall back to bare array
    if let Ok(config) = serde_json::from_str::<Config>(content) {
        Ok(config)
    } else {
        let proxies: Vec<Proxy> = serde_json::from_str(content)?;
        Ok(Config {
            settings: Settings::default(),
            proxies,
        })
    }
}

pub fn save_config(path: &Path, config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let json = serde_json::to_string_pretty(config)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Save only the proxies list (for rproxy-edit backward compat)
pub fn save_proxies(path: &Path, proxies: &[Proxy]) -> Result<(), Box<dyn std::error::Error>> {
    let json = serde_json::to_string_pretty(proxies)?;
    std::fs::write(path, json)?;
    Ok(())
}
