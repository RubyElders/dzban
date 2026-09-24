use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, instrument, warn};

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

    /// Parse a PostgreSQL connection string into DatabaseConfig
    /// Format: "host=localhost port=5432 user=postgres password=pass dbname=mydb"
    pub fn from_connection_string(conn_str: &str) -> Self {
        let mut host = None;
        let mut port = None;
        let mut user = String::from("postgres");
        let mut database = String::from("postgres");
        let mut password = None;

        for part in conn_str.split_whitespace() {
            if let Some((key, value)) = part.split_once('=') {
                match key {
                    "host" => host = Some(value.to_string()),
                    "port" => port = value.parse().ok(),
                    "user" => user = value.to_string(),
                    "dbname" => database = value.to_string(),
                    "password" => password = Some(value.to_string()),
                    _ => {}
                }
            }
        }

        Self {
            name: database.clone(),
            host,
            port,
            user,
            database,
            password,
        }
    }
}

impl Config {
    #[instrument]
    pub fn load() -> Result<Self, String> {
        debug!("Loading configuration...");

        let local_path = Path::new("dzban.toml");
        if local_path.exists() {
            let config = Self::load_from_path(local_path)?;
            info!(
                "Loaded config from local dzban.toml ({} databases)",
                config.databases.len()
            );
            return Ok(config);
        }

        if let Some(config) = Self::load_from_xdg()? {
            info!(
                "Loaded config from XDG directory ({} databases)",
                config.databases.len()
            );
            return Ok(config);
        }

        // Return default empty config
        warn!("No config file found, using empty configuration");
        Ok(Config {
            databases: Vec::new(),
        })
    }

    #[instrument(skip(path))]
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        debug!("Attempting to load config from: {}", path.display());

        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

        toml::from_str(&content).map_err(|e| format!("Failed to parse TOML: {}", e))
    }

    #[instrument]
    fn load_from_xdg() -> Result<Option<Self>, String> {
        let config_dir = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                PathBuf::from(home).join(".config")
            });

        let config_path = config_dir.join("dzban").join("dzban.toml");
        debug!("Checking XDG config path: {}", config_path.display());

        if !config_path.exists() {
            return Ok(None);
        }

        Self::load_from_path(config_path).map(Some)
    }
}
