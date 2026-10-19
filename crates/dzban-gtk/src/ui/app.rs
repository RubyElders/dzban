use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;

use dzban_core::application::ApplicationContext;
use dzban_core::commands::HistoryLocation;
use dzban_core::workspace::{WorkspaceHost, WorkspaceLocation};
use dzban_core::{Config, DatabaseBackend};
use dzban_gtk::runtime::{Dropped, RuntimeBridge};
use gtk4::glib;
use libadwaita as adw;
use tokio::sync::watch;

#[derive(Clone, Debug, Default)]
pub enum ConfigState {
    #[default]
    Loading,
    Ready(Config),
    Failed(String),
}

struct Inner {
    bridge: RuntimeBridge,
    config: RefCell<ConfigState>,
    config_listeners: RefCell<Vec<Rc<dyn Fn()>>>,
    fixed_config: Option<Config>,
    history: HistoryLocation,
    workspace: WorkspaceLocation,
    toasts: RefCell<Option<adw::ToastOverlay>>,
}

#[derive(Clone)]
pub struct App(Rc<Inner>);

pub struct Observer(Option<glib::JoinHandle<()>>);

impl Drop for Observer {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

pub fn observe<T: Clone + 'static>(
    mut receiver: watch::Receiver<T>,
    callback: impl Fn(T) + 'static,
) -> Observer {
    Observer(Some(glib::spawn_future_local(async move {
        loop {
            let value = receiver.borrow_and_update().clone();
            callback(value);
            if receiver.changed().await.is_err() {
                return;
            }
        }
    })))
}

pub fn observe_changes<T: 'static>(
    mut receiver: watch::Receiver<T>,
    callback: impl Fn() + 'static,
) -> Observer {
    Observer(Some(glib::spawn_future_local(async move {
        loop {
            receiver.borrow_and_update();
            callback();
            if receiver.changed().await.is_err() {
                return;
            }
        }
    })))
}

impl App {
    pub fn new(bridge: RuntimeBridge) -> Self {
        Self::build(
            bridge,
            None,
            HistoryLocation::Default,
            WorkspaceLocation {
                host: WorkspaceHost::Gtk,
                path: None,
            },
        )
    }

    #[cfg(test)]
    pub fn with_fixture(
        bridge: RuntimeBridge,
        config: Config,
        directory: &std::path::Path,
    ) -> Self {
        Self::build(
            bridge,
            Some(config),
            HistoryLocation::File(directory.join("history.json")),
            WorkspaceLocation {
                host: WorkspaceHost::Gtk,
                path: Some(directory.join("gtk-workspace.json")),
            },
        )
    }

    fn build(
        bridge: RuntimeBridge,
        fixed_config: Option<Config>,
        history: HistoryLocation,
        workspace: WorkspaceLocation,
    ) -> Self {
        Self(Rc::new(Inner {
            bridge,
            config: RefCell::new(ConfigState::Loading),
            config_listeners: RefCell::new(Vec::new()),
            fixed_config,
            history,
            workspace,
            toasts: RefCell::new(None),
        }))
    }

    pub fn bridge(&self) -> &RuntimeBridge {
        &self.0.bridge
    }

    pub fn context(&self) -> &Arc<ApplicationContext> {
        self.0.bridge.context()
    }

    pub fn run<F>(
        &self,
        future: F,
    ) -> impl Future<Output = Result<F::Output, Dropped>> + 'static + use<F>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.0.bridge.run(future)
    }

    pub fn history_location(&self) -> HistoryLocation {
        self.0.history.clone()
    }

    pub fn workspace_location(&self) -> WorkspaceLocation {
        self.0.workspace.clone()
    }

    pub fn set_toast_overlay(&self, overlay: &adw::ToastOverlay) {
        self.0.toasts.replace(Some(overlay.clone()));
    }

    pub fn toast(&self, message: &str) {
        tracing::info!("{message}");
        if let Some(overlay) = self.0.toasts.borrow().as_ref() {
            let toast = adw::Toast::new(message);
            toast.set_timeout(4);
            overlay.add_toast(toast);
        }
    }

    pub fn config_state(&self) -> ConfigState {
        self.0.config.borrow().clone()
    }

    pub fn config(&self) -> Option<Config> {
        match &*self.0.config.borrow() {
            ConfigState::Ready(config) => Some(config.clone()),
            _ => None,
        }
    }

    pub fn profiles(&self) -> Vec<(String, DatabaseBackend)> {
        self.config()
            .map(|config| {
                config
                    .databases
                    .iter()
                    .map(|profile| (profile.name.clone(), profile.backend))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn backend(&self, profile: &str) -> Option<DatabaseBackend> {
        self.profiles()
            .into_iter()
            .find(|(name, _)| name == profile)
            .map(|(_, backend)| backend)
    }

    pub fn connect_config_changed(&self, callback: impl Fn() + 'static) {
        self.0.config_listeners.borrow_mut().push(Rc::new(callback));
    }

    fn set_config(&self, state: ConfigState) {
        self.0.config.replace(state);
        let listeners = self.0.config_listeners.borrow().clone();
        for listener in listeners {
            listener();
        }
    }

    pub fn reload_config(&self) -> impl Future<Output = ConfigState> + 'static + use<> {
        let app = self.clone();
        let fixed = self.0.fixed_config.clone();
        let loading = self.run(async move {
            match fixed {
                Some(config) => Ok(config),
                None => tokio::task::spawn_blocking(Config::load)
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result.map_err(|error| error.to_string())),
            }
        });
        async move {
            let state = match loading.await {
                Ok(Ok(config)) => ConfigState::Ready(config),
                Ok(Err(error)) => ConfigState::Failed(error),
                Err(error) => ConfigState::Failed(error.to_string()),
            };
            app.set_config(state.clone());
            state
        }
    }

    pub fn resolve_profile(&self, profile: &str) -> Result<dzban_core::DatabaseConfig, String> {
        let config = self
            .config()
            .ok_or_else(|| "Connection profiles are not loaded".to_string())?;
        config
            .resolve_profile(&dzban_core::ConnectionProfileId(profile.to_string()), None)
            .map_err(|error| error.to_string())
    }
}
