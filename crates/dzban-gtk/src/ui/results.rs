use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dzban_core::ResultColumn;
use dzban_core::documents::DocumentId;
use dzban_core::execution::SessionId;
use dzban_core::outputs::view::{RestorationState, ResultViewRequest};
use dzban_core::outputs::{OutputExport, RowRange};
use dzban_core::outputs::{ResultReader, ResultScope};
use dzban_core::presentation::{DocumentPresentation, ResultProgress};
use dzban_core::preview::{PeekEvent, PreviewSurface};
use dzban_core::tabs::TabId;
use dzban_gtk::result_model::{PageReader, ResultListModel, decode_row};
use gtk4::prelude::*;
use gtk4::{gdk, glib};
use libadwaita as adw;
use libadwaita::prelude::*;

use super::app::App;
use super::format;
use super::navigation::{Direction, contains_focus};
use super::row_inspector::RowInspector;

const MAX_COLUMN_WIDTH: i32 = 360;
const CELL_SPACE: i32 = 16;
const HEADER_SPACE: i32 = 32;
const PREVIEW_CHARS: usize = 512;
const TOOLTIP_CHARS: usize = 16_384;
const INSPECTOR_CHARS: usize = 8_192;

type CopyDone = Rc<dyn Fn(Result<(), String>)>;
type CellEntry = (
    glib::WeakRef<gtk4::ListItem>,
    glib::WeakRef<gtk4::Label>,
    usize,
);

pub struct Issue {
    pub title: &'static str,
    pub detail: String,
}

struct Table {
    scope: ResultScope,
    reader: ResultReader,
    columns: Vec<ResultColumn>,
    model: ResultListModel,
    view: gtk4::ColumnView,
    selection: gtk4::MultiSelection,
}

struct Restore {
    columns: Vec<ResultColumn>,
    row: Option<usize>,
    column: usize,
    horizontal: f64,
    vertical: f64,
}

struct Inner {
    app: App,
    tab: TabId,
    root: gtk4::Box,
    progress: gtk4::ProgressBar,
    banner: gtk4::Revealer,
    banner_text: gtk4::Label,
    retry: gtk4::Button,
    scroll: gtk4::ScrolledWindow,
    state: ResultState,
    summary: gtk4::Box,
    summary_title: gtk4::Label,
    summary_elapsed: gtk4::Label,
    summary_started: gtk4::Label,
    summary_rows: gtk4::Label,
    summary_estimate: gtk4::Label,
    inspector: RowInspector,
    table: RefCell<Option<Table>>,
    building: Cell<Option<ResultScope>>,
    document: RefCell<Option<DocumentPresentation>>,
    load_error: RefCell<Option<String>>,
    copy_error: RefCell<Option<String>>,
    copying: RefCell<Option<Arc<AtomicBool>>>,
    restore: RefCell<Option<Restore>>,
    pending_scroll: Cell<Option<(f64, f64)>>,
    scroll_scheduled: Cell<bool>,
    column: Cell<usize>,
    cells: RefCell<Vec<CellEntry>>,
    space: Cell<Option<Instant>>,
    peek_request: Cell<u64>,
    pulse: RefCell<Option<glib::SourceId>>,
    clock: RefCell<Option<glib::SourceId>>,
    listeners: RefCell<Vec<Rc<dyn Fn()>>>,
    retired: Cell<bool>,
}

#[derive(Clone)]
pub struct ResultPanel(Rc<Inner>);

fn label(classes: &[&str]) -> gtk4::Label {
    let label = gtk4::Label::new(None);
    for class in classes {
        label.add_css_class(class);
    }
    label
}

