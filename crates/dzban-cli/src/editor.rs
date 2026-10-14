#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Text(String),
    Enter,
    Submit,
    Accept,
    KillLine,
    KillBefore,
    KillWord,
    Yank,
    Interrupt,
    Escape,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Edit,
    Submit(String),
    SubmitExpanded(String),
    Cancel,
    Busy,
}

#[derive(Default, Debug)]
pub struct Editor {
    text: String,
    cursor: usize,
    preferred_column: Option<usize>,
    killed: String,
    history_position: Option<usize>,
    history_draft: String,
    history_entries: Vec<String>,
    dialect: crate::statement::Dialect,
}

impl Editor {
    pub fn set_dialect(&mut self, dialect: crate::statement::Dialect) {
        self.dialect = dialect;
        self.history_position = None;
    }
    pub fn recall(&mut self, entries: &[String], previous: bool) {
        let position = match (self.history_position, previous) {
            (None, true) if !entries.is_empty() => {
                self.history_draft.clone_from(&self.text);
                self.history_entries = entries.to_vec();
                Some(0)
            }
            (Some(index), true) => {
                Some((index + 1).min(self.history_entries.len().saturating_sub(1)))
            }
            (Some(0), false) => None,
            (Some(index), false) => Some(index - 1),
            _ => return,
        };
        self.text = position
            .and_then(|index| self.history_entries.get(index))
            .unwrap_or(&self.history_draft)
            .clone();
        self.cursor = self.text.len();
        self.history_position = position;
    }

    pub fn at_history_boundary(&self, previous: bool) -> bool {
        self.history_position.is_some()
            || if previous {
                !self.text[..self.cursor].contains('\n')
            } else {
                !self.text[self.cursor..].contains('\n')
            }
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn client_command(&self) -> Option<(&str, &str)> {
        if self.cursor != self.text.len() {
            return None;
        }
        let start = self.text.rfind('\n').map_or(0, |index| index + 1);
        let line = self.text[start..].trim();
        (line.starts_with('\\') && !matches!(line, "\\g" | "\\gx"))
            .then_some((line, &self.text[..start]))
    }

    pub fn apply(&mut self, input: Input, running: bool) -> Effect {
        self.history_position = None;
        if !matches!(input, Input::Up | Input::Down) {
            self.preferred_column = None;
        }
        match input {
            Input::Text(text) => {
                self.text.insert_str(self.cursor, &text);
                self.cursor += text.len();
            }
            Input::Submit => return self.submit(running),
            Input::Accept if crate::statement::complete(&self.text, self.dialect) => {
                return self.submit(running);
            }
            Input::Accept => return self.apply(Input::Enter, running),
            Input::KillLine => {
                let end = self.text[self.cursor..]
                    .find('\n')
                    .map_or(self.text.len(), |end| self.cursor + end);
                let end = if end == self.cursor && end < self.text.len() {
                    end + 1
                } else {
                    end
                };
                self.killed = self.text.drain(self.cursor..end).collect();
            }
            Input::KillBefore => {
                let start = self.line_start();
                self.killed = self.text.drain(start..self.cursor).collect();
                self.cursor = start;
            }
            Input::KillWord => {
                let prefix = &self.text[..self.cursor];
                let end = prefix.trim_end().len();
                let start = prefix[..end]
                    .char_indices()
                    .rev()
                    .find(|(_, character)| character.is_whitespace())
                    .map_or(0, |(index, character)| index + character.len_utf8());
                self.killed = self.text.drain(start..self.cursor).collect();
                self.cursor = start;
            }
            Input::Yank => {
                self.text.insert_str(self.cursor, &self.killed);
                self.cursor += self.killed.len();
            }
            Input::Enter => {
                let command = self.text.rsplit('\n').next().unwrap_or_default();
                if self.cursor == self.text.len() && matches!(command, "\\g" | "\\gx") {
                    if running {
                        return Effect::Busy;
                    }
                    let expanded = command == "\\gx";
                    self.text.truncate(self.text.len() - command.len());
                    self.cursor = self.text.len();
                    return match self.submit(false) {
                        Effect::Submit(sql) if expanded => Effect::SubmitExpanded(sql),
                        effect => effect,
                    };
                }
                self.text.insert(self.cursor, '\n');
                self.cursor += 1;
            }
            Input::Interrupt | Input::Escape if running => return Effect::Cancel,
            Input::Interrupt => {
                self.text.clear();
                self.cursor = 0;
            }
            Input::Escape => {}
            Input::Backspace => {
                if let Some((start, _)) = self.text[..self.cursor].char_indices().next_back() {
                    self.text.drain(start..self.cursor);
                    self.cursor = start;
                }
            }
            Input::Delete => {
                if let Some(character) = self.text[self.cursor..].chars().next() {
                    self.text
                        .drain(self.cursor..self.cursor + character.len_utf8());
                }
            }
            Input::Left => {
                self.cursor = self.text[..self.cursor]
                    .char_indices()
                    .next_back()
                    .map_or(0, |(index, _)| index);
            }
            Input::Right => {
                if let Some(ch) = self.text[self.cursor..].chars().next() {
                    self.cursor += ch.len_utf8();
                }
            }
            Input::Up => self.vertical(-1),
            Input::Down => self.vertical(1),
            Input::Home => self.cursor = self.line_start(),
            Input::End => {
                self.cursor += self.text[self.cursor..]
                    .find('\n')
                    .unwrap_or(self.text.len() - self.cursor)
            }
        }
        Effect::Edit
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1)
    }

