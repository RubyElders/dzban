use dzban_core::catalog::{
    Capabilities, CatalogAction, CatalogEntry, CatalogError, CatalogRequest, Detail,
    ObjectIdentity, ObjectKind, Visibility, matches_glob,
};
use dzban_core::{DatabaseBackend, ResultCell, ResultSet};

pub(crate) fn capabilities(backend: DatabaseBackend) -> Capabilities {
    let mut catalog = [
        ObjectKind::Database,
        ObjectKind::Table,
        ObjectKind::View,
        ObjectKind::User,
        ObjectKind::Role,
        ObjectKind::Index,
    ]
    .into_iter()
    .map(CatalogAction::List)
    .collect::<std::collections::BTreeSet<_>>();
    if backend == DatabaseBackend::Postgresql {
        catalog.insert(CatalogAction::List(ObjectKind::Schema));
    }
    catalog.extend(
        [ObjectKind::Table, ObjectKind::View]
            .into_iter()
            .map(CatalogAction::Describe),
    );
    Capabilities {
        catalog,
        ..Default::default()
    }
}

pub(crate) fn query(
    backend: DatabaseBackend,
    database: &str,
    request: &CatalogRequest,
) -> Result<CatalogQuery, CatalogError> {
    let base = base_query(backend, database, request)?;
    let mut query = CatalogQuery {
        sql: String::new(),
        parameters: Vec::new(),
    };
    let mut filters = Vec::new();
    let (kind, requested_database, schema, glob) = match request {
        CatalogRequest::Find { kind, .. } => (*kind, None, None, None),
        CatalogRequest::List {
            kind,
            database,
            schema,
            glob,
            ..
        } => (
            *kind,
            database.as_deref(),
            schema.as_deref(),
            glob.as_deref(),
        ),
        CatalogRequest::Describe { object, .. } => (
            object.kind,
            Some(object.database.as_str()),
            object.schema.as_deref(),
            None,
        ),
    };
    let relational = matches!(
        kind,
        ObjectKind::Table | ObjectKind::View | ObjectKind::Index
    );
    if kind == ObjectKind::Database {
        if let Some(database) = requested_database {
            filters.push(format!("name = {}", query.bind(backend, database)));
        }
    } else if backend == DatabaseBackend::Clickhouse && relational {
        filters.push(format!(
            "namespace = {}",
            query.bind(backend, requested_database.unwrap_or(database))
        ));
    } else if let Some(selected) = requested_database {
        filters.push(format!(
            "{} = {}",
            query.bind(backend, database),
            query.bind(backend, selected)
        ));
    }
    if let Some(schema) = schema {
        if backend != DatabaseBackend::Postgresql || !relational {
            return Err(CatalogError::Unsupported(request.action()));
        }
        filters.push(format!("namespace = {}", query.bind(backend, schema)));
    }
    if let CatalogRequest::Describe { object, .. } = request {
        filters.push(format!("name = {}", query.bind(backend, &object.name)));
        if backend == DatabaseBackend::Postgresql && schema.is_none() {
            filters.push(format!("namespace = {}", query.bind(backend, "")));
        }
    }
    if let CatalogRequest::Find { name, .. } = request {
        let parameter = query.bind(backend, name);
        let prefix = if kind == ObjectKind::Database {
            "name".to_owned()
        } else if relational {
            "namespace".into()
        } else {
            query.bind(backend, database)
        };
        filters.push(match backend {
            DatabaseBackend::Postgresql => {
                format!("(name = {parameter} OR ({prefix} || '.' || name) = {parameter})")
            }
            DatabaseBackend::Clickhouse => {
                format!("(name = {parameter} OR concat({prefix}, '.', name) = {parameter})")
            }
        });
    }
    if let Some(glob) = glob {
        let parameter = query.bind(backend, &glob_regex(glob));
        let prefix = if kind == ObjectKind::Database {
            "name".to_owned()
        } else if relational {
            "namespace".into()
        } else {
            query.bind(backend, database)
        };
        filters.push(match backend {
            DatabaseBackend::Postgresql => {
                format!("(name ~ {parameter} OR ({prefix} || '.' || name) ~ {parameter})")
            }
            DatabaseBackend::Clickhouse => format!(
                "(match(name, {parameter}) OR match(concat({prefix}, '.', name), {parameter}))"
            ),
        });
    }
    let order = if matches!(request, CatalogRequest::Describe { .. }) {
        "namespace, name, ordinal_position"
    } else if backend == DatabaseBackend::Clickhouse && kind == ObjectKind::Index {
        "namespace, table_name, name"
    } else {
        "namespace, name"
    };
    query.sql = format!(
        "SELECT * FROM ({base}) AS catalog {} ORDER BY {order}",
        if filters.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", filters.join(" AND "))
        }
    );
    Ok(query)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CatalogQuery {
    pub sql: String,
    pub parameters: Vec<String>,
}

