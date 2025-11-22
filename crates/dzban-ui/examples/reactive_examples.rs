// GTK4 Reactive Examples - Runnable Code
// 
// These are complete, working examples demonstrating reactive patterns in GTK4.
// To run: cargo run --example <name>

//! Example 1: Simple Counter with Property Binding
//! 
//! Run with: cargo add gtk4 glib --example reactive_examples
//! Then: cargo run --example counter

use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Button, Label, Box, Orientation};
use glib::{Object, Properties};
use std::cell::Cell;

// ============================================================================
// EXAMPLE 1: SIMPLE COUNTER
// ============================================================================

mod counter_model {
    use super::*;
    
    mod imp {
        use super::*;
        
        #[derive(Properties, Default)]
        #[properties(wrapper_type = super::Counter)]
        pub struct Counter {
            #[property(get, set, notify)]
            count: Cell<i32>,
        }
        
        #[glib::object_subclass]
        impl ObjectSubclass for Counter {
            const NAME: &'static str = "Counter";
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
        
        pub fn increment(&self) {
            self.set_count(self.count() + 1);
        }
        
        pub fn decrement(&self) {
            self.set_count(self.count() - 1);
        }
        
        pub fn reset(&self) {
            self.set_count(0);
        }
    }
}

fn build_counter_ui(app: &Application) {
    use counter_model::Counter;
    
    // MODEL
    let model = Counter::new();
    
    // VIEW
    let window = ApplicationWindow::builder()
        .application(app)
        .title("Counter Example")
        .default_width(300)
        .default_height(200)
        .build();
    
    let vbox = Box::new(Orientation::Vertical, 10);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);
    
    // Label with reactive binding
    let label = Label::new(None);
    label.add_css_class("title-1");
    
    // 🎯 REACTIVE BINDING: count property → label text
    model.bind_property("count", &label, "label")
        .transform_to(|_, count: i32| {
            Some(format!("Count: {}", count))
        })
        .sync_create()  // Sync immediately
        .build();
    
    // Buttons
    let button_box = Box::new(Orientation::Horizontal, 10);
    button_box.set_halign(gtk4::Align::Center);
    
    let inc_button = Button::with_label("+1");
    let dec_button = Button::with_label("-1");
    let reset_button = Button::with_label("Reset");
    
    // 🎯 EVENT HANDLERS: Update model → UI reacts automatically
    inc_button.connect_clicked({
        let model = model.clone();
        move |_| model.increment()
    });
    
    dec_button.connect_clicked({
        let model = model.clone();
        move |_| model.decrement()
    });
    
    reset_button.connect_clicked({
        let model = model.clone();
        move |_| model.reset()
    });
    
    button_box.append(&dec_button);
    button_box.append(&reset_button);
    button_box.append(&inc_button);
    
    vbox.append(&label);
    vbox.append(&button_box);
    window.set_child(Some(&vbox));
    window.present();
}

// ============================================================================
// EXAMPLE 2: TWO-WAY BINDING
// ============================================================================

mod text_model {
    use super::*;
    use std::cell::RefCell;
    
    mod imp {
        use super::*;
        
        #[derive(Properties, Default)]
        #[properties(wrapper_type = super::TextModel)]
        pub struct TextModel {
            #[property(get, set, notify)]
            text: RefCell<String>,
            
            #[property(get, set, notify)]
            char_count: Cell<i32>,
        }
        
        #[glib::object_subclass]
        impl ObjectSubclass for TextModel {
            const NAME: &'static str = "TextModel";
            type Type = super::TextModel;
            type ParentType = Object;
        }
        
