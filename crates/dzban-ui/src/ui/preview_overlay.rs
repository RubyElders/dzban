use gtk4::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewRelease {
    Pin,
    Hide,
}

pub const PREVIEW_HOLD: Duration = Duration::from_millis(250);

pub fn preview_release(elapsed: Duration) -> PreviewRelease {
    if elapsed < PREVIEW_HOLD {
        PreviewRelease::Pin
    } else {
        PreviewRelease::Hide
    }
}

#[derive(Clone)]
pub struct PreviewOverlay {
    widget: gtk4::Widget,
    pinned: Rc<Cell<bool>>,
}

impl PreviewOverlay {
    pub fn new(widget: &impl IsA<gtk4::Widget>) -> Self {
        Self {
            widget: widget.clone().upcast(),
            pinned: Rc::new(Cell::new(false)),
        }
    }

    pub fn show_transient(&self) {
        self.widget.set_visible(true);
    }

    pub fn hide_transient(&self) {
        if !self.pinned.get() {
            self.widget.set_visible(false);
        }
    }

    pub fn pin(&self) -> bool {
        if !self.widget.is_visible() {
            return false;
        }
        self.pinned.set(true);
        true
    }

    pub fn close(&self) -> bool {
        if !self.widget.is_visible() {
            return false;
        }
        self.pinned.set(false);
        self.widget.set_visible(false);
        true
    }

    pub fn is_visible(&self) -> bool {
        self.widget.is_visible()
    }

    pub fn is_pinned(&self) -> bool {
        self.pinned.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taps_pin_and_holds_hide_on_release() {
        assert_eq!(
            preview_release(Duration::from_millis(100)),
            PreviewRelease::Pin
        );
        assert_eq!(preview_release(PREVIEW_HOLD), PreviewRelease::Hide);
        assert_eq!(
            preview_release(Duration::from_secs(1)),
            PreviewRelease::Hide
        );
    }
}
