//! Ordered, embedded schema migrations for the shared application database.
use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};

pub(crate) fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(include_str!("../migrations/001_initial.sql"))])
}

pub(crate) fn migrate(conn: &mut Connection) -> rusqlite_migration::Result<()> {
    migrations().to_latest(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(conn: &Connection) -> i32 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn initial_schema_is_valid_and_repeatable() {
        migrations().validate().unwrap();
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        conn.execute("INSERT INTO kv VALUES ('settings', X'0102')", [])
            .unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn), 1);
        let value: Vec<u8> = conn
            .query_row("SELECT value FROM kv", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, [1, 2]);
    }

    #[test]
    fn pending_migrations_are_atomic_and_can_retry_on_next_open() {
        let path = std::env::temp_dir().join(format!(
            "launchpad-migration-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut conn = Connection::open(&path).unwrap();
        migrate(&mut conn).unwrap();
        let failed = Migrations::new(vec![
            M::up(include_str!("../migrations/001_initial.sql")),
            M::up("INSERT INTO kv VALUES ('new', X'01'); CREATE TABLE added (id INTEGER);"),
            M::up("INSERT INTO missing_table VALUES (1);"),
        ]);
        assert!(failed.to_latest(&mut conn).is_err());
        assert_eq!(version(&conn), 1);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM kv", [], |r| r.get::<_, i32>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='added'",
                [],
                |r| r.get::<_, i32>(0)
            )
            .unwrap(),
            0
        );
        drop(conn);
        let mut conn = Connection::open(&path).unwrap();
        assert_eq!(version(&conn), 1);
        let fixed = Migrations::new(vec![
            M::up(include_str!("../migrations/001_initial.sql")),
            M::up("INSERT INTO kv VALUES ('new', X'01'); CREATE TABLE added (id INTEGER);"),
            M::up("INSERT INTO added VALUES (1);"),
        ]);
        fixed.to_latest(&mut conn).unwrap();
        assert_eq!(version(&conn), 3);
        drop(conn);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_first_migration_leaves_no_partial_schema() {
        let mut conn = Connection::open_in_memory().unwrap();
        let failed = Migrations::new(vec![
            M::up(include_str!("../migrations/001_initial.sql")),
            M::up("invalid SQL"),
        ]);
        assert!(failed.to_latest(&mut conn).is_err());
        assert_eq!(version(&conn), 0);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get::<_, i32>(0)
            )
            .unwrap(),
            0
        );
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn), 1);
    }

    #[test]
    fn newer_database_is_rejected_without_changes() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        conn.pragma_update(None, "user_version", 100).unwrap();
        conn.execute("INSERT INTO kv VALUES ('future', X'42')", [])
            .unwrap();
        assert!(migrate(&mut conn).is_err());
        assert_eq!(version(&conn), 100);
        assert_eq!(
            conn.query_row("SELECT value FROM kv", [], |r| r.get::<_, Vec<u8>>(0))
                .unwrap(),
            [0x42]
        );
    }

    #[test]
    fn pre_release_schema_is_not_repaired_or_reinterpreted() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE icons (app_id TEXT); INSERT INTO icons VALUES ('keep'); CREATE TABLE schema_meta (key TEXT, value INTEGER);").unwrap();
        assert!(migrate(&mut conn).is_err());
        assert_eq!(version(&conn), 0);
        assert_eq!(
            conn.query_row("SELECT app_id FROM icons", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "keep"
        );
    }
}
