use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use dzban_core::application::ApplicationPhase;
use dzban_core::attention::{AttentionSnapshot, CompletionOutcome};
use dzban_core::health::HealthSnapshot;
use dzban_core::presentation::ExecutionPhase;
use dzban_core::tabs::{QueryTab, TabId, TabPhase, TabsSnapshot};
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

use super::app::{App, Observer, observe};
use super::icons;
use super::tab_page::{TabEvent, TabPage};

type Listener = Rc<dyn Fn(WorkspaceEvent)>;
type CloseHandler = Rc<dyn Fn(TabId)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceEvent {
    Selection,
    Tab(TabEvent),
    Health,
    Attention,
    Empty,
}

struct Entry {
    id: TabId,
    page: adw::TabPage,
    content: TabPage,
}

struct Inner {
    app: App,
    view: adw::TabView,
    bar: adw::TabBar,
    container: gtk4::Box,
    stack: gtk4::Stack,
    entries: RefCell<Vec<Entry>>,
    syncing: Cell<bool>,
    removing: RefCell<HashSet<TabId>>,
    selected: Cell<Option<TabId>>,
    health: RefCell<HealthSnapshot>,
    attention: RefCell<AttentionSnapshot>,
    tracked: RefCell<HashSet<TabId>>,
    history_visible: Cell<bool>,
    restoring: Cell<bool>,
    observers: RefCell<Vec<Observer>>,
    listeners: RefCell<Vec<Listener>>,
    close_handler: RefCell<Option<CloseHandler>>,
}

#[derive(Clone)]
pub struct Workspace(Rc<Inner>);

