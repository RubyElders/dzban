use gtk4::gdk;
use gtk4::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Down,
    Up,
    Right,
}

impl Direction {
    pub fn vertical(self) -> Option<i64> {
        match self {
            Self::Up => Some(-1),
            Self::Down => Some(1),
            _ => None,
        }
    }

    pub fn horizontal(self) -> Option<i64> {
        match self {
            Self::Left => Some(-1),
            Self::Right => Some(1),
            _ => None,
        }
    }
}

pub fn vim(key: gdk::Key) -> Option<Direction> {
    match key {
        gdk::Key::h => Some(Direction::Left),
        gdk::Key::j => Some(Direction::Down),
        gdk::Key::k => Some(Direction::Up),
        gdk::Key::l => Some(Direction::Right),
        _ => None,
    }
}

pub fn contains_focus(widget: &impl IsA<gtk4::Widget>) -> bool {
    let widget = widget.as_ref();
    widget
        .root()
        .and_then(|root| root.focus())
        .is_some_and(|focus| focus == *widget || focus.is_ancestor(widget))
}

pub fn is_text_input(widget: &gtk4::Widget) -> bool {
    widget.is::<gtk4::Text>() || widget.is::<gtk4::TextView>() || widget.is::<gtk4::Editable>()
}

pub fn move_list(list: &gtk4::ListBox, delta: i64) -> bool {
    let Some(current) = list.selected_row().map(|row| row.index()) else {
        return list.row_at_index(0).is_some_and(|row| {
            list.select_row(Some(&row));
            row.grab_focus()
        });
    };
    let mut count = 0;
    while list.row_at_index(count).is_some() {
        count += 1;
    }
    let next = (i64::from(current) + delta).clamp(0, i64::from(count) - 1) as i32;
    list.row_at_index(next).is_some_and(|row| {
        list.select_row(Some(&row));
        row.grab_focus()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vim_letters_map_to_arrow_directions() {
        assert_eq!(vim(gdk::Key::h), Some(Direction::Left));
        assert_eq!(vim(gdk::Key::j), Some(Direction::Down));
        assert_eq!(vim(gdk::Key::k), Some(Direction::Up));
        assert_eq!(vim(gdk::Key::l), Some(Direction::Right));
        assert_eq!(vim(gdk::Key::H), None);
        assert_eq!(Direction::Down.vertical(), Some(1));
        assert_eq!(Direction::Left.horizontal(), Some(-1));
        assert_eq!(Direction::Left.vertical(), None);
    }
}
