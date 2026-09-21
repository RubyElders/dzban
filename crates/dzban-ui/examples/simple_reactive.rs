// Simple GTK4 Reactive Examples
// Run with: cargo run -p dzban-ui --example simple_reactive

use glib::subclass::prelude::*;
use glib::{Object, Properties};
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box, Button, Entry, Label, Orientation};
use std::cell::{Cell, RefCell};

// ============================================================================
// Example 1: Counter with Property Binding
// ============================================================================

mod counter {
    use super::*;

    mod imp {
        use super::*;

        #[derive(Properties, Default)]
        #[properties(wrapper_type = super::Counter)]
        pub struct Counter {
            #[property(get, set)]
            count: Cell<i32>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for Counter {
            const NAME: &'static str = "AppCounter";
            type Type = super::Counter;
            type ParentType = Object;
        }

        #[glib::derived_properties]
        impl ObjectImpl for Counter {}
    }

    glib::wrapper! {
        pub struct Counter(ObjectSubclass<imp::Counter>);
    }

    impl Counter {
        pub fn new() -> Self {
            Object::builder().build()
        }
    }

    impl Default for Counter {
        fn default() -> Self {
            Self::new()
        }
    }
}

fn example_counter(app: &Application) {
    use counter::Counter;

    println!("\n🎯 EXAMPLE 1: REACTIVE COUNTER");
    println!("Watch how the label updates automatically when you click!");

    let model = Counter::new();

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Reactive Counter")
        .default_width(300)
        .default_height(200)
        .build();

    let vbox = Box::new(Orientation::Vertical, 10);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);

    let label = Label::new(Some("Count: 0"));
    label.add_css_class("title-1");

    // 🎯 THIS IS THE MAGIC: Property Binding!
    // When model.count changes → label updates automatically
    model
        .bind_property("count", &label, "label")
        .transform_to(|_, count: i32| Some(format!("Count: {}", count)))
        .sync_create()
        .build();

    let button_box = Box::new(Orientation::Horizontal, 10);
    button_box.set_halign(gtk4::Align::Center);

    let inc_btn = Button::with_label("+1");
    let dec_btn = Button::with_label("-1");
    let reset_btn = Button::with_label("Reset");

    inc_btn.connect_clicked({
        let model = model.clone();
        move |_| {
            let new_count = model.count() + 1;
            model.set_count(new_count); // Label updates automatically! ✨
        }
    });

    dec_btn.connect_clicked({
        let model = model.clone();
        move |_| model.set_count(model.count() - 1)
    });

    reset_btn.connect_clicked({
        let model = model.clone();
        move |_| model.set_count(0)
    });

    button_box.append(&dec_btn);
    button_box.append(&reset_btn);
    button_box.append(&inc_btn);

    vbox.append(&label);
    vbox.append(&button_box);

    window.set_child(Some(&vbox));
    window.present();
}

// ============================================================================
// Example 2: Two-Way Binding
// ============================================================================

mod text_model {
    use super::*;

    mod imp {
        use super::*;

        #[derive(Properties, Default)]
        #[properties(wrapper_type = super::TextModel)]
        pub struct TextModel {
            #[property(get, set)]
            text: RefCell<String>,

            #[property(get, set)]
            length: Cell<i32>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for TextModel {
            const NAME: &'static str = "AppTextModel";
            type Type = super::TextModel;
            type ParentType = Object;
        }

        #[glib::derived_properties]
        impl ObjectImpl for TextModel {
            fn constructed(&self) {
                self.parent_constructed();

                // 🎯 Auto-update length when text changes
                self.obj().connect_text_notify(|obj| {
                    let len = obj.text().len() as i32;
                    obj.set_length(len);
                });
            }
        }
    }

    glib::wrapper! {
        pub struct TextModel(ObjectSubclass<imp::TextModel>);
    }

    impl TextModel {
        pub fn new() -> Self {
            Object::builder().build()
        }
    }

    impl Default for TextModel {
        fn default() -> Self {
            Self::new()
        }
    }
}

fn example_two_way(app: &Application) {
    use text_model::TextModel;

    println!("\n🎯 EXAMPLE 2: TWO-WAY BINDING");
    println!("Type in the entry - watch the model AND labels update!");

    let model = TextModel::new();

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Two-Way Binding")
        .default_width(400)
        .default_height(200)
        .build();

    let vbox = Box::new(Orientation::Vertical, 10);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);

    let entry = Entry::new();
    entry.set_placeholder_text(Some("Type something..."));

    let length_label = Label::new(Some("Length: 0"));
    let echo_label = Label::new(Some("Echo:"));

    // 🎯 TWO-WAY BINDING: Entry ↔ Model
    // Typing in entry → updates model
    // model.set_text() → updates entry
    entry
        .bind_property("text", &model, "text")
        .bidirectional()
        .sync_create()
        .build();

    // 🎯 ONE-WAY: Model → Labels
    model
        .bind_property("length", &length_label, "label")
        .transform_to(|_, len: i32| Some(format!("Length: {}", len)))
        .sync_create()
        .build();

