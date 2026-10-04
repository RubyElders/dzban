use std::time::{Duration, Instant};

use dzban_core::result_store::ResultLimits;
use dzban_core::result_stream::{ResultHandle, start_results};
use dzban_core::{ResultCell, ResultColumn};
use dzban_gtk::database::RuntimeBridge;
use dzban_gtk::result_model::{ResultListModel, ResultModelPhase};
use gtk4::prelude::*;

fn capture(rows: usize) -> ResultHandle {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (mut producer, reader) = start_results(ResultLimits {
            memory_rows: 1,
            ..Default::default()
        });
        producer
            .schema(vec![ResultColumn {
                name: "value".into(),
                type_name: "text".into(),
            }])
            .await
            .unwrap();
        for index in 0..rows {
            producer
                .row(vec![ResultCell::Text(format!("row {index}"))])
                .await
                .unwrap();
        }
        producer.finish(Ok(())).await.unwrap();
        reader.borrow().result.clone().unwrap()
    })
}

#[gtk4::glib::async_test]
async fn escaped_wide_rows_respect_projection_budget_without_skipping_rows() {
    let result = tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (mut producer, reader) = start_results(ResultLimits {
            memory_rows: 1,
            ..Default::default()
        });
        producer
            .schema(
                (0..1000)
                    .map(|index| ResultColumn {
                        name: format!("column_{index}"),
                        type_name: "text".into(),
                    })
                    .collect(),
            )
            .await
            .unwrap();
        for index in 0..10 {
            let mut row = vec![ResultCell::Text("\u{1f}".repeat(100)); 1000];
            row[0] = ResultCell::Text(index.to_string());
            producer.row(row).await.unwrap();
        }
        producer.finish(Ok(())).await.unwrap();
        reader.borrow().result.clone().unwrap()
    });
    let bridge = RuntimeBridge::new().unwrap();
    let mut position = 0;
    let mut pages = 0;
    while position < 10 {
        let rows = bridge
            .read_result_rows(result.clone(), position)
            .await
            .unwrap()
            .unwrap();
        assert!(!rows.is_empty());
        assert!(rows.iter().map(String::len).sum::<usize>() <= 2 * 1024 * 1024);
        for (offset, row) in rows.iter().enumerate() {
            let values: Vec<String> = serde_json::from_str(row).unwrap();
            assert_eq!(values.len(), 1000);
            assert_eq!(values[0], (position + offset).to_string());
            assert_eq!(values[999], "\u{1f}".repeat(100));
        }
        position += rows.len();
        pages += 1;
    }
    assert!(pages > 1);
    assert!(
        bridge
            .read_result_rows(result, position)
            .await
            .unwrap()
            .unwrap()
            .is_empty()
    );
}

#[track_caller]
fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "GTK result loading timed out");
        gtk4::glib::MainContext::default().iteration(false);
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn contains_label(root: &gtk4::Widget, text: &str) -> bool {
    if root
        .downcast_ref::<gtk4::Label>()
        .is_some_and(|label| label.text() == text)
    {
        return true;
    }
    let mut child = root.first_child();
    while let Some(widget) = child {
        if contains_label(&widget, text) {
            return true;
        }
        child = widget.next_sibling();
    }
    false
}

#[gtk4::test]
#[ignore = "requires a GTK display"]
fn rows_load_lazily_and_cache_remains_bounded_at_both_ends() {
    let model = ResultListModel::new(capture(10_000), RuntimeBridge::new().unwrap(), 1, 10_000);
    assert_eq!(model.n_items(), 10_000);
    assert_eq!(model.cached_rows(), 0);
    assert!(model.item(10_000).is_none());
    assert_eq!(
        model
            .item(9999)
            .unwrap()
            .downcast::<gtk4::StringObject>()
            .unwrap()
            .string(),
        "[\"\"]"
    );
    wait_until(|| model.cached_rows() != 0);
    assert_eq!(
        model
            .item(9999)
            .unwrap()
            .downcast::<gtk4::StringObject>()
            .unwrap()
            .string(),
        "[\"row 9999\"]"
    );
    model.item(0);
    wait_until(|| model.cached_rows() == 200);
    assert_eq!(
        model
            .item(0)
            .unwrap()
            .downcast::<gtk4::StringObject>()
            .unwrap()
            .string(),
        "[\"row 0\"]"
    );
    assert!(model.cached_bytes() <= 2 * 1024 * 1024);
}

