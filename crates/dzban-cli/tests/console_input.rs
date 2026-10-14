use dzban_cli::editor::{Editor, Effect, Input};

#[test]
fn multiline_sql_and_history_round_trip_without_automatic_execution() {
    let mut editor = Editor::default();
    editor.apply(Input::Text("SELECT ';' AS value".into()), false);
    assert_eq!(editor.apply(Input::Accept, false), Effect::Edit);
    editor.apply(Input::Text("; -- ready".into()), false);
    let Effect::Submit(sql) = editor.apply(Input::Accept, false) else {
        panic!("terminated query did not submit");
    };
    editor.apply(Input::Text("my draft".into()), false);
    editor.recall(std::slice::from_ref(&sql), true);
    assert_eq!(editor.text(), sql);
    editor.recall(&[sql], false);
    assert_eq!(editor.text(), "my draft");
}
