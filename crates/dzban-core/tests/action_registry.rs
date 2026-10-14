use dzban_core::action::{
    ActionArgument, ActionDescriptor, ActionEffect, ActionRegistry, ArgumentKind,
    CapabilityRequirement, RegistryError,
};
use dzban_core::catalog::{Capabilities, CatalogAction, ObjectKind};

#[test]
fn shared_portable_descriptors_are_stable_and_match_owned_registries() {
    let shared = ActionRegistry::shared_portable();
    assert!(std::ptr::eq(shared, ActionRegistry::shared_portable()));
    let owned = ActionRegistry::portable();
    for id in ["query.submit", "query.cancel", "query.plan"] {
        assert_eq!(shared.get(id), owned.get(id));
    }
}

fn native_action() -> ActionDescriptor {
    ActionDescriptor {
        id: "postgresql.inspect.settings".into(),
        name: "Inspect PostgreSQL settings".into(),
        description: "Read session settings without modifying them".into(),
        arguments: Vec::new(),
        capability: CapabilityRequirement::None,
        effect: ActionEffect::ReadOnly,
    }
}

#[test]
fn portable_actions_are_filtered_by_exact_adapter_capabilities() {
    let registry = ActionRegistry::portable();
    let mut capabilities = Capabilities {
        plain_explain: false,
        cancellation: false,
        catalog: Default::default(),
    };
    assert_eq!(
        registry
            .available(&capabilities)
            .map(|action| action.id.as_str())
            .collect::<Vec<_>>(),
        ["query.submit"]
    );
    capabilities.plain_explain = true;
    capabilities.cancellation = true;
    capabilities
        .catalog
        .insert(CatalogAction::List(ObjectKind::Table));
    assert_eq!(
        registry
            .available(&capabilities)
            .map(|action| action.id.as_str())
            .collect::<Vec<_>>(),
        [
            "catalog.list.table",
            "query.cancel",
            "query.plan",
            "query.submit"
        ]
    );
    assert!(registry.get("catalog.describe.table").is_some());
    assert!(
        !registry
            .available(&capabilities)
            .any(|action| action.id == "catalog.describe.table")
    );
}

#[test]
fn portable_descriptors_preserve_argument_and_effect_contracts() {
    let registry = ActionRegistry::portable();
    let submit = registry.get("query.submit").unwrap();
    assert_eq!(submit.effect, ActionEffect::MayWrite);
    assert_eq!(
        submit.arguments,
        [ActionArgument {
            name: "sql".into(),
            kind: ArgumentKind::Sql,
            required: true
        }]
    );
    let plan = registry.get("query.plan").unwrap();
    assert_eq!(plan.effect, ActionEffect::ReadOnly);
    assert_eq!(plan.capability, CapabilityRequirement::PlainExplain);
    let cancel = registry.get("query.cancel").unwrap();
    assert_eq!(cancel.effect, ActionEffect::SessionControl);
    assert_eq!(cancel.arguments[0].kind, ArgumentKind::QueryId);
    assert!(cancel.arguments[0].required);
    for kind in [
        "database",
        "schema",
        "table",
        "view",
        "user",
        "role",
        "index",
        "partition",
    ] {
        let list = registry.get(&format!("catalog.list.{kind}")).unwrap();
        let describe = registry.get(&format!("catalog.describe.{kind}")).unwrap();
        assert_eq!(list.effect, ActionEffect::ReadOnly);
        assert_eq!(describe.effect, ActionEffect::ReadOnly);
        assert_eq!(list.arguments[0].kind, ArgumentKind::Glob);
        assert!(!list.arguments[0].required);
        assert_eq!(describe.arguments[0].kind, ArgumentKind::ObjectIdentity);
        assert!(describe.arguments[0].required);
        for action in [list, describe] {
            assert!(!action.name.trim().is_empty());
            assert!(!action.description.trim().is_empty());
        }
    }
}

#[test]
fn native_actions_can_extend_but_not_replace_registered_contracts() {
    let mut registry = ActionRegistry::portable();
    let original = native_action();
    registry.register(original.clone()).unwrap();
    assert_eq!(registry.get(&original.id), Some(&original));
    let mut replacement = original.clone();
    replacement.effect = ActionEffect::MayWrite;
    assert_eq!(
        registry.register(replacement),
        Err(RegistryError::Duplicate(original.id.clone()))
    );
    assert_eq!(registry.get(&original.id), Some(&original));
}

#[test]
fn malformed_descriptors_do_not_enter_the_registry() {
    for id in [
        "settings",
        ".settings",
        "postgresql.",
        "postgresql..settings",
        "postgresql settings",
        "postgresql.\u{e9}",
    ] {
        let mut registry = ActionRegistry::default();
        let mut action = native_action();
        action.id = id.into();
        assert_eq!(
            registry.register(action),
            Err(RegistryError::InvalidDescriptor(id.into()))
        );
        assert!(registry.get(id).is_none());
    }
    for names in [vec![""], vec!["two words"], vec!["setting", "setting"]] {
        let mut registry = ActionRegistry::default();
        let mut action = native_action();
        action.arguments = names
            .into_iter()
            .map(|name| ActionArgument {
                name: name.into(),
                kind: ArgumentKind::Glob,
                required: false,
            })
            .collect();
        assert!(matches!(
            registry.register(action),
            Err(RegistryError::InvalidDescriptor(_))
        ));
        assert!(registry.get("postgresql.inspect.settings").is_none());
    }
}

struct AdapterSession {
    descriptor: ActionDescriptor,
}

#[async_trait::async_trait]
impl dzban_core::adapter::DatabaseSession for AdapterSession {
    fn action_descriptors(&self) -> Vec<ActionDescriptor> {
        vec![self.descriptor.clone()]
    }
    async fn connect(&self) -> Result<(), String> {
        Ok(())
    }
    async fn ping(&self) -> Result<u64, String> {
        Ok(0)
    }
    async fn execute(&self, _: &str) -> Result<dzban_core::ResultSet, String> {
        Ok(Default::default())
    }
    async fn cancel(&self) -> Result<(), String> {
        Ok(())
    }
    async fn version(&self) -> Result<String, String> {
        Ok("fixture".into())
    }
    async fn explain(&self, _: &str) -> Result<dzban_core::QueryPlan, String> {
        Err("No fixture plan".into())
    }
}

#[test]
fn engine_merges_adapter_descriptors_and_rejects_portable_collisions() {
    let descriptor = native_action();
    let engine = dzban_core::execution::QueryEngine::new(std::sync::Arc::new(AdapterSession {
        descriptor: descriptor.clone(),
    }));
    let registry = engine.action_registry().unwrap();
    assert_eq!(registry.get(&descriptor.id), Some(&descriptor));
    assert!(registry.get("query.submit").is_some());
    let mut collision = descriptor;
    collision.id = "query.submit".into();
    let engine = dzban_core::execution::QueryEngine::new(std::sync::Arc::new(AdapterSession {
        descriptor: collision,
    }));
    assert!(
        matches!(engine.action_registry(), Err(RegistryError::Duplicate(id)) if id == "query.submit")
    );
}
