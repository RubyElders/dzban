use std::io::Write;
use std::process::{Command, Stdio};

fn check(tree: &str) -> std::process::Output {
    let mut child = Command::new("bash")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/test.sh"
        ))
        .args(["portable", "--check-tree"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(tree.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn shared_dependency_gate_accepts_transport_and_platform_trust_libraries() {
    assert!(check("dzban-core v0.1.0\ntokio v1.0.0\nrustls v0.23.0\nsecurity-framework v3.0.0\ncore-foundation v0.10.0\ndzban-db v0.1.0\n").status.success());
}

#[test]
fn shared_dependency_gate_rejects_native_and_terminal_frontends_in_any_tree_position() {
    for package in [
        "dzban-gtk",
        "dzban-cli",
        "glib",
        "glib-sys",
        "gtk4",
        "gtk4-sys",
        "gdk4-x11",
        "libadwaita",
        "cairo-rs",
        "pango",
        "objc2-app-kit",
        "ratatui",
        "reedline",
        "clap",
    ] {
        let output = check(&format!("dzban-core v0.1.0\n{package} v1.0.0 (*)\n"));
        assert!(!output.status.success(), "accepted {package}");
        assert!(String::from_utf8(output.stderr).unwrap().contains(package));
    }
}
