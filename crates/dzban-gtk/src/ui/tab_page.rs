use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use dzban_core::application::ApplicationError;
use dzban_core::commands::{CommandError, EditorCommand, EditorIntent, InputCommand};
use dzban_core::execution::SessionId;
use dzban_core::interaction::InputEvent;
use dzban_core::outputs::ResultScope;
use dzban_core::presentation::{
    DocumentPresentation, ExecutionPhase, InteractionPresentation, PlanPresentation,
};
use dzban_core::tabs::{QueryTab, TabId, TabPhase};
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

use super::app::{App, Observer, observe, observe_changes};
use super::chooser::Chooser;
use super::editor::Editor;
use super::history::HistoryPane;
use super::results::ResultPanel;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabEvent {
    Document,
    Interaction,
    Activity,
    Results,
    HistoryClosed,
}

type Listener = Rc<dyn Fn(TabEvent)>;

struct Inner {
    app: App,
    id: TabId,
    root: adw::OverlaySplitView,
    stack: gtk4::Stack,
    chooser: Chooser,
    history: HistoryPane,
    editor: Editor,
    results: ResultPanel,
    tab: RefCell<QueryTab>,
    session: Cell<Option<SessionId>>,
    document: RefCell<Option<DocumentPresentation>>,
    interaction: RefCell<Option<InteractionPresentation>>,
    observers: RefCell<Vec<Observer>>,
    session_observers: RefCell<Vec<Observer>>,
    interaction_observer: RefCell<Option<Observer>>,
    applying_draft: Cell<bool>,
    inputs: RefCell<VecDeque<InputEvent>>,
    input_busy: Cell<bool>,
    history_request: Cell<u64>,
    listeners: RefCell<Vec<Listener>>,
    retired: Cell<bool>,
}

#[derive(Clone)]
pub struct TabPage(Rc<Inner>);

impl TabPage {
    pub fn new(app: &App, tab: QueryTab) -> Self {
        let chooser = Chooser::new();
        let history = HistoryPane::new();
        let editor = Editor::new();
        let results = ResultPanel::new(app.clone(), tab.id);

        let paned = gtk4::Paned::new(gtk4::Orientation::Vertical);
        paned.add_css_class("query-split");
        paned.set_wide_handle(false);
        paned.set_shrink_start_child(false);
        paned.set_shrink_end_child(false);
        paned.set_resize_start_child(false);
        paned.set_position(220);
        paned.set_start_child(Some(editor.widget()));
        paned.set_end_child(Some(results.widget()));

        let stack = gtk4::Stack::new();
        stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
        stack.add_named(chooser.widget(), Some("chooser"));
        stack.add_named(&paned, Some("workspace"));
        stack.set_visible_child_name("chooser");

        let root = adw::OverlaySplitView::new();
        root.set_sidebar(Some(history.widget()));
        root.set_content(Some(&stack));
        root.set_collapsed(false);
        root.set_min_sidebar_width(240.0);
        root.set_max_sidebar_width(320.0);
        root.set_sidebar_width_fraction(0.22);
        root.set_show_sidebar(false);
        root.set_vexpand(true);

        let page = Self(Rc::new(Inner {
            app: app.clone(),
            id: tab.id,
            root,
            stack,
            chooser,
            history,
            editor,
            results,
            tab: RefCell::new(tab.clone()),
            session: Cell::new(None),
            document: RefCell::new(None),
            interaction: RefCell::new(None),
            observers: RefCell::new(Vec::new()),
            session_observers: RefCell::new(Vec::new()),
            interaction_observer: RefCell::new(None),
            applying_draft: Cell::new(false),
            inputs: RefCell::new(VecDeque::new()),
            input_busy: Cell::new(false),
            history_request: Cell::new(0),
            listeners: RefCell::new(Vec::new()),
            retired: Cell::new(false),
        }));

        page.0.editor.set_text(&tab.document.draft());
        let weak = Rc::downgrade(&page.0);
        page.0.editor.connect_changed(move || {
            if let Some(inner) = weak.upgrade()
                && !inner.applying_draft.get()
            {
                inner.tab.borrow().document.set_draft(inner.editor.text());
            }
        });
        let weak = Rc::downgrade(&page.0);
        page.0.editor.connect_context_changed(move || {
            if let Some(inner) = weak.upgrade()
                && !inner.applying_draft.get()
            {
                inner.tab.borrow().document.query_context_changed();
            }
        });

        let weak = Rc::downgrade(&page.0);
        page.0
            .observers
            .borrow_mut()
            .push(observe_changes(tab.document.subscribe(), move || {
                if let Some(inner) = weak.upgrade() {
                    TabPage(inner).document_changed();
                }
            }));

        let weak = Rc::downgrade(&page.0);
        page.0.results.connect_changed(move || {
            if let Some(inner) = weak.upgrade() {
                TabPage(inner).emit(TabEvent::Results);
            }
        });

        page.bind_chooser();
        page.bind_history();
        page.update(&tab);
        page
    }

