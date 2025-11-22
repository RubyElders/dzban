mod ui;
mod config;
mod db;

use gtk4::prelude::*;
use libadwaita as adw;
use adw::Application;

const APP_ID: &str = "com.fuga.dzban";

fn main() {
    // Initialize Adwaita
    adw::init().expect("Failed to initialize Adwaita");

    // Create application
    let app = Application::builder()
        .application_id(APP_ID)
        .build();

    app.connect_activate(ui::build_ui);

    // Run the application
    app.run();
}

