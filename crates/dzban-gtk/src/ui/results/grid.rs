use super::*;
use dzban_core::outputs::grid::StickyLayout;
use dzban_core::outputs::view::ResultPosition;

pub(super) struct StickyViews {
    pub left: gtk4::ScrolledWindow,
    pub right: gtk4::ScrolledWindow,
    layout: RefCell<Option<StickyLayout>>,
}

impl StickyViews {
    pub fn new(overlay: &gtk4::Overlay, scroll: &gtk4::ScrolledWindow) -> Self {
        let side = |align| {
            let side = gtk4::ScrolledWindow::new();
            side.set_policy(gtk4::PolicyType::External, gtk4::PolicyType::External);
            let main = scroll.vadjustment();
            side.vadjustment().connect_changed(move |adjustment| {
                adjustment.set_value(main.value());
            });
            let wheel =
                gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::BOTH_AXES);
            wheel.set_propagation_phase(gtk4::PropagationPhase::Capture);
            let main = scroll.vadjustment();
            let horizontal = scroll.hadjustment();
            wheel.connect_scroll(move |controller, dx, dy| {
                let step = if controller.unit() == gdk::ScrollUnit::Surface {
                    1.0
                } else {
                    main.step_increment().max(1.0) * 3.0
                };
                main.set_value(main.value() + dy * step);
                horizontal.set_value(horizontal.value() + dx * step);
                glib::Propagation::Stop
            });
            side.add_controller(wheel);
            side.set_halign(align);
            side.set_valign(gtk4::Align::Fill);
            side.set_visible(false);
            side.add_css_class("result-frozen");
            overlay.add_overlay(&side);
            side
        };
        Self {
            left: side(gtk4::Align::Start),
            right: side(gtk4::Align::End),
            layout: RefCell::new(None),
        }
    }
    pub fn clear(&self) {
        self.left.set_visible(false);
        self.right.set_visible(false);
        self.left.set_child(None::<&gtk4::Widget>);
        self.right.set_child(None::<&gtk4::Widget>);
        self.layout.replace(None);
    }
}

fn gesture(modifiers: gdk::ModifierType) -> SelectionGesture {
    if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
        SelectionGesture::Extend
    } else if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
        SelectionGesture::Toggle
    } else {
        SelectionGesture::Replace
    }
}

impl ResultPanel {
    pub(super) fn select_grid_cell(&self, row: usize, column: usize, extend: bool) {
        if let Some(grid) = self.0.grid.borrow_mut().as_mut() {
            grid.select_cell(ResultPosition { row, column }, extend);
        }
        self.apply_grid_selection();
        self.focus_grid();
    }

    pub(super) fn select_grid_row(&self, row: usize, modifiers: gdk::ModifierType) {
        if let Some(grid) = self.0.grid.borrow_mut().as_mut() {
            grid.select_row(row, gesture(modifiers));
        }
        self.apply_grid_selection();
        self.focus_grid();
    }

    pub(super) fn select_grid_column(&self, column: usize, modifiers: gdk::ModifierType) {
        if let Some(grid) = self.0.grid.borrow_mut().as_mut() {
            grid.select_column(column, gesture(modifiers));
        }
        self.apply_grid_selection();
        self.focus_grid();
    }

    pub fn toggle_result_rows(&self) {
        let visible = self.grid_layout().visible;
        if let Some(grid) = self.0.grid.borrow_mut().as_mut() {
            grid.toggle_rows(&visible);
        }
        self.apply_grid_selection();
    }

    pub fn toggle_frozen_column(&self) {
        let column = self
            .0
            .grid
            .borrow()
            .as_ref()
            .and_then(ResultGrid::cursor)
            .map(|cursor| cursor.column);
        if let Some(column) = column {
            self.toggle_grid_frozen(column);
        }
    }

    fn toggle_grid_frozen(&self, column: usize) {
        if let Some(grid) = self.0.grid.borrow_mut().as_mut() {
            grid.toggle_frozen(column);
        }
        self.refresh_grid_view();
    }

    pub(super) fn apply_grid_selection(&self) {
        let selected = gtk4::Bitset::new_empty();
        if let Some(grid) = self.0.grid.borrow().as_ref() {
            for row in grid.rows() {
                selected.add(*row as u32);
            }
        }
        let selection = self
            .0
            .table
            .borrow()
            .as_ref()
            .map(|table| table.selection.clone());
        if let Some(selection) = selection {
            selection.set_selection(&selected, &gtk4::Bitset::new_range(0, selection.n_items()));
        }
        self.refresh_cursor();
        self.refresh_grid_view();
        if self.0.inspector.is_visible() {
            self.refresh_inspector();
        }
    }