    fn bind_chooser(&self) {
        let app = &self.0.app;
        let chooser = &self.0.chooser;
        let weak = Rc::downgrade(&self.0);
        chooser.connect_open(move |profile| {
            if let Some(inner) = weak.upgrade() {
                TabPage(inner).connect(&profile);
            }
        });
        let probe_app = app.clone();
        chooser.connect_test(move |profile| {
            let context = probe_app.context().clone();
            let config = probe_app.config();
            let work = probe_app.run(async move {
                context
                    .health
                    .check_named_profile(
                        context.clone(),
                        config.as_ref(),
                        dzban_core::ConnectionProfileId(profile),
                    )
                    .await
            });
            glib::spawn_future_local(async move {
                let _ = work.await;
            });
        });
        let probe_app = app.clone();
        chooser.connect_test_all(move || test_all(&probe_app));
        let reload_app = app.clone();
        chooser.connect_reload(move || {
            let reload = reload_app.reload_config();
            glib::spawn_future_local(async move {
                reload.await;
            });
        });
        let weak = Rc::downgrade(&self.0);
        chooser.connect_cancel(move || {
            if let Some(inner) = weak.upgrade() {
                TabPage(inner).cancel_chooser();
            }
        });
        self.apply_profiles();
        let weak = Rc::downgrade(&self.0);
        app.connect_config_changed(move || {
            if let Some(inner) = weak.upgrade() {
                TabPage(inner).apply_profiles();
            }
        });
    }

    fn apply_profiles(&self) {
        self.0
            .chooser
            .set_profiles(&self.0.app.config_state(), &self.0.app.profiles());
    }

    fn bind_history(&self) {
        let weak = Rc::downgrade(&self.0);
        self.0.history.connect_refresh(move || {
            if let Some(inner) = weak.upgrade() {
                TabPage(inner).refresh_history();
            }
        });
        let weak = Rc::downgrade(&self.0);
        self.0.history.connect_restore(move |revision, index| {
            if let Some(inner) = weak.upgrade() {
                TabPage(inner).restore_history(revision, index);
            }
        });
        let weak = Rc::downgrade(&self.0);
        self.0.history.connect_close(move || {
            if let Some(inner) = weak.upgrade() {
                inner.root.set_show_sidebar(false);
                TabPage(inner.clone()).emit(TabEvent::HistoryClosed);
                inner.editor.focus();
            }
        });
    }

    pub fn widget(&self) -> &adw::OverlaySplitView {
        &self.0.root
    }

    pub fn id(&self) -> TabId {
        self.0.id
    }

    pub fn tab(&self) -> QueryTab {
        self.0.tab.borrow().clone()
    }

    pub fn session(&self) -> Option<SessionId> {
        self.0.session.get()
    }

    pub fn document(&self) -> Option<DocumentPresentation> {
        self.0.document.borrow().clone()
    }

    pub fn interaction(&self) -> Option<InteractionPresentation> {
        self.0.interaction.borrow().clone()
    }

    pub fn results(&self) -> &ResultPanel {
        &self.0.results
    }

    pub fn editor(&self) -> &Editor {
        &self.0.editor
    }

    pub fn chooser(&self) -> &Chooser {
        &self.0.chooser
    }

    pub fn history(&self) -> &HistoryPane {
        &self.0.history
    }

    pub fn connect_event(&self, callback: impl Fn(TabEvent) + 'static) {
        self.0.listeners.borrow_mut().push(Rc::new(callback));
    }

    fn emit(&self, event: TabEvent) {
        if self.0.retired.get() {
            return;
        }
        let listeners = self.0.listeners.borrow().clone();
        for listener in listeners {
            listener(event);
        }
    }

    pub fn is_workspace(&self) -> bool {
        self.0.stack.visible_child_name().as_deref() == Some("workspace")
    }

    #[cfg(test)]
    pub fn is_transitioning(&self) -> bool {
        self.0.stack.is_transition_running()
    }