    fn vertical(&mut self, direction: isize) {
        let start = self.line_start();
        let column = *self
            .preferred_column
            .get_or_insert_with(|| self.text[start..self.cursor].chars().count());
        let target = if direction < 0 {
            if start == 0 {
                return;
            }
            self.text[..start - 1]
                .rfind('\n')
                .map_or(0, |index| index + 1)
        } else {
            let Some(end) = self.text[self.cursor..].find('\n') else {
                return;
            };
            self.cursor + end + 1
        };
        let end = self.text[target..]
            .find('\n')
            .map_or(self.text.len(), |index| target + index);
        self.cursor = self.text[target..end]
            .char_indices()
            .nth(column)
            .map_or(end, |(index, _)| target + index);
    }

    fn submit(&mut self, running: bool) -> Effect {
        if running {
            return Effect::Busy;
        }
        if self.text.trim().is_empty() {
            return Effect::Edit;
        }
        self.cursor = 0;
        Effect::Submit(std::mem::take(&mut self.text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_waits_for_a_real_terminator_and_preserves_busy_draft() {
        let mut editor = Editor::default();
        editor.apply(Input::Text("SELECT ';'".into()), false);
        assert_eq!(editor.apply(Input::Accept, false), Effect::Edit);
        assert_eq!(editor.text(), "SELECT ';'\n");
        editor.apply(Input::Text("; -- done".into()), false);
        assert_eq!(editor.apply(Input::Accept, true), Effect::Busy);
        assert_eq!(
            editor.apply(Input::Accept, false),
            Effect::Submit("SELECT ';'\n; -- done".into())
        );
    }

    #[test]
    fn history_preserves_draft_and_recalls_whole_multiline_queries() {
        let entries = vec!["SELECT\n2;".into(), "SELECT 1;".into()];
        let mut editor = Editor::default();
        editor.apply(Input::Text("unfinished".into()), false);
        editor.recall(&entries, true);
        assert_eq!(editor.text(), "SELECT\n2;");
        editor.recall(&["newly completed query;".into()], true);
        assert_eq!(editor.text(), "SELECT 1;");
        editor.recall(&entries, false);
        editor.recall(&entries, false);
        assert_eq!(editor.text(), "unfinished");
        editor.recall(&[], true);
        assert_eq!(editor.text(), "unfinished");
    }

    #[test]
    fn readline_editing_preserves_unicode_and_yanks_killed_text() {
        let mut editor = Editor::default();
        editor.apply(Input::Text("SELECT žluťoučký".into()), false);
        editor.apply(Input::KillWord, false);
        assert_eq!(editor.text(), "SELECT ");
        editor.apply(Input::Yank, false);
        assert_eq!(editor.text(), "SELECT žluťoučký");
        editor.apply(Input::Home, false);
        editor.apply(Input::KillLine, false);
        assert_eq!(editor.text(), "");
        editor.apply(Input::Yank, false);
        editor.apply(Input::KillBefore, false);
        assert_eq!(editor.text(), "");
        editor.apply(Input::Text("SELECT\u{2003}žluťoučký".into()), false);
        editor.apply(Input::KillWord, false);
        assert_eq!(editor.text(), "SELECT\u{2003}");
    }

    #[test]
    fn indented_quit_is_a_client_command_even_after_a_draft() {
        let mut editor = Editor::default();
        editor.apply(Input::Text("SELECT 1\n  \\q  ".into()), false);
        assert_eq!(editor.client_command(), Some(("\\q", "SELECT 1\n")));
    }

    #[test]
    fn expanded_submission_is_explicit_and_preserves_busy_input() {
        let mut editor = Editor::default();
        editor.apply(Input::Text("SELECT 'č';\n\\gx".into()), false);
        assert!(editor.client_command().is_none());
        assert_eq!(editor.apply(Input::Enter, true), Effect::Busy);
        assert_eq!(editor.text(), "SELECT 'č';\n\\gx");
        assert_eq!(
            editor.apply(Input::Enter, false),
            Effect::SubmitExpanded("SELECT 'č';\n".into())
        );
        editor.apply(Input::Text("SELECT 2\n\\g".into()), false);
        assert_eq!(
            editor.apply(Input::Enter, false),
            Effect::Submit("SELECT 2\n".into())
        );
        editor.apply(Input::Text("SELECT 3\n\\gx".into()), false);
        assert_eq!(
            editor.apply(Input::Submit, false),
            Effect::Submit("SELECT 3\n\\gx".into())
        );
    }

    #[test]
    fn client_commands_keep_the_opaque_draft_separate_from_the_command_line() {
        let mut editor = Editor::default();
        let sql = "SELECT '\\e' AS value;\n-- no SQL parsing\n";
        editor.apply(Input::Text(format!("{sql}\\edit")), false);
        assert_eq!(editor.client_command(), Some(("\\edit", sql)));
        let mut submitted = Editor::default();
        submitted.apply(Input::Text(format!("{sql}\\edit")), false);
        assert!(
            matches!(submitted.apply(Input::Submit, false), Effect::Submit(text) if text == format!("{sql}\\edit"))
        );
        editor.apply(Input::Left, false);
        assert_eq!(editor.client_command(), None);
        editor.apply(Input::End, false);
        assert_eq!(editor.client_command(), Some(("\\edit", sql)));
        editor.apply(Input::Interrupt, false);
        editor.apply(Input::Text("\\e".into()), false);
        assert_eq!(editor.client_command(), Some(("\\e", "")));
        editor.apply(Input::Interrupt, false);
        editor.apply(Input::Text(format!("{sql}\\g")), false);
        assert_eq!(editor.client_command(), None);
        assert!(matches!(editor.apply(Input::Enter, false), Effect::Submit(text) if text == sql));
    }

    #[test]
    fn vertical_navigation_preserves_column_across_short_unicode_lines() {
        let mut editor = Editor::default();
        editor.apply(Input::Text("abcd\nž\nab界d".into()), false);
        editor.apply(Input::Up, false);
        assert_eq!(editor.cursor(), "abcd\nž".len());
        editor.apply(Input::Up, false);
        assert_eq!(editor.cursor(), 4);
        editor.apply(Input::Down, false);
        editor.apply(Input::Down, false);
        assert_eq!(editor.cursor(), editor.text().len());
        editor.apply(Input::Home, false);
        assert_eq!(editor.cursor(), "abcd\nž\n".len());
        editor.apply(Input::Delete, false);
        assert_eq!(editor.text(), "abcd\nž\nb界d");
    }

    #[test]
    fn enter_preserves_opaque_sql_and_only_explicit_submit_executes() {
        let mut editor = Editor::default();
        editor.apply(Input::Text("select ';'".into()), false);
        assert_eq!(editor.apply(Input::Enter, false), Effect::Edit);
        editor.apply(Input::Text("union select 'x';".into()), false);
        assert_eq!(
            editor.apply(Input::Submit, false),
            Effect::Submit("select ';'\nunion select 'x';".into())
        );
    }

    #[test]
    fn busy_submission_keeps_next_query_and_interrupt_only_cancels() {
        let mut editor = Editor::default();
        editor.apply(Input::Text("select 2".into()), true);
        assert_eq!(editor.apply(Input::Submit, true), Effect::Busy);
        assert_eq!(editor.apply(Input::Interrupt, true), Effect::Cancel);
        assert_eq!(editor.text(), "select 2");
        editor.apply(Input::Interrupt, false);
        assert!(editor.text().is_empty());
    }

    #[test]
    fn dismissing_transient_ui_cancels_active_work_without_destroying_the_draft() {
        let mut editor = Editor::default();
        editor.apply(Input::Text("SELECT next".into()), false);
        assert_eq!(editor.apply(Input::Escape, true), Effect::Cancel);
        assert_eq!(editor.text(), "SELECT next");
        assert_eq!(editor.apply(Input::Escape, false), Effect::Edit);
        assert_eq!(editor.text(), "SELECT next");
    }

    #[test]
    fn unicode_editing_never_splits_a_character() {
        let mut editor = Editor::default();
        editor.apply(Input::Text("žbán".into()), false);
        editor.apply(Input::Left, false);
        editor.apply(Input::Backspace, false);
        assert_eq!(editor.text(), "žbn");
        assert_eq!(editor.cursor(), "žb".len());
    }

    #[test]
    fn standalone_send_command_submits_without_sql_parsing() {
        let mut editor = Editor::default();
        editor.apply(Input::Text("select 1;\n\\g".into()), false);
        assert_eq!(
            editor.apply(Input::Enter, false),
            Effect::Submit("select 1;\n".into())
        );
    }
}
