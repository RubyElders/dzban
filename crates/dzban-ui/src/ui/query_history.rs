use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_ENTRIES: usize = 100;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct QueryHistoryEntry {
    pub sql: String,
    pub executed_at: u64,
    pub outcome: QueryHistoryOutcome,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum QueryHistoryOutcome {
    Success {
        duration_ms: u64,
        rows: usize,
        #[serde(default)]
        result: Option<QueryResultSnapshot>,
        #[serde(default)]
        explain: Option<serde_json::Value>,
        #[serde(default)]
        explain_error: Option<String>,
    },
    Failure {
        error: String,
    },
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct QueryResultSnapshot {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub total_rows: usize,
}

impl QueryHistoryEntry {
    pub fn success(
        sql: String,
        duration: Duration,
        rows: usize,
        result: QueryResultSnapshot,
        explain: Option<serde_json::Value>,
        explain_error: Option<String>,
    ) -> Self {
        Self {
            sql,
            executed_at: timestamp(),
            outcome: QueryHistoryOutcome::Success {
                duration_ms: duration.as_millis().min(u64::MAX as u128) as u64,
                rows,
                result: Some(result),
                explain,
                explain_error,
            },
        }
    }

    pub fn failure(sql: String, error: String) -> Self {
        Self {
            sql,
            executed_at: timestamp(),
            outcome: QueryHistoryOutcome::Failure { error },
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct QueryHistoryFile {
    version: u32,
    database: String,
    entries: Vec<QueryHistoryEntry>,
}

pub struct QueryHistoryStore {
    path: PathBuf,
    file: QueryHistoryFile,
}

impl QueryHistoryStore {
    pub fn load(database: &str) -> Result<Self, String> {
        let data_dir = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".local").join("share"))
            })
            .ok_or_else(|| "HOME and XDG_DATA_HOME are not set".to_string())?;
        let path = history_path(&data_dir, database);
        match Self::load_from_path(path.clone(), database) {
            Ok(store) => Ok(store),
            Err(error) => {
                tracing::warn!("Ignoring unreadable query history: {}", error);
                Ok(Self::empty(path, database))
            }
        }
    }

    fn load_from_path(path: PathBuf, database: &str) -> Result<Self, String> {
        let file = if path.exists() {
            let content = fs::read_to_string(&path)
                .map_err(|error| format!("Failed to read {}: {}", path.display(), error))?;
            let file: QueryHistoryFile = serde_json::from_str(&content)
                .map_err(|error| format!("Failed to parse {}: {}", path.display(), error))?;
            if file.version != 1 {
                return Err(format!(
                    "History file {} has unsupported version {}",
                    path.display(),
                    file.version
                ));
            }
            if file.database != database {
                return Err(format!(
                    "History file {} belongs to a different database",
                    path.display()
                ));
            }
            file
        } else {
            Self::empty_file(database)
        };
        Ok(Self { path, file })
    }

    fn empty(path: PathBuf, database: &str) -> Self {
        Self {
            path,
            file: Self::empty_file(database),
        }
    }

    fn empty_file(database: &str) -> QueryHistoryFile {
        QueryHistoryFile {
            version: 1,
            database: database.to_string(),
            entries: Vec::new(),
        }
    }

    pub fn entries(&self) -> &[QueryHistoryEntry] {
        &self.file.entries
    }

    pub fn add(&mut self, entry: QueryHistoryEntry) -> Result<(), String> {
        self.file.entries.insert(0, entry);
        self.file.entries.truncate(MAX_ENTRIES);
        self.save()
    }

    fn save(&self) -> Result<(), String> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| format!("Invalid history path: {}", self.path.display()))?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create {}: {}", parent.display(), error))?;
        let content = serde_json::to_vec_pretty(&self.file)
            .map_err(|error| format!("Failed to serialize query history: {}", error))?;
        let temporary_path = self.path.with_extension("json.tmp");
        write_private_file(&temporary_path, &content)?;
        fs::rename(&temporary_path, &self.path).map_err(|error| {
            format!(
                "Failed to replace {} using {}: {}",
                self.path.display(),
                temporary_path.display(),
                error
            )
        })
    }
}

fn history_path(data_dir: &Path, database: &str) -> PathBuf {
    let filename = format!("{}.json", encode_filename(database));
    data_dir.join("dzban").join("query-history").join(filename)
}