    pub fn is_ready(&self) -> bool {
        let tab = self.0.tab.borrow();
        tab.phase == TabPhase::Ready && tab.engine.is_some()
    }

    pub fn update(&self, tab: &QueryTab) {
        if self.0.retired.get() {
            return;
        }
        self.0.tab.replace(tab.clone());
        let session = tab.engine.as_ref().map(|engine| engine.id());
        if session != self.0.session.get() {
            self.0.session.set(session);
            self.bind_session(tab);
        }
        let workspace =
            tab.engine.is_some() && matches!(tab.phase, TabPhase::Ready | TabPhase::Closing);
        let previous = self.is_workspace();
        self.0
            .stack
            .set_visible_child_name(if workspace { "workspace" } else { "chooser" });
        self.0.chooser.set_pending(match tab.phase {
            TabPhase::Connecting => tab.profile.as_ref().map(|profile| profile.0.as_str()),
            _ => None,
        });
        self.0
            .chooser
            .set_available(!matches!(tab.phase, TabPhase::Closing));
        if workspace && !previous && self.0.root.root().is_some() {
            let editor = self.0.editor.clone();
            glib::idle_add_local_once(move || {
                editor.focus();
            });
        }
        self.emit(TabEvent::Activity);
    }

    fn bind_session(&self, tab: &QueryTab) {
        self.0.session_observers.borrow_mut().clear();
        self.0.interaction_observer.replace(None);
        self.0.interaction.replace(None);
        match &tab.engine {
            Some(engine) => {
                let mut observers = self.0.session_observers.borrow_mut();
                for receiver in [
                    observe_changes(engine.subscribe_operation(), self.activity_callback()),
                    observe_changes(engine.subscribe(), self.activity_callback()),
                    observe_changes(engine.subscribe_session(), self.activity_callback()),
                ] {
                    observers.push(receiver);
                }
                drop(observers);
                self.observe_interaction();
                self.refresh_history();
            }
            None => self.0.history.set_unavailable(),
        }
    }

