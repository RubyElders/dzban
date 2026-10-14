use std::collections::{BTreeMap, BTreeSet};

use crate::catalog::{Capabilities, CatalogAction, ObjectKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapabilityRequirement {
    None,
    PlainExplain,
    Cancellation,
    Catalog(CatalogAction),
}

impl CapabilityRequirement {
    pub fn supported(&self, capabilities: &Capabilities) -> bool {
        match self {
            Self::None => true,
            Self::PlainExplain => capabilities.plain_explain,
            Self::Cancellation => capabilities.cancellation,
            Self::Catalog(action) => capabilities.catalog.contains(action),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionEffect {
    ReadOnly,
    MayWrite,
    SessionControl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgumentKind {
    Sql,
    QueryId,
    ObjectIdentity,
    Glob,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionArgument {
    pub name: String,
    pub kind: ArgumentKind,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionDescriptor {
    pub id: String,
    pub name: String,
    pub description: String,
    pub arguments: Vec<ActionArgument>,
    pub capability: CapabilityRequirement,
    pub effect: ActionEffect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryError {
    Duplicate(String),
    InvalidDescriptor(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duplicate(id) => write!(formatter, "Action already registered: {id}"),
            Self::InvalidDescriptor(id) => write!(formatter, "Invalid action descriptor: {id}"),
        }
    }
}

impl std::error::Error for RegistryError {}

#[derive(Default)]
pub struct ActionRegistry {
    actions: BTreeMap<String, ActionDescriptor>,
}

impl ActionRegistry {
    pub fn shared_portable() -> &'static Self {
        static REGISTRY: std::sync::OnceLock<ActionRegistry> = std::sync::OnceLock::new();
        REGISTRY.get_or_init(Self::portable)
    }

    pub fn register(&mut self, action: ActionDescriptor) -> Result<(), RegistryError> {
        if self.actions.contains_key(&action.id) {
            return Err(RegistryError::Duplicate(action.id));
        }
        let mut argument_names = BTreeSet::new();
        if !action.id.contains('.')
            || action.id.split('.').any(|part| {
                part.is_empty()
                    || !part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            })
            || action.name.trim().is_empty()
            || action.description.trim().is_empty()
            || action.arguments.iter().any(|argument| {
                argument.name.is_empty()
                    || !argument
                        .name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                    || !argument_names.insert(&argument.name)
            })
        {
            return Err(RegistryError::InvalidDescriptor(action.id));
        }
        self.actions.insert(action.id.clone(), action);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&ActionDescriptor> {
        self.actions.get(id)
    }

    pub fn available<'a>(
        &'a self,
        capabilities: &'a Capabilities,
    ) -> impl Iterator<Item = &'a ActionDescriptor> {
        self.actions
            .values()
            .filter(|action| action.capability.supported(capabilities))
    }

    pub fn portable() -> Self {
        let mut registry = Self::default();
        for (id, name, description, capability, effect, argument) in [
            (
                "query.submit",
                "Run query",
                "Explain and execute the submitted SQL",
                CapabilityRequirement::None,
                ActionEffect::MayWrite,
                ArgumentKind::Sql,
            ),
            (
                "query.plan",
                "Query plan",
                "Request a plain EXPLAIN without executing SQL",
                CapabilityRequirement::PlainExplain,
                ActionEffect::ReadOnly,
                ArgumentKind::Sql,
            ),
            (
                "query.cancel",
                "Cancel query",
                "Cancel the specified active query",
                CapabilityRequirement::Cancellation,
                ActionEffect::SessionControl,
                ArgumentKind::QueryId,
            ),
        ] {
            registry
                .register(ActionDescriptor {
                    id: id.into(),
                    name: name.into(),
                    description: description.into(),
                    arguments: vec![ActionArgument {
                        name: if argument == ArgumentKind::Sql {
                            "sql"
                        } else {
                            "query"
                        }
                        .into(),
                        kind: argument,
                        required: true,
                    }],
                    capability,
                    effect,
                })
                .expect("Portable action IDs are unique and valid");
        }
        for (kind, label) in [
            (ObjectKind::Database, "database"),
            (ObjectKind::Schema, "schema"),
            (ObjectKind::Table, "table"),
            (ObjectKind::View, "view"),
            (ObjectKind::User, "user"),
            (ObjectKind::Role, "role"),
            (ObjectKind::Index, "index"),
            (ObjectKind::Partition, "partition"),
        ] {
            for (verb, capability, argument) in [
                ("list", CatalogAction::List(kind), ArgumentKind::Glob),
                (
                    "describe",
                    CatalogAction::Describe(kind),
                    ArgumentKind::ObjectIdentity,
                ),
            ] {
                registry
                    .register(ActionDescriptor {
                        id: format!("catalog.{verb}.{label}"),
                        name: format!("{verb} {label}"),
                        description: format!("Read {label} metadata from the backend catalog"),
                        arguments: vec![ActionArgument {
                            name: if verb == "list" { "glob" } else { "object" }.into(),
                            kind: argument,
                            required: verb == "describe",
                        }],
                        capability: CapabilityRequirement::Catalog(capability),
                        effect: ActionEffect::ReadOnly,
                    })
                    .expect("Portable action IDs are unique and valid");
            }
        }
        registry
    }
}