    fn focus_grid(&self) {
        if !contains_focus(&self.0.root)
            && let Some(table) = self.0.table.borrow().as_ref()
        {
            let horizontal = self.0.scroll.hadjustment().value();
            let vertical = self.0.scroll.vadjustment().value();
            table.view.grab_focus();
            self.0.scroll.hadjustment().set_value(horizontal);
            self.0.scroll.vadjustment().set_value(vertical);
        }
    }

    pub(super) fn grid_layout(&self) -> StickyLayout {
        let table = self.0.table.borrow();
        let Some(table) = table.as_ref() else {
            return StickyLayout::default();
        };
        let columns = table.view.columns();
        let widths = (0..columns.n_items())
            .filter_map(|index| {
                columns
                    .item(index)
                    .and_downcast::<gtk4::ColumnViewColumn>()
                    .map(|column| f64::from(column.fixed_width()))
            })
            .collect::<Vec<_>>();
        self.0
            .grid
            .borrow()
            .as_ref()
            .map(|grid| {
                grid.sticky_layout(
                    &widths,
                    self.0.scroll.hadjustment().value(),
                    self.0.scroll.hadjustment().page_size(),
                )
            })
            .unwrap_or_default()
    }

    pub(super) fn refresh_grid_view(&self) {
        let Ok(mut last) = self.0.sticky.layout.try_borrow_mut() else {
            return;
        };
        let layout = self.grid_layout();
        let table = self.0.table.borrow();
        let Some(table) = table.as_ref() else {
            return;
        };
        let indices = table.view.columns();
        for (side, columns, previous) in [
            (
                &self.0.sticky.left,
                &layout.left,
                last.as_ref().map(|layout| &layout.left),
            ),
            (
                &self.0.sticky.right,
                &layout.right,
                last.as_ref().map(|layout| &layout.right),
            ),
        ] {
            if previous != Some(columns) {
                if columns.is_empty() {
                    side.set_child(None::<&gtk4::Widget>);
                } else {
                    let view = gtk4::ColumnView::new(Some(table.selection.clone()));
                    view.add_css_class("result-table");
                    view.add_css_class("result-sticky-table");
                    if self
                        .0
                        .grid
                        .borrow()
                        .as_ref()
                        .is_none_or(|grid| grid.mode() != SelectionMode::Rows)
                    {
                        view.add_css_class("cell-selection");
                    }
                    view.set_show_column_separators(true);
                    view.set_focusable(false);
                    for index in columns {
                        if let Some(source) = indices
                            .item(*index as u32)
                            .and_downcast::<gtk4::ColumnViewColumn>()
                        {
                            let column = gtk4::ColumnViewColumn::new(
                                source.title().as_deref(),
                                source.factory(),
                            );
                            column.set_fixed_width(source.fixed_width());
                            view.append_column(&column);
                        }
                    }
                    self.install_grid_headers(&view, columns);
                    side.set_child(Some(&view));
                }
            }
            let width = columns
                .iter()
                .filter_map(|index| {
                    indices
                        .item(*index as u32)
                        .and_downcast::<gtk4::ColumnViewColumn>()
                })
                .map(|column| column.fixed_width())
                .sum::<i32>();
            if let Some(view) = side.child().and_downcast::<gtk4::ColumnView>() {
                for (index, original) in columns.iter().enumerate() {
                    if let (Some(column), Some(source)) = (
                        view.columns()
                            .item(index as u32)
                            .and_downcast::<gtk4::ColumnViewColumn>(),
                        indices
                            .item(*original as u32)
                            .and_downcast::<gtk4::ColumnViewColumn>(),
                    ) && column.fixed_width() != source.fixed_width()
                    {
                        column.set_fixed_width(source.fixed_width());
                    }
                }
            }
            side.set_size_request(width, -1);
            side.set_margin_bottom(self.0.scroll.hscrollbar().height());
            if side == &self.0.sticky.right {
                side.set_margin_end(self.0.scroll.vscrollbar().width());
            }
            side.set_visible(!columns.is_empty());
            side.vadjustment()
                .set_value(self.0.scroll.vadjustment().value());
        }
        *last = Some(layout);
        let grid = self.0.grid.borrow();
        if let Some(grid) = grid.as_ref() {
            let mut icons = self.0.header_locks.borrow_mut();
            icons.retain(|(icon, _)| icon.upgrade().is_some());
            for (icon, column) in icons.iter() {
                if let Some(icon) = icon.upgrade() {
                    let name = if *column == 0 {
                        "#"
                    } else {
                        grid.schema()
                            .get(column - 1)
                            .map_or("", |column| column.name.as_str())
                    };
                    let label = format!(
                        "{} column: {name}",
                        if grid.is_frozen(*column) {
                            "Frozen"
                        } else {
                            "Unfrozen"
                        }
                    );
                    icon.update_property(&[gtk4::accessible::Property::Label(&label)]);
                    if let Some(title) = icon.parent().and_then(|parent| parent.parent()) {
                        if grid.mode() == SelectionMode::Columns && grid.columns().contains(column)
                        {
                            title.add_css_class("result-column-selected");
                        } else {
                            title.remove_css_class("result-column-selected");
                        }
                    }
                    icon.set_opacity(if grid.is_frozen(*column) { 1.0 } else { 0.0 });
                }
            }
            for (column, action) in self.0.freeze_actions.borrow().iter().enumerate() {
                action.set_state(&grid.is_frozen(column).to_variant());
            }
        }
    }

