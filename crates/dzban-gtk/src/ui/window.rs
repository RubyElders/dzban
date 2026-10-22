use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::time::Instant;

use adw::prelude::*;
use dzban_core::application::ApplicationPhase;
use dzban_core::attention::{AttentionInput, AttentionSnapshot, ViewedOutput};
use dzban_core::commands::EditorIntent;
use dzban_core::interaction::{InputEvent, InputKey, PrimaryAction};
use dzban_core::presentation::{InputHint, PlanPresentation};
use dzban_core::tabs::{TabId, TabPhase};
use dzban_gtk::runtime::RuntimeBridge;
use gtk4::{gdk, gio, glib};
use libadwaita as adw;

use super::action_search::{ActionItem, ActionSearch};
use super::app::App;
use super::format;
use super::navigation;
use super::plan_view::{PlanContent, PlanScroll, PlanView};
use super::shortcuts::{self, ShortcutAction};
use super::statusbar::{FooterState, Statusbar};
use super::tab_page::{TabEvent, TabPage};
use super::workspace::{Workspace, WorkspaceEvent};
use crate::APP_NAME;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Primary,
    Preview,
}

pub fn modifier_role(key: gdk::Key) -> Option<Role> {
    match key {
        gdk::Key::Control_L | gdk::Key::Control_R => Some(Role::Primary),
        gdk::Key::Shift_L | gdk::Key::Shift_R => Some(Role::Preview),
        _ => None,
    }
}

#[derive(Default)]
pub struct KeyState {
    pressed: BTreeSet<gdk::Key>,
    primary: bool,
    preview: bool,
    inspect: bool,
}

impl KeyState {
    pub fn press(&mut self, key: gdk::Key) -> bool {
        self.pressed.insert(key)
    }

    pub fn release(&mut self, key: gdk::Key) -> bool {
        self.pressed.remove(&key)
    }

    fn held(&self, role: Role) -> bool {
        self.pressed
            .iter()
            .any(|key| modifier_role(*key) == Some(role))
    }

    pub fn sync(&mut self, allowed: bool) -> Vec<(InputKey, bool)> {
        let mut events = Vec::new();
        let primary = allowed && self.held(Role::Primary);
        let preview = allowed && self.held(Role::Preview);
        if primary != self.primary {
            self.primary = primary;
            events.push((InputKey::Primary, primary));
        }
        if preview != self.preview {
            self.preview = preview;
            events.push((InputKey::Preview, preview));
        }
        events
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn primary(&self) -> bool {
        self.primary
    }

    pub fn preview(&self) -> bool {
        self.preview
    }
}

pub fn input_hint(hint: &InputHint) -> (String, Option<&'static str>) {
    match hint {
        InputHint::Default => (default_hint(), None),
        InputHint::PrimaryGuide => (
            "Enter: run - Shift: estimate - Shift+Enter: estimate and run".into(),
            Some("guidance"),
        ),
        InputHint::Estimating => (
            "Enter to run - Space for plan - estimating...".into(),
            Some("progress"),
        ),
        InputHint::EstimateReady { plan } => (
            format!(
                "Enter to run - Space for plan - {}",
                format::compact_estimate(plan)
            ),
            Some("ready"),
        ),
        InputHint::EstimateError { error } => (
            format!("Enter to run - estimate unavailable: {error}"),
            Some("error"),
        ),
        InputHint::Planning => (
            "Estimating query before execution...".into(),
            Some("progress"),
        ),
        InputHint::Running { plan: Some(plan) } => (
            format!("Running query - {}", format::compact_estimate(plan)),
            Some("progress"),
        ),
        InputHint::Running { plan: None } => ("Running query...".into(), Some("progress")),
        InputHint::Canceling { plan: Some(plan) } => (
            format!("Stopping query - {}", format::compact_estimate(plan)),
            Some("progress"),
        ),
        InputHint::Canceling { plan: None } => ("Stopping query...".into(), Some("progress")),
    }
}

fn default_hint() -> String {
    format!(
        "Search actions and connections ({})",
        shortcuts::shortcut(ShortcutAction::FocusSearch).accelerator()
    )
}

struct Inner {
    app: App,
    window: adw::ApplicationWindow,
    workspace: Workspace,
    statusbar: Statusbar,
    search: ActionSearch,
    plan: PlanView,
    history_button: gtk4::Button,
    plan_button: gtk4::Button,
    run_button: gtk4::Button,
    shortcuts: adw::Dialog,
    keys: RefCell<KeyState>,
    clock: Instant,
    saved_plan: RefCell<Option<(TabId, u64, PlanPresentation)>>,
    viewed: RefCell<Option<(bool, Option<ViewedOutput>)>>,
    attention_request: Cell<u64>,
    controls: Cell<PrimaryAction>,
    closing: Cell<bool>,
}

#[derive(Clone)]
pub struct Shell(Rc<Inner>);

pub fn build_ui(application: &adw::Application) {
    install_styles();
    let bridge = RuntimeBridge::new().expect("Failed to create the Tokio runtime");
    let app = App::new(bridge);
    let window = adw::ApplicationWindow::builder()
        .application(application)
        .default_width(1150)
        .default_height(760)
        .width_request(900)
        .height_request(500)
        .title(APP_NAME)
        .build();
    let shell = Shell::new(app, window.clone());
    window.present();
    shell.start();
    let retained = RefCell::new(Some(shell));
    window.connect_destroy(move |_| {
        retained.take();
    });
}

impl Shell {
    pub fn new(app: App, window: adw::ApplicationWindow) -> Self {
        let workspace = Workspace::new(&app);
        let statusbar = Statusbar::new();
        let search = ActionSearch::new();
        let plan = PlanView::new();
        let toasts = adw::ToastOverlay::new();
        app.set_toast_overlay(&toasts);

        let header = adw::HeaderBar::new();
        let history_button = gtk4::Button::from_icon_name("sidebar-show-symbolic");
        history_button.set_size_request(34, 34);
        history_button.set_tooltip_text(Some(
            &shortcuts::shortcut(ShortcutAction::ToggleHistory).tooltip(),
        ));
        history_button
            .update_property(&[gtk4::accessible::Property::Label("Toggle query history")]);
        header.pack_start(&history_button);

        let title = adw::Clamp::new();
        title.set_maximum_size(600);
        title.set_tightening_threshold(380);
        title.set_child(Some(search.widget()));
        header.set_title_widget(Some(&title));

        let menu = gio::Menu::new();
        menu.append(Some("Test All Connections"), Some("win.test-all"));
        menu.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
        menu.append(Some("Quit"), Some("win.quit"));
        let menu_button = gtk4::MenuButton::new();
        menu_button.set_icon_name("open-menu-symbolic");
        menu_button.set_menu_model(Some(&menu));
        menu_button.set_tooltip_text(Some("Main Menu"));
        menu_button.update_property(&[gtk4::accessible::Property::Label("Main Menu")]);
        header.pack_end(&menu_button);

        let run_button = gtk4::Button::from_icon_name("media-playback-start-symbolic");
        run_button.set_size_request(34, 34);
        run_button.add_css_class("suggested-action");
        header.pack_end(&run_button);
        let plan_button = gtk4::Button::from_icon_name(super::icons::PLAN);
        plan_button.set_size_request(34, 34);
        plan_button.set_tooltip_text(Some("Preview the current query plan (Ctrl+Shift+Space)"));
        plan_button.update_property(&[
            gtk4::accessible::Property::Label("Preview query plan"),
            gtk4::accessible::Property::KeyShortcuts("Control+Shift+space"),
        ]);
        header.pack_end(&plan_button);

        let overlay = gtk4::Overlay::new();
        overlay.set_child(Some(workspace.widget()));
        overlay.add_overlay(plan.widget());
        toasts.set_child(Some(&overlay));
        toasts.set_vexpand(true);

        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        content.append(&toasts);
        content.append(statusbar.widget());
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&content));
        window.set_content(Some(&toolbar));

