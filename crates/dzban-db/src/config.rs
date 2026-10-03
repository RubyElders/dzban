use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, instrument, warn};

const CONFIG_ENV: &str = "DZBAN_CONFIG";
const CONFIG_DIR: &str = "dzban";
const CONFIG_FILE: &str = "dzban.toml";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseBackend {
    #[default]
    Postgresql,
    Clickhouse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub databases: Vec<DatabaseConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub name: String,
    #[serde(default)]
    pub backend: DatabaseBackend,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: String,
    pub database: String,
    pub password: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

impl DatabaseConfig {
    pub fn connection_string(&self) -> String {
        if self.backend == DatabaseBackend::Clickhouse {
            return self.url.clone().unwrap_or_default();
        }
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

    pub fn validate(&self) -> Result<(), String> {
        match self.backend {
            DatabaseBackend::Postgresql => Ok(()),
            DatabaseBackend::Clickhouse => match self.url.as_deref() {
                Some(url) => {
                    let parsed = reqwest::Url::parse(url).map_err(|error| {
                        format!(
                            "ClickHouse connection '{}' has an invalid URL: {error}",
                            self.name
                        )
                    })?;
                    if !matches!(parsed.scheme(), "http" | "https") {
                        return Err(format!(
                            "ClickHouse connection '{}' must use an http:// or https:// URL",
                            self.name
                        ));
                    }
                    if !parsed.username().is_empty() || parsed.password().is_some() {
                        return Err(format!(
                            "ClickHouse connection '{}' must keep credentials in user and password fields",
                            self.name
                        ));
                    }
                    Ok(())
                }
                None => Err(format!(
                    "ClickHouse connection '{}' requires a URL",
                    self.name
                )),
            },
        }
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
            backend: DatabaseBackend::Postgresql,
            host,
            port,
            user,
            database,
            password,
            url: None,
        }
    }
}

impl Config {
    #[instrument]
    pub fn load() -> Result<Self, String> {
        debug!("Loading configuration...");

        let explicit = nonempty_env(CONFIG_ENV).map(PathBuf::from);
        let fallbacks = config_fallback_paths(
            std::env::current_dir().ok(),
            nonempty_env("XDG_CONFIG_HOME").map(PathBuf::from),
            nonempty_env("HOME").map(PathBuf::from),
            nonempty_env("XDG_CONFIG_DIRS"),
        );

        Self::load_from_locations(explicit, fallbacks)
    }

    pub fn user_config_path() -> Option<PathBuf> {
        user_config_path(
            nonempty_env("XDG_CONFIG_HOME").map(PathBuf::from),
            nonempty_env("HOME").map(PathBuf::from),
        )
    }

    #[instrument(skip(path))]
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        debug!("Attempting to load config from: {}", path.display());

        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

        let config: Self =
            toml::from_str(&content).map_err(|e| format!("Failed to parse TOML: {}", e))?;
        for database in &config.databases {
            database.validate()?;
        }
        Ok(config)
    }

    fn load_from_locations(
        explicit: Option<PathBuf>,
        fallbacks: Vec<PathBuf>,
    ) -> Result<Self, String> {
        if let Some(path) = explicit {
            let config = Self::load_from_path(&path)?;
            info!(
                "Loaded config from {} ({} databases)",
                path.display(),
                config.databases.len()
            );
            return Ok(config);
        }

        for path in fallbacks {
            debug!("Checking config path: {}", path.display());
            if path.exists() {
                let config = Self::load_from_path(&path)?;
                info!(
                    "Loaded config from {} ({} databases)",
                    path.display(),
                    config.databases.len()
                );
                return Ok(config);
            }
        }

        warn!("No config file found, using empty configuration");
        Ok(Self {
            databases: Vec::new(),
        })
    }
}

fn nonempty_env(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

fn config_fallback_paths(
    current_dir: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
    home: Option<PathBuf>,
    xdg_config_dirs: Option<OsString>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(current_dir) = current_dir {
        paths.push(current_dir.join(CONFIG_FILE));
    }
    if let Some(path) = user_config_path(xdg_config_home, home) {
        paths.push(path);
    }

    let system_dirs = xdg_config_dirs
        .map(|dirs| {
            std::env::split_paths(&dirs)
                .filter(|path| path.is_absolute())
                .collect::<Vec<_>>()
        })
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| vec![PathBuf::from("/etc/xdg")]);
    paths.extend(
        system_dirs
            .into_iter()
            .map(|path| path.join(CONFIG_DIR).join(CONFIG_FILE)),
    );

    paths
}

fn user_config_path(xdg_config_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    xdg_config_home
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.filter(|path| path.is_absolute())
                .map(|path| path.join(".config"))
        })
        .map(|path| path.join(CONFIG_DIR).join(CONFIG_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "dzban-config-{name}-{}-{unique}",
            std::process::id()
        ))
    }

    fn write_config(path: &Path, name: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!(
                r#"[[databases]]
name = "{name}"
user = "reader"
database = "test"
"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn searches_local_then_user_then_system() {
        let root = temp_dir("order");
        let local = root.join("project").join(CONFIG_FILE);
        let user = root.join("user").join(CONFIG_DIR).join(CONFIG_FILE);
        let system = root.join("system").join(CONFIG_DIR).join(CONFIG_FILE);
        write_config(&local, "local");
        write_config(&user, "user");
        write_config(&system, "system");

        let paths = config_fallback_paths(
            local.parent().map(Path::to_path_buf),
            Some(root.join("user")),
            None,
            Some(std::env::join_paths([root.join("system")]).unwrap()),
        );
        let config = Config::load_from_locations(None, paths).unwrap();

        assert_eq!(config.databases[0].name, "local");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn falls_back_to_user_then_system() {
        let root = temp_dir("fallback");
        let user = root.join("user").join(CONFIG_DIR).join(CONFIG_FILE);
        let system = root.join("system").join(CONFIG_DIR).join(CONFIG_FILE);
        write_config(&user, "user");
        write_config(&system, "system");
        let paths = config_fallback_paths(
            Some(root.join("project")),
            Some(root.join("user")),
            None,
            Some(std::env::join_paths([root.join("system")]).unwrap()),
        );

        let config = Config::load_from_locations(None, paths.clone()).unwrap();
        assert_eq!(config.databases[0].name, "user");

        fs::remove_file(user).unwrap();
        let config = Config::load_from_locations(None, paths).unwrap();
        assert_eq!(config.databases[0].name, "system");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_path_is_authoritative() {
        let root = temp_dir("explicit");
        let explicit = root.join("selected.toml");
        let fallback = root.join(CONFIG_FILE);
        write_config(&explicit, "explicit");
        write_config(&fallback, "fallback");

        let config = Config::load_from_locations(Some(explicit), vec![fallback]).unwrap();

        assert_eq!(config.databases[0].name, "explicit");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ignores_relative_xdg_locations() {
        assert_eq!(
            user_config_path(Some(PathBuf::from("relative")), None),
            None
        );

        let paths = config_fallback_paths(None, None, None, Some(OsString::from("relative")));
        assert_eq!(paths, vec![PathBuf::from("/etc/xdg/dzban/dzban.toml")]);
    }
}
