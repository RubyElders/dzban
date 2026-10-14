use gtk4::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

use dzban_core::preview::PeekState;
pub use dzban_core::preview::{PreviewRelease, preview_release};

#[derive(Clone)]
pub struct PreviewOverlay {
    widget: gtk4::Widget,
    state: Rc<Cell<PeekState>>,
}

impl PreviewOverlay {
    pub fn new(widget: &impl IsA<gtk4::Widget>) -> Self {
        Self {
            widget: widget.clone().upcast(),
            state: Rc::new(Cell::new(PeekState::default())),
        }
    }

    fn update(&self, transition: impl FnOnce(&mut PeekState)) {
        let mut state = self.state.get();
        transition(&mut state);
        self.state.set(state);
        self.widget.set_visible(state.is_visible());
    }

    pub fn show_transient(&self) {
        self.update(PeekState::show_transient);
    }
    pub fn hide_transient(&self) {
        self.update(PeekState::hide_transient);
    }
    pub fn pin(&self) -> bool {
        let mut accepted = false;
        self.update(|state| accepted = state.pin());
        accepted
    }
    pub fn close(&self) -> bool {
        let mut accepted = false;
        self.update(|state| accepted = state.close());
        accepted
    }
    pub fn is_visible(&self) -> bool {
        self.state.get().is_visible()
    }
    pub fn is_pinned(&self) -> bool {
        self.state.get().is_pinned()
    }
}
