fn main() {
    glib_build_tools::compile_resources(
        &["resources"],
        "resources/dzban.gresource.xml",
        "dzban.gresource",
    );
}
