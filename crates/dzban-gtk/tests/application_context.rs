use std::sync::Arc;

use dzban_gtk::database::{DbManager, RuntimeBridge};

#[test]
fn manager_profiles_and_tabs_share_the_injected_core_context() {
    let bridge = RuntimeBridge::default();
    let context = bridge.application_context();
    let manager = DbManager::with_runtime_bridge(bridge);
    assert!(Arc::ptr_eq(&context, &manager.application_context()));
    for name in ["first", "second"] {
        assert!(manager.add_config(
            name.into(),
            "host=localhost user=fixture dbname=fixture".into()
        ));
        let config = manager.get_config(name).unwrap();
        assert!(Arc::ptr_eq(
            &context,
            &config.connection().application_context()
        ));
        config.add_tab(None);
        config.add_tab(None);
        for tab in config.get_all_tabs() {
            assert!(Arc::ptr_eq(
                &context,
                &tab.connection().unwrap().application_context()
            ));
        }
    }
    manager.close_all();
}

#[test]
fn separate_application_managers_do_not_share_ambient_context() {
    let first = DbManager::new();
    let second = DbManager::new();
    assert!(!Arc::ptr_eq(
        &first.application_context(),
        &second.application_context()
    ));
}
