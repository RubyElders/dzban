use dzban_core::{Config, ConnectionProfileId, DatabaseBackend, DatabaseConfig, ProfileError};

fn profile(name: &str) -> DatabaseConfig {
    DatabaseConfig {
        name: name.into(),
        backend: DatabaseBackend::Postgresql,
        host: None,
        port: None,
        user: "fixture".into(),
        database: "original".into(),
        password: None,
        url: None,
    }
}

#[test]
fn profile_names_are_exact_and_overrides_do_not_mutate_configuration() {
    let config = Config {
        databases: vec![profile("local"), profile("local dev")],
    };
    let selected = config
        .resolve_profile(&ConnectionProfileId("local dev".into()), Some("temporary"))
        .unwrap();
    assert_eq!(selected.name, "local dev");
    assert_eq!(selected.database, "temporary");
    assert_eq!(config.databases[1].database, "original");
    let missing = ConnectionProfileId("LOCAL".into());
    assert_eq!(
        config.resolve_profile(&missing, None).err().unwrap(),
        ProfileError::Unknown(missing)
    );
}

#[test]
fn duplicate_profile_names_are_rejected_instead_of_picking_the_first() {
    let config = Config {
        databases: vec![profile("local"), profile("local")],
    };
    let id = ConnectionProfileId("local".into());
    assert_eq!(
        config.resolve_profile(&id, None).err().unwrap(),
        ProfileError::Ambiguous(id)
    );
}