impl CatalogQuery {
    fn bind(&mut self, backend: DatabaseBackend, value: &str) -> String {
        self.parameters.push(value.into());
        match backend {
            DatabaseBackend::Postgresql => format!("${}::text", self.parameters.len()),
            DatabaseBackend::Clickhouse => format!("{{p{}:String}}", self.parameters.len()),
        }
    }

    pub fn clickhouse_parameters(&self) -> Vec<(String, String)> {
        self.parameters
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let mut escaped = String::new();
                for character in value.chars() {
                    match character {
                        '\\' => escaped.push_str("\\\\"),
                        '\n' => escaped.push_str("\\n"),
                        '\r' => escaped.push_str("\\r"),
                        '\t' => escaped.push_str("\\t"),
                        '\0' => escaped.push_str("\\0"),
                        character => escaped.push(character),
                    }
                }
                (format!("param_p{}", index + 1), escaped)
            })
            .collect()
    }
}

fn glob_regex(glob: &str) -> String {
    let mut regex = "(?s)^".to_owned();
    for character in glob.chars() {
        match character {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            '\\' | '.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' => {
                regex.push('\\');
                regex.push(character);
            }
            character => regex.push(character),
        }
    }
    regex.push('$');
    regex
}

fn base_query(
    backend: DatabaseBackend,
    database: &str,
    request: &CatalogRequest,
) -> Result<String, CatalogError> {
    if let CatalogRequest::Find { kind, .. } = request {
        return base_query(
            backend,
            database,
            &CatalogRequest::List {
                kind: *kind,
                database: None,
                schema: None,
                glob: None,
                visibility: Visibility::AllObjects,
            },
        );
    }
    if let CatalogRequest::Describe { object, detail } = request {
        if !matches!(object.kind, ObjectKind::Table | ObjectKind::View)
            || (backend == DatabaseBackend::Postgresql && object.database != database)
            || (backend == DatabaseBackend::Clickhouse && object.schema.is_some())
        {
            return Err(CatalogError::Unsupported(request.action()));
        }
        return Ok(match backend {
            DatabaseBackend::Postgresql => format!(
                "SELECT n.nspname AS namespace, c.relname AS name, a.attname AS column_name, a.attnum AS ordinal_position, pg_catalog.format_type(a.atttypid,a.atttypmod) AS data_type, NOT a.attnotnull AS nullable, pg_catalog.pg_get_expr(d.adbin,d.adrelid) AS default_expression {} FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid = a.attrelid JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum WHERE a.attnum > 0 AND NOT a.attisdropped AND c.relkind IN ({}) ORDER BY n.nspname,c.relname,a.attnum",
                if *detail == Detail::Full {
                    ", pg_catalog.col_description(c.oid,a.attnum) AS comment, a.attstorage::text AS storage"
                } else {
                    ""
                },
                if object.kind == ObjectKind::Table {
                    "'r','p','f'"
                } else {
                    "'v','m'"
                }
            ),
            DatabaseBackend::Clickhouse => format!(
                "SELECT x.database AS namespace, x.table AS name, x.name AS column_name, x.position AS ordinal_position, x.type AS data_type, x.default_kind, x.default_expression {} FROM system.columns x JOIN system.tables t ON t.database = x.database AND t.name = x.table WHERE t.engine {} ('View','MaterializedView','LiveView','WindowView') ORDER BY x.database,x.table,x.position",
                if *detail == Detail::Full {
                    ", x.comment, x.compression_codec AS codec, t.engine"
                } else {
                    ""
                },
                if object.kind == ObjectKind::Table {
                    "NOT IN"
                } else {
                    "IN"
                }
            ),
        });
    }
    let CatalogRequest::List {
        kind,
        database: requested_database,
        schema,
        visibility,
        ..
    } = request
    else {
        return Err(CatalogError::Unsupported(request.action()));
    };
    if (backend == DatabaseBackend::Clickhouse && schema.is_some())
        || (backend == DatabaseBackend::Postgresql
            && *kind != ObjectKind::Database
            && requested_database
                .as_deref()
                .is_some_and(|name| name != database))
    {
        return Err(CatalogError::Unsupported(request.action()));
    }
    let visible = *visibility == Visibility::UserObjects;
    let sql = match (backend, kind) {
        (DatabaseBackend::Postgresql, ObjectKind::Database) => format!(
            "SELECT '' AS namespace, datname AS name, pg_catalog.pg_get_userbyid(datdba) AS owner FROM pg_catalog.pg_database {} ORDER BY datname",
            if visible { "WHERE NOT datistemplate" } else { "" }),
        (DatabaseBackend::Postgresql, ObjectKind::Schema) => format!(
            "SELECT '' AS namespace, nspname AS name, pg_catalog.pg_get_userbyid(nspowner) AS owner FROM pg_catalog.pg_namespace {} ORDER BY nspname",
            if visible { "WHERE nspname NOT LIKE 'pg_%' AND nspname <> 'information_schema'" } else { "" }),
        (DatabaseBackend::Postgresql, ObjectKind::User | ObjectKind::Role) => format!(
            "SELECT '' AS namespace, rolname AS name, rolcanlogin AS login, rolsuper AS superuser FROM pg_catalog.pg_roles {} ORDER BY rolname",
            match (*kind, visible) {
                (ObjectKind::User, true) => "WHERE rolcanlogin AND rolname NOT LIKE 'pg_%'",
                (ObjectKind::User, false) => "WHERE rolcanlogin",
                (_, true) => "WHERE rolname NOT LIKE 'pg_%'",
                _ => "",
            }),
        (DatabaseBackend::Postgresql, ObjectKind::Table | ObjectKind::View | ObjectKind::Index) => {
            let filter = match kind {
                ObjectKind::Table => "c.relkind IN ('r','p','f')",
                ObjectKind::View => "c.relkind IN ('v','m')",
                _ => "c.relkind IN ('i','I')",
            };
            let attributes = if *kind == ObjectKind::Index {
                ", 'PostgreSQL index' AS index_kind, pg_catalog.pg_get_indexdef(c.oid) AS definition"
            } else {
                ", c.relkind::text AS relation_kind, pg_catalog.pg_get_userbyid(c.relowner) AS owner"
            };
            format!("SELECT n.nspname AS namespace, c.relname AS name{attributes} FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE {filter} {} ORDER BY n.nspname, c.relname",
                if visible { "AND n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_toast%'" } else { "" })
        }
        (DatabaseBackend::Clickhouse, ObjectKind::Database) => format!(
            "SELECT '' AS namespace, name, engine FROM system.databases {} ORDER BY name",
            if visible { "WHERE name NOT IN ('system','information_schema','INFORMATION_SCHEMA')" } else { "" }),
        (DatabaseBackend::Clickhouse, ObjectKind::User) => "SELECT '' AS namespace, name FROM system.users ORDER BY name".into(),
        (DatabaseBackend::Clickhouse, ObjectKind::Role) => "SELECT '' AS namespace, name FROM system.roles ORDER BY name".into(),
        (DatabaseBackend::Clickhouse, ObjectKind::Index) =>
            "SELECT database AS namespace, name, table AS table_name, 'ClickHouse data-skipping index' AS index_kind, type_full AS index_type, expr AS expression FROM system.data_skipping_indices ORDER BY database, table, name".into(),
        (DatabaseBackend::Clickhouse, ObjectKind::Table | ObjectKind::View) => format!(
            "SELECT database AS namespace, name, engine FROM system.tables WHERE engine {} ('View','MaterializedView','LiveView','WindowView') ORDER BY database, name",
            if *kind == ObjectKind::Table { "NOT IN" } else { "IN" }),
        _ => return Err(CatalogError::Unsupported(request.action())),
    };
    Ok(sql)
}

