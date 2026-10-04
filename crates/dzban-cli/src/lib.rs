pub mod args;
pub mod console;
pub mod console_command;
pub mod context;
pub mod dispatch;
pub mod editor;
pub mod editor_view;
pub mod external_editor;
pub mod keyboard;
pub mod output;
pub mod pager;
pub mod picker;
pub mod statement;
pub mod streamed_output;

#[derive(Debug)]
pub struct CliError {
    pub message: String,
    pub code: u8,
}

impl CliError {
    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 2,
        }
    }
    pub fn runtime(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 1,
        }
    }
}