impl Workspace {
    pub fn new(app: &App) -> Self {
        let view = adw::TabView::new();
        view.set_vexpand(true);
        view.set_shortcuts(adw::TabViewShortcuts::empty());
        let bar = adw::TabBar::new();
        bar.set_view(Some(&view));
        bar.set_autohide(false);
        bar.set_expand_tabs(false);
        bar.update_property(&[gtk4::accessible::Property::Label("Query tabs")]);
        let new_tab = gtk4::Button::from_icon_name("tab-new-symbolic");
        new_tab.add_css_class("flat");
        new_tab.set_tooltip_text(Some("New tab: choose a connection (Ctrl+T)"));
        new_tab.update_property(&[
            gtk4::accessible::Property::Label("New Tab"),
            gtk4::accessible::Property::KeyShortcuts("Control+t"),
        ]);
        bar.set_end_action_widget(Some(&new_tab));

        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        container.set_vexpand(true);
        let tabs = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        tabs.append(&bar);
        tabs.append(&view);
        let loading = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        loading.set_halign(gtk4::Align::Center);
        loading.set_valign(gtk4::Align::Center);
        let spinner = gtk4::Spinner::new();
        spinner.set_spinning(true);
        loading.append(&spinner);
        loading.append(&gtk4::Label::new(Some("Restoring workspace")));
        let stack = gtk4::Stack::new();
        stack.set_vexpand(true);
        stack.add_named(&loading, Some("restoring"));
        stack.add_named(&tabs, Some("tabs"));
        container.append(&stack);

        let workspace = Self(Rc::new(Inner {
            app: app.clone(),
            view: view.clone(),
            bar,
            container,
            stack,
            entries: RefCell::new(Vec::new()),
            syncing: Cell::new(false),
            removing: RefCell::new(HashSet::new()),
            selected: Cell::new(None),
            health: RefCell::new(HealthSnapshot::default()),
            attention: RefCell::new(app.context().attention().snapshot()),
            tracked: RefCell::new(HashSet::new()),
            history_visible: Cell::new(false),
            restoring: Cell::new(true),
            observers: RefCell::new(Vec::new()),
            listeners: RefCell::new(Vec::new()),
            close_handler: RefCell::new(None),
        }));

        let weak = Rc::downgrade(&workspace.0);
        new_tab.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                Workspace(inner).new_tab();
            }
        });

        let weak = Rc::downgrade(&workspace.0);
        view.connect_close_page(move |view, page| {
            let Some(inner) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let workspace = Workspace(inner);
            let Some(id) = workspace.page_id(page) else {
                return glib::Propagation::Proceed;
            };
            if workspace.0.removing.borrow().contains(&id) {
                return glib::Propagation::Proceed;
            }
            view.close_page_finish(page, false);
            let handler = workspace.0.close_handler.borrow().clone();
            match handler {
                Some(handler) => handler(id),
                None => workspace.close_tab(id),
            }
            glib::Propagation::Stop
        });

        let weak = Rc::downgrade(&workspace.0);
        view.connect_selected_page_notify(move |view| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            if inner.syncing.get() {
                return;
            }
            let workspace = Workspace(inner);
            let Some(id) = view
                .selected_page()
                .and_then(|page| workspace.page_id(&page))
            else {
                return;
            };
            if workspace.0.selected.get() != Some(id) {
                workspace.0.app.context().tabs.select_at(Some(id), None);
                workspace.reconcile();
            }
        });

        let weak = Rc::downgrade(&workspace.0);
        view.connect_page_reordered(move |_, page, position| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            if inner.syncing.get() {
                return;
            }
            let workspace = Workspace(inner);
            if let Some(id) = workspace.page_id(page) {
                workspace
                    .0
                    .app
                    .context()
                    .tabs
                    .move_tab(id, position.max(0) as usize);
            }
        });

        let weak = Rc::downgrade(&workspace.0);
        let tabs = observe(app.context().tabs.subscribe(), move |_: TabsSnapshot| {
            if let Some(inner) = weak.upgrade() {
                Workspace(inner).reconcile();
            }
        });
        let weak = Rc::downgrade(&workspace.0);
        let health = observe(app.context().health.subscribe(), move |snapshot| {
            if let Some(inner) = weak.upgrade() {
                Workspace(inner).apply_health(snapshot);
            }
        });
        let weak = Rc::downgrade(&workspace.0);
        let attention = observe(app.context().attention().subscribe(), move |snapshot| {
            if let Some(inner) = weak.upgrade() {
                Workspace(inner).apply_attention(snapshot);
            }
        });
        workspace
            .0
            .observers
            .borrow_mut()
            .extend([tabs, health, attention]);
        workspace
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.0.container
    }

    pub fn connect_event(&self, callback: impl Fn(WorkspaceEvent) + 'static) {
        self.0.listeners.borrow_mut().push(Rc::new(callback));
    }

    fn emit(&self, event: WorkspaceEvent) {
        let listeners = self.0.listeners.borrow().clone();
        for listener in listeners {
            listener(event);
        }
    }

    pub fn set_close_handler(&self, handler: impl Fn(TabId) + 'static) {
        self.0.close_handler.replace(Some(Rc::new(handler)));
    }

    pub fn request_close(&self, id: TabId) {
        let handler = self.0.close_handler.borrow().clone();
        match handler {
            Some(handler) => handler(id),
            None => self.close_tab(id),
        }
    }

    pub fn set_restoring(&self, restoring: bool) {
        self.0.restoring.set(restoring);
        if !restoring {
            self.reconcile();
        }
        self.0
            .stack
            .set_visible_child_name(if restoring { "restoring" } else { "tabs" });
    }

    #[cfg(test)]
    pub fn is_restoring(&self) -> bool {
        self.0.stack.visible_child_name().as_deref() == Some("restoring")
    }

    fn page_id(&self, page: &adw::TabPage) -> Option<TabId> {
        self.0
            .entries
            .borrow()
            .iter()
            .find(|entry| &entry.page == page)
            .map(|entry| entry.id)
    }

    pub fn page(&self, id: TabId) -> Option<TabPage> {
        self.0
            .entries
            .borrow()
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.content.clone())
    }

    pub fn selected(&self) -> Option<TabPage> {
        self.0.selected.get().and_then(|id| self.page(id))
    }

    pub fn pages(&self) -> Vec<TabPage> {
        self.0
            .entries
            .borrow()
            .iter()
            .map(|entry| entry.content.clone())
            .collect()
    }

    pub fn health(&self) -> HealthSnapshot {
        self.0.health.borrow().clone()
    }

    pub fn attention(&self) -> AttentionSnapshot {
        self.0.attention.borrow().clone()
    }

    pub fn tab_count(&self) -> usize {
        self.0.entries.borrow().len()
    }

    pub fn reconcile(&self) {
        let context = self.0.app.context().clone();
        let snapshot = context.tabs.snapshot();
        let ids = snapshot
            .tabs
            .iter()
            .map(|tab| tab.id)
            .collect::<HashSet<_>>();

        let removed = self
            .0
            .entries
            .borrow()
            .iter()
            .filter(|entry| !ids.contains(&entry.id))
            .map(|entry| (entry.id, entry.page.clone(), entry.content.clone()))
            .collect::<Vec<_>>();
        self.0.syncing.set(true);
        for (id, page, content) in removed {
            self.0.removing.borrow_mut().insert(id);
            self.0.entries.borrow_mut().retain(|entry| entry.id != id);
            self.0.view.close_page(&page);
            self.0.removing.borrow_mut().remove(&id);
            self.0.tracked.borrow_mut().remove(&id);
            content.retire();
        }

        for (index, tab) in snapshot.tabs.iter().enumerate() {
            let existing = self.page(tab.id);
            let content = match existing {
                Some(content) => {
                    content.update(tab);
                    content
                }
                None => self.insert(tab, index),
            };
            let page = self
                .0
                .entries
                .borrow()
                .iter()
                .find(|entry| entry.id == tab.id)
                .map(|entry| entry.page.clone())
                .unwrap();
            if self.0.view.page_position(&page) != index as i32 {
                self.0.view.reorder_page(&page, index as i32);
            }
            self.decorate(&page, &content, tab);
            if tab.phase == TabPhase::Ready
                && tab.engine.is_some()
                && !self.0.tracked.borrow().contains(&tab.id)
            {
                let _runtime = self.0.app.bridge().enter();
                if context.health.track_tab(&context, tab.id).is_ok() {
                    self.0.tracked.borrow_mut().insert(tab.id);
                    let probe = context.health.check_tab(&context, tab.id);
                    let work = self.0.app.run(probe.wait());
                    glib::spawn_future_local(async move {
                        let _ = work.await;
                    });
                }
            }
        }

        let previous = self.0.selected.replace(snapshot.selected);
        if let Some(page) = snapshot.selected.and_then(|id| {
            self.0
                .entries
                .borrow()
                .iter()
                .find(|entry| entry.id == id)
                .map(|entry| entry.page.clone())
        }) && self.0.view.selected_page().as_ref() != Some(&page)
        {
            self.0.view.set_selected_page(&page);
        }
        self.0.syncing.set(false);

        if previous != snapshot.selected {
            if let Some(page) = self.selected() {
                page.set_history_visible(self.0.history_visible.get());
                if page.history_visible() {
                    page.refresh_history();
                }
            }
            self.emit(WorkspaceEvent::Selection);
        }
        if snapshot.tabs.is_empty()
            && !self.0.restoring.get()
            && *context.subscribe_phase().borrow() == ApplicationPhase::Running
        {
            self.emit(WorkspaceEvent::Empty);
        }
    }

    fn insert(&self, tab: &QueryTab, index: usize) -> TabPage {
        let content = TabPage::new(&self.0.app, tab.clone());
        content.chooser().apply_health(&self.0.health.borrow());
        content.set_history_visible(self.0.history_visible.get());
        let page = self.0.view.insert(content.widget(), index as i32);
        let weak = Rc::downgrade(&self.0);
        let id = tab.id;
        content.connect_event(move |event| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let workspace = Workspace(inner);
            if matches!(event, TabEvent::Document | TabEvent::Activity)
                && let Some(entry) = workspace
                    .0
                    .entries
                    .borrow()
                    .iter()
                    .find(|entry| entry.id == id)
            {
                workspace.decorate(&entry.page, &entry.content, &entry.content.tab());
            }
            if event == TabEvent::HistoryClosed {
                workspace.0.history_visible.set(false);
            }
            if workspace.0.selected.get() == Some(id) {
                workspace.emit(WorkspaceEvent::Tab(event));
            }
        });
        self.0.entries.borrow_mut().push(Entry {
            id: tab.id,
            page,
            content: content.clone(),
        });
        content
    }

    fn decorate(&self, page: &adw::TabPage, content: &TabPage, tab: &QueryTab) {
        let profile = tab.profile.as_ref().map(|profile| profile.0.clone());
        let document = tab.document.snapshot();
        let title = match (&profile, &tab.phase, tab.engine.is_some()) {
            (Some(profile), _, true) => format!("{profile} - {}", document.title),
            (Some(profile), TabPhase::Connecting, false) => format!("{profile} - Connecting"),
            (Some(profile), _, false) => format!("{profile} - Reconnect"),
            (None, _, _) => "New Query".to_string(),
        };
        if page.title() != title {
            page.set_title(&title);
        }
        let backend = tab
            .backend
            .or_else(|| profile.as_deref().and_then(|name| self.0.app.backend(name)));
        let icon = backend.map_or("network-server-symbolic", icons::database_backend);
        page.set_icon(Some(&gtk4::gio::ThemedIcon::new(icon)));
        let active = content.document().is_some_and(|document| {
            matches!(
                document.execution,
                ExecutionPhase::Planning | ExecutionPhase::Running | ExecutionPhase::Canceling
            )
        });
        page.set_loading(active || tab.phase == TabPhase::Connecting);
        let unseen = self
            .0
            .attention
            .borrow()
            .tabs
            .iter()
            .find(|attention| attention.tab == tab.id.0)
            .and_then(|attention| attention.unseen.as_ref().map(|unseen| unseen.outcome));
        let (indicator, tooltip) = match unseen {
            Some(CompletionOutcome::Failure) if !active => (
                Some("dialog-error-symbolic"),
                "Query failed - select tab to view",
            ),
            Some(_) if !active => (
                Some(icons::UNSEEN_RESULT),
                "Query completed - select tab to view",
            ),
            _ => (None, ""),
        };
        page.set_indicator_icon(
            indicator
                .map(|icon| gtk4::gio::ThemedIcon::new(icon).upcast::<gtk4::gio::Icon>())
                .as_ref(),
        );
        page.set_indicator_tooltip(tooltip);
        page.set_needs_attention(indicator.is_some());
    }

    fn apply_health(&self, snapshot: HealthSnapshot) {
        let previous = self.0.health.replace(snapshot.clone());
        for session in &snapshot.sessions {
            let recoveries = previous
                .sessions
                .iter()
                .find(|entry| entry.session == session.session)
                .map_or(0, |entry| entry.status.recoveries);
            if session.status.recoveries > recoveries {
                self.0.app.toast("Connection restored. Session settings and open transactions were reset; queries were not replayed.");
            }
        }
        for entry in self.0.entries.borrow().iter() {
            entry.content.chooser().apply_health(&snapshot);
        }
        self.emit(WorkspaceEvent::Health);
    }

    fn apply_attention(&self, snapshot: AttentionSnapshot) {
        self.0.attention.replace(snapshot);
        for entry in self.0.entries.borrow().iter() {
            self.decorate(&entry.page, &entry.content, &entry.content.tab());
        }
        self.emit(WorkspaceEvent::Attention);
    }

    pub fn new_tab(&self) -> Option<TabId> {
        match self.0.app.context().open_tab(None, String::new()) {
            Ok(id) => {
                let reload = self.0.app.reload_config();
                glib::spawn_future_local(async move {
                    reload.await;
                });
                self.reconcile();
                if let Some(page) = self.page(id) {
                    page.focus_primary();
                }
                Some(id)
            }
            Err(error) => {
                self.0.app.toast(&error.to_string());
                None
            }
        }
    }

    pub fn open_profile(&self, profile: &str) {
        let target = match self.selected() {
            Some(page) if matches!(page.tab().phase, TabPhase::Choosing | TabPhase::Failed(_)) => {
                Some(page)
            }
            _ => self.new_tab().and_then(|id| self.page(id)),
        };
        if let Some(page) = target {
            page.connect(profile);
        }
    }

    pub fn close_tab(&self, id: TabId) {
        let context = self.0.app.context().clone();
        let closing = {
            let _runtime = self.0.app.bridge().enter();
            context.start_close_tab(id)
        };
        match closing {
            Ok(handle) => {
                let completion = self.0.app.run(handle.wait());
                let app = self.0.app.clone();
                let weak = Rc::downgrade(&self.0);
                glib::spawn_future_local(async move {
                    let result = completion.await;
                    if let Some(inner) = weak.upgrade() {
                        Workspace(inner).reconcile();
                    }
                    match result {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            app.toast(&format!("Could not close tab: {error}"));
                        }
                        Err(error) => app.toast(&format!("Could not close tab: {error}")),
                    }
                });
            }
            Err(error) => self.0.app.toast(&format!("Could not close tab: {error}")),
        }
        self.reconcile();
    }

    pub fn close_selected(&self) -> bool {
        match self.0.selected.get() {
            Some(id) => {
                self.request_close(id);
                true
            }
            None => false,
        }
    }

    pub fn is_last_chooser(&self, id: TabId) -> bool {
        let snapshot = self.0.app.context().tabs.snapshot();
        snapshot.tabs.len() == 1
            && snapshot.tabs.first().is_some_and(|tab| {
                tab.id == id && tab.engine.is_none() && tab.connection_request.is_none()
            })
    }

    pub fn cycle(&self, direction: i64) -> bool {
        let changed = self.0.app.context().tabs.cycle(direction, None);
        self.reconcile();
        changed
    }

    pub fn select(&self, id: TabId) -> bool {
        let changed = self.0.app.context().tabs.select_at(Some(id), None);
        self.reconcile();
        changed
    }

    pub fn tab_titles(&self) -> Vec<(TabId, String)> {
        let mut titles = self
            .0
            .entries
            .borrow()
            .iter()
            .map(|entry| {
                (
                    self.0.view.page_position(&entry.page),
                    entry.id,
                    entry.page.title().to_string(),
                )
            })
            .collect::<Vec<_>>();
        titles.sort_by_key(|(position, _, _)| *position);
        titles
            .into_iter()
            .map(|(_, id, title)| (id, title))
            .collect()
    }

    #[cfg(test)]
    pub fn bar(&self) -> &adw::TabBar {
        &self.0.bar
    }

    pub fn focus_tabs(&self) -> bool {
        self.0.bar.grab_focus()
    }

    pub fn set_history_visible(&self, visible: bool) {
        self.0.history_visible.set(visible);
        if let Some(page) = self.selected() {
            page.set_history_visible(visible);
            if visible {
                page.refresh_history();
            }
        }
    }

    pub fn history_visible(&self) -> bool {
        self.0.history_visible.get()
    }
}