    fn activity_callback(&self) -> impl Fn() + 'static {
        let weak = Rc::downgrade(&self.0);
        move || {
            if let Some(inner) = weak.upgrade() {
                TabPage(inner).emit(TabEvent::Activity);
            }
        }
    }

    fn observe_interaction(&self) {
        if self.0.interaction_observer.borrow().is_some() {
            return;
        }
        let Ok(receiver) = self.0.app.context().tabs.subscribe_interaction(self.0.id) else {
            return;
        };
        let weak = Rc::downgrade(&self.0);
        let observer = observe(receiver, move |projection| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let page = TabPage(inner);
            if let Some(projection) = projection {
                page.apply_interaction(projection);
            }
        });
        self.0.interaction_observer.replace(Some(observer));
    }

    fn apply_interaction(&self, projection: InteractionPresentation) {
        let newer = self
            .0
            .interaction
            .borrow()
            .as_ref()
            .is_none_or(|current| projection.revision > current.revision);
        if !newer {
            return;
        }
        let retired = projection.retired;
        self.0.interaction.replace(Some(projection));
        if retired {
            self.0.interaction_observer.replace(None);
        }
        self.emit(TabEvent::Interaction);
    }

    fn document_changed(&self) {
        if self.0.retired.get() {
            return;
        }
        let tab = self.0.tab.borrow().clone();
        let presentation = self.0.app.context().outputs.document(&tab.document);
        let previous = self.0.document.replace(Some(presentation.clone()));
        if presentation.draft != self.0.editor.text() {
            self.0.applying_draft.set(true);
            self.0.editor.set_text(&presentation.draft);
            self.0.applying_draft.set(false);
        }
        let draft_only = previous.as_ref().is_some_and(|previous| {
            let mut previous = previous.clone();
            previous.draft.clone_from(&presentation.draft);
            previous.draft_revision = presentation.draft_revision;
            previous == presentation
        });
        if !draft_only {
            self.0.results.render(&presentation);
        }
        let finished = previous.as_ref().is_some_and(|previous| {
            matches!(
                previous.execution,
                ExecutionPhase::Planning | ExecutionPhase::Running | ExecutionPhase::Canceling
            )
        }) && !matches!(
            presentation.execution,
            ExecutionPhase::Planning | ExecutionPhase::Running | ExecutionPhase::Canceling
        );
        if finished {
            self.refresh_history();
        }
        self.emit(TabEvent::Document);
    }

    pub fn connect(&self, profile: &str) {
        let config = match self.0.app.resolve_profile(profile) {
            Ok(config) => config,
            Err(error) => {
                self.0.chooser.record_failure(profile, &error);
                self.0
                    .app
                    .toast(&format!("Could not open {profile}: {error}"));
                return;
            }
        };
        let handle = {
            let _runtime = self.0.app.bridge().enter();
            self.0.app.context().start_connect_in_tab(self.0.id, config)
        };
        self.0.chooser.set_pending(Some(profile));
        let completion = self.0.app.run(handle.wait());
        let weak = Rc::downgrade(&self.0);
        let profile = profile.to_string();
        glib::spawn_future_local(async move {
            let result = completion.await.unwrap_or_else(|error| {
                Err(ApplicationError::Operation(
                    dzban_core::error::ExecutionError::Message(error.to_string()),
                ))
            });
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let page = TabPage(inner);
            let Err(error) = result else {
                return;
            };
            let failed = page
                .0
                .app
                .context()
                .tabs
                .find(page.0.id)
                .is_some_and(|tab| matches!(tab.phase, TabPhase::Failed(_)));
            if failed {
                let message = error.to_string();
                page.0.chooser.record_failure(&profile, &message);
                page.0
                    .app
                    .context()
                    .health
                    .fail_profile(profile.clone(), message.clone());
                page.0
                    .app
                    .toast(&format!("Could not connect to {profile}: {message}"));
            }
        });
    }

    pub fn cancel_chooser(&self) {
        let phase = self.0.tab.borrow().phase.clone();
        let context = self.0.app.context().clone();
        let closing = {
            let _runtime = self.0.app.bridge().enter();
            match phase {
                TabPhase::Connecting => Some(context.start_cancel_connection(self.0.id)),
                TabPhase::Choosing | TabPhase::Failed(_)
                    if context.tabs.snapshot().tabs.len() > 1 =>
                {
                    Some(context.start_close_tab(self.0.id))
                }
                _ => None,
            }
        };
        let Some(closing) = closing else {
            return;
        };
        match closing {
            Ok(handle) => {
                let completion = self.0.app.run(handle.wait());
                let app = self.0.app.clone();
                glib::spawn_future_local(async move {
                    if let Ok(Err(error)) = completion.await {
                        app.toast(&error.to_string());
                    }
                });
            }
            Err(error) => self.0.app.toast(&error.to_string()),
        }
    }

    pub fn send_input(&self, event: InputEvent) {
        if self.0.retired.get() || !self.is_ready() {
            return;
        }
        self.0.inputs.borrow_mut().push_back(event);
        if !self.0.input_busy.get() {
            self.next_input();
        }
    }

    fn next_input(&self) {
        let Some(event) = self.0.inputs.borrow_mut().pop_front() else {
            self.0.input_busy.set(false);
            return;
        };
        self.0.input_busy.set(true);
        let tab = self.0.tab.borrow().clone();
        let command = InputCommand {
            tab: self.0.id,
            document: tab.document.id(),
            revision: tab.document.snapshot().draft_revision,
            draft: self.0.editor.text(),
            selection: self.0.editor.scalar_selection(),
            event,
        };
        let handle = {
            let _runtime = self.0.app.bridge().enter();
            self.0
                .app
                .context()
                .start_input_command(command, self.0.app.history_location())
        };
        let completion = self.0.app.run(handle.wait());
        let weak = Rc::downgrade(&self.0);
        glib::spawn_future_local(async move {
            let result = completion.await;
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let page = TabPage(inner);
            match result {
                Ok(Ok(outcome)) => {
                    page.observe_interaction();
                    page.apply_interaction(outcome.interaction);
                }
                Ok(Err(error)) => page.report_command(error),
                Err(error) => tracing::warn!("Input command was dropped: {error}"),
            }
            page.emit(TabEvent::Activity);
            page.next_input();
        });
    }

    pub fn editor_command(&self, intent: EditorIntent) {
        if self.0.retired.get() || !self.is_ready() {
            return;
        }
        let tab = self.0.tab.borrow().clone();
        let command = EditorCommand {
            tab: self.0.id,
            document: tab.document.id(),
            draft: self.0.editor.text(),
            selection: self.0.editor.scalar_selection(),
            intent,
        };
        let handle = {
            let _runtime = self.0.app.bridge().enter();
            self.0
                .app
                .context()
                .start_editor_command(command, self.0.app.history_location())
        };
        let completion = self.0.app.run(handle.wait());
        let weak = Rc::downgrade(&self.0);
        glib::spawn_future_local(async move {
            let result = completion.await;
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let page = TabPage(inner);
            match result {
                Ok(Ok(_)) => page.observe_interaction(),
                Ok(Err(error)) => page.report_command(error),
                Err(error) => tracing::warn!("Editor command was dropped: {error}"),
            }
            page.emit(TabEvent::Activity);
        });
    }

    fn report_command(&self, error: CommandError) {
        match error {
            CommandError::Application(ApplicationError::SelectionSuperseded) => {}
            CommandError::Preview(dzban_core::preview::PreviewError::Invalidated(_)) => {}
            error => self.0.app.toast(&error.to_string()),
        }
    }

    pub fn dismiss_plan(&self) {
        if let Some(interaction) = self.0.interaction.borrow_mut().as_mut() {
            interaction.visible = false;
            interaction.loading = false;
            interaction.input.pinned = false;
            interaction.plan = None;
            interaction.error = None;
        }
        let _ = self.0.app.context().tabs.cancel_preview(self.0.id, None);
        self.send_input(InputEvent::PreviewCanceled);
        self.emit(TabEvent::Interaction);
    }

    pub fn refresh_history(&self) {
        let Some(session) = self.0.session.get() else {
            self.0.history.set_unavailable();
            return;
        };
        let request = self.0.history_request.get().wrapping_add(1);
        self.0.history_request.set(request);
        self.0.history.set_loading(true);
        let context = self.0.app.context().clone();
        let location = self.0.app.history_location();
        let list = self
            .0
            .app
            .run(async move { context.history_list(session, location).await });
        let weak = Rc::downgrade(&self.0);
        glib::spawn_future_local(async move {
            let result = list.await;
            let Some(inner) = weak.upgrade() else {
                return;
            };
            if inner.history_request.get() != request || inner.session.get() != Some(session) {
                return;
            }
            match result {
                Ok(Ok(history)) => inner.history.set_presentation(history),
                Ok(Err(error)) => inner.history.set_error(&error.to_string()),
                Err(error) => inner.history.set_error(&error.to_string()),
            }
        });
    }

    fn restore_history(&self, revision: uuid::Uuid, index: usize) {
        let Some(session) = self.0.session.get() else {
            return;
        };
        let document = self.0.tab.borrow().document.id();
        let restored = {
            let _runtime = self.0.app.bridge().enter();
            self.0
                .app
                .context()
                .restore_history(session, document, revision, index)
        };
        match restored {
            Ok(_) => {
                self.document_changed();
                self.0.editor.focus();
            }
            Err(dzban_core::outputs::OutputError::StaleHistory) => {
                self.0.app.toast("History has changed; refreshed the list");
                self.refresh_history();
            }
            Err(error) => self.0.app.toast(&error.to_string()),
        }
    }

    pub fn saved_plan(&self) -> Result<Option<PlanPresentation>, String> {
        let session = self.0.session.get().ok_or("Query tab is not connected")?;
        let document = self.document().ok_or("Query tab has no output")?;
        self.0
            .app
            .context()
            .saved_plan(ResultScope {
                session,
                document: dzban_core::documents::DocumentId(document.document),
                generation: document.generation,
            })
            .map_err(|error| error.to_string())
    }

    pub fn set_history_visible(&self, visible: bool) {
        self.0.root.set_show_sidebar(visible);
    }

    pub fn history_visible(&self) -> bool {
        self.0.root.shows_sidebar()
    }

    pub fn rename(&self, title: &str) -> bool {
        self.0.tab.borrow().document.rename(title)
    }

    pub fn focus_primary(&self) -> bool {
        if self.is_workspace() {
            self.0.editor.focus()
        } else {
            self.0.chooser.focus()
        }
    }

    pub fn retire(&self) {
        self.0.retired.set(true);
        self.0.observers.borrow_mut().clear();
        self.0.session_observers.borrow_mut().clear();
        self.0.interaction_observer.replace(None);
        self.0.inputs.borrow_mut().clear();
        self.0.listeners.borrow_mut().clear();
        self.0.results.retire();
    }
}

pub fn test_all(app: &App) {
    let Some(config) = app.config() else {
        return;
    };
    let context = app.context().clone();
    let work = app.run(async move { context.health.check_all(context.clone(), config).await });
    glib::spawn_future_local(async move {
        let _ = work.await;
    });
}