impl ResultPanel {
    pub fn new(app: App, tab: TabId) -> Self {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        root.set_vexpand(true);
        root.set_size_request(-1, 160);

        let progress = gtk4::ProgressBar::new();
        progress.add_css_class("query-progress");
        progress.set_pulse_step(0.08);
        progress.set_opacity(0.0);
        progress.set_can_target(false);
        progress.update_property(&[gtk4::accessible::Property::Label("Query progress")]);
        root.append(&progress);

        let banner_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        banner_box.add_css_class("result-issues");
        let banner_text = gtk4::Label::new(None);
        banner_text.set_xalign(0.0);
        banner_text.set_hexpand(true);
        banner_text.set_wrap(true);
        banner_text.set_selectable(true);
        banner_text.add_css_class("monospace");
        banner_box.append(&banner_text);
        let retry = gtk4::Button::with_label("Retry");
        retry.set_valign(gtk4::Align::Center);
        retry.set_visible(false);
        banner_box.append(&retry);
        let banner = gtk4::Revealer::new();
        banner.set_child(Some(&banner_box));
        root.append(&banner);

        let overlay = gtk4::Overlay::new();
        overlay.set_vexpand(true);
        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_hexpand(true);
        scroll.update_property(&[
            gtk4::accessible::Property::Label("Query results"),
            gtk4::accessible::Property::Description(
                "Output, status, or error from the selected query",
            ),
        ]);
        overlay.set_child(Some(&scroll));

        let state = ResultState::new();
        overlay.add_overlay(&state.root);

        let summary = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        summary.add_css_class("card");
        summary.add_css_class("run-summary");
        summary.set_halign(gtk4::Align::Center);
        summary.set_valign(gtk4::Align::Center);
        summary.set_can_target(false);
        summary.set_visible(false);
        let summary_header = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        summary_header.set_halign(gtk4::Align::Center);
        let spinner = gtk4::Spinner::new();
        spinner.set_spinning(true);
        summary_header.append(&spinner);
        let summary_title = label(&["heading"]);
        summary_header.append(&summary_title);
        summary.append(&summary_header);
        let summary_elapsed = label(&["run-summary-elapsed", "numeric"]);
        summary.append(&summary_elapsed);
        let summary_started = label(&["dim-label", "caption"]);
        summary.append(&summary_started);
        let summary_rows = label(&["dim-label", "caption", "numeric"]);
        summary.append(&summary_rows);
        let summary_estimate = label(&["dim-label", "caption"]);
        summary.append(&summary_estimate);
        overlay.add_overlay(&summary);

        let inspector = RowInspector::new();
        overlay.add_overlay(inspector.widget());
        root.append(&overlay);

        let panel = Self(Rc::new(Inner {
            app,
            tab,
            root,
            progress,
            banner,
            banner_text,
            retry: retry.clone(),
            scroll,
            state,
            summary,
            summary_title,
            summary_elapsed,
            summary_started,
            summary_rows,
            summary_estimate,
            inspector,
            table: RefCell::new(None),
            building: Cell::new(None),
            document: RefCell::new(None),
            load_error: RefCell::new(None),
            copy_error: RefCell::new(None),
            copying: RefCell::new(None),
            restore: RefCell::new(None),
            pending_scroll: Cell::new(None),
            scroll_scheduled: Cell::new(false),
            column: Cell::new(0),
            cells: RefCell::new(Vec::new()),
            space: Cell::new(None),
            peek_request: Cell::new(0),
            pulse: RefCell::new(None),
            clock: RefCell::new(None),
            listeners: RefCell::new(Vec::new()),
            retired: Cell::new(false),
        }));

        let weak = Rc::downgrade(&panel.0);
        retry.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                ResultPanel(inner).retry();
            }
        });
        let weak = Rc::downgrade(&panel.0);
        panel.0.inspector.connect_close(move || {
            if let Some(inner) = weak.upgrade() {
                ResultPanel(inner).peek(PeekEvent::Close, Duration::ZERO);
            }
        });
        let weak = Rc::downgrade(&panel.0);
        panel.0.inspector.connect_pin(move || {
            if let Some(inner) = weak.upgrade() {
                ResultPanel(inner).peek(PeekEvent::Pin, Duration::ZERO);
            }
        });
        let weak = Rc::downgrade(&panel.0);
        panel.0.inspector.connect_move_row(move |direction| {
            if let Some(inner) = weak.upgrade() {
                ResultPanel(inner).move_inspected_row(direction);
            }
        });
        let weak = Rc::downgrade(&panel.0);
        panel.0.inspector.connect_open_cell(move |column| {
            if let Some(inner) = weak.upgrade() {
                ResultPanel(inner).open_inspected_cell(column);
            }
        });
        for adjustment in [panel.0.scroll.hadjustment(), panel.0.scroll.vadjustment()] {
            let weak = Rc::downgrade(&panel.0);
            adjustment.connect_changed(move |_| {
                if let Some(inner) = weak.upgrade() {
                    ResultPanel(inner).schedule_pending_scroll();
                }
            });
        }
        let wheel = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::BOTH_AXES);
        let click = gtk4::GestureClick::new();
        click.set_button(0);
        let keys = gtk4::EventControllerKey::new();
        for controller in [
            wheel.clone().upcast::<gtk4::EventController>(),
            click.clone().upcast(),
            keys.clone().upcast(),
        ] {
            controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
            panel.0.scroll.add_controller(controller);
        }
        let weak = Rc::downgrade(&panel.0);
        wheel.connect_scroll(move |_, _, _| {
            if let Some(inner) = weak.upgrade() {
                inner.pending_scroll.set(None);
            }
            glib::Propagation::Proceed
        });
        let weak = Rc::downgrade(&panel.0);
        click.connect_pressed(move |_, _, _, _| {
            if let Some(inner) = weak.upgrade() {
                inner.pending_scroll.set(None);
            }
        });
        let weak = Rc::downgrade(&panel.0);
        keys.connect_key_pressed(move |_, _, _, _| {
            if let Some(inner) = weak.upgrade() {
                inner.pending_scroll.set(None);
            }
            glib::Propagation::Proceed
        });
        panel.render_state();
        panel
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.0.root
    }

    pub fn connect_changed(&self, callback: impl Fn() + 'static) {
        self.0.listeners.borrow_mut().push(Rc::new(callback));
    }

    fn changed(&self) {
        let listeners = self.0.listeners.borrow().clone();
        for listener in listeners {
            listener();
        }
    }

    pub fn retire(&self) {
        self.0.retired.set(true);
        self.cancel_copy();
        if let Some(source) = self.0.pulse.borrow_mut().take() {
            source.remove();
        }
        if let Some(source) = self.0.clock.borrow_mut().take() {
            source.remove();
        }
        self.0.table.replace(None);
        self.0.scroll.set_child(None::<&gtk4::Widget>);
    }

    pub fn is_loading(&self) -> bool {
        self.0.building.get().is_some()
            || self
                .0
                .table
                .borrow()
                .as_ref()
                .is_some_and(|table| table.model.phase() != Default::default())
    }

    pub fn is_copying(&self) -> bool {
        self.0.copying.borrow().is_some()
    }

    pub fn issues(&self) -> Vec<Issue> {
        let mut issues = Vec::new();
        let document = self.0.document.borrow();
        if let Some(diagnostic) = document
            .as_ref()
            .and_then(|document| document.feedback.query_diagnostic.as_ref())
        {
            issues.push(Issue {
                title: "Query failed",
                detail: diagnostic.text.clone(),
            });
        }
        if let Some(error) = self.0.load_error.borrow().clone() {
            issues.push(Issue {
                title: "Unable to load results",
                detail: error,
            });
        }
        if let Some(error) = self.0.copy_error.borrow().clone() {
            issues.push(Issue {
                title: "Unable to copy results",
                detail: error,
            });
        }
        if let Some(diagnostic) = document
            .as_ref()
            .and_then(|document| document.feedback.capture_diagnostic.as_ref())
        {
            issues.push(Issue {
                title: "Result capture stopped",
                detail: diagnostic.text.clone(),
            });
        }
        issues
    }

    pub fn available_rows(&self) -> usize {
        self.0
            .table
            .borrow()
            .as_ref()
            .map_or(0, |table| table.model.n_items() as usize)
    }

    pub fn render(&self, document: &DocumentPresentation) {
        if self.0.retired.get() {
            return;
        }
        let scope = document.session.map(|session| ResultScope {
            session: SessionId(session),
            document: DocumentId(document.document),
            generation: document.generation,
        });
        let replaced = self
            .0
            .table
            .borrow()
            .as_ref()
            .is_some_and(|table| Some(table.scope) != scope);
        if replaced {
            self.save_restore();
            self.0.table.replace(None);
            self.0.scroll.set_child(None::<&gtk4::Widget>);
            self.0.load_error.replace(None);
            self.0.copy_error.replace(None);
            self.cancel_copy();
            self.peek(PeekEvent::Reset, Duration::ZERO);
        }
        if self
            .0
            .building
            .get()
            .is_some_and(|building| Some(building) != scope)
        {
            self.0.building.set(None);
        }
        if document.generation == 0 {
            self.0.load_error.replace(None);
        }
        self.0.document.replace(Some(document.clone()));
        if let Some(table) = self.0.table.borrow().as_ref() {
            table.model.set_rows(document.rows);
        } else if let Some(scope) = scope
            && document.generation != 0
            && document.result_available
            && self.0.building.get().is_none()
        {
            self.build(scope);
        }
        self.apply_restore();
        self.render_state();
        self.changed();
    }

    fn build(&self, scope: ResultScope) {
        let reader = match self.0.app.context().result_reader(scope) {
            Ok(reader) => reader,
            Err(dzban_core::outputs::OutputError::Unavailable) => return,
            Err(error) => {
                self.0.load_error.replace(Some(error.to_string()));
                return;
            }
        };
        self.0.building.set(Some(scope));
        let schema = {
            let reader = reader.clone();
            self.0.app.run(async move { reader.page(0, 0).await })
        };
        let weak = Rc::downgrade(&self.0);
        glib::spawn_future_local(async move {
            let schema = schema.await;
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let panel = ResultPanel(inner);
            if panel.0.building.get() != Some(scope) || panel.0.retired.get() {
                return;
            }
            panel.0.building.set(None);
            match schema {
                Ok(Ok(page)) => panel.install(scope, reader, page.columns),
                Ok(Err(dzban_core::outputs::OutputError::StaleOutput)) => {}
                Ok(Err(error)) => {
                    panel.0.load_error.replace(Some(error.to_string()));
                }
                Err(error) => {
                    panel.0.load_error.replace(Some(error.to_string()));
                }
            }
            panel.apply_restore();
            panel.render_state();
            panel.changed();
        });
    }

    fn install(&self, scope: ResultScope, reader: ResultReader, columns: Vec<ResultColumn>) {
        self.0.cells.borrow_mut().clear();
        let rows = self
            .0
            .document
            .borrow()
            .as_ref()
            .filter(|document| document.generation == scope.generation)
            .map_or(0, |document| document.rows);
        let model = ResultListModel::new(
            PageReader::from_output(reader.clone()),
            self.0.app.bridge().clone(),
            columns.len(),
            rows,
        );
        let selection = gtk4::MultiSelection::new(Some(model.clone()));
        keep_selection_across_page_loads(&selection);
        let view = self.create_view(&selection, &model, &columns);
        let weak = Rc::downgrade(&self.0);
        model.connect_local("load-error", false, move |values| {
            let error = values[1].get::<String>().unwrap_or_default();
            if let Some(inner) = weak.upgrade() {
                let panel = ResultPanel(inner);
                panel.0.load_error.replace(Some(error));
                panel.render_state();
                panel.changed();
            }
            None
        });
        let weak = Rc::downgrade(&self.0);
        model.connect_local("idle", false, move |_| {
            if let Some(inner) = weak.upgrade() {
                ResultPanel(inner).changed();
            }
            None
        });
        self.0.scroll.set_child(Some(&view));
        self.0.column.set(0);
        self.0.table.replace(Some(Table {
            scope,
            reader,
            columns,
            model,
            view,
            selection,
        }));
    }

    fn create_view(
        &self,
        selection: &gtk4::MultiSelection,
        model: &ResultListModel,
        columns: &[ResultColumn],
    ) -> gtk4::ColumnView {
        let view = gtk4::ColumnView::new(Some(selection.clone()));
        view.add_css_class("result-table");
        view.set_focusable(true);
        view.set_show_column_separators(true);
        view.set_show_row_separators(false);
        view.set_reorderable(false);
        view.update_property(&[gtk4::accessible::Property::Label("Query results")]);

        let numbers = gtk4::SignalListItemFactory::new();
        numbers.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
            let label = cell_label();
            label.set_xalign(1.0);
            label.add_css_class("dim-label");
            item.set_child(Some(&label));
        });
        numbers.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
            if let Some(label) = item.child().and_downcast::<gtk4::Label>() {
                label.set_text(&(item.position() + 1).to_string());
            }
        });
        let number_column = gtk4::ColumnViewColumn::new(None, Some(numbers));
        number_column.set_fixed_width(56);
        view.append_column(&number_column);

        for (index, column) in columns.iter().enumerate() {
            let factory = gtk4::SignalListItemFactory::new();
            let weak = Rc::downgrade(&self.0);
            let weak_model = model.downgrade();
            factory.connect_setup(move |_, item| {
                let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
                let label = cell_label();
                if let Some(inner) = weak.upgrade() {
                    inner
                        .cells
                        .borrow_mut()
                        .push((item.downgrade(), label.downgrade(), index));
                }
                let click = gtk4::GestureClick::new();
                let weak_item = item.downgrade();
                let weak_panel = weak.clone();
                click.connect_pressed(move |_, presses, _, _| {
                    let (Some(inner), Some(item)) = (weak_panel.upgrade(), weak_item.upgrade())
                    else {
                        return;
                    };
                    inner.column.set(index);
                    let panel = ResultPanel(inner);
                    panel.refresh_cursor();
                    if presses == 2 {
                        panel.open_cell_dialog(item.position() as usize, index);
                    }
                });
                label.add_controller(click);
                let weak_item = item.downgrade();
                let tooltip = Rc::new(RefCell::new(None::<(u32, String)>));
                let requested = Rc::new(Cell::new(None::<u32>));
                let weak_model = weak_model.clone();
                label.set_has_tooltip(true);
                label.connect_query_tooltip(move |label, _, _, _, tip| {
                    let (Some(item), Some(model)) = (weak_item.upgrade(), weak_model.upgrade())
                    else {
                        return false;
                    };
                    let position = item.position();
                    if let Some((row, text)) = &*tooltip.borrow()
                        && *row == position
                    {
                        tip.set_text(Some(text));
                        return true;
                    }
                    if requested.get() == Some(position) {
                        return false;
                    }
                    requested.set(Some(position));
                    let page = model.read_page(position as usize, 1);
                    let tooltip = tooltip.clone();
                    let requested = requested.clone();
                    let weak_label = label.downgrade();
                    let weak_item = item.downgrade();
                    glib::spawn_future_local(async move {
                        let page = page.await;
                        if requested.get() != Some(position) {
                            return;
                        }
                        requested.set(None);
                        let Some(value) = page.ok().and_then(|page| {
                            page.rows.first().and_then(|row| row.get(index)).map(
                                |cell| match cell {
                                    dzban_core::ResultCell::Null => "NULL".to_string(),
                                    dzban_core::ResultCell::Text(text) => {
                                        text.chars().take(TOOLTIP_CHARS).collect()
                                    }
                                },
                            )
                        }) else {
                            return;
                        };
                        tooltip.replace(Some((position, value)));
                        if weak_item
                            .upgrade()
                            .is_some_and(|item| item.position() == position)
                            && let Some(label) = weak_label.upgrade()
                        {
                            label.trigger_tooltip_query();
                        }
                    });
                    false
                });
                item.set_child(Some(&label));
            });
            let weak = Rc::downgrade(&self.0);
            factory.connect_bind(move |_, item| {
                let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
                let Some(label) = item.child().and_downcast::<gtk4::Label>() else {
                    return;
                };
                if let Some(inner) = weak.upgrade() {
                    ResultPanel(inner).mark_cursor(item, &label, index);
                }
                let row = item
                    .item()
                    .and_downcast::<gtk4::StringObject>()
                    .map(|row| decode_row(&row.string()))
                    .unwrap_or_default();
                set_cell(&label, row.get(index).cloned().flatten().as_deref());
            });
            let title = column.name.replace('_', "__");
            let view_column = gtk4::ColumnViewColumn::new(Some(&title), Some(factory));
            view_column.set_resizable(true);
            view_column.set_fixed_width(column_width(&view, &column.name, &[], index));
            view.append_column(&view_column);
        }

        let weak_view = view.downgrade();
        let weak_model = model.downgrade();
        let sized = Rc::new(Cell::new(false));
        model.connect_local("page-ready", false, move |_| {
            let (Some(view), Some(model)) = (weak_view.upgrade(), weak_model.upgrade()) else {
                return None;
            };
            if sized.replace(true) {
                return None;
            }
            let rows = model.cached_values();
            let columns = view.columns();
            for index in 1..columns.n_items() {
                let Some(column) = columns.item(index).and_downcast::<gtk4::ColumnViewColumn>()
                else {
                    continue;
                };
                let title = column.title().map(|title| title.replace("__", "_"));
                let width = column_width(
                    &view,
                    title.as_deref().unwrap_or_default(),
                    &rows,
                    index as usize - 1,
                );
                if width > column.fixed_width() {
                    column.set_fixed_width(width);
                }
            }
            None
        });

        let keys = gtk4::EventControllerKey::new();
        keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let weak = Rc::downgrade(&self.0);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(inner) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let panel = ResultPanel(inner);
            let modifiers = modifiers
                & (gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::SHIFT_MASK
                    | gdk::ModifierType::ALT_MASK);
            match (key, modifiers) {
                (gdk::Key::space, gdk::ModifierType::NO_MODIFIER_MASK) => {
                    panel.space_pressed();
                    glib::Propagation::Stop
                }
                (gdk::Key::Return | gdk::Key::KP_Enter, gdk::ModifierType::NO_MODIFIER_MASK) => {
                    if let Some(row) = panel.selected_row() {
                        panel.open_cell_dialog(row, panel.0.column.get());
                    }
                    glib::Propagation::Stop
                }
                (gdk::Key::c, gdk::ModifierType::CONTROL_MASK) => {
                    panel.copy_selection();
                    glib::Propagation::Stop
                }
                (gdk::Key::Left | gdk::Key::Right, gdk::ModifierType::NO_MODIFIER_MASK) => {
                    panel.move_cursor(if key == gdk::Key::Left { -1 } else { 1 });
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        let weak = Rc::downgrade(&self.0);
        keys.connect_key_released(move |_, key, _, _| {
            if key == gdk::Key::space
                && let Some(inner) = weak.upgrade()
            {
                ResultPanel(inner).space_released();
            }
        });
        view.add_controller(keys);

        let weak = Rc::downgrade(&self.0);
        selection.connect_selection_changed(move |_, _, _| {
            if let Some(inner) = weak.upgrade() {
                let panel = ResultPanel(inner);
                panel.refresh_cursor();
                if panel.0.inspector.is_visible() {
                    panel.refresh_inspector();
                }
            }
        });
        view
    }

    fn render_state(&self) {
        let document = self.0.document.borrow().clone();
        let issues = self.issues();
        let rows = self.available_rows();
        let progress = document
            .as_ref()
            .and_then(|document| document.feedback.progress);
        self.set_progress(progress.is_some());

        if rows > 0 && !issues.is_empty() {
            self.0.banner_text.set_text(
                &issues
                    .iter()
                    .map(|issue| format!("{}: {}", issue.title, issue.detail))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            self.0.banner.set_reveal_child(true);
        } else {
            self.0.banner.set_reveal_child(false);
        }
        self.0
            .retry
            .set_visible(self.0.load_error.borrow().is_some());

        let summary = document.as_ref().filter(|document| {
            progress.is_some()
                && document.started_at_seconds.is_some()
                && document.history != Some(true)
        });
        if rows == 0 && !issues.is_empty() {
            self.0.summary.set_visible(false);
            self.0.state.show(
                "dialog-error-symbolic",
                issues[0].title,
                &issues
                    .iter()
                    .map(|issue| issue.detail.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n"),
                true,
            );
        } else if let Some(document) = summary {
            self.0.state.hide();
            self.render_summary(document, progress.unwrap());
            self.0.summary.set_visible(true);
        } else if rows == 0 && !self.is_loading() {
            self.0.summary.set_visible(false);
            let (icon, title, detail) = match document.as_ref() {
                Some(document) => (
                    format::empty_icon(document.feedback.empty),
                    format::empty_title(document.feedback.empty),
                    format::empty_detail(document),
                ),
                None => (
                    format::empty_icon(Default::default()),
                    format::empty_title(Default::default()),
                    "Run your SQL to explore the results.".to_string(),
                ),
            };
            self.0.state.show(icon, title, &detail, false);
        } else {
            self.0.summary.set_visible(false);
            self.0.state.hide();
        }
        self.update_clock(summary.is_some());
    }

    fn render_summary(&self, document: &DocumentPresentation, progress: ResultProgress) {
        self.0.summary_title.set_text(match progress {
            ResultProgress::Planning => "Planning query",
            ResultProgress::Running => "Running query",
            ResultProgress::Canceling => "Stopping query",
        });
        if let Some(started) = document.started_at_seconds {
            let elapsed = (glib::real_time() as f64 / 1_000_000.0 - started).max(0.0);
            self.0
                .summary_elapsed
                .set_text(&format!("Elapsed {elapsed:.1}s"));
            let started = glib::DateTime::from_unix_local(started as i64)
                .and_then(|time| time.format("%H:%M:%S"))
                .map(|text| format!("Started {text}"))
                .unwrap_or_default();
            self.0.summary_started.set_text(&started);
        }
        let received = document.received_rows.unwrap_or(document.rows);
        self.0
            .summary_rows
            .set_text(&format!("{received} rows received"));
        let estimate = document
            .execution_plan
            .as_ref()
            .and_then(|plan| {
                plan.estimate
                    .rows
                    .map(|rows| format::row_estimate(plan.backend, rows))
            })
            .unwrap_or_default();
        self.0.summary_estimate.set_visible(!estimate.is_empty());
        self.0.summary_estimate.set_text(&estimate);
    }

    fn update_clock(&self, active: bool) {
        let running = self.0.clock.borrow().is_some();
        if active && !running {
            let weak = Rc::downgrade(&self.0);
            let source = glib::timeout_add_local(Duration::from_millis(100), move || {
                let Some(inner) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let panel = ResultPanel(inner);
                let document = panel.0.document.borrow().clone();
                match document {
                    Some(document) if document.feedback.progress.is_some() => {
                        panel.render_summary(&document, document.feedback.progress.unwrap());
                        glib::ControlFlow::Continue
                    }
                    _ => {
                        panel.0.clock.replace(None);
                        glib::ControlFlow::Break
                    }
                }
            });
            self.0.clock.replace(Some(source));
        } else if !active && let Some(source) = self.0.clock.borrow_mut().take() {
            source.remove();
        }
    }

    fn set_progress(&self, busy: bool) {
        let running = self.0.pulse.borrow().is_some();
        if busy && !running {
            self.0.progress.set_opacity(1.0);
            let animate = gtk4::Settings::default()
                .is_none_or(|settings| settings.is_gtk_enable_animations());
            if animate {
                let weak = self.0.progress.downgrade();
                let source = glib::timeout_add_local(Duration::from_millis(80), move || {
                    let Some(progress) = weak.upgrade() else {
                        return glib::ControlFlow::Break;
                    };
                    progress.pulse();
                    glib::ControlFlow::Continue
                });
                self.0.pulse.replace(Some(source));
            } else {
                self.0.progress.set_fraction(1.0);
            }
        } else if !busy {
            if let Some(source) = self.0.pulse.borrow_mut().take() {
                source.remove();
            }
            self.0.progress.set_fraction(0.0);
            self.0.progress.set_opacity(0.0);
        }
    }

    fn retry(&self) {
        self.0.load_error.replace(None);
        self.0.table.replace(None);
        self.0.scroll.set_child(None::<&gtk4::Widget>);
        let document = self.0.document.borrow().clone();
        if let Some(document) = document {
            self.render(&document);
        }
    }

    fn save_restore(&self) {
        let table = self.0.table.borrow();
        let Some(table) = table.as_ref() else {
            return;
        };
        let (horizontal, vertical) = self.0.pending_scroll.take().unwrap_or_else(|| {
            (
                self.0.scroll.hadjustment().value(),
                self.0.scroll.vadjustment().value(),
            )
        });
        self.0.restore.replace(Some(Restore {
            columns: table.columns.clone(),
            row: first_selected(&table.selection),
            column: self.0.column.get(),
            horizontal,
            vertical,
        }));
    }

    fn apply_restore(&self) {
        let Some(restore) = self.0.restore.borrow_mut().take() else {
            return;
        };
        let table = self.0.table.borrow();
        let document = self.0.document.borrow();
        let terminal = document
            .as_ref()
            .is_some_and(|document| document.feedback.progress.is_none());
        let current = table
            .as_ref()
            .map(|table| table.columns.clone())
            .unwrap_or_default();
        if table.is_none()
            && document
                .as_ref()
                .is_some_and(|document| !document.result_available && terminal)
        {
            return;
        }
        let rows = table
            .as_ref()
            .map_or(0, |table| table.model.n_items() as usize);
        let decision = ResultViewRequest::Restore {
            previous: restore.columns.clone(),
            current,
            row: restore.row,
            column: restore.column,
            rows,
            terminal,
        }
        .evaluate();
        match decision.restoration {
            Some(RestorationState::Apply) if !decision.selection_pending => {
                let Some(table) = table.as_ref() else {
                    return;
                };
                if let Some(position) = decision.selection {
                    self.0.column.set(position.column);
                    table.selection.select_item(position.row as u32, true);
                }
                self.0
                    .pending_scroll
                    .set(Some((restore.horizontal, restore.vertical)));
                self.schedule_pending_scroll();
            }
            Some(RestorationState::SchemaChanged) => {}
            _ => {
                drop(table);
                drop(document);
                self.0.restore.replace(Some(restore));
            }
        }
    }

    fn schedule_pending_scroll(&self) {
        if self.0.pending_scroll.get().is_none() || self.0.scroll_scheduled.replace(true) {
            return;
        }
        let weak = Rc::downgrade(&self.0);
        glib::idle_add_local_once(move || {
            if let Some(inner) = weak.upgrade() {
                inner.scroll_scheduled.set(false);
                ResultPanel(inner).apply_pending_scroll();
            }
        });
    }

    fn apply_pending_scroll(&self) {
        let Some((horizontal, vertical)) = self.0.pending_scroll.get() else {
            return;
        };
        let (h, v) = (self.0.scroll.hadjustment(), self.0.scroll.vadjustment());
        h.set_value(horizontal);
        v.set_value(vertical);
        if (h.value() - horizontal).abs() < 0.5 && (v.value() - vertical).abs() < 0.5 {
            self.0.pending_scroll.set(None);
        }
    }

    pub fn focus(&self) -> bool {
        let view = self
            .0
            .table
            .borrow()
            .as_ref()
            .map(|table| table.view.clone());
        match view {
            Some(view) => {
                let target = self.0.table.borrow().as_ref().and_then(|table| {
                    (table.model.n_items() > 0)
                        .then(|| first_selected(&table.selection).unwrap_or(0))
                });
                match target {
                    Some(row) => {
                        let column = self.0.column.get();
                        self.focus_cell(row, column);
                        true
                    }
                    None => view.grab_focus(),
                }
            }
            None => self.0.scroll.grab_focus(),
        }
    }

    fn selected_row(&self) -> Option<usize> {
        self.0
            .table
            .borrow()
            .as_ref()
            .and_then(|table| first_selected(&table.selection))
    }

    fn space_pressed(&self) {
        if self.0.space.get().is_some() {
            return;
        }
        if self.0.inspector.is_pinned() {
            self.peek(PeekEvent::Close, Duration::ZERO);
            return;
        }
        if self.selected_row().is_none() {
            return;
        }
        self.0.space.set(Some(Instant::now()));
        self.peek(PeekEvent::Show, Duration::ZERO);
    }

    fn space_released(&self) {
        let Some(started) = self.0.space.take() else {
            return;
        };
        self.peek(PeekEvent::Release, started.elapsed());
    }

    pub fn show_row_preview(&self) -> bool {
        if self.selected_row().is_none() {
            return false;
        }
        self.peek(PeekEvent::Open, Duration::ZERO);
        true
    }

    fn peek(&self, event: PeekEvent, elapsed: Duration) {
        let visibility =
            self.0
                .app
                .context()
                .tabs
                .peek(self.0.tab, PreviewSurface::Row, event, elapsed);
        let Ok(visibility) = visibility else {
            self.0.inspector.set_visibility(false, false);
            return;
        };
        let was_visible = self.0.inspector.is_visible();
        if visibility.visible && (!was_visible || self.0.inspector.row() != self.selected_row()) {
            self.refresh_inspector();
            if !was_visible {
                self.0.inspector.select_column(self.0.column.get());
            }
        }
        self.0
            .inspector
            .set_visibility(visibility.visible, visibility.pinned);
        if !visibility.visible && was_visible {
            let view = self
                .0
                .table
                .borrow()
                .as_ref()
                .map(|table| table.view.clone());
            if let Some(view) = view {
                view.grab_focus();
            }
        }
    }

    pub fn navigate(&self, direction: Direction) -> bool {
        if self.0.inspector.is_visible() {
            return self.0.inspector.navigate(direction);
        }
        if !contains_focus(&self.0.root) {
            return false;
        }
        let table = self.0.table.borrow();
        let Some(table) = table.as_ref() else {
            return false;
        };
        let rows = table.model.n_items() as i64;
        if let Some(delta) = direction.vertical() {
            if rows == 0 {
                return true;
            }
            let current = first_selected(&table.selection).map_or(-1, |row| row as i64);
            let next = (current + delta).clamp(0, rows - 1) as usize;
            let column = self.0.column.get();
            self.focus_cell(next, column);
        } else if let Some(delta) = direction.horizontal() {
            self.move_cursor(delta);
        }
        true
    }

    #[cfg(test)]
    pub fn inspector(&self) -> &RowInspector {
        &self.0.inspector
    }

    #[cfg(test)]
    pub fn table(&self) -> Option<gtk4::ColumnView> {
        self.0
            .table
            .borrow()
            .as_ref()
            .map(|table| table.view.clone())
    }

    #[cfg(test)]
    pub fn selected(&self) -> Option<usize> {
        self.selected_row()
    }

    #[cfg(test)]
    pub fn focused(&self) -> bool {
        contains_focus(&self.0.root)
    }

    #[cfg(test)]
    pub fn column(&self) -> usize {
        self.0.column.get()
    }

    fn move_cursor(&self, delta: i64) {
        let row = {
            let table = self.0.table.borrow();
            let Some(table) = table.as_ref() else {
                return;
            };
            let count = table.columns.len() as i64;
            if count == 0 {
                return;
            }
            let next = (self.0.column.get() as i64 + delta).clamp(0, count - 1);
            self.0.column.set(next as usize);
            first_selected(&table.selection)
        };
        self.refresh_cursor();
        if let Some(row) = row {
            self.reveal_cell(row, self.0.column.get(), gtk4::ListScrollFlags::NONE);
        }
    }

    fn focus_cell(&self, row: usize, column: usize) {
        self.0.column.set(column);
        self.reveal_cell(
            row,
            column,
            gtk4::ListScrollFlags::FOCUS | gtk4::ListScrollFlags::SELECT,
        );
        self.refresh_cursor();
    }

    fn reveal_cell(&self, row: usize, column: usize, flags: gtk4::ListScrollFlags) {
        let table = self.0.table.borrow();
        let Some(table) = table.as_ref() else {
            return;
        };
        if row >= table.model.n_items() as usize {
            return;
        }
        if flags.contains(gtk4::ListScrollFlags::FOCUS) && !contains_focus(&table.view) {
            table.view.grab_focus();
        }
        let view_column = table
            .view
            .columns()
            .item(column as u32 + 1)
            .and_downcast::<gtk4::ColumnViewColumn>();
        table
            .view
            .scroll_to(row as u32, view_column.as_ref(), flags, None);
    }

    fn mark_cursor(&self, item: &gtk4::ListItem, label: &gtk4::Label, column: usize) {
        let cursor =
            self.selected_row() == Some(item.position() as usize) && self.0.column.get() == column;
        let target = label.parent().unwrap_or_else(|| label.clone().upcast());
        if cursor {
            target.add_css_class("result-cursor");
        } else {
            target.remove_css_class("result-cursor");
        }
    }

    fn refresh_cursor(&self) {
        let cells = {
            let mut cells = self.0.cells.borrow_mut();
            cells.retain(|(item, label, _)| item.upgrade().is_some() && label.upgrade().is_some());
            cells
                .iter()
                .filter_map(|(item, label, column)| {
                    Some((item.upgrade()?, label.upgrade()?, *column))
                })
                .collect::<Vec<_>>()
        };
        for (item, label, column) in cells {
            self.mark_cursor(&item, &label, column);
        }
    }

    pub fn close_inspector(&self) -> bool {
        if !self.0.inspector.is_visible() {
            return false;
        }
        if !self.0.inspector.back() {
            self.peek(PeekEvent::Close, Duration::ZERO);
        }
        true
    }

    #[cfg(test)]
    pub fn inspector_visible(&self) -> bool {
        self.0.inspector.is_visible()
    }

    fn refresh_inspector(&self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let (model, columns) = {
            let table = self.0.table.borrow();
            let Some(table) = table.as_ref() else {
                return;
            };
            (table.model.clone(), table.columns.clone())
        };
        let cached = model
            .item(row as u32)
            .and_downcast::<gtk4::StringObject>()
            .map(|item| decode_row(&item.string()))
            .unwrap_or_default();
        let values = columns
            .iter()
            .enumerate()
            .map(|(index, column)| (column.name.clone(), cached.get(index).cloned().flatten()))
            .collect();
        self.0.inspector.show_row(row, values);
        let request = self.0.peek_request.get().wrapping_add(1);
        self.0.peek_request.set(request);
        let page = model.read_page(row, 1);
        let weak = Rc::downgrade(&self.0);
        glib::spawn_future_local(async move {
            let Ok(page) = page.await else {
                return;
            };
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let panel = ResultPanel(inner);
            if panel.0.peek_request.get() != request
                || !panel.0.inspector.is_visible()
                || panel.selected_row() != Some(row)
            {
                return;
            }
            let values = page
                .rows
                .first()
                .map(|cells| {
                    page.columns
                        .iter()
                        .zip(cells)
                        .map(|(column, cell)| {
                            (
                                column.name.clone(),
                                match cell {
                                    dzban_core::ResultCell::Null => None,
                                    dzban_core::ResultCell::Text(text) => {
                                        Some(text.chars().take(INSPECTOR_CHARS).collect())
                                    }
                                },
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            panel.0.inspector.show_row(row, values);
        });
    }

    fn move_inspected_row(&self, direction: i64) {
        let table = self.0.table.borrow();
        let Some(table) = table.as_ref() else {
            return;
        };
        let rows = table.model.n_items() as usize;
        let Some(current) = first_selected(&table.selection) else {
            return;
        };
        let decision = ResultViewRequest::MoveRow {
            position: dzban_core::outputs::view::ResultPosition {
                row: current,
                column: self.0.column.get(),
            },
            direction,
            rows,
            columns: table.columns.len(),
        }
        .evaluate();
        if let Some(position) = decision.selection {
            table.selection.select_item(position.row as u32, true);
            table
                .view
                .scroll_to(position.row as u32, None, gtk4::ListScrollFlags::NONE, None);
        }
    }

    fn open_inspected_cell(&self, column: usize) {
        let Some(row) = self.0.inspector.row() else {
            return;
        };
        let reader = self
            .0
            .table
            .borrow()
            .as_ref()
            .map(|table| table.reader.clone());
        let Some(reader) = reader else {
            return;
        };
        let chunk = self.0.app.run(async move {
            reader
                .cell(row, column, 0, dzban_core::outputs::MAX_CELL_CHUNK_SCALARS)
                .await
        });
        let weak = Rc::downgrade(&self.0);
        glib::spawn_future_local(async move {
            let chunk = chunk.await;
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let panel = ResultPanel(inner);
            if panel.0.inspector.row() != Some(row) {
                return;
            }
            match chunk {
                Ok(Ok(chunk)) => {
                    panel
                        .0
                        .inspector
                        .show_cell(column, chunk.text.as_deref(), chunk.total)
                }
                Ok(Err(error)) => panel.0.app.toast(&error.to_string()),
                Err(error) => panel.0.app.toast(&error.to_string()),
            }
        });
    }

    pub fn copy_selection(&self) {
        let (reader, ranges) = {
            let table = self.0.table.borrow();
            let Some(table) = table.as_ref() else {
                return;
            };
            (table.reader.clone(), selected_ranges(&table.selection))
        };
        if ranges.is_empty() {
            return;
        }
        self.copy(async move { reader.export_markdown(ranges).await }, None);
    }

    fn copy<F>(&self, start: F, done: Option<CopyDone>)
    where
        F: std::future::Future<Output = Result<OutputExport, dzban_core::outputs::OutputError>>
            + Send
            + 'static,
    {
        self.cancel_copy();
        let canceled = Arc::new(AtomicBool::new(false));
        self.0.copying.replace(Some(canceled.clone()));
        self.0.copy_error.replace(None);
        self.changed();
        let worker = canceled.clone();
        let export = self.0.app.run(async move {
            let mut export = start.await.map_err(|error| error.to_string())?;
            let mut text = String::new();
            loop {
                if worker.load(Ordering::SeqCst) {
                    return Err("Copy canceled".to_string());
                }
                let chunk = export
                    .next_chunk()
                    .await
                    .map_err(|error| error.to_string())?;
                text.push_str(&chunk.text);
                if chunk.done {
                    return Ok(text);
                }
            }
        });
        let weak = Rc::downgrade(&self.0);
        glib::spawn_future_local(async move {
            let result = export.await.unwrap_or_else(|error| Err(error.to_string()));
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let panel = ResultPanel(inner);
            let current = panel
                .0
                .copying
                .borrow()
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, &canceled));
            if !current || canceled.load(Ordering::SeqCst) {
                return;
            }
            panel.0.copying.replace(None);
            let outcome = match result {
                Ok(text) => {
                    panel.0.root.clipboard().set_text(&text);
                    Ok(())
                }
                Err(error) => {
                    panel.0.copy_error.replace(Some(error.clone()));
                    Err(error)
                }
            };
            if let Some(done) = done {
                done(outcome);
            }
            panel.render_state();
            panel.changed();
        });
    }

    pub fn cancel_copy(&self) {
        if let Some(active) = self.0.copying.borrow_mut().take() {
            active.store(true, Ordering::SeqCst);
        }
    }

    pub fn open_cell_dialog(&self, row: usize, column: usize) {
        let (reader, name) = {
            let table = self.0.table.borrow();
            let Some(table) = table.as_ref() else {
                return;
            };
            let Some(name) = table.columns.get(column).map(|column| column.name.clone()) else {
                return;
            };
            (table.reader.clone(), name)
        };
        let dialog = adw::Dialog::new();
        dialog.set_title(&format!("Row {}, column {}", row + 1, column + 1));
        dialog.set_content_width(720);
        dialog.set_content_height(480);
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        toolbar.add_top_bar(&header);
        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.set_margin_bottom(12);
        let heading = gtk4::Label::new(Some(&name));
        heading.add_css_class("heading");
        heading.set_xalign(0.0);
        content.append(&heading);
        let text = gtk4::TextView::new();
        text.set_editable(false);
        text.set_monospace(true);
        text.set_wrap_mode(gtk4::WrapMode::WordChar);
        text.update_property(&[gtk4::accessible::Property::Label("Cell value")]);
        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_child(Some(&text));
        scroll.add_css_class("card");
        content.append(&scroll);
        let footer = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let detail = gtk4::Label::new(Some("Loading..."));
        detail.add_css_class("dim-label");
        detail.add_css_class("caption");
        detail.set_hexpand(true);
        detail.set_xalign(0.0);
        footer.append(&detail);
        let spinner = gtk4::Spinner::new();
        footer.append(&spinner);
        let retry = gtk4::Button::with_label("Retry");
        retry.set_visible(false);
        footer.append(&retry);
        let copy = gtk4::Button::with_label("Copy Entire Cell");
        copy.add_css_class("suggested-action");
        footer.append(&copy);
        content.append(&footer);
        toolbar.set_content(Some(&content));
        dialog.set_child(Some(&toolbar));

        let load = {
            let app = self.0.app.clone();
            let reader = reader.clone();
            let text = text.clone();
            let detail = detail.clone();
            let retry = retry.clone();
            Rc::new(move || {
                retry.set_visible(false);
                detail.set_text("Loading...");
                let reader = reader.clone();
                let chunk = app.run(async move {
                    reader
                        .cell(row, column, 0, dzban_core::outputs::MAX_CELL_CHUNK_SCALARS)
                        .await
                });
                let text = text.clone();
                let detail = detail.clone();
                let retry = retry.clone();
                glib::spawn_future_local(async move {
                    match chunk.await {
                        Ok(Ok(chunk)) => match &chunk.text {
                            Some(value) => {
                                text.buffer().set_text(value);
                                let shown = value.chars().count();
                                detail.set_text(&if chunk.total == 0 {
                                    "Empty value".to_string()
                                } else {
                                    format!("Characters 1-{shown} of {}", chunk.total)
                                });
                            }
                            None => {
                                text.buffer().set_text("NULL");
                                detail.set_text("SQL NULL");
                            }
                        },
                        Ok(Err(error)) => {
                            detail.set_text(&error.to_string());
                            retry.set_visible(true);
                        }
                        Err(error) => {
                            detail.set_text(&error.to_string());
                            retry.set_visible(true);
                        }
                    }
                });
            })
        };
        load();
        let load_again = load.clone();
        retry.connect_clicked(move |_| load_again());
        let weak = Rc::downgrade(&self.0);
        let spinner_for_copy = spinner.clone();
        let detail_for_copy = detail.clone();
        copy.connect_clicked(move |button| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let panel = ResultPanel(inner);
            if panel.is_copying() {
                panel.cancel_copy();
                spinner_for_copy.set_spinning(false);
                button.set_label("Copy Entire Cell");
                return;
            }
            button.set_label("Cancel Copy");
            spinner_for_copy.set_spinning(true);
            let reader = reader.clone();
            let button = button.clone();
            let spinner = spinner_for_copy.clone();
            let detail = detail_for_copy.clone();
            panel.copy(
                async move { reader.export_cell(row, column).await },
                Some(Rc::new(move |result| {
                    button.set_label("Copy Entire Cell");
                    spinner.set_spinning(false);
                    if let Err(error) = result {
                        detail.set_text(&error);
                    }
                })),
            );
        });
        let weak = Rc::downgrade(&self.0);
        dialog.connect_closed(move |_| {
            if let Some(inner) = weak.upgrade() {
                ResultPanel(inner).cancel_copy();
            }
        });
        dialog.present(Some(&self.0.root));
    }
}

struct ResultState {
    root: gtk4::Box,
    tile: gtk4::Box,
    icon: gtk4::Image,
    title: gtk4::Label,
    detail: gtk4::Label,
}

impl ResultState {
    fn new() -> Self {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        root.set_halign(gtk4::Align::Center);
        root.set_valign(gtk4::Align::Center);
        root.set_margin_start(24);
        root.set_margin_end(24);
        root.add_css_class("result-state");
        let tile = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        tile.add_css_class("result-state-tile");
        tile.set_halign(gtk4::Align::Center);
        let icon = gtk4::Image::new();
        icon.set_pixel_size(22);
        icon.set_halign(gtk4::Align::Center);
        icon.set_valign(gtk4::Align::Center);
        icon.set_vexpand(true);
        tile.append(&icon);
        root.append(&tile);
        let title = gtk4::Label::new(None);
        title.add_css_class("result-state-title");
        root.append(&title);
        let detail = gtk4::Label::new(None);
        detail.set_wrap(true);
        detail.set_max_width_chars(80);
        detail.set_justify(gtk4::Justification::Center);
        detail.add_css_class("dim-label");
        root.append(&detail);
        Self {
            root,
            tile,
            icon,
            title,
            detail,
        }
    }

    fn show(&self, icon: &str, title: &str, detail: &str, error: bool) {
        self.icon.set_icon_name(Some(icon));
        self.title.set_text(title);
        self.detail.set_text(detail);
        self.detail.set_selectable(error);
        for widget in [
            self.tile.upcast_ref::<gtk4::Widget>(),
            self.detail.upcast_ref(),
        ] {
            if error {
                widget.add_css_class("error");
            } else {
                widget.remove_css_class("error");
            }
        }
        if error {
            self.detail.add_css_class("monospace");
            self.detail.remove_css_class("dim-label");
        } else {
            self.detail.remove_css_class("monospace");
            self.detail.add_css_class("dim-label");
        }
        self.root.set_can_target(error);
        self.root.set_visible(true);
    }

    fn hide(&self) {
        self.root.set_visible(false);
    }
}

fn first_selected(selection: &gtk4::MultiSelection) -> Option<usize> {
    let bitset = selection.selection();
    (!bitset.is_empty()).then(|| bitset.minimum() as usize)
}

fn selected_ranges(selection: &gtk4::MultiSelection) -> Vec<RowRange> {
    let bitset = selection.selection();
    let mut ranges: Vec<RowRange> = Vec::new();
    let size = bitset.size();
    for index in 0..size {
        let row = bitset.nth(index as u32) as usize;
        match ranges.last_mut() {
            Some(range) if range.end == row => range.end += 1,
            _ => ranges.push(RowRange {
                start: row,
                end: row + 1,
            }),
        }
    }
    ranges
}

fn keep_selection_across_page_loads(selection: &gtk4::MultiSelection) {
    let remembered = Rc::new(RefCell::new(gtk4::Bitset::new_empty()));
    {
        let remembered = remembered.clone();
        selection.connect_selection_changed(move |selection, _, _| {
            remembered.replace(selection.selection().copy());
        });
    }
    selection.connect_items_changed(move |selection, position, removed, added| {
        if removed == 0 || removed != added {
            return;
        }
        let mask = gtk4::Bitset::new_range(position, added);
        let kept = remembered.borrow().copy();
        selection.set_selection(&kept, &mask);
    });
}

fn cell_label() -> gtk4::Label {
    let label = gtk4::Label::new(None);
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_single_line_mode(true);
    label.set_lines(1);
    label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    label.set_margin_start(4);
    label.set_margin_end(4);
    label.add_css_class("monospace");
    label.add_css_class("result-cell");
    label
}

fn set_cell(label: &gtk4::Label, value: Option<&str>) {
    match value {
        Some(value) => {
            label.set_text(&format::cell_preview(value, PREVIEW_CHARS));
            label.remove_css_class("result-null");
        }
        None => {
            label.set_text("NULL");
            label.add_css_class("result-null");
        }
    }
}

fn column_width(
    view: &gtk4::ColumnView,
    name: &str,
    rows: &[Vec<Option<String>>],
    index: usize,
) -> i32 {
    let layout = view.create_pango_layout(Some(name));
    let (header, _) = layout.pixel_size();
    let mut width = header + HEADER_SPACE;
    let measure = cell_label();
    for row in rows.iter().take(200) {
        let Some(value) = row.get(index) else {
            continue;
        };
        set_cell(&measure, value.as_deref());
        let (_, natural, _, _) = measure.measure(gtk4::Orientation::Horizontal, -1);
        width = width.max(natural + CELL_SPACE);
    }
    width.clamp(48, MAX_COLUMN_WIDTH)
}
