use gtk4::prelude::*;
use gtk4::{Box, Label, Orientation, ScrolledWindow, ListBox};
use libadwaita as adw;
use adw::ActionRow;
use adw::prelude::*;

pub fn create_query_sidebar(db_name: &str) -> Box {
    let sidebar_box = Box::new(Orientation::Vertical, 0);
    sidebar_box.set_vexpand(true);
    
    // Header
    let header = Box::new(Orientation::Horizontal, 6);
    header.set_margin_start(12);
    header.set_margin_end(12);
    header.set_margin_top(12);
    header.set_margin_bottom(6);
    
    let title = Label::new(Some("Query History"));
    title.add_css_class("title-4");
    title.set_halign(gtk4::Align::Start);
    title.set_hexpand(true);
    header.append(&title);
    
    sidebar_box.append(&header);
    
    // Scrolled window for query list
    let scrolled = ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
    
    // Use ListBox with modern ActionRows
    let listbox = ListBox::new();
    listbox.add_css_class("navigation-sidebar");
    listbox.set_selection_mode(gtk4::SelectionMode::Single);
    
    // Add sample query history
    for (query, time, rows) in get_sample_history(db_name) {
        let row = create_history_row(query, time, rows);
        listbox.append(&row);
    }
    
    scrolled.set_child(Some(&listbox));
    sidebar_box.append(&scrolled);
    
    sidebar_box
}

fn create_history_row(query: &str, time: &str, rows: i32) -> ActionRow {
    let row = ActionRow::new();
    row.set_title(query);
    row.set_subtitle(&format!("⏱️ {} · 📊 {} rows", time, rows));
    row.set_title_lines(2);
    row.set_subtitle_lines(1);
    
    // Add action buttons
    let button_box = Box::new(Orientation::Horizontal, 6);
    
    let open_btn = gtk4::Button::from_icon_name("document-open-symbolic");
    open_btn.set_valign(gtk4::Align::Center);
    open_btn.set_tooltip_text(Some("Open in Editor"));
    open_btn.add_css_class("flat");
    open_btn.add_css_class("circular");
    button_box.append(&open_btn);
    
    let explain_btn = gtk4::Button::from_icon_name("documentinfo-symbolic");
    explain_btn.set_valign(gtk4::Align::Center);
    explain_btn.set_tooltip_text(Some("Explain Query"));
    explain_btn.add_css_class("flat");
    explain_btn.add_css_class("circular");
    button_box.append(&explain_btn);
    
    row.add_suffix(&button_box);
    row.set_activatable(true);
    
    row
}

fn get_sample_history(_db_name: &str) -> Vec<(&'static str, &'static str, i32)> {
    vec![
        ("SELECT * FROM users WHERE active = true", "2m ago", 1523),
        ("SELECT COUNT(*) FROM orders WHERE status = 'completed'", "5m ago", 1),
        ("UPDATE products SET price = price * 1.1 WHERE category = 'electronics'", "15m ago", 342),
        ("SELECT users.name, COUNT(orders.id) FROM users JOIN orders...", "1h ago", 45),
        ("DELETE FROM sessions WHERE expired_at < NOW()", "2h ago", 1829),
        ("SELECT AVG(price) FROM products GROUP BY category", "3h ago", 12),
        ("INSERT INTO logs (message, level, created_at) VALUES...", "Yesterday", 1),
        ("SELECT * FROM analytics WHERE date >= '2024-01-01'", "2 days ago", 98234),
    ]
}