    pub(super) fn init_grid_actions(&self, columns: usize) {
        let group = gtk4::gio::SimpleActionGroup::new();
        let mut actions = Vec::new();
        for column in 0..columns {
            let frozen = self
                .0
                .grid
                .borrow()
                .as_ref()
                .is_some_and(|grid| grid.is_frozen(column));
            let action = gtk4::gio::SimpleAction::new_stateful(
                &format!("freeze-{column}"),
                None,
                &frozen.to_variant(),
            );
            let weak = Rc::downgrade(&self.0);
            action.connect_activate(move |_, _| {
                if let Some(inner) = weak.upgrade() {
                    ResultPanel(inner).toggle_grid_frozen(column);
                }
            });
            group.add_action(&action);
            actions.push(action);
        }
        self.0.freeze_actions.replace(actions);
        self.0.root.insert_action_group("results", Some(&group));
    }

    pub(super) fn install_grid_headers(&self, view: &gtk4::ColumnView, columns: &[usize]) {
        let Some(header) = view.first_child() else {
            return;
        };
        let mut title = header.first_child();
        for (index, column) in columns.iter().copied().enumerate() {
            let Some(widget) = title else {
                break;
            };
            title = widget.next_sibling();
            let menu = gtk4::gio::Menu::new();
            menu.append(
                Some("Freeze Column"),
                Some(&format!("results.freeze-{column}")),
            );
            if let Some(view_column) = view
                .columns()
                .item(index as u32)
                .and_downcast::<gtk4::ColumnViewColumn>()
            {
                view_column.set_header_menu(Some(&menu));
            }
            if let Some(box_) = widget.first_child().and_downcast::<gtk4::Box>() {
                let icon = gtk4::Image::from_icon_name("changes-prevent-symbolic");
                icon.set_pixel_size(16);
                icon.set_opacity(
                    if self
                        .0
                        .grid
                        .borrow()
                        .as_ref()
                        .is_some_and(|grid| grid.is_frozen(column))
                    {
                        1.0
                    } else {
                        0.0
                    },
                );
                icon.set_margin_start(4);
                box_.insert_child_after(&icon, box_.first_child().as_ref());
                self.0
                    .header_locks
                    .borrow_mut()
                    .push((icon.downgrade(), column));
            }
            let click = gtk4::GestureClick::new();
            click.set_name(Some("result-column-selection"));
            click.set_button(1);
            click.set_propagation_phase(gtk4::PropagationPhase::Capture);
            let weak = Rc::downgrade(&self.0);
            click.connect_pressed(move |gesture, _, x, _| {
                if let Some(widget) = gesture.widget()
                    && (x < 6.0 || x > f64::from(widget.width() - 6))
                {
                    return;
                }
                if let Some(inner) = weak.upgrade() {
                    ResultPanel(inner).select_grid_column(column, gesture.current_event_state());
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                }
            });
            widget.add_controller(click);
        }
    }

    pub(super) fn grid_move(&self, horizontal: i64, vertical: i64, extend: bool) {
        if let Some(grid) = self.0.grid.borrow_mut().as_mut() {
            grid.move_cursor(horizontal, vertical, extend);
        }
        self.apply_grid_selection();
        let cursor = self.0.grid.borrow().as_ref().and_then(ResultGrid::cursor);
        if let Some(cursor) = cursor {
            self.reveal_grid_cell(cursor);
        }
    }

    pub(super) fn reveal_grid_cell(&self, cursor: ResultPosition) {
        if let Some(table) = self.0.table.borrow().as_ref() {
            let columns = table.view.columns();
            let widths = (0..columns.n_items())
                .filter_map(|index| {
                    columns
                        .item(index)
                        .and_downcast::<gtk4::ColumnViewColumn>()
                        .map(|column| f64::from(column.fixed_width()))
                })
                .collect::<Vec<_>>();
            let adjustment = self.0.scroll.hadjustment();
            let target = self
                .0
                .grid
                .borrow()
                .as_ref()
                .map_or(adjustment.value(), |grid| {
                    grid.reveal_column(
                        &widths,
                        adjustment.value(),
                        adjustment.page_size(),
                        cursor.column,
                    )
                });
            let flags = if contains_focus(&table.view) {
                gtk4::ListScrollFlags::FOCUS
            } else {
                gtk4::ListScrollFlags::NONE
            };
            table.view.scroll_to(cursor.row as u32, None, flags, None);
            adjustment.set_value(target);
        }
    }
}
