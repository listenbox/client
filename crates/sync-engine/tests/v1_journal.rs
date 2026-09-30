//! Historical journal setup for the parent API's integrated CLI recovery proof.
//! This frozen V1 is the journal shipped before saved source outcomes existed.
use core::prelude::v1::test;
use refinery::{Migration, Runner};
use rusqlite::Connection;

#[test]
#[ignore = "fixture setup invoked by the parent API E2E recovery scenario"]
fn prepare_v1_checkpoint() {
    let directory = std::path::PathBuf::from(std::env::var_os("LISTENBOX_PROFILE_DIR").unwrap());
    let current = directory.join("sync.sqlite");
    let historical = directory.join("sync-v1.sqlite");
    let mut connection = Connection::open(&historical).unwrap();
    let migration =
        Migration::unapplied("V1__initial", include_str!("fixtures/V1__initial.sql")).unwrap();
    Runner::new(&[migration]).run(&mut connection).unwrap();
    connection
        .execute(
            "ATTACH DATABASE ?1 AS checkpoint",
            [current.to_str().unwrap()],
        )
        .unwrap();
    // Carry the actual interrupted CLI's checkpoint unchanged into the schema
    // that existed when it was saved. No remote state or prepared media changes.
    connection.execute_batch(
        "INSERT INTO transfers SELECT * FROM checkpoint.transfers;
         INSERT INTO parts SELECT * FROM checkpoint.parts;
         INSERT INTO downloads SELECT * FROM checkpoint.downloads;
         INSERT INTO download_ranges SELECT * FROM checkpoint.download_ranges;
         INSERT INTO source_items SELECT origin, show_slug, collection_url, source_url, position FROM checkpoint.source_items;"
    ).unwrap();
    let prepared: i64 = connection
        .query_row(
            "SELECT count(*) FROM transfers WHERE manifest IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        prepared, 1,
        "historical fixture has no prepared transfer to recover"
    );
    drop(connection);
    std::fs::rename(&current, directory.join("sync-checkpoint-copy.sqlite")).unwrap();
    std::fs::rename(historical, current).unwrap();
}
