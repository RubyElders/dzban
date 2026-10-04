use std::sync::Once;

use dzban_gtk::database::DatabaseBackend;

pub const POSTGRESQL: &str = "dzban-postgresql";
pub const CLICKHOUSE: &str = "dzban-clickhouse";
pub const QUERY_TIME: &str = "dzban-query-time-symbolic";
pub const RESULT_ROWS: &str = "dzban-result-rows-symbolic";
pub const PING: &str = "dzban-ping-symbolic";

static REGISTER_RESOURCES: Once = Once::new();

pub fn register() {
    REGISTER_RESOURCES.call_once(|| {
        gtk4::gio::resources_register_include!("dzban.gresource")
            .expect("Failed to register application resources");
    });
    let display = gtk4::gdk::Display::default().expect("GTK display is initialized");
    gtk4::IconTheme::for_display(&display).add_resource_path("/com/fuga/dzban/icons");
}

pub fn database_backend(backend: DatabaseBackend) -> &'static str {
    match backend {
        DatabaseBackend::Postgresql => POSTGRESQL,
        DatabaseBackend::Clickhouse => CLICKHOUSE,
    }
}

pub fn database_backend_label(backend: DatabaseBackend) -> &'static str {
    match backend {
        DatabaseBackend::Postgresql => "PostgreSQL database backend",
        DatabaseBackend::Clickhouse => "ClickHouse database backend",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk4::prelude::*;

    #[test]
    fn database_backends_have_distinct_icons_and_labels() {
        assert_eq!(database_backend(DatabaseBackend::Postgresql), POSTGRESQL);
        assert_eq!(database_backend(DatabaseBackend::Clickhouse), CLICKHOUSE);
        assert_ne!(
            database_backend_label(DatabaseBackend::Postgresql),
            database_backend_label(DatabaseBackend::Clickhouse)
        );
    }

    #[gtk4::test]
    #[ignore = "requires a graphical display"]
    fn embedded_footer_icons_are_available_to_gtk() {
        register();
        let display = gtk4::gdk::Display::default().unwrap();
        let theme = gtk4::IconTheme::for_display(&display);

        for name in [POSTGRESQL, CLICKHOUSE, QUERY_TIME, RESULT_ROWS, PING] {
            assert!(theme.has_icon(name), "missing embedded icon {name}");
            let paintable = theme.lookup_icon(
                name,
                &[],
                16,
                1,
                gtk4::TextDirection::Ltr,
                gtk4::IconLookupFlags::PRELOAD,
            );
            assert!(
                paintable.file().is_some(),
                "embedded footer icon did not resolve to a resource"
            );
        }

        let icons = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        for name in [POSTGRESQL, CLICKHOUSE, QUERY_TIME, RESULT_ROWS, PING] {
            let image = gtk4::Image::from_icon_name(name);
            image.set_pixel_size(16);
            icons.append(&image);
        }
        let window = gtk4::Window::new();
        window.set_child(Some(&icons));
        window.present();
        gtk4::test_widget_wait_for_draw(&icons);
        window.close();
    }
}
