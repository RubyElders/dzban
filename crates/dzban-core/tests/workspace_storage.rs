use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use dzban_core::adapter::AdapterRegistry;
use dzban_core::application::ApplicationContext;
use dzban_core::workspace::{SavedTab, SavedWorkspace, WorkspaceHost, WorkspaceLocation};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("dzban-workspace-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn location(&self) -> WorkspaceLocation {
        WorkspaceLocation {
            host: WorkspaceHost::Macos,
            path: Some(self.0.join("workspace.json")),
        }
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn saved(draft: &str) -> SavedWorkspace {
    SavedWorkspace {
        version: 1,
        tabs: vec![
            SavedTab {
                profile: Some("fixture".into()),
                draft: draft.into(),
            },
            SavedTab {
                profile: None,
                draft: "unconnected 😀".into(),
            },
        ],
        selected: 1,
    }
}

#[test]
fn host_workspace_namespaces_preserve_the_native_path_and_stay_separate() {
    let mut paths = Vec::new();
    for host in [WorkspaceHost::Macos, WorkspaceHost::Gtk, WorkspaceHost::Cli] {
        let location = WorkspaceLocation { host, path: None };
        paths.push(location.in_directory(std::path::Path::new("/fixture/data")));
    }
    assert_eq!(
        paths[0],
        PathBuf::from("/fixture/data/dzban/native-workspace.json")
    );
    assert_ne!(paths[0], paths[1]);
    assert_ne!(paths[0], paths[2]);
    assert_ne!(paths[1], paths[2]);
}

#[tokio::test]
async fn legacy_workspace_round_trips_unicode_optional_profiles_and_private_atomic_files() {
    let directory = Directory::new();
    let location = directory.location();
    assert!(location.load().await.unwrap().is_none());
    let expected = saved("SELECT 'žluť\n😀';");
    std::fs::write(location.path().unwrap(), br#"{"version":1,"tabs":[{"profile":"fixture","draft":"first"},{"draft":"unconnected"}],"selected":-5}"#).unwrap();
    let legacy = location.load().await.unwrap().unwrap();
    assert_eq!(legacy.selected, -5);
    assert_eq!(legacy.tabs[1].profile, None);
    location.save(expected.clone()).await.unwrap();
    assert_eq!(location.load().await.unwrap(), Some(expected));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(location.path().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 1);
}

#[tokio::test]
async fn malformed_unsupported_and_empty_workspaces_fall_back_to_one_chooser() {
    for (content, error) in [
        ("", true),
        ("{", true),
        (r#"{"version":1,"tabs":[],"selected":0}"#, false),
        (
            r#"{"version":0,"tabs":[{"draft":"old"}],"selected":0}"#,
            false,
        ),
    ] {
        let directory = Directory::new();
        let location = directory.location();
        std::fs::write(location.path().unwrap(), content).unwrap();
        let context = Arc::new(ApplicationContext::new(AdapterRegistry::default()));
        let result = context.start_restore_workspace(location, None).wait().await;
        assert!(result.finished && !result.restored);
        assert_eq!(result.error.is_some(), error);
        let tabs = context.tabs.snapshot();
        assert_eq!(tabs.tabs.len(), 1);
        assert_eq!(tabs.selected, Some(tabs.tabs[0].id));
        assert!(tabs.tabs[0].document.draft().is_empty());
        context.start_shutdown().unwrap().wait().await.unwrap();
    }
}

#[tokio::test]
async fn save_uses_canonical_order_selection_and_explicit_editor_drafts() {
    let directory = Directory::new();
    let location = directory.location();
    let context = Arc::new(ApplicationContext::new(AdapterRegistry::default()));
    let first = context
        .open_tab(
            Some(dzban_core::ConnectionProfileId("fixture".into())),
            "old".into(),
        )
        .unwrap();
    let second = context.open_tab(None, "chooser".into()).unwrap();
    context.tabs.select_at(Some(first), None);
    context
        .save_workspace(
            location.clone(),
            HashMap::from([(first, "unsent 😀".into())]),
        )
        .await
        .unwrap();
    let persisted = location.load().await.unwrap().unwrap();
    assert_eq!(persisted.selected, 0);
    assert_eq!(persisted.tabs[0].draft, "unsent 😀");
    assert_eq!(persisted.tabs[0].profile.as_deref(), Some("fixture"));
    assert_eq!(persisted.tabs[1].draft, "chooser");
    assert_eq!(context.tabs.find(first).unwrap().document.draft(), "old");
    context
        .start_close_tab(second)
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert!(
        context
            .save_workspace(location.clone(), HashMap::from([(second, "stale".into())]))
            .await
            .is_err()
    );
    assert_eq!(location.load().await.unwrap(), Some(persisted));
    context.start_shutdown().unwrap().wait().await.unwrap();
    assert!(
        context
            .save_workspace(location.clone(), HashMap::new())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn concurrent_workspace_writes_publish_complete_snapshots_without_temporary_files() {
    let directory = Directory::new();
    let location = directory.location();
    let first = saved(&"😀".repeat(1000));
    let second = saved(&"next".repeat(1000));
    location.save(first.clone()).await.unwrap();
    let a = {
        let location = location.clone();
        let first = first.clone();
        tokio::spawn(async move {
            for _ in 0..10 {
                location.save(first.clone()).await.unwrap();
            }
        })
    };
    let b = {
        let location = location.clone();
        let second = second.clone();
        tokio::spawn(async move {
            for _ in 0..10 {
                location.save(second.clone()).await.unwrap();
            }
        })
    };
    for _ in 0..20 {
        let loaded = location.load().await.unwrap().unwrap();
        assert!(loaded == first || loaded == second);
    }
    a.await.unwrap();
    b.await.unwrap();
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 1);
}

#[tokio::test]
async fn failed_atomic_publication_cleans_its_private_temporary_file() {
    let directory = Directory::new();
    let location = directory.location();
    std::fs::create_dir(location.path().unwrap()).unwrap();
    assert!(location.save(saved("draft")).await.is_err());
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 1);
    assert!(location.path().unwrap().is_dir());
}

#[tokio::test]
async fn restoration_does_not_replace_or_append_to_an_existing_workspace() {
    let directory = Directory::new();
    let location = directory.location();
    location.save(saved("stored")).await.unwrap();
    let context = Arc::new(ApplicationContext::new(AdapterRegistry::default()));
    let existing = context.open_tab(None, "already open".into()).unwrap();
    let result = context.start_restore_workspace(location, None).wait().await;
    assert!(!result.restored && result.error.is_none());
    let tabs = context.tabs.snapshot();
    assert_eq!(tabs.tabs.len(), 1);
    assert_eq!(tabs.selected, Some(existing));
    assert_eq!(tabs.tabs[0].document.draft(), "already open");
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn workspace_size_limits_reject_oversized_input_and_keep_the_last_saved_file() {
    let directory = Directory::new();
    let location = directory.location();
    let previous = saved("keep");
    location.save(previous.clone()).await.unwrap();
    let oversized = saved(&"x".repeat(32 * 1024 * 1024));
    assert!(
        location
            .save(oversized)
            .await
            .unwrap_err()
            .to_string()
            .contains("32 MiB")
    );
    assert_eq!(location.load().await.unwrap(), Some(previous));
    std::fs::OpenOptions::new()
        .write(true)
        .open(location.path().unwrap())
        .unwrap()
        .set_len(32 * 1024 * 1024 + 1)
        .unwrap();
    assert!(
        location
            .load()
            .await
            .unwrap_err()
            .to_string()
            .contains("32 MiB")
    );
}