        let shell = Self(Rc::new(Inner {
            app,
            window,
            workspace,
            statusbar,
            search,
            plan,
            history_button: history_button.clone(),
            plan_button: plan_button.clone(),
            run_button: run_button.clone(),
            shortcuts: shortcuts::create_dialog(),
            keys: RefCell::new(KeyState::default()),
            clock: Instant::now(),
            saved_plan: RefCell::new(None),
            viewed: RefCell::new(None),
            attention_request: Cell::new(0),
            controls: Cell::new(PrimaryAction::Unavailable),
            closing: Cell::new(false),
        }));

        let weak = shell.weak();
        history_button.connect_clicked(move |_| {
            if let Some(shell) = upgrade(&weak) {
                shell.execute(ShortcutAction::ToggleHistory);
            }
        });
        let weak = shell.weak();
        run_button.connect_clicked(move |_| {
            if let Some(page) = upgrade(&weak).and_then(|shell| shell.0.workspace.selected()) {
                page.editor_command(EditorIntent::Primary);
            }
        });
        let weak = shell.weak();
        plan_button.connect_clicked(move |_| {
            if let Some(shell) = upgrade(&weak) {
                shell.open_plan();
            }
        });
        let weak = shell.weak();
        shell.0.plan.connect_pin(move || {
            if let Some(shell) = upgrade(&weak)
                && shell.0.saved_plan.borrow().is_none()
                && let Some(page) = shell.0.workspace.selected()
            {
                page.send_input(InputEvent::Pin);
            }
        });
        let weak = shell.weak();
        shell.0.plan.connect_close(move || {
            if let Some(shell) = upgrade(&weak) {
                shell.close_plan();
            }
        });

        let weak = shell.weak();
        shell.0.workspace.connect_event(move |event| {
            if let Some(shell) = upgrade(&weak) {
                shell.workspace_event(event);
            }
        });
        let weak = shell.weak();
        shell.0.workspace.set_close_handler(move |id| {
            if let Some(shell) = upgrade(&weak) {
                if shell.0.workspace.is_last_chooser(id) {
                    shell.0.window.close();
                } else {
                    shell.0.workspace.close_tab(id);
                }
            }
        });