        #[glib::derived_properties]
        impl ObjectImpl for TextModel {
            fn constructed(&self) {
                self.parent_constructed();
                
                // 🎯 AUTO-UPDATE: When text changes → update char_count
                self.obj().connect_text_notify(|obj| {
                    let count = obj.text().chars().count() as i32;
                    obj.set_char_count(count);
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
}

fn build_text_ui(app: &Application) {
    use text_model::TextModel;
    use gtk4::Entry;
    
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
    
    let count_label = Label::new(None);
    let echo_label = Label::new(None);
    echo_label.add_css_class("dim-label");
    
    // 🎯 TWO-WAY BINDING: Entry ↔ Model
    entry.bind_property("text", &model, "text")
        .bidirectional()
        .sync_create()
        .build();
    
    // 🎯 ONE-WAY BINDING: Model → Labels
    model.bind_property("char-count", &count_label, "label")
        .transform_to(|_, count: i32| {
            Some(format!("Characters: {}", count))
        })
        .sync_create()
        .build();
    
    model.bind_property("text", &echo_label, "label")
        .transform_to(|_, text: String| {
            Some(format!("You typed: {}", text))
        })
        .sync_create()
        .build();
    
    vbox.append(&entry);
    vbox.append(&count_label);
    vbox.append(&echo_label);
    window.set_child(Some(&vbox));
    window.present();
}

// ============================================================================
// EXAMPLE 3: CONNECTION STATUS (Like Your Project!)
// ============================================================================

mod connection_model {
    use super::*;
    use std::cell::RefCell;
    
    #[derive(Debug, Clone, Copy, PartialEq, Eq, glib::Enum)]
    #[enum_type(name = "ConnectionState")]
    pub enum ConnectionState {
        Disconnected,
        Connecting,
        Connected,
        Error,
    }
    
    impl Default for ConnectionState {
        fn default() -> Self {
            Self::Disconnected
        }
    }
    
    mod imp {
        use super::*;
        
        #[derive(Properties, Default)]
        #[properties(wrapper_type = super::ConnectionModel)]
        pub struct ConnectionModel {
            #[property(get, set, notify)]
            state: Cell<ConnectionState>,
            
            #[property(get, set, nullable, notify)]
            server_name: RefCell<Option<String>>,
            
            #[property(get, set, nullable, notify)]
            latency: Cell<Option<i32>>,
        }
        
        #[glib::object_subclass]
        impl ObjectSubclass for ConnectionModel {
            const NAME: &'static str = "ConnectionModel";
            type Type = super::ConnectionModel;
            type ParentType = Object;
        }
        
        #[glib::derived_properties]
        impl ObjectImpl for ConnectionModel {}
    }
    
    glib::wrapper! {
        pub struct ConnectionModel(ObjectSubclass<imp::ConnectionModel>);
    }
    
    impl ConnectionModel {
        pub fn new() -> Self {
            Object::builder().build()
        }
        
        pub fn simulate_connect(&self) {
            self.set_state(ConnectionState::Connecting);
            self.set_server_name(Some("postgres.example.com".to_string()));
            
            // Simulate async connection
            glib::timeout_add_seconds_local_once(2, {
                let model = self.clone();
                move || {
                    model.set_state(ConnectionState::Connected);
                    model.set_latency(Some(42));
                }
            });
        }
        
        pub fn simulate_disconnect(&self) {
            self.set_state(ConnectionState::Disconnected);
            self.set_server_name(None);
            self.set_latency(None);
        }
    }
}

fn build_connection_ui(app: &Application) {
    use connection_model::{ConnectionModel, ConnectionState};
    use gtk4::Image;
    
    let model = ConnectionModel::new();
    
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
    
    // Status display
    let status_box = Box::new(Orientation::Horizontal, 10);
    let status_icon = Image::new();
    status_icon.set_pixel_size(24);
    
    let status_label = Label::new(None);
    status_label.add_css_class("title-3");
    
    let server_label = Label::new(None);
    server_label.add_css_class("dim-label");
    
    let latency_label = Label::new(None);
    
    // 🎯 REACTIVE BINDINGS
    
    // Server name binding
    model.bind_property("server-name", &server_label, "label")
        .transform_to(|_, name: Option<String>| {
            name.or(Some("Not connected".to_string()))
        })
        .sync_create()
        .build();
    
    // Latency binding with transformation
    model.bind_property("latency", &latency_label, "label")
        .transform_to(|_, latency: Option<i32>| {
            latency.map(|ms| format!("Latency: {}ms", ms))
                .or(Some("--".to_string()))
        })
        .sync_create()
        .build();
    
    // 🎯 STATE-BASED UI UPDATES
    model.connect_state_notify({
        let status_icon = status_icon.clone();
        let status_label = status_label.clone();
        move |obj| {
            let (icon, text, css) = match obj.state() {
                ConnectionState::Disconnected => {
                    ("network-offline-symbolic", "Disconnected", "error")
                }
                ConnectionState::Connecting => {
                    ("network-transmit-receive-symbolic", "Connecting...", "warning")
                }
                ConnectionState::Connected => {
                    ("network-idle-symbolic", "Connected", "success")
                }
                ConnectionState::Error => {
                    ("network-error-symbolic", "Error", "error")
                }
            };
            
            status_icon.set_icon_name(Some(icon));
            status_label.set_text(text);
            
            // Update CSS class
            status_label.remove_css_class("error");
            status_label.remove_css_class("warning");
            status_label.remove_css_class("success");
            status_label.add_css_class(css);
        }
    });
    
    status_box.append(&status_icon);
    status_box.append(&status_label);
    
    // Buttons
    let button_box = Box::new(Orientation::Horizontal, 10);
    let connect_btn = Button::with_label("Connect");
    let disconnect_btn = Button::with_label("Disconnect");
    
    connect_btn.connect_clicked({
        let model = model.clone();
        move |_| model.simulate_connect()
    });
    
    disconnect_btn.connect_clicked({
        let model = model.clone();
        move |_| model.simulate_disconnect()
    });
    
    button_box.append(&connect_btn);
    button_box.append(&disconnect_btn);
    
    vbox.append(&status_box);
    vbox.append(&server_label);
    vbox.append(&latency_label);
    vbox.append(&button_box);
    
    window.set_child(Some(&vbox));
    window.present();
}

// ============================================================================
// EXAMPLE 4: LIST OF ITEMS WITH SIGNALS
// ============================================================================

mod list_model {
    use super::*;
    use std::cell::RefCell;
    use std::sync::OnceLock;
    use glib::subclass::Signal;
    
    mod imp {
        use super::*;
        
        #[derive(Default)]
        pub struct ItemList {
            items: RefCell<Vec<String>>,
        }
        
        #[glib::object_subclass]
        impl ObjectSubclass for ItemList {
            const NAME: &'static str = "ItemList";
            type Type = super::ItemList;
            type ParentType = Object;
        }
        
        impl ObjectImpl for ItemList {
            fn signals() -> &'static [Signal] {
                static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
                SIGNALS.get_or_init(|| {
                    vec![
                        Signal::builder("item-added")
                            .param_types([String::static_type()])
                            .build(),
                        Signal::builder("item-removed")
                            .param_types([usize::static_type()])
                            .build(),
                    ]
                })
            }
        }
    }
    
    glib::wrapper! {
        pub struct ItemList(ObjectSubclass<imp::ItemList>);
    }
    
    impl ItemList {
        pub fn new() -> Self {
            Object::builder().build()
        }
        
        pub fn add_item(&self, item: String) {
            self.imp().items.borrow_mut().push(item.clone());
            // 🎯 EMIT SIGNAL
            self.emit_by_name::<()>("item-added", &[&item]);
        }
        
        pub fn remove_item(&self, index: usize) {
            let mut items = self.imp().items.borrow_mut();
            if index < items.len() {
                items.remove(index);
                // 🎯 EMIT SIGNAL
                self.emit_by_name::<()>("item-removed", &[&index]);
            }
        }
        
        pub fn items(&self) -> Vec<String> {
            self.imp().items.borrow().clone()
        }
    }
}

fn build_list_ui(app: &Application) {
    use list_model::ItemList;
    use gtk4::{ListBox, Entry, ListBoxRow};
    
    let model = ItemList::new();
    
    let window = ApplicationWindow::builder()
        .application(app)
        .title("Signal-Based List")
        .default_width(400)
        .default_height(400)
        .build();
    
    let vbox = Box::new(Orientation::Vertical, 10);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);
    
    // Input
    let input_box = Box::new(Orientation::Horizontal, 10);
    let entry = Entry::new();
    entry.set_placeholder_text(Some("Add item..."));
    let add_button = Button::with_label("Add");
    
    input_box.append(&entry);
    input_box.append(&add_button);
    
    // List
    let listbox = ListBox::new();
    listbox.set_vexpand(true);
    
    // 🎯 LISTEN TO SIGNALS
    model.connect_local("item-added", false, {
        let listbox = listbox.clone();
        move |args| {
            let item = args[1].get::<String>().unwrap();
            let row = ListBoxRow::new();
            let label = Label::new(Some(&item));
            label.set_xalign(0.0);
            row.set_child(Some(&label));
            listbox.append(&row);
            None
        }
    });
    
    model.connect_local("item-removed", false, {
        let listbox = listbox.clone();
        move |args| {
            let index = args[1].get::<usize>().unwrap();
            if let Some(row) = listbox.row_at_index(index as i32) {
                listbox.remove(&row);
            }
            None
        }
    });
    
    // Add button handler
    add_button.connect_clicked({
        let entry = entry.clone();
        let model = model.clone();
        move |_| {
            let text = entry.text();
            if !text.is_empty() {
                model.add_item(text.to_string());
                entry.set_text("");
            }
        }
    });
    
    vbox.append(&input_box);
    vbox.append(&listbox);
    
    window.set_child(Some(&vbox));
    window.present();
}

// ============================================================================
// MAIN - Choose which example to run
// ============================================================================

fn main() {
    let app = Application::builder()
        .application_id("com.example.reactive")
        .build();
    
    // Change this to run different examples:
    // - build_counter_ui
    // - build_text_ui
    // - build_connection_ui
    // - build_list_ui
    
    app.connect_activate(build_connection_ui);  // 👈 Change this!
    
    app.run();
}
