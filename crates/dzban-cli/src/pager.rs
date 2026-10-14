use std::io::Write;
use std::process::{Command, Stdio};

pub async fn run_streamed(
    command: &str,
    result: &dzban_core::execution::StreamedQueryResult,
    format: crate::args::OutputFormat,
) -> Result<(), String> {
    let words = shell_words::split(command).map_err(|error| error.to_string())?;
    let (program, arguments) = words.split_first().ok_or("Pager command is empty")?;
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Cannot start pager {program}: {error}"))?;
    let mut input = child.stdin.take().expect("pager input is piped");
    let written = crate::streamed_output::write_to(result, format, &mut input).await;
    drop(input);
    let status = child.wait().map_err(|error| error.to_string())?;
    if let Err(error) = written
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(error.to_string());
    }
    if !status.success() {
        return Err(format!("Pager exited with {status}"));
    }
    Ok(())
}

pub fn run(command: &str, text: &str) -> Result<(), String> {
    let words = shell_words::split(command).map_err(|error| error.to_string())?;
    let (program, arguments) = words.split_first().ok_or("Pager command is empty")?;
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Cannot start pager {program}: {error}"))?;
    let written = child
        .stdin
        .take()
        .expect("pager input is piped")
        .write_all(text.as_bytes());
    let status = child.wait().map_err(|error| error.to_string())?;
    if let Err(error) = written
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(error.to_string());
    }
    if !status.success() {
        return Err(format!("Pager exited with {status}"));
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn early_exit_is_not_an_output_failure() {
        run("/bin/sh -c 'exit 0'", &"large result\n".repeat(100_000)).unwrap();
    }

    #[test]
    fn malformed_missing_and_failed_pagers_are_reported() {
        assert!(run("'", "result").is_err());
        assert!(
            run("/nonexistent-dzb-test-pager", "result")
                .unwrap_err()
                .contains("Cannot start pager")
        );
        assert!(
            run("/bin/sh -c 'exit 1'", "result")
                .unwrap_err()
                .contains("Pager exited")
        );
    }

    #[test]
    fn shell_metacharacters_are_not_executed() {
        assert!(run("/nonexistent-dzb-test-pager ; /bin/true", "result").is_err());
    }
}
