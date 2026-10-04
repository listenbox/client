use anyhow::{Context, Result};
use rusqlite::Connection;

mod embedded {
    refinery::embed_migrations!("migrations");
}

fn runner() -> refinery::Runner {
    embedded::migrations::runner()
        .set_abort_divergent(true)
        .set_abort_missing(true)
        .set_grouped(true)
}

pub fn run(connection: &mut Connection) -> Result<()> {
    runner().run(connection).context("Migrate sync journal")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use refinery::{Migration, Runner};

    #[test]
    fn reopening_preserves_work_and_rejects_changed_or_missing_history() {
        let mut connection = Connection::open_in_memory().unwrap();
        run(&mut connection).unwrap();
        connection.execute("INSERT INTO transfers VALUES ('origin', 'show', 'video', 'playlist', 'operation', NULL, NULL)", []).unwrap();
        let before = runner().get_applied_migrations(&mut connection).unwrap();
        run(&mut connection).unwrap();
        assert_eq!(
            runner().get_applied_migrations(&mut connection).unwrap(),
            before
        );
        assert_eq!(
            connection
                .query_row("SELECT operation_id FROM transfers", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "operation"
        );
        connection
            .execute("UPDATE refinery_schema_history SET checksum='0'", [])
            .unwrap();
        assert!(run(&mut connection).is_err());
        connection
            .execute("DROP TABLE refinery_schema_history", [])
            .unwrap();
        assert!(run(&mut connection).is_err());
    }

    #[test]
    fn grouped_failure_rolls_back_pending_schema_and_history() {
        let mut connection = Connection::open_in_memory().unwrap();
        run(&mut connection).unwrap();
        let before = runner().get_applied_migrations(&mut connection).unwrap();
        let mut migrations = runner().get_migrations().clone();
        migrations.push(
            Migration::unapplied("V4__pending", "CREATE TABLE pending(id INTEGER);").unwrap(),
        );
        migrations
            .push(Migration::unapplied("V5__broken", "INSERT INTO absent VALUES(1);").unwrap());
        assert!(
            Runner::new(&migrations)
                .set_grouped(true)
                .run(&mut connection)
                .is_err()
        );
        assert_eq!(
            runner().get_applied_migrations(&mut connection).unwrap(),
            before
        );
        assert!(connection.prepare("SELECT * FROM pending").is_err());
    }
}
