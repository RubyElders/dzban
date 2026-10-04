use dzban_core::catalog::{CatalogRequest, ObjectKind, Visibility};
use dzban_core::{DatabaseBackend, DatabaseConfig};

fn fixture(backend: DatabaseBackend) -> DatabaseConfig {
    let clickhouse = backend == DatabaseBackend::Clickhouse;
    DatabaseConfig {
        name: "catalog-fixture".into(),
        backend,
        host: Some("127.0.0.1".into()),
        port: Some(if clickhouse { 58123 } else { 5415 }),
        user: if clickhouse { "dzban" } else { "testuser" }.into(),
        password: Some(if clickhouse { "dzban-test" } else { "testpass" }.into()),
        database: if clickhouse { "dzban_test" } else { "testdb" }.into(),
        url: clickhouse.then(|| "http://127.0.0.1:58123".into()),
    }
}

fn list(kind: ObjectKind, glob: Option<&str>, visibility: Visibility) -> CatalogRequest {
    CatalogRequest::List {
        kind,
        glob: glob.map(str::to_owned),
        database: None,
        schema: None,
        visibility,
    }
}

#[tokio::test]
#[ignore = "requires PostgreSQL and ClickHouse from docker-compose.test.yml"]
async fn server_bound_patterns_match_unicode_literal_and_newline_glob_semantics() {
    for backend in [DatabaseBackend::Postgresql, DatabaseBackend::Clickhouse] {
        let engine = dzban_db::adapters::registry()
            .connect(fixture(backend))
            .await
            .unwrap();
        let all = engine
            .catalog(list(ObjectKind::Table, None, Visibility::UserObjects))
            .await
            .unwrap();
        assert!(
            all.iter()
                .any(|entry| entry.identity.name == "catalog žluť[1]\\tail")
        );
        assert!(
            all.iter()
                .any(|entry| entry.identity.name == "catalog line\nbreak")
        );
        for glob in [
            "catalog ž?uť*",
            "catalog žluť[1]\\tail",
            "catalog line?break",
            "catalog line*",
            "*.catalog ž*",
            "catalog*",
            "*",
            "'; DROP TABLE users; --",
        ] {
            let expected = all
                .iter()
                .filter(|entry| {
                    let qualified = format!(
                        "{}.{}",
                        entry
                            .identity
                            .schema
                            .as_deref()
                            .unwrap_or(&entry.identity.database),
                        entry.identity.name
                    );
                    dzban_core::catalog::matches_glob(glob, &entry.identity.name)
                        || dzban_core::catalog::matches_glob(glob, &qualified)
                })
                .cloned()
                .collect::<Vec<_>>();
            let filtered = engine
                .catalog(list(ObjectKind::Table, Some(glob), Visibility::UserObjects))
                .await
                .unwrap();
            assert_eq!(filtered, expected, "{backend:?}: {glob:?}");
        }
        let exact = engine
            .describe_named(
                "catalog žluť[1]\\tail".into(),
                dzban_core::catalog::Detail::Summary,
            )
            .await
            .unwrap();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].identity.name, "catalog žluť[1]\\tail");
        let literal = engine
            .catalog(CatalogRequest::Find {
                kind: ObjectKind::Table,
                name: "catalog ?star*".into(),
            })
            .await
            .unwrap();
        assert_eq!(literal.len(), 1);
        assert_eq!(literal[0].identity.name, "catalog ?star*");
        let described = engine
            .describe_named(
                "catalog ?star*".into(),
                dzban_core::catalog::Detail::Summary,
            )
            .await
            .unwrap();
        assert_eq!(described.len(), 1);
        engine.close().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires PostgreSQL and ClickHouse from docker-compose.test.yml"]