    model
        .bind_property("text", &echo_label, "label")
        .transform_to(|_, text: String| Some(format!("Echo: {}", text)))
        .sync_create()
        .build();

    vbox.append(&entry);
    vbox.append(&length_label);
    vbox.append(&echo_label);

    window.set_child(Some(&vbox));
    window.present();
}

// ============================================================================
// Example 3: Notify Signals (Like Your Connection Status!)
// ============================================================================

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, glib::Enum)]
#[enum_type(name = "DemoConnectionState")]
pub enum DemoState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error,
}

mod connection {
    use super::*;

    mod imp {
        use super::*;

        #[derive(Properties)]
        #[properties(wrapper_type = super::Connection)]
        pub struct Connection {
            #[property(get, set, default)]
            state: Cell<DemoState>,

            #[property(get, set)]
            latency: Cell<i32>, // -1 means no latency
        }

        impl Default for Connection {
            fn default() -> Self {
                Self {
                    state: Cell::new(DemoState::Disconnected),
                    latency: Cell::new(-1),
                }
            }
        }

        #[glib::object_subclass]
        impl ObjectSubclass for Connection {
            const NAME: &'static str = "AppConnection";
            type Type = super::Connection;
            type ParentType = Object;
        }

        #[glib::derived_properties]
        impl ObjectImpl for Connection {}
    }

    glib::wrapper! {
        pub struct Connection(ObjectSubclass<imp::Connection>);
    }

    impl Connection {
        pub fn new() -> Self {
            Object::builder().build()
        }
    }

    impl Default for Connection {
        fn default() -> Self {
            Self::new()
        }
    }
}

fn example_notify(app: &Application) {
    use connection::Connection;
    use gtk4::Image;

    println!("\n🎯 EXAMPLE 3: NOTIFY SIGNALS");
    println!("Click Connect - watch the icon and status change reactively!");

    let model = Connection::new();

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Connection Status")
        .default_width(400)
        .default_height(250)
        .build();

    let vbox = Box::new(Orientation::Vertical, 15);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);

    let status_box = Box::new(Orientation::Horizontal, 10);
    let icon = Image::new();
    icon.set_pixel_size(32);
    let status_label = Label::new(Some("Disconnected"));
    status_label.add_css_class("title-2");

    let latency_label = Label::new(Some("Latency: --"));

    // 🎯 REACT TO STATE CHANGES
    model.connect_state_notify({
        let icon = icon.clone();
        let status_label = status_label.clone();
        move |obj| {
            let (icon_name, text) = match obj.state() {
                DemoState::Disconnected => ("network-offline-symbolic", "Disconnected"),
                DemoState::Connecting => ("network-transmit-receive-symbolic", "Connecting..."),
                DemoState::Connected => ("network-idle-symbolic", "✅ Connected"),
                DemoState::Error => ("network-error-symbolic", "❌ Error"),
            };
            icon.set_icon_name(Some(icon_name));
            status_label.set_text(text);

            println!("State changed to: {:?}", obj.state());
        }
    });

    // 🎯 BINDING FOR LATENCY
    model
        .bind_property("latency", &latency_label, "label")
        .transform_to(|_, latency: i32| {
            if latency < 0 {
                Some("Latency: --".to_string())
            } else {
                Some(format!("Latency: {}ms", latency))
            }
        })
        .sync_create()
        .build();

    status_box.append(&icon);
    status_box.append(&status_label);

    let button_box = Box::new(Orientation::Horizontal, 10);
    let connect_btn = Button::with_label("Connect");
    let disconnect_btn = Button::with_label("Disconnect");

    connect_btn.connect_clicked({
        let model = model.clone();
        move |_| {
            model.set_state(DemoState::Connecting);
            model.set_latency(-1);

            // Simulate async connection
            glib::timeout_add_seconds_local_once(2, {
                let model = model.clone();
                move || {
                    model.set_state(DemoState::Connected);
                    model.set_latency(42);
                }
            });
        }
    });

    disconnect_btn.connect_clicked({
        let model = model.clone();
        move |_| {
            model.set_state(DemoState::Disconnected);
            model.set_latency(-1);
        }
    });

    button_box.append(&connect_btn);
    button_box.append(&disconnect_btn);

    vbox.append(&status_box);
    vbox.append(&latency_label);
    vbox.append(&button_box);

    window.set_child(Some(&vbox));
    window.present();
}

// ============================================================================
// MAIN - Choose which example to run
// ============================================================================

fn main() {
    let example = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "notify".to_string());
    let app = Application::builder()
        .application_id("com.example.simple.reactive")
        .build();

    // 👇 Change this to try different examples:
    // - example_counter (property binding)
    // - example_two_way (two-way binding)
    // - example_notify (notify signals - like your connection status!)

    app.connect_activate(move |app| match example.as_str() {
        "counter" => example_counter(app),
        "two-way" => example_two_way(app),
        _ => example_notify(app),
    });

    app.run();
}
