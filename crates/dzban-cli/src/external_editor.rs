use std::io::Write;
use std::path::PathBuf;

pub fn save_buffer(path: &std::path::Path, text: &str) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("Cannot create query file: {error}"))?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("Cannot save query buffer: {error}"))
}

struct BufferFile {
    directory: PathBuf,
    file: PathBuf,
}

impl BufferFile {
    fn new(text: &str) -> Result<Self, String> {
        let directory = std::env::temp_dir().join(format!("dzb-editor-{}", uuid::Uuid::new_v4()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&directory)
            .map_err(|error| error.to_string())?;
        let buffer = Self {
            file: directory.join("query.sql"),
            directory,
        };
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&buffer.file)
            .map_err(|error| error.to_string())?;
        file.write_all(text.as_bytes())
            .map_err(|error| error.to_string())?;
        Ok(buffer)
    }
}

impl Drop for BufferFile {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.directory) {
            tracing::warn!("Could not remove temporary editor directory: {error}");
        }
    }
}

pub fn choose_editor(dzban: Option<&str>, visual: Option<&str>, editor: Option<&str>) -> String {
    [dzban, visual, editor]
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
        .unwrap_or("vi")
        .into()
}

pub fn configured_editor() -> String {
    choose_editor(
        std::env::var("DZBAN_EDITOR").ok().as_deref(),
        std::env::var("VISUAL").ok().as_deref(),
        std::env::var("EDITOR").ok().as_deref(),
    )
}

pub fn edit(command: &str, text: &str) -> Result<String, String> {
    let words = shell_words::split(command).map_err(|error| error.to_string())?;
    let (program, arguments) = words.split_first().ok_or("Editor command is empty")?;
    let buffer = BufferFile::new(text)?;
    let status = std::process::Command::new(program)
        .args(arguments)
        .arg(&buffer.file)
        .status()
        .map_err(|error| format!("Cannot start editor {program}: {error}"))?;
    if !status.success() {
        return Err(format!("Editor exited with {status}"));
    }
    std::fs::read_to_string(&buffer.file)
        .map_err(|error| format!("Cannot read edited SQL: {error}"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn saving_preserves_unicode_and_refuses_existing_files() {
        use std::os::unix::fs::PermissionsExt;
        let temporary = BufferFile::new("").unwrap();
        let path = temporary.directory.join("saved query.sql");
        let sql = "SELECT 'č';\n-- multiline\n";
        save_buffer(&path, sql).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), sql);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(save_buffer(&path, "replacement").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), sql);
        assert!(save_buffer(&temporary.directory.join("missing/query.sql"), sql).is_err());
    }

    #[test]
    fn editor_precedence_is_explicit_and_empty_settings_are_skipped() {
        assert_eq!(
            choose_editor(Some("custom"), Some("visual"), Some("editor")),
            "custom"
        );
        assert_eq!(
            choose_editor(Some(" "), Some("visual"), Some("editor")),
            "visual"
        );
        assert_eq!(choose_editor(None, None, Some("editor")), "editor");
        assert_eq!(choose_editor(None, None, None), "vi");
    }

    #[test]
    fn edited_unicode_multiline_sql_is_returned_without_execution() {
        assert_eq!(
            edit("/bin/sh -c 'exit 0'", "SELECT 1;\n").unwrap(),
            "SELECT 1;\n"
        );
        let command = r#"/bin/sh -c 'printf "SELECT 2;\n-- unicode: \303\251\n" > "$1"' editor"#;
        assert_eq!(
            edit(command, "old SQL").unwrap(),
            "SELECT 2;\n-- unicode: \u{e9}\n"
        );
    }

    #[test]
    fn editor_failures_and_malformed_commands_are_reported() {
        assert!(edit("'", "SQL").is_err());
        assert!(edit("", "SQL").is_err());
        assert!(
            edit("/bin/sh -c 'exit 1'", "SQL")
                .unwrap_err()
                .contains("Editor exited")
        );
        assert!(
            edit("/nonexistent-dzb-editor ; /bin/true", "SQL")
                .unwrap_err()
                .contains("Cannot start editor")
        );
        assert!(
            edit(r#"/bin/sh -c 'printf "\377" > "$1"' editor"#, "SQL")
                .unwrap_err()
                .contains("Cannot read edited SQL")
        );
    }

    #[test]
    fn temporary_buffer_is_private_and_removed_after_use() {
        use std::os::unix::fs::PermissionsExt;
        let buffer = BufferFile::new("private SQL").unwrap();
        let directory = buffer.directory.clone();
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&buffer.file)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(buffer);
        assert!(!directory.exists());
    }
}
