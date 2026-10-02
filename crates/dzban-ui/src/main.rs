mod ui;

use adw::Application;
use gtk4::prelude::*;
use libadwaita as adw;
use tracing_subscriber::{EnvFilter, fmt};

const APP_ID: &str = "com.fuga.dzban";
pub(crate) const APP_NAME: &str = "Dzban";

fn main() {
    gtk4::glib::set_prgname(Some(APP_NAME));
    gtk4::glib::set_application_name(APP_NAME);

    // Initialize logging
    // Use RUST_LOG environment variable or default to showing debug logs for dzban
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("dzban_db=debug,dzban_ui=debug")),
        )
        .with_target(true)
        .with_thread_ids(false)
        .with_line_number(true)
        .init();

    tracing::info!("Starting {APP_NAME}");

    // Initialize Adwaita
    adw::init().expect("Failed to initialize Adwaita");
    ui::icons::register();

    // Create application
    let app = Application::builder().application_id(APP_ID).build();

    app.connect_activate(ui::build_ui);

    // Run the application
    app.run();
}
