// Minimal GTK4 Reactive Example
// Run with: cargo run -p dzban-ui --example minimal_reactive

use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Button, Label, Box, Orientation};
use glib::{Object, Properties};
use glib::subclass::prelude::*;
use std::cell::Cell;

// Simple counter model with GObject properties
mod counter {
    use super::*;
    
    mod imp {
        use super::*;
        
        #[derive(Properties, Default)]
        #[properties(wrapper_type = super::Counter)]
        pub struct Counter {
            #[property(get, set)]
            value: Cell<i32>,
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
    }

    impl Default for Counter {
        fn default() -> Self {
            Self::new()
        }
    }
}

fn main() {
    let app = Application::builder()
        .application_id("com.example.minimal.reactive")
        .build();
    
    app.connect_activate(|app| {
        println!("\n🎯 GTK4 REACTIVE EXAMPLE");
        println!("===============================");
        println!("Click the buttons - notice the label updates automatically!");
        println!("This is REACTIVE PROGRAMMING with GObject properties.\n");
        
        // MODEL (the data)
        let model = counter::Counter::new();
        
        // WINDOW
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
        
        // LABEL (the view)
        let label = Label::new(Some("Count: 0"));
        label.add_css_class("title-1");
        
        // 🎯 THE MAGIC: Property Binding
        // This creates a reactive link: model.value → label.text
        // When model.value changes, label updates AUTOMATICALLY!
        model.bind_property("value", &label, "label")
            .transform_to(|_, value: i32| {
                Some(format!("Count: {}", value))
            })
            .sync_create()  // Sync immediately on creation
            .build();
        
        // BUTTONS
        let button_box = Box::new(Orientation::Horizontal, 10);
        button_box.set_halign(gtk4::Align::Center);
        
        let inc_btn = Button::with_label("+1");
        let dec_btn = Button::with_label("-1");
        let reset_btn = Button::with_label("Reset");
        
        // 🎯 EVENT HANDLERS
        // We only update the MODEL - the VIEW updates automatically!
        inc_btn.connect_clicked({
            let model = model.clone();
            move |_| {
                let new_value = model.value() + 1;
                model.set_value(new_value);
                // ✨ Label updates automatically - no manual label.set_text()!
                println!("Incremented to {}", new_value);
            }
        });
        
        dec_btn.connect_clicked({
            let model = model.clone();
            move |_| {
                let new_value = model.value() - 1;
                model.set_value(new_value);
                println!("Decremented to {}", new_value);
            }
        });
        
        reset_btn.connect_clicked({
            let model = model.clone();
            move |_| {
                model.set_value(0);
                println!("Reset to 0");
            }
        });
        
        // Also listen to value changes
        model.connect_value_notify(|model| {
            println!("🔔 Property changed! New value: {}", model.value());
        });
        
        button_box.append(&dec_btn);
        button_box.append(&reset_btn);
        button_box.append(&inc_btn);
        
        vbox.append(&label);
        vbox.append(&button_box);
        
        window.set_child(Some(&vbox));
        window.present();
        
        println!("Window opened! Try clicking the buttons.");
    });
    
    app.run();
}