async fn catalogs_preserve_identities_patterns_and_backend_specific_metadata() {
    for backend in [DatabaseBackend::Postgresql, DatabaseBackend::Clickhouse] {
        let profile = fixture(backend);
        let database = profile.database.clone();
        let user = profile.user.clone();
        let engine = dzban_db::adapters::registry()
            .connect(profile)
            .await
            .unwrap();
        for (kind, expected) in [
            (ObjectKind::Database, database.as_str()),
            (ObjectKind::User, user.as_str()),
            (ObjectKind::Role, "catalog_fixture_role"),
        ] {
            let entries = engine
                .catalog(list(kind, Some(expected), Visibility::UserObjects))
                .await
                .unwrap();
            assert_eq!(entries.len(), 1, "{backend:?}: {kind:?}");
            assert_eq!(entries[0].identity.name, expected);
            if kind == ObjectKind::Database {
                assert_eq!(entries[0].identity.database, expected);
            }
        }
        let quoted = engine
            .catalog(list(
                ObjectKind::Table,
                Some("catalog odd's table"),
                Visibility::UserObjects,
            ))
            .await
            .unwrap();
        assert_eq!(quoted.len(), 1);
        assert_eq!(quoted[0].identity.database, database);
        assert_eq!(
            quoted[0].identity.schema.as_deref(),
            if backend == DatabaseBackend::Postgresql {
                Some("catalog odd's schema")
            } else {
                None
            }
        );
        let qualified = format!(
            "{}.catalog odd's table",
            quoted[0].identity.schema.as_deref().unwrap_or(&database)
        );
        assert_eq!(
            engine
                .catalog(list(
                    ObjectKind::Table,
                    Some(&qualified),
                    Visibility::UserObjects
                ))
                .await
                .unwrap(),
            quoted
        );
        assert!(
            engine
                .catalog(list(
                    ObjectKind::Table,
                    Some("'; DROP TABLE users; --"),
                    Visibility::UserObjects
                ))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            engine
                .catalog(list(
                    ObjectKind::Table,
                    Some("catalog odd's t?ble"),
                    Visibility::UserObjects
                ))
                .await
                .unwrap(),
            quoted
        );
        let indexes = engine
            .catalog(list(ObjectKind::Index, None, Visibility::UserObjects))
            .await
            .unwrap();
        assert!(!indexes.is_empty());
        let label = if backend == DatabaseBackend::Postgresql {
            "PostgreSQL index"
        } else {
            "ClickHouse data-skipping index"
        };
        assert!(
            indexes
                .iter()
                .all(|entry| entry.attributes["index_kind"].display() == label)
        );
        assert!(
            engine
                .catalog(list(
                    ObjectKind::Table,
                    Some("missing-*"),
                    Visibility::UserObjects
                ))
                .await
                .unwrap()
                .is_empty()
        );
        if backend == DatabaseBackend::Postgresql {
            let selected = engine
                .catalog(CatalogRequest::List {
                    kind: ObjectKind::Table,
                    database: Some(database.clone()),
                    schema: Some("catalog odd's schema".into()),
                    glob: None,
                    visibility: Visibility::UserObjects,
                })
                .await
                .unwrap();
            assert_eq!(selected, quoted);
            assert!(matches!(
                engine
                    .catalog(CatalogRequest::List {
                        kind: ObjectKind::Table,
                        database: Some("not-this-database".into()),
                        schema: None,
                        glob: None,
                        visibility: Visibility::UserObjects,
                    })
                    .await,
                Err(dzban_core::catalog::CatalogError::Unsupported(_))
            ));
            assert!(
                engine
                    .catalog(list(
                        ObjectKind::Schema,
                        Some("pg_catalog"),
                        Visibility::UserObjects
                    ))
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                engine
                    .catalog(list(
                        ObjectKind::Schema,
                        Some("pg_catalog"),
                        Visibility::AllObjects
                    ))
                    .await
                    .unwrap()
                    .len(),
                1
            );
        } else {
            let system = engine
                .catalog(CatalogRequest::List {
                    kind: ObjectKind::Table,
                    database: Some("system".into()),
                    schema: None,
                    glob: Some("tables".into()),
                    visibility: Visibility::AllObjects,
                })
                .await
                .unwrap();
            assert_eq!(system.len(), 1);
            assert_eq!(system[0].identity.database, "system");
            assert!(matches!(
                engine
                    .catalog(list(ObjectKind::Schema, None, Visibility::UserObjects))
                    .await,
                Err(dzban_core::catalog::CatalogError::Unsupported(_))
            ));
        }
        assert!(engine.subscribe().borrow().plan.is_none());
        engine.close().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires ClickHouse from docker-compose.test.yml"]
async fn denied_catalog_access_is_not_reported_as_empty_or_unsupported() {
    let mut profile = fixture(DatabaseBackend::Clickhouse);
    profile.user = "catalog_limited".into();
    profile.password = Some("catalog-fixture-password".into());
    let engine = dzban_db::adapters::registry()
        .connect(profile)
        .await
        .unwrap();
    assert!(matches!(
        engine
            .catalog(list(ObjectKind::User, None, Visibility::UserObjects))
            .await,
        Err(dzban_core::catalog::CatalogError::PermissionDenied { .. })
    ));
    engine.close().await.unwrap();
}

#[tokio::test]
#[ignore = "requires PostgreSQL from docker-compose.test.yml"]
async fn postgres_catalog_permission_denial_preserves_recovery_contract() {
    let engine = dzban_db::adapters::registry()
        .connect(fixture(DatabaseBackend::Postgresql))
        .await
        .unwrap();
    for sql in [
        "BEGIN",
        "REVOKE SELECT ON pg_catalog.pg_roles FROM PUBLIC",
        "SET LOCAL ROLE catalog_fixture_role",
    ] {
        engine
            .start_detailed(sql.into())
            .unwrap()
            .wait_detailed()
            .await
            .unwrap();
    }
    let denied = engine
        .catalog(list(ObjectKind::User, None, Visibility::UserObjects))
        .await;
    engine
        .start_detailed("ROLLBACK".into())
        .unwrap()
        .wait_detailed()
        .await
        .unwrap();
    assert!(matches!(
        denied,
        Err(dzban_core::catalog::CatalogError::PermissionDenied { .. })
    ));
    assert!(
        !engine
            .catalog(list(ObjectKind::User, None, Visibility::UserObjects))
            .await
            .unwrap()
            .is_empty()
    );
    engine.close().await.unwrap();
}

#[tokio::test]
#[ignore = "requires PostgreSQL and ClickHouse from docker-compose.test.yml"]
async fn adapters_list_real_tables_and_views_without_explain_or_history() {
    for (backend, port, user, password, database, expected) in [
        (
            DatabaseBackend::Postgresql,
            5415,
            "testuser",
            "testpass",
            "testdb",
            "users",
        ),
        (
            DatabaseBackend::Clickhouse,
            58123,
            "dzban",
            "dzban-test",
            "dzban_test",
            "events",
        ),
    ] {
        let profile = DatabaseConfig {
            name: "catalog-fixture".into(),
            backend,
            host: Some("127.0.0.1".into()),
            port: Some(port),
            user: user.into(),
            password: Some(password.into()),
            database: database.into(),
            url: (backend == DatabaseBackend::Clickhouse)
                .then(|| format!("http://127.0.0.1:{port}")),
        };
        let engine = dzban_db::adapters::registry()
            .connect(profile)
            .await
            .unwrap();
        for kind in [ObjectKind::Table, ObjectKind::View] {
            let entries = engine
                .catalog(CatalogRequest::List {
                    kind,
                    database: None,
                    schema: None,
                    glob: None,
                    visibility: Visibility::UserObjects,
                })
                .await
                .unwrap();
            if kind == ObjectKind::Table {
                assert!(entries.iter().any(|entry| entry.identity.name == expected));
                assert!(
                    entries
                        .iter()
                        .all(|entry| entry.identity.database == database)
                );
            } else if backend == DatabaseBackend::Postgresql {
                assert!(
                    entries
                        .iter()
                        .any(|entry| entry.identity.name == "user_post_counts")
                );
            } else {
                assert!(
                    entries
                        .iter()
                        .any(|entry| entry.identity.name == "event_names")
                );
            }
            assert!(engine.subscribe().borrow().plan.is_none());
            assert!(entries_schema_matches_backend(&engine, backend).await);
        }
        engine.close().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires PostgreSQL and ClickHouse from docker-compose.test.yml"]
async fn table_and_view_descriptions_preserve_column_order_types_and_detail() {
    use dzban_core::catalog::Detail;
    for backend in [DatabaseBackend::Postgresql, DatabaseBackend::Clickhouse] {
        let engine = dzban_db::adapters::registry()
            .connect(fixture(backend))
            .await
            .unwrap();
        let (table, view, columns, native_type) = if backend == DatabaseBackend::Postgresql {
            (
                "users",
                "user_post_counts",
                vec!["id", "username", "email", "created_at"],
                "integer",
            )
        } else {
            (
                "events",
                "event_names",
                vec!["id", "name", "occurred_at"],
                "UInt64",
            )
        };
        let summary = engine
            .describe_named(table.into(), Detail::Summary)
            .await
            .unwrap();
        assert_eq!(
            summary
                .iter()
                .map(|entry| entry.attributes["column_name"].display())
                .collect::<Vec<_>>(),
            columns
        );
        assert_eq!(summary[0].attributes["data_type"].display(), native_type);
        assert_eq!(summary[0].attributes["ordinal_position"].display(), "1");
        assert!(!summary[0].attributes.contains_key("comment"));
        if backend == DatabaseBackend::Postgresql {
            assert!(
                summary[0].attributes["default_expression"]
                    .display()
                    .contains("nextval")
            );
        } else {
            assert!(!summary[0].attributes.contains_key("nullable"));
        }
        let full = engine
            .describe_named(table.into(), Detail::Full)
            .await
            .unwrap();
        assert!(full[0].attributes.contains_key("comment"));
        assert_eq!(
            full.iter().map(|entry| &entry.identity).collect::<Vec<_>>(),
            summary
                .iter()
                .map(|entry| &entry.identity)
                .collect::<Vec<_>>()
        );
        let view = engine
            .describe_named(view.into(), Detail::Full)
            .await
            .unwrap();
        assert_eq!(view.len(), 2);
        assert!(
            view.iter()
                .all(|entry| entry.identity.kind == ObjectKind::View)
        );
        let quoted = engine
            .describe_named("catalog odd's table".into(), Detail::Summary)
            .await
            .unwrap();
        assert_eq!(quoted.len(), 1);
        assert_eq!(quoted[0].attributes["column_name"].display(), "id");
        assert!(
            engine
                .describe_named("'; DROP TABLE users; --".into(), Detail::Summary)
                .await
                .unwrap_err()
                .to_string()
                .contains("No matching object")
        );
        assert_eq!(
            engine
                .describe_named(table.into(), Detail::Summary)
                .await
                .unwrap(),
            summary
        );
        assert!(engine.subscribe().borrow().plan.is_none());
        engine.close().await.unwrap();
    }
}

async fn entries_schema_matches_backend(
    engine: &dzban_core::execution::QueryEngine,
    backend: DatabaseBackend,
) -> bool {
    engine
        .catalog(CatalogRequest::List {
            kind: ObjectKind::Table,
            database: None,
            schema: None,
            glob: None,
            visibility: Visibility::UserObjects,
        })
        .await
        .unwrap()
        .iter()
        .all(|entry| entry.identity.schema.is_some() == (backend == DatabaseBackend::Postgresql))
}
