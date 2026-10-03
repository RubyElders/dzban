use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub databases: Vec<DatabaseConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub name: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: String,
    pub database: String,
    pub password: Option<String>,
}

impl DatabaseConfig {
    pub fn connection_string(&self) -> String {
        let mut parts = vec![
            format!("user={}", self.user),
            format!("dbname={}", self.database),
        ];
        
        if let Some(host) = &self.host {
            parts.push(format!("host={}", host));
        }
        
        if let Some(port) = self.port {
            parts.push(format!("port={}", port));
        }
        
        if let Some(password) = &self.password {
            parts.push(format!("password={}", password));
        }
        
        parts.join(" ")
    }
}

impl Config {
    pub fn load() -> Result<Self, String> {
        // Try local file first (for testing)
        if let Ok(config) = Self::load_from_path("config.toml") {
            return Ok(config);
        }
        
        // Try XDG config directory
        if let Ok(config) = Self::load_from_xdg() {
            return Ok(config);
        }
        
        // Return default empty config
        Ok(Config {
            databases: Vec::new(),
        })
    }
    
    fn load_from_path(path: &str) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path, e))?;
        
        toml::from_str(&content)
            .map_err(|e| format!("Failed to parse TOML: {}", e))
    }
    
    fn load_from_xdg() -> Result<Self, String> {
        let config_dir = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                PathBuf::from(home).join(".config")
            });
        
        let config_path = config_dir.join("dzban").join("config.toml");
        
        if !config_path.exists() {
            return Err("Config file not found".to_string());
        }
        
        Self::load_from_path(config_path.to_str().unwrap())
    }
}