pub(crate) fn entries(
    backend: DatabaseBackend,
    database: &str,
    request: &CatalogRequest,
    result: ResultSet,
) -> Result<Vec<CatalogEntry>, CatalogError> {
    let (kind, filter_database, schema, glob) = match request {
        CatalogRequest::Find { kind, .. } => (kind, None, None, None),
        CatalogRequest::List {
            kind,
            database,
            schema,
            glob,
            ..
        } => (kind, database.as_deref(), schema.as_ref(), glob.as_ref()),
        CatalogRequest::Describe { object, .. } => (
            &object.kind,
            Some(object.database.as_str()),
            object.schema.as_ref(),
            None,
        ),
    };
    let mut entries = Vec::new();
    for row in result.rows {
        if row.len() != result.columns.len() || row.len() < 2 {
            return Err(CatalogError::Backend {
                message: "Malformed catalog row".into(),
                code: None,
            });
        }
        let (ResultCell::Text(namespace), ResultCell::Text(name)) = (&row[0], &row[1]) else {
            return Err(CatalogError::Backend {
                message: "Catalog identity is not text".into(),
                code: None,
            });
        };
        let identity = ObjectIdentity {
            database: if *kind == ObjectKind::Database {
                name.clone()
            } else if backend == DatabaseBackend::Clickhouse && !namespace.is_empty() {
                namespace.clone()
            } else {
                database.into()
            },
            schema: (backend == DatabaseBackend::Postgresql && !namespace.is_empty())
                .then(|| namespace.clone()),
            kind: *kind,
            name: name.clone(),
        };
        let default_database = matches!(
            kind,
            ObjectKind::Table | ObjectKind::View | ObjectKind::Index
        );
        let selected_database = filter_database.or(default_database.then_some(database));
        if selected_database.is_some_and(|selected| identity.database != selected)
            || schema
                .as_ref()
                .is_some_and(|schema| identity.schema.as_ref() != Some(schema))
        {
            continue;
        }
        if let CatalogRequest::Describe { object, .. } = request
            && (identity.name != object.name || identity.schema != object.schema)
        {
            continue;
        }
        if let CatalogRequest::Find { name, .. } = request {
            let qualified = format!(
                "{}.{}",
                identity.schema.as_deref().unwrap_or(&identity.database),
                identity.name
            );
            if identity.name != *name && qualified != *name {
                continue;
            }
        }
        if let Some(glob) = glob {
            let qualified = format!(
                "{}.{}",
                identity.schema.as_deref().unwrap_or(&identity.database),
                name
            );
            if !matches_glob(glob, name) && !matches_glob(glob, &qualified) {
                continue;
            }
        }
        entries.push(CatalogEntry {
            identity,
            attributes: result
                .columns
                .iter()
                .zip(row)
                .skip(2)
                .map(|(column, value)| (column.name.clone(), value))
                .collect(),
        });
    }
    Ok(entries)
}

