pub(crate) fn format_error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut messages = Vec::new();
    let mut current = Some(error);
    while let Some(error) = current {
        let message = error.to_string();
        if messages.last() != Some(&message) {
            messages.push(message);
        }
        current = error.source();
    }
    messages.join(": ")
}
