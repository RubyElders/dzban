use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::editor::{Editor, Input};

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Edit(Input),
    Clear,
    History(bool),
    SearchHistory,
    EndOfInput,
    ClientCommand,
    Ignore,
}

pub fn action(key: KeyEvent, editor: &Editor) -> Action {
    if key.kind == KeyEventKind::Release {
        return Action::Ignore;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('l') => Action::Clear,
            KeyCode::Char('p') => Action::History(true),
            KeyCode::Char('n') => Action::History(false),
            KeyCode::Char('r') => Action::SearchHistory,
            KeyCode::Char('d') => Action::EndOfInput,
            KeyCode::Char('c') => Action::Edit(Input::Interrupt),
            KeyCode::Char('a') => Action::Edit(Input::Home),
            KeyCode::Char('e') => Action::Edit(Input::End),
            KeyCode::Char('b') => Action::Edit(Input::Left),
            KeyCode::Char('f') => Action::Edit(Input::Right),
            KeyCode::Char('h') => Action::Edit(Input::Backspace),
            KeyCode::Char('k') => Action::Edit(Input::KillLine),
            KeyCode::Char('u') => Action::Edit(Input::KillBefore),
            KeyCode::Char('w') => Action::Edit(Input::KillWord),
            KeyCode::Char('y') => Action::Edit(Input::Yank),
            KeyCode::Char('j') => Action::Edit(Input::Enter),
            KeyCode::Enter => enter(key, editor),
            _ => Action::Ignore,
        };
    }
    match key.code {
        KeyCode::Enter => enter(key, editor),
        KeyCode::Esc => Action::Edit(Input::Escape),
        KeyCode::Backspace => Action::Edit(Input::Backspace),
        KeyCode::Delete => Action::Edit(Input::Delete),
        KeyCode::Left => Action::Edit(Input::Left),
        KeyCode::Right => Action::Edit(Input::Right),
        KeyCode::Up | KeyCode::Down if editor.at_history_boundary(key.code == KeyCode::Up) => {
            Action::History(key.code == KeyCode::Up)
        }
        KeyCode::Up => Action::Edit(Input::Up),
        KeyCode::Down => Action::Edit(Input::Down),
        KeyCode::Home => Action::Edit(Input::Home),
        KeyCode::End => Action::Edit(Input::End),
        KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::ALT) => {
            Action::Edit(Input::Text(character.to_string()))
        }
        _ => Action::Ignore,
    }
}

fn enter(key: KeyEvent, editor: &Editor) -> Action {
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        Action::Edit(Input::Enter)
    } else if editor.client_command().is_some() {
        Action::ClientCommand
    } else {
        Action::Edit(Input::Accept)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_shortcuts_have_explicit_actions() {
        let editor = Editor::default();
        for (character, expected) in [
            ('l', Action::Clear),
            ('p', Action::History(true)),
            ('n', Action::History(false)),
            ('r', Action::SearchHistory),
            ('a', Action::Edit(Input::Home)),
            ('e', Action::Edit(Input::End)),
            ('k', Action::Edit(Input::KillLine)),
            ('u', Action::Edit(Input::KillBefore)),
            ('w', Action::Edit(Input::KillWord)),
            ('y', Action::Edit(Input::Yank)),
        ] {
            assert_eq!(
                action(
                    KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL),
                    &editor
                ),
                expected
            );
        }
    }

    #[test]
    fn enter_and_history_navigation_respect_the_draft() {
        let mut editor = Editor::default();
        for modifiers in [KeyModifiers::NONE, KeyModifiers::CONTROL] {
            assert_eq!(
                action(KeyEvent::new(KeyCode::Enter, modifiers), &editor),
                Action::Edit(Input::Accept)
            );
        }
        editor.apply(Input::Text("SELECT 1\n\\q".into()), false);
        assert_eq!(
            action(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
                &editor
            ),
            Action::ClientCommand
        );
        assert_eq!(
            action(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT), &editor),
            Action::Edit(Input::Enter)
        );
        assert_eq!(
            action(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &editor),
            Action::Edit(Input::Up)
        );
        editor.apply(Input::Up, false);
        assert_eq!(
            action(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &editor),
            Action::History(true)
        );
    }
}
