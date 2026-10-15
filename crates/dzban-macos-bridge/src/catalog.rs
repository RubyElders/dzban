use dzban_core::action::{ActionDescriptor, ActionEffect, ArgumentKind};
use dzban_core::catalog::{
    CatalogEntry, CatalogRequest, Detail, ObjectIdentity, ObjectKind, Visibility,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Database,
    Schema,
    Table,
    View,
    User,
    Role,
    Index,
    Partition,
}

impl Kind {
    pub fn core(self) -> ObjectKind {
        match self {
            Self::Database => ObjectKind::Database,
            Self::Schema => ObjectKind::Schema,
            Self::Table => ObjectKind::Table,
            Self::View => ObjectKind::View,
            Self::User => ObjectKind::User,
            Self::Role => ObjectKind::Role,
            Self::Index => ObjectKind::Index,
            Self::Partition => ObjectKind::Partition,
        }
    }
}

pub fn kind_name(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Database => "database",
        ObjectKind::Schema => "schema",
        ObjectKind::Table => "table",
        ObjectKind::View => "view",
        ObjectKind::User => "user",
        ObjectKind::Role => "role",
        ObjectKind::Index => "index",
        ObjectKind::Partition => "partition",
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    database: String,
    schema: Option<String>,
    kind: Kind,
    name: String,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    List {
        kind: Kind,
        database: Option<String>,
        schema: Option<String>,
        glob: Option<String>,
        #[serde(default)]
        include_system: bool,
    },
    Describe {
        object: Identity,
        #[serde(default)]
        full: bool,
    },
}

impl Request {
    pub fn core(self) -> CatalogRequest {
        match self {
            Self::List {
                kind,
                database,
                schema,
                glob,
                include_system,
            } => CatalogRequest::List {
                kind: kind.core(),
                database,
                schema,
                glob,
                visibility: if include_system {
                    Visibility::AllObjects
                } else {
                    Visibility::UserObjects
                },
            },
            Self::Describe { object, full } => CatalogRequest::Describe {
                object: ObjectIdentity {
                    database: object.database,
                    schema: object.schema,
                    kind: object.kind.core(),
                    name: object.name,
                },
                detail: if full { Detail::Full } else { Detail::Summary },
            },
        }
    }
}

pub fn entry(entry: &CatalogEntry) -> Value {
    json!({"identity": {"database":entry.identity.database,"schema":entry.identity.schema,"kind":kind_name(entry.identity.kind),"name":entry.identity.name}, "attributes":entry.attributes})
}

pub fn action(action: &ActionDescriptor) -> Value {
    let effect = match action.effect {
        ActionEffect::ReadOnly => "read_only",
        ActionEffect::MayWrite => "may_write",
        ActionEffect::SessionControl => "session_control",
    };
    let arguments: Vec<_> = action
        .arguments
        .iter()
        .map(|argument| {
            let kind = match argument.kind {
                ArgumentKind::Sql => "sql",
                ArgumentKind::QueryId => "query_id",
                ArgumentKind::ObjectIdentity => "object_identity",
                ArgumentKind::Glob => "glob",
            };
            json!({"name":argument.name,"kind":kind,"required":argument.required})
        })
        .collect();
    json!({"id":action.id,"name":action.name,"description":action.description,"effect":effect,"arguments":arguments})
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Plan,
    Row,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeekEvent {
    Show,
    Open,
    Pin,
    Close,
    Release,
    Reset,
}

pub fn error(error: dzban_core::catalog::CatalogError) -> Value {
    use dzban_core::catalog::CatalogError;
    let (kind, code, action) = match &error {
        CatalogError::Busy => ("busy", None, None),
        CatalogError::Unsupported(action) => {
            ("catalog_unsupported", None, Some(format!("{action:?}")))
        }
        CatalogError::PermissionDenied { code, .. } => ("catalog_permission", code.clone(), None),
        CatalogError::Connection { .. } => ("catalog_connection", None, None),
        CatalogError::Backend { code, .. } => ("catalog_backend", code.clone(), None),
    };
    json!({"failure":{"kind":kind,"message":error.to_string(),"diagnostic":{"code":code,"action":action}}})
}
