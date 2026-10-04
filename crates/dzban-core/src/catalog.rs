use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::ResultCell;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ObjectKind {
    Database,
    Schema,
    Table,
    View,
    User,
    Role,
    Index,
    Partition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectIdentity {
    pub database: String,
    pub schema: Option<String>,
    pub kind: ObjectKind,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    UserObjects,
    AllObjects,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    Summary,
    Full,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogRequest {
    Find {
        kind: ObjectKind,
        name: String,
    },
    List {
        kind: ObjectKind,
        database: Option<String>,
        schema: Option<String>,
        glob: Option<String>,
        visibility: Visibility,
    },
    Describe {
        object: ObjectIdentity,
        detail: Detail,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CatalogAction {
    List(ObjectKind),
    Describe(ObjectKind),
}

impl CatalogRequest {
    pub fn action(&self) -> CatalogAction {
        match self {
            Self::List { kind, .. } | Self::Find { kind, .. } => CatalogAction::List(*kind),
            Self::Describe { object, .. } => CatalogAction::Describe(object.kind),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogEntry {
    pub identity: ObjectIdentity,
    pub attributes: BTreeMap<String, ResultCell>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub plain_explain: bool,
    pub cancellation: bool,
    pub catalog: BTreeSet<CatalogAction>,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            plain_explain: true,
            cancellation: true,
            catalog: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogError {
    Busy,
    Unsupported(CatalogAction),
    PermissionDenied {
        message: String,
        code: Option<String>,
    },
    Connection {
        message: String,
    },
    Backend {
        message: String,
        code: Option<String>,
    },
}

impl fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => formatter.write_str("Connection is busy"),
            Self::Unsupported(action) => {
                write!(formatter, "Unsupported catalog action: {action:?}")
            }
            Self::PermissionDenied { message, code } | Self::Backend { message, code } => {
                write!(formatter, "{message}")?;
                if let Some(code) = code {
                    write!(formatter, " ({code})")?;
                }
                Ok(())
            }
            Self::Connection { message } => formatter.write_str(message),
        }
    }
}

impl std::error::Error for CatalogError {}

pub fn resolve_object(
    entries: &[CatalogEntry],
    name: &str,
) -> Result<ObjectIdentity, CatalogError> {
    let candidates = entries
        .iter()
        .filter(|entry| {
            entry.identity.name == name
                || format!(
                    "{}.{}",
                    entry
                        .identity
                        .schema
                        .as_deref()
                        .unwrap_or(&entry.identity.database),
                    entry.identity.name
                ) == name
        })
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [entry] => Ok(entry.identity.clone()),
        [] => Err(CatalogError::Backend {
            message: format!("No matching object: {name}"),
            code: None,
        }),
        _ => Err(CatalogError::Backend {
            message: format!("Ambiguous object: {name}; use a qualified name"),
            code: None,
        }),
    }
}

pub fn matches_glob(pattern: &str, value: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let value = value.chars().collect::<Vec<_>>();
    let (mut p, mut v) = (0, 0);
    let mut wildcard = None;
    while v < value.len() {
        if p < pattern.len() && (pattern[p] == '?' || (pattern[p] != '*' && pattern[p] == value[v]))
        {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            wildcard = Some((p, v));
            p += 1;
        } else if let Some((star, matched)) = wildcard {
            v = matched + 1;
            wildcard = Some((star, v));
            p = star + 1;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::matches_glob;

    #[test]
    fn globs_are_case_sensitive_unicode_patterns_not_sql_or_regex() {
        for (pattern, name, expected) in [
            ("*", "", true),
            ("*", "*abc", true),
            ("ž?*", "žluťoučký", true),
            ("foo*bar", "fooxbarbar", true),
            ("foo*bar", "fooxbarx", false),
            ("public.*", "public.users", true),
            ("users", "Users", false),
            ("u%", "users", false),
            ("a[bc]", "ab", false),
            ("a?", "a", false),
        ] {
            assert_eq!(matches_glob(pattern, name), expected, "{pattern}: {name}");
        }
    }
}
