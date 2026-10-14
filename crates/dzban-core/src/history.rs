use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_ENTRIES: usize = 100;
pub const MAX_HISTORY_RESULT_ROWS: usize = 200;
pub const MAX_HISTORY_RESULT_BYTES: usize = 1_000_000;

pub struct SuccessfulQuery {
    pub command: Option<crate::CommandAcknowledgment>,
    pub duration: Duration,
    pub rows: usize,
    pub result: QueryResultSnapshot,
}

pub enum CompletedQuery {
    Success(SuccessfulQuery),
    Failure(crate::error::ExecutionError),
}

pub struct PendingQueryHistory {
    sql: Option<String>,
    outcome: Option<CompletedQuery>,
    planning: Option<Result<crate::QueryPlan, crate::error::ExecutionError>>,
}

impl PendingQueryHistory {
    pub fn new(sql: String) -> Self {
        Self {
            sql: Some(sql),
            outcome: None,
            planning: None,
        }
    }

    pub fn complete(&mut self, outcome: CompletedQuery) {
        if self.sql.is_some() && self.outcome.is_none() {
            self.outcome = Some(outcome);
        }
    }

    pub fn planned(&mut self, planning: Result<crate::QueryPlan, crate::error::ExecutionError>) {
        if self.sql.is_some() && self.planning.is_none() {
            self.planning = Some(planning);
        }
    }

