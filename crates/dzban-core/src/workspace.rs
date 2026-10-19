use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::ConnectionProfileId;
use crate::application::{ApplicationContext, ApplicationPhase};
use crate::config::Config;
use crate::tabs::TabId;

const MAX_WORKSPACE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Default)]
struct WorkspaceContent {
    bytes: Vec<u8>,
    exceeded: bool,
}

impl Write for WorkspaceContent {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_WORKSPACE_BYTES as usize - self.bytes.len() {
            self.exceeded = true;
            return Err(std::io::Error::other("Workspace exceeds the 32 MiB limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceHost {
    Macos,
    Gtk,
    Cli,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceLocation {
    pub host: WorkspaceHost,
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedTab {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub draft: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedWorkspace {
    pub version: i64,
    pub tabs: Vec<SavedTab>,
    pub selected: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceError(pub String);

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for WorkspaceError {}

impl WorkspaceLocation {
    pub fn path(&self) -> Result<PathBuf, WorkspaceError> {
        if let Some(path) = &self.path {
            return Ok(path.clone());
        }
        let directory = std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .ok_or_else(|| WorkspaceError("Cannot find workspace data directory".into()))?;
        Ok(self.in_directory(&directory))
    }

    pub fn in_directory(&self, directory: &Path) -> PathBuf {
        directory.join("dzban").join(match self.host {
            WorkspaceHost::Macos => "native-workspace.json",
            WorkspaceHost::Gtk => "gtk-workspace.json",
            WorkspaceHost::Cli => "cli-workspace.json",
        })
    }

    pub async fn load(&self) -> Result<Option<SavedWorkspace>, WorkspaceError> {
        let path = self.path()?;
        tokio::task::spawn_blocking(move || {
            let file = match std::fs::File::open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(WorkspaceError(error.to_string())),
            };
            let mut content = Vec::new();
            file.take(MAX_WORKSPACE_BYTES + 1)
                .read_to_end(&mut content)
                .map_err(io_error)?;
            if content.len() as u64 > MAX_WORKSPACE_BYTES {
                return Err(WorkspaceError("Workspace exceeds the 32 MiB limit".into()));
            }
            let saved: SavedWorkspace = serde_json::from_slice(&content)
                .map_err(|error| WorkspaceError(error.to_string()))?;
            Ok((saved.version == 1).then_some(saved))
        })
        .await
        .map_err(|error| WorkspaceError(error.to_string()))?
    }

    pub async fn save(&self, saved: SavedWorkspace) -> Result<(), WorkspaceError> {
        let path = self.path()?;
        tokio::task::spawn_blocking(move || {
            let mut content = WorkspaceContent::default();
            if let Err(error) = serde_json::to_writer(&mut content, &saved) {
                return Err(WorkspaceError(if content.exceeded {
                    "Workspace exceeds the 32 MiB limit".into()
                } else {
                    error.to_string()
                }));
            }
            let directory = path
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            std::fs::create_dir_all(directory).map_err(io_error)?;
            let temporary =
                directory.join(format!(".dzban-workspace-{}.tmp", uuid::Uuid::new_v4()));
            let result = (|| {
                let mut options = std::fs::OpenOptions::new();
                options.create_new(true).write(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(&temporary).map_err(io_error)?;
                file.write_all(&content.bytes).map_err(io_error)?;
                file.sync_all().map_err(io_error)?;
                std::fs::rename(&temporary, &path).map_err(io_error)
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(temporary);
            }
            result
        })
        .await
        .map_err(|error| WorkspaceError(error.to_string()))?
    }
}

fn io_error(error: std::io::Error) -> WorkspaceError {
    WorkspaceError(error.to_string())
}

#[derive(Clone, Debug, Serialize)]
pub struct RestoreIssue {
    pub tab: uuid::Uuid,
    pub profile: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct RestoreProgress {
    pub finished: bool,
    pub restored: bool,
    pub issues: Vec<RestoreIssue>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct WorkspaceRestore {
    progress: watch::Receiver<RestoreProgress>,
}

impl WorkspaceRestore {
    pub fn subscribe(&self) -> watch::Receiver<RestoreProgress> {
        self.progress.clone()
    }
    pub async fn wait(mut self) -> RestoreProgress {
        loop {
            let current = self.progress.borrow_and_update().clone();
            if current.finished {
                return current;
            }
            if self.progress.changed().await.is_err() {
                return RestoreProgress {
                    finished: true,
                    error: Some("Workspace restoration tracking was dropped".into()),
                    ..current
                };
            }
        }
    }
}

#[derive(Default)]
pub struct WorkspaceService {
    restore: Mutex<Option<WorkspaceRestore>>,
}

impl WorkspaceService {
    pub(crate) async fn drain(&self) {
        let restore = self.restore.lock().unwrap().clone();
        if let Some(restore) = restore {
            restore.wait().await;
        }
    }
}

impl ApplicationContext {
    pub fn start_restore_workspace(
        self: &Arc<Self>,
        location: WorkspaceLocation,
        config: Option<Config>,
    ) -> WorkspaceRestore {
        let mut retained = self.workspace.restore.lock().unwrap();
        if let Some(restore) = &*retained {
            return restore.clone();
        }
        if *self.subscribe_phase().borrow() != ApplicationPhase::Running {
            return WorkspaceRestore {
                progress: watch::channel(RestoreProgress {
                    finished: true,
                    error: Some("Application is closing".into()),
                    ..Default::default()
                })
                .1,
            };
        }
        let (progress, receiver) = watch::channel(RestoreProgress::default());
        let restore = WorkspaceRestore { progress: receiver };
        *retained = Some(restore.clone());
        let context = self.clone();
        tokio::spawn(async move {
            let mut report = RestoreProgress::default();
            if let Err(error) = context
                .restore_workspace(location, config, &progress, &mut report)
                .await
            {
                report.error = Some(error.to_string());
            }
            let running = *context.subscribe_phase().borrow() == ApplicationPhase::Running;
            if running
                && let Err(error) = context.open_initial_workspace_tab(None, String::new())
                && report.error.is_none()
            {
                report.error = Some(error.to_string());
            }
            report.finished = true;
            progress.send_replace(report);
        });
        restore
    }

    async fn restore_workspace(
        self: &Arc<Self>,
        location: WorkspaceLocation,
        config: Option<Config>,
        progress: &watch::Sender<RestoreProgress>,
        report: &mut RestoreProgress,
    ) -> Result<(), WorkspaceError> {
        let saved = location.load().await?;
        if !self.tabs.snapshot().tabs.is_empty() {
            return Ok(());
        }
        let Some(saved) = saved.filter(|saved| !saved.tabs.is_empty()) else {
            return Ok(());
        };
        let selected = usize::try_from(saved.selected.max(0)).unwrap_or(usize::MAX);
        let selected = selected.min(saved.tabs.len() - 1);
        let mut selected_tab = None;
        for (index, saved) in saved.tabs.into_iter().enumerate() {
            if *self.subscribe_phase().borrow() != ApplicationPhase::Running {
                break;
            }
            let tab = if index == 0 {
                let Some(tab) = self.open_initial_workspace_tab(
                    saved.profile.clone().map(ConnectionProfileId),
                    saved.draft,
                )?
                else {
                    return Ok(());
                };
                report.restored = true;
                tab
            } else {
                self.open_tab(saved.profile.clone().map(ConnectionProfileId), saved.draft)
                    .map_err(|error| WorkspaceError(error.to_string()))?
            };
            if index == selected {
                selected_tab = Some(tab);
            }
            progress.send_replace(report.clone());
            if let Some(profile) = saved.profile {
                let resolved = config
                    .as_ref()
                    .ok_or_else(|| "Load profiles before connecting".to_owned())
                    .and_then(|config| {
                        config
                            .resolve_profile(&ConnectionProfileId(profile.clone()), None)
                            .map_err(|error| error.to_string())
                    });
                let issue = match resolved {
                    Ok(profile) => match self.start_connect_in_tab(tab, profile).wait().await {
                        Ok(_) => None,
                        Err(error)
                            if self.tabs.find(tab).is_some_and(|current| {
                                matches!(current.phase, crate::tabs::TabPhase::Failed(_))
                            }) =>
                        {
                            Some(error.to_string())
                        }
                        Err(_) => None,
                    },
                    Err(error) => Some(error),
                };
                if let Some(message) = issue {
                    report.issues.push(RestoreIssue {
                        tab: tab.0,
                        profile: Some(profile),
                        message,
                    });
                }
                progress.send_replace(report.clone());
            }
        }
        if *self.subscribe_phase().borrow() == ApplicationPhase::Running
            && let Some(selected) = selected_tab
        {
            self.tabs.select_at(Some(selected), None);
        }
        Ok(())
    }

    fn open_initial_workspace_tab(
        &self,
        profile: Option<ConnectionProfileId>,
        draft: String,
    ) -> Result<Option<TabId>, WorkspaceError> {
        let phase = self.subscribe_phase();
        let phase = phase.borrow();
        if *phase != ApplicationPhase::Running {
            return Err(WorkspaceError("Application is closing".into()));
        }
        Ok(self.tabs.open_if_empty(profile, draft))
    }

    pub async fn save_workspace(
        &self,
        location: WorkspaceLocation,
        drafts: HashMap<TabId, String>,
    ) -> Result<(), WorkspaceError> {
        if *self.subscribe_phase().borrow() != ApplicationPhase::Running {
            return Err(WorkspaceError("Application is closing".into()));
        }
        let tabs = self.tabs.snapshot();
        if drafts
            .keys()
            .any(|id| !tabs.tabs.iter().any(|tab| tab.id == *id))
        {
            return Err(WorkspaceError(
                "Workspace draft refers to a closed tab".into(),
            ));
        }
        let saved = SavedWorkspace {
            version: 1,
            selected: tabs
                .tabs
                .iter()
                .position(|tab| Some(tab.id) == tabs.selected)
                .unwrap_or(0) as i64,
            tabs: tabs
                .tabs
                .iter()
                .map(|tab| SavedTab {
                    profile: tab.profile.as_ref().map(|profile| profile.0.clone()),
                    draft: drafts
                        .get(&tab.id)
                        .cloned()
                        .unwrap_or_else(|| tab.document.draft()),
                })
                .collect(),
        };
        location.save(saved).await
    }
}