#[gtk4::test]
#[ignore = "requires a GTK display"]
fn growing_and_shrinking_model_reports_valid_ranges_without_stale_reads() {
    let model = ResultListModel::new(capture(500), RuntimeBridge::new().unwrap(), 1, 1);
    let changes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = changes.clone();
    model.connect_items_changed(move |_, start, removed, added| {
        observed.borrow_mut().push((start, removed, added))
    });
    model.set_rows(500);
    assert_eq!(changes.borrow()[0], (1, 0, 499));
    model.item(499);
    wait_until(|| model.phase() == ResultModelPhase::Reading);
    model.set_rows(0);
    wait_until(|| model.phase() == ResultModelPhase::Idle);
    assert!(model.item(0).is_none());
    assert_eq!(model.n_items(), 0);
    assert_eq!(model.cached_rows(), 0);
    assert!(
        changes
            .borrow()
            .iter()
            .all(|(start, removed, added)| *removed != *added || *start == 0)
    );
}

#[gtk4::test]
#[ignore = "requires a GTK display"]
fn native_column_view_pages_large_results_without_resetting_scroll() {
    let model = ResultListModel::new(capture(10_000), RuntimeBridge::new().unwrap(), 1, 10_000);
    let selection = gtk4::SingleSelection::new(Some(model.clone()));
    let view = gtk4::ColumnView::new(Some(selection));
    let factory = gtk4::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
        let label = gtk4::Label::new(None);
        label.set_xalign(0.0);
        label.set_single_line_mode(true);
        label.set_lines(1);
        label.set_margin_top(4);
        label.set_margin_bottom(4);
        item.set_child(Some(&label));
    });
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
        let row = item
            .item()
            .unwrap()
            .downcast::<gtk4::StringObject>()
            .unwrap();
        let values: Vec<String> = serde_json::from_str(&row.string()).unwrap();
        item.child()
            .unwrap()
            .downcast::<gtk4::Label>()
            .unwrap()
            .set_text(&values[0]);
    });
    let column = gtk4::ColumnViewColumn::new(Some("value"), Some(factory));
    column.set_fixed_width(200);
    view.append_column(&column);
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_child(Some(&view));
    let window = gtk4::Window::builder()
        .title("Dzban paged results test")
        .default_width(600)
        .default_height(400)
        .child(&scroll)
        .build();
    window.present();
    wait_until(|| model.cached_rows() != 0 && scroll.vadjustment().upper() > 1000.0);
    wait_until(|| contains_label(window.upcast_ref(), "row 0"));
    gtk4::test_widget_wait_for_draw(&window);
    let adjustment = scroll.vadjustment();
    let target = adjustment.upper() - adjustment.page_size();
    adjustment.set_value(target);
    view.scroll_to(9999, None, gtk4::ListScrollFlags::NONE, None);
    wait_until(|| {
        model
            .item(9999)
            .unwrap()
            .downcast::<gtk4::StringObject>()
            .unwrap()
            .string()
            .contains("row 9999")
    });
    wait_until(|| contains_label(window.upcast_ref(), "row 9999"));
    assert!((adjustment.value() - target).abs() < 1.0);
    assert!(model.cached_rows() <= 200);
    assert!(model.cached_bytes() <= 2 * 1024 * 1024);
    if let Ok(path) = std::env::var("DZBAN_MODEL_SCREENSHOT") {
        gtk4::test_widget_wait_for_draw(&window);
        assert!(
            std::process::Command::new("import")
                .args(["-window", "root", &path])
                .status()
                .unwrap()
                .success()
        );
    }
    window.close();
}
