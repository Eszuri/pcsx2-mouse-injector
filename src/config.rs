use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum InjectionMode {
    Off,
    #[serde(alias = "On", alias = "on")]
    #[default]
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub mode: InjectionMode,
    pub sensitivity: u32,
    pub invert_pitch: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mode: InjectionMode::Auto,
            sensitivity: 20,
            invert_pitch: false,
        }
    }
}

impl AppConfig {
    pub fn load_or_default(path: &str) -> Self {
        if let Ok(content) = fs::read_to_string(path)
            && let Ok(mut cfg) = serde_json::from_str::<AppConfig>(&content)
        {
            cfg.sensitivity = cfg.sensitivity.clamp(1, 100);
            return cfg;
        }
        let default = Self::default();
        default.save(path);
        default
    }

    pub fn save(&self, path: &str) {
        if let Some(parent) = std::path::Path::new(path).parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, json);
        }
    }
}