        shell.bind_statusbar();
        shell.bind_search();
        shell.bind_actions();
        shell.bind_keys();
        shell.bind_lifecycle();
        shell.refresh();
        shell
    }

    fn weak(&self) -> std::rc::Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    #[cfg(test)]
    pub fn app(&self) -> &App {
        &self.0.app
    }

    #[cfg(test)]
    pub fn workspace(&self) -> &Workspace {
        &self.0.workspace
    }

    pub fn start(&self) {
        let context = self.0.app.context().clone();
        context.attention().input(AttentionInput::Lifecycle {
            restoring: true,
            quitting: false,
        });
        self.0.workspace.set_restoring(true);
        let reload = self.0.app.reload_config();
        let weak = self.weak();
        glib::spawn_future_local(async move {
            let state = reload.await;
            let Some(shell) = upgrade(&weak) else {
                return;
            };
            let config = match state {
                super::app::ConfigState::Ready(config) => Some(config),
                super::app::ConfigState::Failed(error) => {
                    shell
                        .0
                        .app
                        .toast(&format!("Could not load connection profiles: {error}"));
                    None
                }
                super::app::ConfigState::Loading => None,
            };
            let restore = {
                let _runtime = shell.0.app.bridge().enter();
                context.start_restore_workspace(shell.0.app.workspace_location(), config)
            };
            let progress = shell.0.app.run(restore.wait());
            shell.0.workspace.reconcile();
            let progress = progress.await;
            let Some(shell) = upgrade(&weak) else {
                return;
            };
            context.attention().input(AttentionInput::Lifecycle {
                restoring: false,
                quitting: false,
            });
            shell.0.workspace.set_restoring(false);
            if let Ok(progress) = progress {
                for issue in &progress.issues {
                    let profile = issue.profile.as_deref().unwrap_or("A saved tab");
                    shell
                        .0
                        .app
                        .toast(&format!("Could not reopen {profile}: {}", issue.message));
                }
                if let Some(error) = progress.error {
                    shell
                        .0
                        .app
                        .toast(&format!("Could not restore open tabs: {error}"));
                }
            }
            if shell.0.workspace.tab_count() == 0 {
                shell.0.workspace.new_tab();
            }
            if let Some(page) = shell.0.workspace.selected() {
                page.focus_primary();
            }
            shell.refresh();
        });
    }

    fn workspace_event(&self, event: WorkspaceEvent) {
        match event {
            WorkspaceEvent::Selection => {
                self.0.saved_plan.replace(None);
                self.reset_keys();
                self.refresh();
            }
            WorkspaceEvent::Tab(TabEvent::Interaction) => self.render_interaction(),
            WorkspaceEvent::Tab(TabEvent::Document) => {
                let replaced =
                    self.0
                        .saved_plan
                        .borrow()
                        .as_ref()
                        .is_some_and(|(_, generation, _)| {
                            self.0
                                .workspace
                                .selected()
                                .and_then(|page| page.document())
                                .is_none_or(|document| document.generation != *generation)
                        });
                if replaced {
                    self.0.saved_plan.replace(None);
                    self.render_interaction();
                }
                self.refresh_controls();
                self.refresh_footer();
                self.report_attention();
            }
            WorkspaceEvent::Tab(_) | WorkspaceEvent::Health => {
                self.refresh_controls();
                self.refresh_footer();
            }
            WorkspaceEvent::Attention => self.apply_attention(self.0.workspace.attention()),
            WorkspaceEvent::Empty => {
                if !self.0.closing.get() {
                    self.0.workspace.new_tab();
                }
            }
        }
    }

    fn refresh(&self) {
        self.refresh_controls();
        self.render_interaction();
        self.refresh_footer();
        self.report_attention();
        if self.0.workspace.history_visible() {
            self.0.history_button.add_css_class("raised");
        } else {
            self.0.history_button.remove_css_class("raised");
        }
    }

    fn refresh_controls(&self) {
        let ready = self
            .0
            .workspace
            .selected()
            .is_some_and(|page| page.is_ready());
        let action = if ready {
            self.0.app.context().tabs.controls().primary_action
        } else {
            PrimaryAction::Unavailable
        };
        self.0.controls.set(action);
        let (icon, label, sensitive, keys) = match action {
            PrimaryAction::Run => (
                "media-playback-start-symbolic",
                "Run query",
                true,
                "Control+Return Control+Shift+Return",
            ),
            PrimaryAction::Unavailable => (
                "media-playback-start-symbolic",
                "Run query",
                false,
                "Control+Return Control+Shift+Return",
            ),
            PrimaryAction::Stop => ("media-playback-stop-symbolic", "Stop query", true, "Escape"),
            PrimaryAction::Stopping => {
                ("media-playback-stop-symbolic", "Stopping query", false, "")
            }
        };
        let button = &self.0.run_button;
        button.set_icon_name(icon);
        button.set_sensitive(sensitive);
        button.set_tooltip_text(Some(&match action {
            PrimaryAction::Stop => "Stop query (Escape)".to_string(),
            _ => format!("{label} (Ctrl+Enter)"),
        }));
        button.update_property(&[
            gtk4::accessible::Property::Label(label),
            gtk4::accessible::Property::KeyShortcuts(keys),
        ]);
        self.0
            .plan_button
            .set_sensitive(ready && action == PrimaryAction::Run);
    }

    fn render_interaction(&self) {
        let page = self.0.workspace.selected();
        let interaction = page.as_ref().and_then(TabPage::interaction);
        let (hint, class) = interaction
            .as_ref()
            .map(|interaction| input_hint(&interaction.input.hint))
            .unwrap_or_else(|| (default_hint(), None));
        self.0.search.set_hint(&hint, class);
        if let Some((tab, _, plan)) = self.0.saved_plan.borrow().as_ref()
            && page.as_ref().is_some_and(|page| page.id() == *tab)
        {
            self.0
                .plan
                .show(PlanContent::Ready(Box::new(plan.clone())), true);
            return;
        }
        match interaction.filter(|interaction| interaction.visible && !interaction.retired) {
            Some(interaction) => {
                let content = match (&interaction.plan, &interaction.error) {
                    (Some(plan), _) => PlanContent::Ready(Box::new(plan.clone())),
                    (None, Some(error)) => PlanContent::Error(error.clone()),
                    _ => PlanContent::Loading,
                };
                self.0.plan.show(content, interaction.input.pinned);
            }
            None => self.0.plan.hide(),
        }
    }

    fn footer_state(&self) -> FooterState {
        let Some(page) = self.0.workspace.selected() else {
            return FooterState::default();
        };
        let tab = page.tab();
        let profile = tab.profile.as_ref().map(|profile| profile.0.clone());
        let health = self.0.workspace.health();
        let status = page
            .session()
            .and_then(|session| {
                health
                    .sessions
                    .iter()
                    .find(|status| status.session == session.0)
                    .map(|status| status.status.clone())
            })
            .or_else(|| {
                profile.as_ref().and_then(|profile| {
                    health
                        .profiles
                        .iter()
                        .find(|status| &status.profile == profile)
                        .map(|status| status.status.clone())
                })
            });
        let interaction = page.interaction();
        FooterState {
            backend: tab
                .backend
                .or_else(|| profile.as_deref().and_then(|name| self.0.app.backend(name))),
            profile,
            connecting: tab.phase == TabPhase::Connecting,
            health: status,
            failure: match &tab.phase {
                TabPhase::Failed(error) => Some(error.to_string()),
                _ => None,
            },
            document: page.document().filter(|_| page.is_workspace()),
            loading: page.results().is_loading(),
            copying: page.results().is_copying(),
            ping_available: page.is_ready()
                && self.0.controls.get() != PrimaryAction::Stop
                && !interaction.is_some_and(|interaction| interaction.loading),
        }
    }

    fn refresh_footer(&self) {
        self.0.statusbar.update(&self.footer_state());
    }

    fn bind_statusbar(&self) {
        let weak = self.weak();
        self.0.statusbar.connect_ping(move || {
            let Some(shell) = upgrade(&weak) else {
                return;
            };
            let Some(page) = shell.0.workspace.selected() else {
                return;
            };
            let context = shell.0.app.context().clone();
            let probe = {
                let _runtime = shell.0.app.bridge().enter();
                context.health.check_tab(&context, page.id())
            };
            let work = shell.0.app.run(probe.wait());
            glib::spawn_future_local(async move {
                let _ = work.await;
            });
        });
        let weak = self.weak();
        self.0.statusbar.connect_saved_plan(move || {
            if let Some(shell) = upgrade(&weak) {
                shell.show_saved_plan();
            }
        });
        let weak = self.weak();
        self.0.statusbar.connect_cancel_copy(move || {
            if let Some(page) = upgrade(&weak).and_then(|shell| shell.0.workspace.selected()) {
                page.results().cancel_copy();
            }
        });
        let weak = self.weak();
        self.0.statusbar.connect_shortcuts(move || {
            if let Some(shell) = upgrade(&weak) {
                shell.execute(ShortcutAction::ShowShortcuts);
            }
        });
    }

    fn show_saved_plan(&self) {
        let Some(page) = self.0.workspace.selected() else {
            return;
        };
        match page.saved_plan() {
            Ok(Some(plan)) => {
                let generation = page.document().map_or(0, |document| document.generation);
                self.0
                    .saved_plan
                    .replace(Some((page.id(), generation, plan)));
                self.render_interaction();
            }
            Ok(None) => self.0.app.toast("No saved plan for this query"),
            Err(error) => self.0.app.toast(&error),
        }
    }

    fn open_plan(&self) {
        if let Some(page) = self.0.workspace.selected()
            && page.is_ready()
        {
            self.0.saved_plan.replace(None);
            page.send_input(InputEvent::OpenPlan);
        }
    }

    fn close_plan(&self) {
        if self.0.saved_plan.borrow_mut().take().is_some() {
            self.render_interaction();
            return;
        }
        if let Some(page) = self.0.workspace.selected() {
            page.dismiss_plan();
        }
        self.0.plan.hide();
    }

    fn bind_search(&self) {
        let weak = self.weak();
        self.0.search.set_provider(move || {
            upgrade(&weak).map_or_else(Vec::new, |shell| shell.search_items())
        });
    }

    fn search_items(&self) -> Vec<ActionItem> {
        use ShortcutAction as Action;
        let page = self.0.workspace.selected();
        let ready = page.as_ref().is_some_and(TabPage::is_ready);
        let multiple = self.0.workspace.tab_count() > 1;
        let action = self.0.controls.get();
        let mut items = Vec::new();
        let mut add = |shortcut: Action, title: Option<&str>, enabled: bool| {
            let entry = shortcuts::shortcut(shortcut);
            let weak = self.weak();
            items.push(
                ActionItem::new(
                    title.unwrap_or(entry.name),
                    entry.description,
                    "system-run-symbolic",
                    move || {
                        if let Some(shell) = upgrade(&weak) {
                            shell.execute(shortcut);
                        }
                    },
                )
                .keywords(entry.keywords)
                .enabled(enabled),
            );
        };
        add(Action::FocusEditor, None, ready);
        add(Action::FocusResults, None, ready);
        add(Action::ShowConnections, None, true);
        add(Action::ToggleHistory, None, true);
        add(Action::FocusHistory, None, true);
        add(Action::NewTab, None, true);
        add(
            Action::RunQuery,
            Some(if action == PrimaryAction::Stop {
                "Stop Query"
            } else {
                "Run Query"
            }),
            matches!(action, PrimaryAction::Run | PrimaryAction::Stop),
        );
        add(
            Action::OpenPlan,
            None,
            ready && action == PrimaryAction::Run,
        );
        add(Action::Disconnect, None, ready);
        add(Action::TestAllConnections, None, true);
        add(Action::ShowShortcuts, None, true);
        add(Action::RenameTab, None, page.is_some());
        add(Action::CloseTab, None, page.is_some());
        add(Action::NextTab, None, multiple);
        add(Action::PreviousTab, None, multiple);
        add(Action::FocusTabs, None, true);
        add(Action::Quit, None, true);
        for (profile, _) in self.0.app.profiles() {
            let weak = self.weak();
            let name = profile.clone();
            items.push(
                ActionItem::new(
                    profile.clone(),
                    "Open database connection",
                    "network-server-symbolic",
                    move || {
                        if let Some(shell) = upgrade(&weak) {
                            shell.0.workspace.open_profile(&name);
                        }
                    },
                )
                .keywords(&["connect", "database"]),
            );
        }
        for (id, title) in self.0.workspace.tab_titles() {
            let weak = self.weak();
            items.push(
                ActionItem::new(
                    format!("Tab: {title}"),
                    "Select query tab",
                    "go-jump-symbolic",
                    move || {
                        if let Some(shell) = upgrade(&weak) {
                            shell.0.workspace.select(id);
                            if let Some(page) = shell.0.workspace.page(id) {
                                page.focus_primary();
                            }
                        }
                    },
                )
                .keywords(&["switch", "query tab"]),
            );
        }
        items
    }

    pub fn execute(&self, action: ShortcutAction) -> bool {
        use ShortcutAction as Action;
        let workspace = &self.0.workspace;
        let page = workspace.selected();
        match action {
            Action::FocusSearch => {
                let restore = gtk4::prelude::GtkWindowExt::focus(&self.0.window);
                if let Some(page) = &page {
                    page.send_input(InputEvent::Reset);
                }
                self.0.search.open("", restore)
            }
            Action::FocusEditor => {
                page.is_some_and(|page| page.is_workspace() && page.editor().focus())
            }
            Action::FocusResults => {
                page.is_some_and(|page| page.is_workspace() && page.results().focus())
            }
            Action::CycleFocus => page.is_some_and(|page| {
                if !page.is_workspace() {
                    return false;
                }
                if page.editor().has_focus() {
                    page.results().focus()
                } else {
                    page.editor().focus()
                }
            }),
            Action::FocusHistory => {
                workspace.set_history_visible(true);
                self.refresh();
                page.is_some_and(|page| page.history().focus())
            }
            Action::ToggleHistory => {
                let visible = !workspace.history_visible();
                workspace.set_history_visible(visible);
                self.refresh();
                if !visible && let Some(page) = &page {
                    page.focus_primary();
                }
                true
            }
            Action::FocusTabs => workspace.focus_tabs(),
            Action::ShowConnections => {
                match &page {
                    Some(page) if !page.is_workspace() => {
                        page.chooser().focus();
                    }
                    _ => {
                        workspace.new_tab();
                    }
                }
                true
            }
            Action::TestAllConnections => {
                super::tab_page::test_all(&self.0.app);
                true
            }
            Action::NewTab => workspace.new_tab().is_some(),
            Action::CloseTab | Action::Disconnect => {
                if action == Action::Disconnect && !page.as_ref().is_some_and(TabPage::is_ready) {
                    return false;
                }
                if !workspace.close_selected() {
                    self.0.window.close();
                }
                true
            }
            Action::NextTab => workspace.cycle(1),
            Action::PreviousTab => workspace.cycle(-1),
            Action::RenameTab => self.rename_dialog(),
            Action::RunQuery | Action::StopQuery => page.is_some_and(|page| {
                page.editor_command(if action == Action::StopQuery {
                    EditorIntent::Cancel
                } else {
                    EditorIntent::Primary
                });
                true
            }),
            Action::EstimateAndRun => false,
            Action::Estimate | Action::OpenPlan => {
                self.open_plan();
                true
            }
            Action::PreviewRow => {
                page.is_some_and(|page| page.results().focus() && page.results().show_row_preview())
            }
            Action::CopyResults => page.is_some_and(|page| {
                page.results().copy_selection();
                true
            }),
            Action::ShowShortcuts => {
                if self.0.shortcuts.parent().is_none() {
                    self.0.shortcuts.present(Some(&self.0.window));
                }
                true
            }
            Action::Fullscreen => {
                if self.0.window.is_fullscreen() {
                    self.0.window.unfullscreen();
                } else {
                    self.0.window.fullscreen();
                }
                true
            }
            Action::Quit => {
                self.0.window.close();
                true
            }
        }
    }

    fn rename_dialog(&self) -> bool {
        let Some(page) = self.0.workspace.selected() else {
            return false;
        };
        let dialog = adw::AlertDialog::new(Some("Rename Query Tab"), None);
        let entry = gtk4::Entry::new();
        entry.set_text(&page.tab().document.snapshot().title);
        entry.set_activates_default(true);
        entry.update_property(&[gtk4::accessible::Property::Label("Query tab name")]);
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("rename", "Rename");
        dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("rename"));
        dialog.set_close_response("cancel");
        let weak = self.weak();
        dialog.connect_response(None, move |_, response| {
            if response == "rename"
                && let Some(shell) = upgrade(&weak)
            {
                if page.rename(&entry.text()) {
                    shell.0.workspace.reconcile();
                } else {
                    shell.0.app.toast("Tab names cannot be empty");
                }
            }
        });
        dialog.present(Some(&self.0.window));
        true
    }

    fn bind_actions(&self) {
        let window = &self.0.window;
        for (name, action) in [
            ("test-all", ShortcutAction::TestAllConnections),
            ("shortcuts", ShortcutAction::ShowShortcuts),
            ("quit", ShortcutAction::Quit),
        ] {
            let entry = gio::SimpleAction::new(name, None);
            let weak = self.weak();
            entry.connect_activate(move |_, _| {
                if let Some(shell) = upgrade(&weak) {
                    shell.execute(action);
                }
            });
            window.add_action(&entry);
        }
    }

    fn input_allowed(&self) -> bool {
        let Some(page) = self.0.workspace.selected() else {
            return false;
        };
        if !page.is_ready() {
            return false;
        }
        let search = self.0.search.widget();
        !gtk4::prelude::GtkWindowExt::focus(&self.0.window).is_some_and(|focus| {
            focus == *search.upcast_ref::<gtk4::Widget>() || focus.is_ancestor(search)
        })
    }

    fn at(&self) -> u64 {
        self.0.clock.elapsed().as_millis() as u64
    }

    fn sync_modifiers(&self) {
        let allowed = self.input_allowed();
        let events = self.0.keys.borrow_mut().sync(allowed);
        if events.is_empty() {
            return;
        }
        let Some(page) = self.0.workspace.selected() else {
            return;
        };
        let at = self.at();
        for (key, pressed) in events {
            page.send_input(if pressed {
                InputEvent::Press { key, at_ms: at }
            } else {
                InputEvent::Release { key, at_ms: at }
            });
        }
    }

    fn reset_keys(&self) {
        self.0.keys.borrow_mut().reset();
        if let Some(page) = self.0.workspace.selected() {
            page.send_input(InputEvent::Reset);
        }
    }

    fn send_key(&self, key: InputKey) {
        if let Some(page) = self.0.workspace.selected() {
            let at = self.at();
            page.send_input(InputEvent::Press { key, at_ms: at });
            page.send_input(InputEvent::Release { key, at_ms: at });
        }
    }

    fn key_pressed(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
        let modifiers = modifiers
            & (gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::SHIFT_MASK
                | gdk::ModifierType::ALT_MASK);
        let repeat = !self.0.keys.borrow_mut().press(key);
        if modifier_role(key).is_some() {
            self.sync_modifiers();
            return glib::Propagation::Proceed;
        }
        if modifiers.is_empty()
            && let Some(direction) = navigation::vim(key)
            && !gtk4::prelude::GtkWindowExt::focus(&self.0.window)
                .is_some_and(|focus| navigation::is_text_input(&focus))
            && let Some(page) = self.0.workspace.selected()
            && (page.results().navigate(direction)
                || page.history().navigate(direction)
                || page.chooser().navigate(direction))
        {
            return glib::Propagation::Stop;
        }
        let allowed = self.input_allowed();
        let page = self.0.workspace.selected();
        let control = gdk::ModifierType::CONTROL_MASK;
        let shift = gdk::ModifierType::SHIFT_MASK;
        if key == gdk::Key::space && modifiers == control | shift {
            if allowed {
                let keys = self.0.keys.borrow();
                let (primary, preview) = (keys.primary(), keys.preview());
                drop(keys);
                if primary && preview {
                    if !repeat {
                        self.0.keys.borrow_mut().inspect = true;
                        self.0.saved_plan.replace(None);
                        if let Some(page) = &page {
                            page.send_input(InputEvent::Press {
                                key: InputKey::Inspect,
                                at_ms: self.at(),
                            });
                        }
                    }
                    return glib::Propagation::Stop;
                }
                if !repeat {
                    self.open_plan();
                }
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }
        if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter)
            && modifiers.contains(control)
            && allowed
        {
            if !repeat {
                self.send_key(InputKey::Submit);
            }
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::Escape && modifiers.is_empty() {
            if let Some(page) = &page {
                if page.results().close_inspector() {
                    return glib::Propagation::Stop;
                }
                if self.0.workspace.history_visible() && page.history().has_focus() {
                    return glib::Propagation::Proceed;
                }
                if allowed && self.0.controls.get() == PrimaryAction::Stop {
                    self.send_key(InputKey::Cancel);
                    return glib::Propagation::Stop;
                }
            }
            let estimating =
                page.as_ref()
                    .and_then(TabPage::interaction)
                    .is_some_and(|interaction| {
                        interaction.loading
                            || interaction.visible
                            || matches!(interaction.input.hint, InputHint::Estimating)
                    });
            if self.0.plan.is_visible() || estimating {
                self.close_plan();
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }
        if self.0.plan.is_visible() && self.0.keys.borrow().preview() {
            let scroll = match key {
                gdk::Key::Up => Some(PlanScroll::LineUp),
                gdk::Key::Down => Some(PlanScroll::LineDown),
                gdk::Key::Page_Up => Some(PlanScroll::PageUp),
                gdk::Key::Page_Down => Some(PlanScroll::PageDown),
                _ => None,
            };
            if let Some(scroll) = scroll
                && self.0.plan.scroll(scroll)
            {
                return glib::Propagation::Stop;
            }
        }
        if let Some(action) = shortcuts::matching(key, modifiers) {
            use ShortcutAction as Action;
            if matches!(
                action,
                Action::RunQuery
                    | Action::EstimateAndRun
                    | Action::Estimate
                    | Action::OpenPlan
                    | Action::StopQuery
                    | Action::PreviewRow
                    | Action::CopyResults
            ) {
                return glib::Propagation::Proceed;
            }
            if !repeat || matches!(action, Action::NextTab | Action::PreviousTab) {
                self.execute(action);
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    }

    fn key_released(&self, key: gdk::Key) {
        self.0.keys.borrow_mut().release(key);
        if key == gdk::Key::space
            && std::mem::take(&mut self.0.keys.borrow_mut().inspect)
            && let Some(page) = self.0.workspace.selected()
        {
            page.send_input(InputEvent::Release {
                key: InputKey::Inspect,
                at_ms: self.at(),
            });
        }
        if modifier_role(key).is_some() {
            self.sync_modifiers();
        }
    }

    fn bind_keys(&self) {
        let controller = gtk4::EventControllerKey::new();
        controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let weak = self.weak();
        controller.connect_key_pressed(move |_, key, _, modifiers| {
            upgrade(&weak).map_or(glib::Propagation::Proceed, |shell| {
                shell.key_pressed(key, modifiers)
            })
        });
        let weak = self.weak();
        controller.connect_key_released(move |_, key, _, _| {
            if let Some(shell) = upgrade(&weak) {
                shell.key_released(key);
            }
        });
        self.0.window.add_controller(controller);
    }

    fn bind_lifecycle(&self) {
        let weak = self.weak();
        self.0.window.connect_is_active_notify(move |window| {
            let Some(shell) = upgrade(&weak) else {
                return;
            };
            if !window.is_active() {
                shell.reset_keys();
            } else if let Some(page) = shell.0.workspace.selected()
                && page.history_visible()
            {
                page.refresh_history();
            }
            shell.report_attention();
        });
        let weak = self.weak();
        self.0.window.connect_close_request(move |_| {
            upgrade(&weak).map_or(glib::Propagation::Proceed, |shell| shell.close_request())
        });
    }

    fn report_attention(&self) {
        let visible = self.0.window.is_active() && self.0.window.is_visible();
        let viewed = self.0.workspace.selected().and_then(|page| {
            let document = page.document()?;
            (page.is_ready() && document.generation != 0).then(|| ViewedOutput {
                tab: page.id().0,
                document: document.document,
                generation: document.generation,
            })
        });
        let current = Some((visible, viewed));
        if *self.0.viewed.borrow() == current {
            return;
        }
        self.0.viewed.replace(current);
        self.0
            .app
            .context()
            .attention()
            .input(AttentionInput::Visibility { visible, viewed });
    }

    fn apply_attention(&self, snapshot: AttentionSnapshot) {
        let Some(application) = self.0.window.application() else {
            return;
        };
        if snapshot.cancel_attention {
            application.withdraw_notification("query-completed");
        }
        let previous = self.0.attention_request.replace(snapshot.request_revision);
        if snapshot.request_revision > previous && !snapshot.application_visible {
            let unseen = snapshot
                .tabs
                .iter()
                .filter(|tab| tab.unseen.is_some())
                .count();
            if unseen > 0 {
                let notification = gio::Notification::new("Query finished");
                notification.set_body(Some(&if unseen == 1 {
                    "A query has new results".to_string()
                } else {
                    format!("{unseen} queries have new results")
                }));
                application.send_notification(Some("query-completed"), &notification);
            }
        }
    }

    fn close_request(&self) -> glib::Propagation {
        let context = self.0.app.context().clone();
        let phase = *context.subscribe_phase().borrow();
        match phase {
            ApplicationPhase::Closed => glib::Propagation::Proceed,
            ApplicationPhase::Draining => glib::Propagation::Stop,
            ApplicationPhase::Running => {
                if self.0.closing.replace(true) {
                    return glib::Propagation::Stop;
                }
                let drafts = self
                    .0
                    .workspace
                    .pages()
                    .into_iter()
                    .map(|page| (page.id(), page.editor().text()))
                    .collect::<HashMap<_, _>>();
                let location = self.0.app.workspace_location();
                let save_context = context.clone();
                let saving = self
                    .0
                    .app
                    .run(async move { save_context.save_workspace(location, drafts).await });
                let weak = self.weak();
                glib::spawn_future_local(async move {
                    let saved = saving.await;
                    let Some(shell) = upgrade(&weak) else {
                        return;
                    };
                    match saved {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            shell.0.closing.set(false);
                            shell
                                .0
                                .app
                                .toast(&format!("Could not save open tabs: {error}"));
                            return;
                        }
                        Err(error) => {
                            shell.0.closing.set(false);
                            shell.0.app.toast(&error.to_string());
                            return;
                        }
                    }
                    context.attention().input(AttentionInput::Lifecycle {
                        restoring: false,
                        quitting: true,
                    });
                    let shutdown = {
                        let _runtime = shell.0.app.bridge().enter();
                        context.start_shutdown()
                    };
                    let shutdown = match shutdown {
                        Ok(handle) => shell.0.app.run(handle.wait()),
                        Err(error) => {
                            shell.0.closing.set(false);
                            shell.0.app.toast(&error.to_string());
                            return;
                        }
                    };
                    let result = shutdown.await;
                    let Some(shell) = upgrade(&weak) else {
                        return;
                    };
                    match result {
                        Ok(Ok(())) => shell.0.window.close(),
                        Ok(Err(error)) => {
                            shell.0.closing.set(false);
                            context.attention().input(AttentionInput::Lifecycle {
                                restoring: false,
                                quitting: false,
                            });
                            shell.0.workspace.reconcile();
                            shell.0.app.toast(&format!(
                                "Cannot close Dzban: {error}. Finish or cancel running work and try again."
                            ));
                        }
                        Err(error) => {
                            shell.0.closing.set(false);
                            shell
                                .0
                                .app
                                .toast(&format!("Shutdown did not complete: {error}"));
                        }
                    }
                });
                glib::Propagation::Stop
            }
        }
    }

    #[cfg(test)]
    pub fn search(&self) -> &ActionSearch {
        &self.0.search
    }

    #[cfg(test)]
    pub fn plan(&self) -> &PlanView {
        &self.0.plan
    }

    #[cfg(test)]
    pub fn footer(&self) -> &Statusbar {
        &self.0.statusbar
    }

    #[cfg(test)]
    pub fn plan_button(&self) -> &gtk4::Button {
        &self.0.plan_button
    }

    #[cfg(test)]
    pub fn run_button(&self) -> &gtk4::Button {
        &self.0.run_button
    }

    #[cfg(test)]
    pub fn history_button(&self) -> &gtk4::Button {
        &self.0.history_button
    }

    #[cfg(test)]
    pub fn press(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
        self.key_pressed(key, modifiers)
    }

    #[cfg(test)]
    pub fn release(&self, key: gdk::Key) {
        self.key_released(key)
    }
}

fn upgrade(weak: &std::rc::Weak<Inner>) -> Option<Shell> {
    weak.upgrade().map(Shell)
}

pub fn install_styles() {
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(
        "entry.query-action-field {
            min-height: 30px;
        }
        entry.query-action-field.guidance,
        entry.query-action-field.ready {
            outline: 1px solid alpha(@accent_color, 0.6);
            background-color: alpha(@accent_bg_color, 0.12);
        }
        entry.query-action-field.progress {
            outline: 1px solid alpha(@accent_color, 0.7);
            background-color: alpha(@accent_bg_color, 0.16);
        }
        entry.query-action-field.error {
            outline: 1px solid alpha(@error_color, 0.7);
            background-color: alpha(@error_color, 0.10);
        }
        progressbar.query-progress,
        progressbar.query-progress trough,
        progressbar.query-progress progress {
            min-height: 2px;
            margin: 0;
            padding: 0;
            border: none;
            border-radius: 0;
        }
        progressbar.query-progress trough {
            background-color: transparent;
        }
        progressbar.query-progress progress {
            background-color: @accent_bg_color;
        }
        .floating-inspector {
            background-color: @window_bg_color;
            box-shadow: 0 6px 24px alpha(black, 0.28);
        }
        .run-summary {
            padding: 18px 28px;
            background-color: alpha(@window_bg_color, 0.92);
        }
        .run-summary-elapsed {
            font-size: 24pt;
            font-weight: 600;
        }
        .result-issues {
            padding: 8px 12px;
            background-color: alpha(@error_color, 0.04);
            border-bottom: 1px solid alpha(@window_fg_color, 0.12);
        }
        .result-null {
            color: alpha(@window_fg_color, 0.5);
            font-style: italic;
        }
        .result-state-tile {
            min-width: 44px;
            min-height: 44px;
            border-radius: 10px;
            background-color: alpha(@window_fg_color, 0.08);
        }
        .result-state-tile.error {
            color: @error_color;
            background-color: alpha(@error_color, 0.12);
        }
        .result-state-title {
            font-size: 15px;
            font-weight: 600;
        }
        columnview.result-table > listview > row {
            min-height: 24px;
            padding: 0;
        }
        columnview.result-table > listview > row > cell {
            padding: 2px 6px;
        }
        columnview.result-table row:nth-child(even) {
            background-color: alpha(@window_fg_color, 0.03);
        }
        columnview.result-table row:selected {
            background-color: alpha(@accent_bg_color, 0.25);
        }
        columnview.result-table .result-cursor {
            background-color: alpha(@accent_bg_color, 0.45);
            box-shadow: inset 0 0 0 1px @accent_color;
        }
        .query-history-pane {
            background-color: @view_bg_color;
            border-right: 1px solid alpha(@window_fg_color, 0.12);
        }
        .query-history-content {
            padding: 8px 12px;
        }
        .query-history-title {
            font-size: 9pt;
        }
        .workspace-list row:selected {
            background-color: alpha(@accent_bg_color, 0.16);
            color: @view_fg_color;
        }
        .connection-list row:selected {
            background-color: alpha(@accent_bg_color, 0.12);
            box-shadow: inset 0 0 0 1.5px @accent_color;
        }
        paned.query-split > separator {
            min-height: 1px;
            background-color: alpha(@window_fg_color, 0.12);
        }
        .query-editor {
            font-size: 11pt;
        }
        .workspace-statusbar {
            min-height: 30px;
            padding: 0 32px 0 19px;
            border-top: 1px solid alpha(@window_fg_color, 0.12);
            background-color: @headerbar_bg_color;
        }
        .workspace-statusbar button {
            min-height: 22px;
            padding: 0 4px;
        }
        .status-dot {
            min-width: 7px;
            min-height: 7px;
            border-radius: 999px;
            background-color: alpha(@window_fg_color, 0.4);
        }
        .status-dot.status-connected {
            background-color: @success_color;
        }
        .status-dot.status-connecting {
            background-color: @orange_4;
        }
        @keyframes connection-pulse {
            0%, 100% { opacity: 1; }
            50% { opacity: 0.45; }
        }
        .status-dot.connection-pulsing {
            animation: connection-pulse 1.6s ease-in-out infinite;
        }
        .status-dot.status-error {
            background-color: @error_color;
        }",
    );
    gtk4::style_context_add_provider_for_display(
        &gdk::Display::default().expect("a display is available after GTK initialization"),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_aliases_release_only_after_the_last_physical_key() {
        let mut keys = KeyState::default();
        keys.press(gdk::Key::Control_L);
        keys.press(gdk::Key::Control_R);
        assert_eq!(keys.sync(true), vec![(InputKey::Primary, true)]);
        keys.release(gdk::Key::Control_L);
        assert!(keys.sync(true).is_empty());
        keys.release(gdk::Key::Control_R);
        assert_eq!(keys.sync(true), vec![(InputKey::Primary, false)]);
        keys.press(gdk::Key::Shift_L);
        assert!(keys.sync(false).is_empty());
        assert_eq!(keys.sync(true), vec![(InputKey::Preview, true)]);
        keys.reset();
        assert!(!keys.preview());
        assert!(keys.sync(false).is_empty());
    }

    #[test]
    fn repeated_presses_are_reported_once() {
        let mut keys = KeyState::default();
        assert!(keys.press(gdk::Key::space));
        assert!(!keys.press(gdk::Key::space));
        assert!(keys.release(gdk::Key::space));
        assert!(!keys.release(gdk::Key::space));
    }

    #[test]
    fn hints_follow_the_shared_interaction_state() {
        let plan = PlanPresentation::from(&dzban_core::QueryPlan {
            backend: dzban_core::DatabaseBackend::Postgresql,
            document: serde_json::json!([]),
            estimate: dzban_core::QueryEstimate {
                rows: Some(42),
                startup_cost: Some(1.25),
                total_cost: Some(9.5),
                ..Default::default()
            },
        });
        assert_eq!(
            input_hint(&InputHint::EstimateReady { plan: plan.clone() }).0,
            "Enter to run - Space for plan - ~42 rows - cost 1.25..9.50"
        );
        assert_eq!(
            input_hint(&InputHint::Running { plan: Some(plan) }).0,
            "Running query - ~42 rows - cost 1.25..9.50"
        );
        assert_eq!(
            input_hint(&InputHint::EstimateError {
                error: "Database is not connected".into()
            })
            .0,
            "Enter to run - estimate unavailable: Database is not connected"
        );
        assert_eq!(input_hint(&InputHint::Default).0, default_hint());
        assert_eq!(input_hint(&InputHint::PrimaryGuide).1, Some("guidance"));
    }
}
