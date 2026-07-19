use rusqlite_migration::{M, Migrations};

pub(crate) const SCHEMA_VERSION: u32 = 1;

pub(crate) fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(include_str!("../migrations/0001_initial.sql"))])
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, OptionalExtension};
    use rusqlite_migration::{M, Migrations};

    use super::{SCHEMA_VERSION, migrations};

    #[test]
    fn migration_is_fresh_and_latest_to_latest_is_idempotent() {
        let mut connection = Connection::open_in_memory().expect("memory database");
        migrations()
            .to_latest(&mut connection)
            .expect("fresh migration");
        migrations()
            .to_latest(&mut connection)
            .expect("latest migration no-op");
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version");
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn failed_migration_rolls_back_all_statements() {
        let mut connection = Connection::open_in_memory().expect("memory database");
        let broken = Migrations::new(vec![M::up(
            "CREATE TABLE rollback_probe(id INTEGER); THIS IS NOT SQL;",
        )]);
        assert!(broken.to_latest(&mut connection).is_err());
        let table: Option<String> = connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE name='rollback_probe'",
                [],
                |row| row.get(0),
            )
            .optional()
            .expect("schema query");
        assert!(table.is_none());
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version");
        assert_eq!(version, 0);
    }

    #[test]
    fn schema_never_declares_plaintext_secret_columns() {
        let schema = include_str!("../migrations/0001_initial.sql").to_ascii_lowercase();
        for forbidden in [
            "access_token",
            "refresh_token",
            "provider_password",
            "database_key",
            "authorization_code",
        ] {
            assert!(!schema.contains(forbidden), "forbidden column: {forbidden}");
        }
    }
}
