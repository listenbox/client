//! Private transfer journal shared by CLI and desktop. Catalogs remain live API data.
use crate::{
    downloads::{Download, Phase},
    innertube::Video,
    publicapi as p,
};
use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

mod migrations;

#[derive(Clone)]
pub struct Database {
    connections: Arc<Connections>,
    root: PathBuf,
}

struct Connections {
    writer: Mutex<Connection>,
    readers: Mutex<Vec<Connection>>,
    path: PathBuf,
}

fn configure(connection: &Connection) -> Result<()> {
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;",
    )?;
    Ok(())
}

impl Database {
    pub fn open(directory: &Path, cancel: &tokio_util::sync::CancellationToken) -> Result<Self> {
        std::fs::create_dir_all(directory)?;
        // CLI and desktop may initialize different shows concurrently. Schema
        // inspection and migration are one operation, protected across processes.
        let initialization = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("sync-schema.lock"))?;
        crate::cancellation::file(&initialization, cancel, "sync journal")?;
        let path = directory.join("sync.sqlite");
        let mut connection = Connection::open(&path)?;
        configure(&connection)?;
        connection.execute_batch("PRAGMA journal_mode=WAL;")?;
        migrations::run(&mut connection)?;
        Ok(Self {
            connections: Arc::new(Connections {
                writer: Mutex::new(connection),
                readers: Mutex::new(Vec::new()),
                path,
            }),
            root: directory.join("transfers"),
        })
    }

    fn read<T>(&self, query: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let cached = self.connections.readers.lock().pop();
        let connection = match cached {
            Some(connection) => connection,
            None => {
                let connection = Connection::open_with_flags(
                    &self.connections.path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                configure(&connection)?;
                connection.execute_batch("PRAGMA query_only=ON;")?;
                connection
            }
        };
        // Neither pool nor writer mutex is held while the query runs.
        let result = query(&connection);
        let mut readers = self.connections.readers.lock();
        if readers.len() < 8 {
            readers.push(connection);
        }
        result
    }

    pub fn snapshot(
        &self,
        origin: &str,
        show: &str,
        collection: &str,
        videos: &[Video],
        can_remove: bool,
    ) -> Result<()> {
        let mut connection = self.connections.writer.lock();
        let tx = connection.transaction()?;
        let urls: std::collections::HashSet<_> = videos
            .iter()
            .map(|video| format!("https://www.youtube.com/watch?v={}", video.id))
            .collect();
        if can_remove {
            let previous = {
                let mut statement = tx.prepare(
                    "SELECT source_url FROM source_items WHERE origin IS ?1 AND show_slug IS ?2",
                )?;
                statement
                    .query_map(params![origin, show], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?
            };
            for url in previous {
                if !urls.contains(&url) {
                    tx.execute("DELETE FROM source_items WHERE origin IS ?1 AND show_slug IS ?2 AND source_url IS ?3", params![origin, show, url])?;
                }
            }
        }
        for (position, video) in videos.iter().enumerate() {
            let url = format!("https://www.youtube.com/watch?v={}", video.id);
            tx.execute(
                "INSERT INTO source_items (origin, show_slug, collection_url, source_url, position, title, duration_seconds, phase)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued')
                 ON CONFLICT(origin, show_slug, source_url) DO UPDATE SET
                 position=excluded.position, title=excluded.title, duration_seconds=excluded.duration_seconds",
                params![origin, show, collection, url, position as i64, video.title, video.duration_seconds.map(i64::try_from).transpose()?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn outcome(&self, origin: &str, show: &str, item: &Download) -> Result<()> {
        let phase = match item.phase {
            Phase::Complete => "complete",
            Phase::Skipped => "skipped",
            Phase::Failed => "failed",
            _ => "queued",
        };
        self.connections.writer.lock().execute(
            "UPDATE source_items SET title=?4, duration_seconds=?5, phase=?6, reason=?7, error=?8
             WHERE origin IS ?1 AND show_slug IS ?2 AND source_url IS ?3",
            params![
                origin,
                show,
                item.source_url,
                item.title,
                item.duration_seconds.map(i64::try_from).transpose()?,
                phase,
                item.reason,
                item.error
            ],
        )?;
        Ok(())
    }

    /// Reconcile one authoritative inventory in one transaction. Transfer
    /// cleanup follows this commit in the caller.
    pub fn published<'a>(
        &self,
        origin: &str,
        show: &str,
        sources: impl IntoIterator<Item = &'a str>,
    ) -> Result<()> {
        let mut connection = self.connections.writer.lock();
        let tx = connection.transaction()?;
        {
            let mut statement = tx.prepare(
                "UPDATE source_items SET phase='complete', reason=NULL, error=NULL WHERE origin IS ?1 AND show_slug IS ?2 AND source_url IS ?3",
            )?;
            for source in sources {
                statement.execute(params![origin, show, source])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn items(&self, origin: &str, show: &str) -> Result<Vec<Download>> {
        self.read(|connection| {
            let mut statement = connection.prepare(
                "SELECT source_url, position, title, duration_seconds, phase, reason, error
                 FROM source_items WHERE origin IS ?1 AND show_slug IS ?2 ORDER BY position, source_url",
            )?;
            let rows = statement.query_map(params![origin, show], |row| {
                let source_url: String = row.get(0)?;
                let video_id = source_url.strip_prefix("https://www.youtube.com/watch?v=").ok_or(rusqlite::Error::InvalidQuery)?;
                let mut item = Download::queued(show, show, video_id, &row.get::<_, String>(2)?);
                item.source_url = source_url;
                item.position = Some(row.get(1)?);
                item.duration_seconds = row.get::<_, Option<i64>>(3)?.map(|seconds| seconds as u64);
                item.phase = match row.get::<_, String>(4)?.as_str() {
                    "complete" => Phase::Complete,
                    "skipped" => Phase::Skipped,
                    "failed" => Phase::Failed,
                    "queued" => Phase::Queued,
                    _ => return Err(rusqlite::Error::InvalidQuery),
                };
                item.reason = row.get(5)?;
                item.error = row.get(6)?;
                Ok(item)
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn complete_sync(&self, origin: &str, show: &str) -> Result<()> {
        let completed_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis();
        self.connections.writer.lock().execute(
            "INSERT INTO sync_completions (origin, show_slug, completed_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(origin, show_slug) DO UPDATE SET completed_at=excluded.completed_at",
            params![origin, show, i64::try_from(completed_at)?],
        )?;
        Ok(())
    }

    pub fn last_synced(&self, origin: &str, show: &str) -> Result<Option<std::time::SystemTime>> {
        self.read(|connection| {
            let completed_at: Option<i64> = connection
                .query_row(
                    "SELECT completed_at FROM sync_completions WHERE origin IS ?1 AND show_slug IS ?2",
                    params![origin, show],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(completed_at.map(|millis| {
                std::time::UNIX_EPOCH + std::time::Duration::from_millis(millis as u64)
            }))
        })
    }

    pub fn download(&self, operation: &str, name: &str, identity: &str) -> Result<()> {
        let mut connection = self.connections.writer.lock();
        let tx = connection.transaction()?;
        tx.execute(
            "DELETE FROM downloads WHERE operation_id IS ?1 AND name IS ?2 AND identity IS NOT ?3",
            params![operation, name, identity],
        )?;
        tx.execute(
            "INSERT INTO downloads VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
            params![operation, name, identity],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn range_hash(&self, operation: &str, name: &str, start: u64) -> Result<Option<String>> {
        self.read(|connection| Ok(connection.query_row("SELECT sha256 FROM download_ranges WHERE operation_id IS ?1 AND name IS ?2 AND start IS ?3", params![operation, name, start as i64], |row| row.get(0)).optional()?))
    }

    pub fn save_range(&self, operation: &str, name: &str, start: u64, hash: &str) -> Result<()> {
        self.connections.writer.lock().execute("INSERT INTO download_ranges VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO UPDATE SET sha256=excluded.sha256", params![operation, name, start as i64, hash])?;
        Ok(())
    }

    pub fn operation(
        &self,
        origin: &str,
        show: &str,
        source: &str,
        collection: &str,
    ) -> Result<String> {
        let mut connection = self.connections.writer.lock();
        let transaction = connection.transaction()?;
        transaction.execute("INSERT INTO transfers (origin, show_slug, source_url, collection_url, operation_id) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT DO NOTHING", params![origin, show, source, collection, uuid::Uuid::new_v4().to_string()])?;
        let id = transaction.query_row("SELECT operation_id FROM transfers WHERE origin IS ?1 AND show_slug IS ?2 AND source_url IS ?3 AND collection_url IS ?4", params![origin, show, source, collection], |row| row.get(0))?;
        transaction.commit()?;
        Ok(id)
    }

    pub fn directory(&self, operation: &str) -> Result<PathBuf> {
        let id = uuid::Uuid::parse_str(operation).context("Invalid transfer operation")?;
        let directory = self.root.join(id.to_string());
        std::fs::create_dir_all(&directory)?;
        Ok(directory)
    }

    pub fn prepared(&self, operation: &str) -> Result<Option<p::CreateEpisodePackage>> {
        let json: Option<String> = self.read(|connection| {
            Ok(connection.query_row(
                "SELECT manifest FROM transfers WHERE operation_id IS ?1",
                [operation],
                |row| row.get(0),
            )?)
        })?;
        json.map(|json| serde_json::from_str(&json).map_err(Into::into))
            .transpose()
    }

    pub fn save_prepared(&self, manifest: &p::CreateEpisodePackage) -> Result<()> {
        self.connections.writer.lock().execute(
            "UPDATE transfers SET manifest = ?2 WHERE operation_id IS ?1",
            params![manifest.operation_id, serde_json::to_string(manifest)?],
        )?;
        Ok(())
    }

    pub fn session(&self, operation: &str, session: &str) -> Result<()> {
        let mut connection = self.connections.writer.lock();
        let tx = connection.transaction()?;
        tx.execute("DELETE FROM parts WHERE operation_id IS ?1 AND NOT EXISTS (SELECT 1 FROM transfers WHERE operation_id IS ?1 AND upload_session_id IS ?2)", params![operation, session])?;
        tx.execute(
            "UPDATE transfers SET upload_session_id = ?2 WHERE operation_id IS ?1",
            params![operation, session],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn has_part(&self, operation: &str, object: usize, part: i64) -> Result<bool> {
        self.read(|connection| Ok(connection.query_row("SELECT 1 FROM parts WHERE operation_id IS ?1 AND object_index IS ?2 AND part_number IS ?3", params![operation, object as i64, part], |_| Ok(())).optional()?.is_some()))
    }

    pub fn save_part(&self, operation: &str, object: usize, part: i64) -> Result<()> {
        self.connections.writer.lock().execute(
            "INSERT INTO parts VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
            params![operation, object as i64, part],
        )?;
        Ok(())
    }

    pub fn forget(&self, origin: &str, show: &str, source: &str) -> Result<()> {
        let ids = self.read(|connection| {
            let mut statement = connection.prepare("SELECT operation_id FROM transfers WHERE origin IS ?1 AND show_slug IS ?2 AND source_url IS ?3")?;
            Ok(statement
                .query_map(params![origin, show, source], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })?;
        // Delete files first. A crash before row deletion is harmless: remote inventory reconciles it again.
        for id in ids {
            let directory = self.directory(&id)?;
            std::fs::remove_dir_all(directory)?;
        }
        self.connections.writer.lock().execute(
            "DELETE FROM transfers WHERE origin IS ?1 AND show_slug IS ?2 AND source_url IS ?3",
            params![origin, show, source],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn readers_see_committed_snapshots_while_the_only_writer_is_busy() {
        let directory = tempfile::tempdir().unwrap();
        let db = Database::open(
            directory.path(),
            &tokio_util::sync::CancellationToken::new(),
        )
        .unwrap();
        let operation = db
            .operation(
                "https://api.test",
                "show",
                "https://source.test/a",
                "playlist",
            )
            .unwrap();
        db.download(&operation, "media", "version-1").unwrap();
        db.save_range(&operation, "media", 0, "committed").unwrap();
        let mut writer = db.connections.writer.lock();
        let tx = writer.transaction().unwrap();
        tx.execute("UPDATE download_ranges SET sha256='pending'", [])
            .unwrap();
        let (sender, receiver) = mpsc::channel();
        let barrier = Arc::new(std::sync::Barrier::new(4));
        let readers: Vec<_> = (0..4)
            .map(|_| {
                let (db, operation, sender, barrier) = (
                    db.clone(),
                    operation.clone(),
                    sender.clone(),
                    barrier.clone(),
                );
                std::thread::spawn(move || {
                    db.read(|connection| {
                        barrier.wait(); // Every reader has its own checked-out connection.
                        let hash: String = connection.query_row(
                            "SELECT sha256 FROM download_ranges WHERE operation_id=?1",
                            [operation],
                            |row| row.get(0),
                        )?;
                        sender.send(hash).unwrap();
                        assert!(
                            connection
                                .execute("DELETE FROM download_ranges", [])
                                .is_err()
                        );
                        Ok(())
                    })
                    .unwrap();
                })
            })
            .collect();
        // Fail promptly before releasing the writer if any reader waits on it.
        for _ in 0..4 {
            assert_eq!(
                receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
                "committed"
            );
        }
        tx.commit().unwrap();
        drop(writer);
        for reader in readers {
            reader.join().unwrap();
        }
        assert_eq!(
            db.range_hash(&operation, "media", 0).unwrap().as_deref(),
            Some("pending")
        );
        drop(db);
        assert_eq!(
            Database::open(
                directory.path(),
                &tokio_util::sync::CancellationToken::new()
            )
            .unwrap()
            .range_hash(&operation, "media", 0)
            .unwrap()
            .as_deref(),
            Some("pending")
        );
    }
}
