use std::collections::{HashMap, HashSet};
use std::process::Command;

#[test]
fn core_adapters_and_cli_have_no_native_frontend_dependencies() {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--offline", "--format-version", "1"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let packages: HashMap<_, _> = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|package| {
            (
                package["id"].as_str().unwrap(),
                package["name"].as_str().unwrap(),
            )
        })
        .collect();
    let nodes: HashMap<_, _> = metadata["resolve"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| {
            (
                node["id"].as_str().unwrap(),
                node["dependencies"].as_array().unwrap(),
            )
        })
        .collect();
    assert!(packages.values().any(|name| *name == "dzban-gtk"));
    assert!(!packages.values().any(|name| *name == "dzban-ui"));
    for root in ["dzban-core", "dzban-db", "dzban-cli"] {
        let id = packages.iter().find(|(_, name)| **name == root).unwrap().0;
        let mut pending = vec![*id];
        let mut visited = HashSet::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let name = packages[id];
            for prefix in [
                "glib",
                "gio",
                "gobject",
                "gtk",
                "gdk",
                "gsk",
                "pango",
                "cairo",
                "libadwaita",
                "sourceview",
            ] {
                assert!(
                    !name.starts_with(prefix),
                    "{root} depends on native frontend package {name}"
                );
            }
            assert_ne!(name, "dzban-gtk", "{root} depends on GTK frontend");
            if root == "dzban-core" {
                assert_ne!(name, "dzban-db", "core depends on concrete adapters");
            }
            pending.extend(
                nodes[id]
                    .iter()
                    .map(|dependency| dependency.as_str().unwrap()),
            );
        }
    }
}