pub(crate) fn error(error: dzban_core::error::ExecutionError) -> CatalogError {
    use dzban_core::error::{ErrorKind, ExecutionError, NativeErrorCode};
    match error {
        ExecutionError::Database(error) => {
            let denied = matches!(&error.code, Some(NativeErrorCode::SqlState(code)) if code == "42501")
                || matches!(&error.code, Some(NativeErrorCode::ClickHouse(291 | 497)));
            let code = error.code.map(|code| match code {
                NativeErrorCode::SqlState(code) => format!("SQLSTATE {code}"),
                NativeErrorCode::ClickHouse(code) => format!("ClickHouse code {code}"),
            });
            if denied {
                CatalogError::PermissionDenied {
                    message: error.message,
                    code,
                }
            } else if error.kind == ErrorKind::Transport {
                CatalogError::Connection {
                    message: error.message,
                }
            } else {
                CatalogError::Backend {
                    message: error.message,
                    code,
                }
            }
        }
        ExecutionError::Busy => CatalogError::Busy,
        error => CatalogError::Connection {
            message: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(glob: &str) -> CatalogRequest {
        CatalogRequest::List {
            kind: ObjectKind::Table,
            database: None,
            schema: None,
            glob: Some(glob.into()),
            visibility: Visibility::UserObjects,
        }
    }

    #[test]
    fn names_and_patterns_never_change_catalog_sql() {
        for backend in [DatabaseBackend::Postgresql, DatabaseBackend::Clickhouse] {
            assert_eq!(
                query(backend, "fixture", &request("users")).unwrap().sql,
                query(backend, "fixture", &request("'; DROP TABLE users; --"))
                    .unwrap()
                    .sql
            );
        }
        let request = CatalogRequest::List {
            kind: ObjectKind::Schema,
            database: None,
            schema: None,
            glob: None,
            visibility: Visibility::UserObjects,
        };
        assert!(matches!(
            query(DatabaseBackend::Clickhouse, "fixture", &request),
            Err(CatalogError::Unsupported(_))
        ));
    }

    #[test]
    fn parameters_bind_filters_and_preserve_clickhouse_escaped_strings() {
        let query = query(
            DatabaseBackend::Clickhouse,
            "fixture",
            &request("ž?\\[x]*\n"),
        )
        .unwrap();
        assert!(query.sql.contains("namespace = {p1:String}"));
        assert!(query.sql.contains("match(name, {p2:String})"));
        assert!(!query.sql.contains('ž'));
        assert_eq!(query.parameters, vec!["fixture", "(?s)^ž.\\\\\\[x\\].*\n$"]);
        let query = CatalogQuery {
            sql: String::new(),
            parameters: vec!["a\\b\t\nc\r\0'".into()],
        };
        assert_eq!(
            query.clickhouse_parameters(),
            vec![("param_p1".into(), "a\\\\b\\t\\nc\\r\\0'".into())]
        );
    }

    #[test]
    fn malformed_rows_are_errors_not_empty_catalogs() {
        let result = ResultSet {
            columns: vec![],
            rows: vec![vec![ResultCell::Text("truncated".into())]],
        };
        assert!(matches!(
            entries(
                DatabaseBackend::Postgresql,
                "fixture",
                &request("*"),
                result
            ),
            Err(CatalogError::Backend { .. })
        ));
    }

    #[test]
    fn native_permission_codes_are_preserved_and_classified() {
        use dzban_core::error::{DatabaseError, ErrorKind, ExecutionError, NativeErrorCode};
        for (backend, code) in [
            (
                DatabaseBackend::Postgresql,
                NativeErrorCode::SqlState("42501".into()),
            ),
            (
                DatabaseBackend::Clickhouse,
                NativeErrorCode::ClickHouse(497),
            ),
        ] {
            let converted = error(ExecutionError::Database(DatabaseError {
                backend,
                code: Some(code),
                kind: ErrorKind::Server,
                message: "Access denied".into(),
                detail: None,
                hint: None,
            }));
            assert!(matches!(
                converted,
                CatalogError::PermissionDenied { code: Some(_), .. }
            ));
            assert!(!converted.to_string().contains("SqlState("));
        }
    }
}