fn encode_filename(database: &str) -> String {
    database
        .as_bytes()
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect()
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(unix)]
fn write_private_file(path: &Path, content: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::fs::PermissionsExt;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("Failed to open {}: {}", path.display(), error))?;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("Failed to protect {}: {}", path.display(), error))?;
    file.write_all(content)
        .map_err(|error| format!("Failed to write {}: {}", path.display(), error))
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, content: &[u8]) -> Result<(), String> {
    fs::write(path, content)
        .map_err(|error| format!("Failed to write {}: {}", path.display(), error))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "dzban-history-{}-{}-{}.json",
            std::process::id(),
            timestamp(),
            name
        ))
    }

    #[test]
    fn history_round_trips_successes_and_failures() {
        let path = test_path("round-trip");
        let mut store = QueryHistoryStore::load_from_path(path.clone(), "primary").unwrap();
        store
            .add(QueryHistoryEntry::success(
                "select 1".to_string(),
                Duration::from_millis(12),
                1,
                QueryResultSnapshot {
                    columns: vec!["value".to_string()],
                    rows: vec![vec!["1".to_string()]],
                    total_rows: 1,
                },
                Some(serde_json::json!([{"Plan": {"Node Type": "Result"}}])),
                None,
            ))
            .unwrap();
        store
            .add(QueryHistoryEntry::failure(
                "select broken".to_string(),
                "syntax error".to_string(),
            ))
            .unwrap();

        let loaded = QueryHistoryStore::load_from_path(path.clone(), "primary").unwrap();
        assert_eq!(loaded.entries().len(), 2);
        assert_eq!(loaded.entries()[0].sql, "select broken");
        assert_eq!(loaded.entries()[1].sql, "select 1");
        let QueryHistoryOutcome::Success {
            result, explain, ..
        } = &loaded.entries()[1].outcome
        else {
            panic!("expected successful history entry");
        };
        assert_eq!(result.as_ref().unwrap().rows, [["1"]]);
        assert!(explain.is_some());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn database_names_produce_isolated_safe_filenames() {
        assert_eq!(encode_filename("primary"), "7072696d617279");
        assert_eq!(encode_filename("../../other"), "2e2e2f2e2e2f6f74686572");
        assert_ne!(
            history_path(Path::new("/data"), "primary"),
            history_path(Path::new("/data"), "reporting")
        );
    }

    #[test]
    fn history_is_bounded() {
        let path = test_path("bounded");
        let mut store = QueryHistoryStore::load_from_path(path.clone(), "primary").unwrap();
        for index in 0..=MAX_ENTRIES {
            store
                .add(QueryHistoryEntry::success(
                    format!("select {}", index),
                    Duration::ZERO,
                    1,
                    QueryResultSnapshot::default(),
                    None,
                    None,
                ))
                .unwrap();
        }

        assert_eq!(store.entries().len(), MAX_ENTRIES);
        assert_eq!(store.entries()[0].sql, format!("select {}", MAX_ENTRIES));
        assert_eq!(store.entries().last().unwrap().sql, "select 1");

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn invalid_history_can_be_replaced() {
        let path = test_path("invalid");
        fs::write(&path, "not json").unwrap();
        assert!(QueryHistoryStore::load_from_path(path.clone(), "primary").is_err());

        let mut store = QueryHistoryStore::empty(path.clone(), "primary");
        store
            .add(QueryHistoryEntry::success(
                "select 1".to_string(),
                Duration::ZERO,
                1,
                QueryResultSnapshot::default(),
                None,
                None,
            ))
            .unwrap();
        assert!(QueryHistoryStore::load_from_path(path.clone(), "primary").is_ok());

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn history_without_results_or_explain_still_loads() {
        let file: QueryHistoryFile = serde_json::from_str(
            r#"{
                "version": 1,
                "database": "primary",
                "entries": [{
                    "sql": "select 1",
                    "executed_at": 1,
                    "outcome": {
                        "status": "success",
                        "duration_ms": 2,
                        "rows": 1
                    }
                }]
            }"#,
        )
        .unwrap();

        let QueryHistoryOutcome::Success {
            result, explain, ..
        } = &file.entries[0].outcome
        else {
            panic!("expected success entry");
        };
        assert!(result.is_none());
        assert!(explain.is_none());
    }
}
