use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::editor::Editor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_column: usize,
}

fn display(character: char) -> String {
    match character {
        '\t' => "\\t".into(),
        '\r' => "\\r".into(),
        character if character.is_control() => format!("\\u{{{:x}}}", character as u32),
        character => character.to_string(),
    }
}

pub fn single_line(text: &str, width: usize) -> String {
    let mut result = String::new();
    for character in text
        .chars()
        .flat_map(|ch| display(ch).chars().collect::<Vec<_>>())
    {
        if result.width() + character.width().unwrap_or(0) > width {
            break;
        }
        result.push(character);
    }
    result
}

pub fn layout(editor: &Editor, status: &str, width: usize, height: usize) -> Layout {
    let limit = width.max(6) - 1;
    let mut header = String::new();
    for character in status
        .chars()
        .flat_map(|character| display(character).chars().collect::<Vec<_>>())
    {
        if header.width() + character.width().unwrap_or(0) > limit {
            break;
        }
        header.push(character);
    }
    let mut lines = vec![header, "> ".into()];
    let mut column = 2;
    let mut cursor = (1, 2);
    for (index, character) in editor.text().char_indices() {
        if character != '\n' {
            let first_width = display(character)
                .chars()
                .next()
                .and_then(UnicodeWidthChar::width)
                .unwrap_or(0);
            if column + first_width > limit {
                lines.push("  ".into());
                column = 2;
            }
        }
        if index == editor.cursor() {
            cursor = (lines.len() - 1, column);
        }
        if character == '\n' {
            lines.push("  ".into());
            column = 2;
            continue;
        }
        for visible in display(character).chars() {
            let cells = visible.width().unwrap_or(0);
            if column + cells > limit {
                lines.push("  ".into());
                column = 2;
            }
            lines.last_mut().unwrap().push(visible);
            column += cells;
        }
    }
    if editor.cursor() == editor.text().len() {
        cursor = (lines.len() - 1, column);
    }
    let available = height.saturating_sub(2).max(1);
    let start = cursor.0.saturating_sub(available);
    let mut visible = vec![lines[0].clone()];
    visible.extend(lines.into_iter().skip(start + 1).take(available));
    Layout {
        lines: visible,
        cursor_row: cursor.0 - start,
        cursor_column: cursor.1,
    }
}

#[derive(Default)]
pub struct Renderer {
    cursor_row: usize,
    previous: Option<Layout>,
}

impl Renderer {
    pub fn clear(&mut self) -> String {
        let mut sequence = "\r".to_string();
        if self.cursor_row > 0 {
            sequence.push_str(&format!("\x1b[{}A", self.cursor_row));
        }
        sequence.push_str("\x1b[J");
        self.cursor_row = 0;
        self.previous = None;
        sequence
    }

    pub fn paint(&mut self, layout: &Layout) -> String {
        if self.previous.as_ref() == Some(layout) {
            return String::new();
        }
        let mut sequence = self.clear();
        sequence.push_str(&layout.lines.join("\r\n"));
        let up = layout.lines.len() - 1 - layout.cursor_row;
        if up > 0 {
            sequence.push_str(&format!("\x1b[{up}A"));
        }
        sequence.push_str(&format!("\r\x1b[{}C", layout.cursor_column));
        self.cursor_row = layout.cursor_row;
        self.previous = Some(layout.clone());
        sequence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_lines_fit_display_cells_and_escape_terminal_controls() {
        assert_eq!(single_line("界界abc", 5), "界界a");
        assert_eq!(single_line("abc", 0), "");
        let line = single_line("SELECT\n\u{1b}[31m", 80);
        assert!(!line.contains('\n'));
        assert!(!line.contains('\u{1b}'));
        assert!(line.contains("\\u{1b}"));
    }
    use crate::editor::Input;

    #[test]
    fn multiline_cursor_uses_cells_not_utf8_bytes() {
        let mut editor = Editor::default();
        editor.apply(Input::Text("select\nž界x".into()), false);
        editor.apply(Input::Left, false);
        let view = layout(&editor, "ready", 30, 10);
        assert_eq!(view.lines, ["ready", "> select", "  ž界x"]);
        assert_eq!((view.cursor_row, view.cursor_column), (2, 5));
    }

    #[test]
    fn wrapping_and_resize_keep_cursor_in_view() {
        let mut editor = Editor::default();
        editor.apply(
            Input::Text("abcdefghijk\nsecond\nthird\nfourth".into()),
            false,
        );
        for width in [8, 12, 30] {
            let view = layout(&editor, "long status header", width, 5);
            assert!(view.lines.iter().all(|line| line.width() < width));
            assert!(view.cursor_row < view.lines.len());
            assert!(view.cursor_column < width);
            assert!(view.lines.len() <= 4);
        }
    }

    #[test]
    fn pasted_controls_cannot_emit_terminal_commands() {
        let mut editor = Editor::default();
        editor.apply(Input::Text("\x1b[2J\t".into()), false);
        let view = layout(&editor, "\x1bstatus", 60, 10);
        assert!(view.lines.iter().all(|line| !line.contains('\x1b')));
        assert_eq!(editor.text(), "\x1b[2J\t");
    }
}