    pub fn take_entry(&mut self) -> Option<QueryHistoryEntry> {
        if self.outcome.is_none() || self.planning.is_none() {
            return None;
        }
        let sql = self.sql.take()?;
        let outcome = self.outcome.take()?;
        let planning = self.planning.take()?;
        let (explain, planning_error) = match planning {
            Ok(plan) => (serde_json::to_value(plan).ok(), None),
            Err(error) => (None, Some(error)),
        };
        let explain_error = planning_error.as_ref().map(ToString::to_string);
        let mut entry = match outcome {
            CompletedQuery::Success(success) => QueryHistoryEntry::success(
                sql,
                success.duration,
                success.rows,
                success.result,
                explain,
                explain_error,
            )
            .with_command(success.command),
            CompletedQuery::Failure(error) => {
                QueryHistoryEntry::failure(sql, error.to_string(), explain, explain_error)
                    .with_diagnostic(error)
            }
        };
        entry.planning_error = planning_error;
        Some(entry)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct QueryHistoryEntry {
    pub sql: String,
    pub executed_at: u64,
    pub outcome: QueryHistoryOutcome,
    #[serde(default)]
    pub planning_error: Option<crate::error::ExecutionError>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum QueryHistoryOutcome {
    Success {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cutoff: Option<crate::result_store::ResultCutoff>,
        #[serde(default)]
        command: Option<crate::CommandAcknowledgment>,
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
        #[serde(default)]
        diagnostic: Option<crate::error::ExecutionError>,
        #[serde(default)]
        explain: Option<serde_json::Value>,
        #[serde(default)]
        explain_error: Option<String>,
    },
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct QueryResultSnapshot {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub total_rows: usize,
}

impl QueryResultSnapshot {
    pub fn bounded(&self) -> Self {
        let mut bytes = self.columns.iter().map(String::len).sum::<usize>();
        let rows = self
            .rows
            .iter()
            .take(MAX_HISTORY_RESULT_ROWS)
            .take_while(|row| {
                let size = row.iter().map(String::len).sum::<usize>();
                if bytes.saturating_add(size) > MAX_HISTORY_RESULT_BYTES {
                    return false;
                }
                bytes += size;
                true
            })
            .cloned()
            .collect();
        Self {
            columns: self.columns.clone(),
            rows,
            total_rows: self.total_rows,
        }
    }

    pub fn from_result(result: &crate::ResultSet) -> Self {
        let columns: Vec<String> = result
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect();
        let mut bytes = columns.iter().map(String::len).sum::<usize>();
        let rows = result
            .rows
            .iter()
            .take(MAX_HISTORY_RESULT_ROWS)
            .take_while(|row| {
                let size = row.iter().map(|cell| cell.display().len()).sum::<usize>();
                if bytes.saturating_add(size) > MAX_HISTORY_RESULT_BYTES {
                    return false;
                }
                bytes += size;
                true
            })
            .map(|row| row.iter().map(|cell| cell.display().to_owned()).collect())
            .collect();
        Self {
            columns,
            rows,
            total_rows: result.rows.len(),
        }
    }
}

impl QueryHistoryEntry {
    pub async fn from_streamed(
        sql: String,
        result: &Result<crate::execution::StreamedQueryResult, crate::error::ExecutionError>,
        snapshot: &crate::execution::QuerySnapshot,
    ) -> Result<Self, crate::error::ExecutionError> {
        let explain = snapshot
            .plan
            .as_ref()
            .and_then(|plan| serde_json::to_value(plan).ok());
        let mut entry = match result {
            Ok(result) => {
                let mut sample = QueryResultSnapshot {
                    total_rows: result.capture.rows,
                    ..Default::default()
                };
                if let Some(reader) = &result.capture.result {
                    let schema = reader.read_page(0, 0).await?;
                    sample.columns = schema
                        .columns
                        .into_iter()
                        .map(|column| column.name)
                        .collect();
                    let mut bytes = sample.columns.iter().map(String::len).sum::<usize>();
                    for index in 0..result.capture.rows.min(MAX_HISTORY_RESULT_ROWS) {
                        let mut page = reader.read_page(index, 1).await?;
                        let row = page.rows.pop().ok_or_else(|| {
                            crate::error::ExecutionError::Message(
                                "History result row is missing".into(),
                            )
                        })?;
                        let size = row.iter().map(|cell| cell.display().len()).sum::<usize>();
                        if bytes.saturating_add(size) > MAX_HISTORY_RESULT_BYTES {
                            break;
                        }
                        bytes += size;
                        sample
                            .rows
                            .push(row.iter().map(|cell| cell.display().to_owned()).collect());
                    }
                }
                let mut entry = Self::success(
                    sql,
                    result.duration,
                    result.capture.rows,
                    sample,
                    explain,
                    snapshot.explain_error.clone(),
                )
                .with_command(result.command.clone());
                if let QueryHistoryOutcome::Success { cutoff, .. } = &mut entry.outcome {
                    *cutoff = result.capture.cutoff;
                }
                entry
            }
            Err(error) => Self::failure(
                sql,
                error.to_string(),
                explain,
                snapshot.explain_error.clone(),
            )
            .with_diagnostic(error.clone()),
        };
        entry.planning_error = snapshot.planning_error.clone();
        Ok(entry)
    }
    pub fn with_diagnostic(mut self, error: crate::error::ExecutionError) -> Self {
        if let QueryHistoryOutcome::Failure { diagnostic, .. } = &mut self.outcome {
            *diagnostic = Some(error);
        }
        self
    }
    pub fn with_command(mut self, acknowledgment: Option<crate::CommandAcknowledgment>) -> Self {
        if let QueryHistoryOutcome::Success { command, .. } = &mut self.outcome {
            *command = acknowledgment;
        }
        self
    }
    pub fn from_execution<E: std::fmt::Display>(
        sql: String,
        result: &Result<crate::QueryResult, E>,
        snapshot: &crate::execution::QuerySnapshot,
    ) -> Self {
        let explain = snapshot
            .plan
            .as_ref()
            .and_then(|plan| serde_json::to_value(plan).ok());
        let mut entry = match result {
            Ok(result) => Self::success(
                sql,
                result.duration,
                result.result_set.rows.len(),
                QueryResultSnapshot::from_result(&result.result_set),
                explain,
                snapshot.explain_error.clone(),
            ),
            Err(error) => Self::failure(
                sql,
                error.to_string(),
                explain,
                snapshot.explain_error.clone(),
            ),
        };
        if let (Ok(result), QueryHistoryOutcome::Success { command, .. }) =
            (result, &mut entry.outcome)
        {
            *command = result.command.clone();
        }
        if let QueryHistoryOutcome::Failure { diagnostic, .. } = &mut entry.outcome {
            *diagnostic = snapshot.execution_error.clone();
        }
        entry.planning_error = snapshot.planning_error.clone();
        entry
    }

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
            planning_error: None,
            outcome: QueryHistoryOutcome::Success {
                cutoff: None,
                command: None,
                duration_ms: duration.as_millis().min(u64::MAX as u128) as u64,
                rows,
                result: Some(result.bounded()),
                explain,
                explain_error,
            },
        }
    }

    pub fn failure(
        sql: String,
        error: String,
        explain: Option<serde_json::Value>,
        explain_error: Option<String>,
    ) -> Self {
        Self {
            sql,
            executed_at: timestamp(),
            planning_error: None,
            outcome: QueryHistoryOutcome::Failure {
                diagnostic: None,
                error,
                explain,
                explain_error,
            },
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

#[derive(Serialize, Deserialize)]
struct HistoryRecord {
    version: u32,
    database: String,
    published_at: u64,
    entry: QueryHistoryEntry,
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

    pub fn load_from_path(path: PathBuf, database: &str) -> Result<Self, String> {
        let file = if path.exists() && !path.with_extension("entries").exists() {
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
        let mut store = Self { path, file };
        store.refresh()?;
        Ok(store)
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
        let directory = self.path.with_extension("entries");
        fs::create_dir_all(&directory)
            .map_err(|error| format!("Failed to create history directory: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("Failed to protect history directory: {error}"))?;
        }
        let id = uuid::Uuid::new_v4();
        let temporary = directory.join(format!("{id}.tmp"));
        let destination = directory.join(format!("{id}.json"));
        let record = HistoryRecord {
            version: 1,
            database: self.file.database.clone(),
            published_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .min(u64::MAX as u128) as u64,
            entry,
        };
        let content = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
        write_private_file(&temporary, &content)?;
        fs::rename(&temporary, &destination)
            .map_err(|error| format!("Failed to publish history entry: {error}"))?;
        self.refresh()
    }

    pub fn refresh(&mut self) -> Result<(), String> {
        let mut legacy = Self::empty(self.path.clone(), &self.file.database);
        if self.path.exists() {
            match fs::read_to_string(&self.path).and_then(|content| {
                serde_json::from_str::<QueryHistoryFile>(&content).map_err(std::io::Error::other)
            }) {
                Ok(file) if file.version == 1 && file.database == self.file.database => {
                    legacy.file = file
                }
                _ => tracing::warn!("Could not load legacy query history"),
            }
        }
        legacy.read_records()?;
        self.file = legacy.file;
        Ok(())
    }

    fn read_records(&mut self) -> Result<(), String> {
        let directory = self.path.with_extension("entries");
        let mut entries: Vec<_> = self
            .file
            .entries
            .drain(..)
            .enumerate()
            .map(|(index, entry)| {
                (
                    entry.executed_at.saturating_mul(1_000_000_000),
                    format!("legacy-{:020}", usize::MAX - index),
                    entry,
                )
            })
            .collect();
        if directory.exists() {
            for item in fs::read_dir(&directory).map_err(|error| error.to_string())? {
                let path = item.map_err(|error| error.to_string())?.path();
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let content = match fs::read(&path) {
                    Ok(content) => content,
                    Err(_) => {
                        tracing::warn!(path = %path.display(), "Could not read query history record");
                        continue;
                    }
                };
                let record: HistoryRecord = match serde_json::from_slice(&content) {
                    Ok(record) => record,
                    Err(_) => {
                        tracing::warn!(path = %path.display(), "Invalid query history record");
                        continue;
                    }
                };
                if record.version != 1 || record.database != self.file.database {
                    tracing::warn!(path = %path.display(), "Query history record has an unsupported version or database");
                    continue;
                }
                entries.push((
                    record.published_at,
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    record.entry,
                ));
            }
        }
        entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        self.file.entries = entries
            .into_iter()
            .take(MAX_ENTRIES)
            .map(|(_, _, entry)| entry)
            .collect();
        Ok(())
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
        .and_then(|_| file.sync_all())
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

    #[test]
    fn snapshots_keep_total_count_and_bound_values_not_just_schema() {
        let result = crate::ResultSet {
            columns: vec![crate::ResultColumn {
                name: "value".into(),
                type_name: "text".into(),
            }],
            rows: (0..201)
                .map(|index| vec![crate::ResultCell::Text(index.to_string())])
                .collect(),
        };
        let snapshot = QueryResultSnapshot::from_result(&result);
        assert_eq!(snapshot.total_rows, 201);
        assert_eq!(snapshot.rows.len(), 200);
        assert_eq!(snapshot.columns, ["value"]);
        let oversized = crate::ResultSet {
            columns: result.columns,
            rows: vec![vec![crate::ResultCell::Text("x".repeat(1_000_000))]],
        };
        let snapshot = QueryResultSnapshot::from_result(&oversized);
        assert!(snapshot.rows.is_empty());
        assert_eq!(snapshot.total_rows, 1);
    }

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "dzban-history-{}-{}-{}.json",
            std::process::id(),
            timestamp(),
            name
        ))
    }

    fn cleanup(path: PathBuf) {
        if path.exists() {
            fs::remove_file(&path).unwrap();
        }
        let records = path.with_extension("entries");
        if records.exists() {
            fs::remove_dir_all(records).unwrap();
        }
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
                Some(serde_json::json!([{"Plan": {"Node Type": "Result"}}])),
                None,
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
        let QueryHistoryOutcome::Failure { explain, .. } = &loaded.entries()[0].outcome else {
            panic!("expected failed history entry");
        };
        assert!(explain.is_some());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(
                    fs::read_dir(path.with_extension("entries"))
                        .unwrap()
                        .next()
                        .unwrap()
                        .unwrap()
                        .path()
                )
                .unwrap()
                .permissions()
                .mode()
                    & 0o777,
                0o600
            );
        }

        cleanup(path);
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

        cleanup(path);
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

        cleanup(path);
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
