//! Persistent image cache shared by icons, thumbnails and previews.
//!
//! Only codec helpers and background workers call this module; GUI rendering
//! receives PNG bytes and never opens SQLite. The database is the only image
//! cache: no per-icon or per-thumbnail PNG files are created.
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::thumbnails::Presentation;

const MAX_THUMBNAIL_BYTES: i64 = 128 * 1024 * 1024;
const MAX_THUMBNAIL_ROWS: i64 = 8192;
const MAX_ICON_BYTES: i64 = 64 * 1024 * 1024;
const MAX_ICON_ROWS: i64 = 8192;
const MAX_PREVIEW_BYTES: i64 = 256 * 1024 * 1024;
const MAX_PREVIEW_ROWS: i64 = 2048;
const MAX_PNG: usize = 16 * 1024 * 1024;
const DB_NAME: &str = "thumbnails-v2.sqlite3";
thread_local! { static DB: RefCell<Option<(PathBuf, Connection)>> = const { RefCell::new(None) }; }
#[cfg(test)]
thread_local! { static TEST_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) }; }
#[cfg(test)]
pub(crate) fn set_test_directory(path: PathBuf) {
    TEST_DIR.with(|slot| *slot.borrow_mut() = Some(path));
}
#[cfg(test)]
pub(crate) fn reset_test_directory() {
    TEST_DIR.with(|slot| *slot.borrow_mut() = None);
}

pub fn default_directory() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("RustExplorer/cache/thumbnails")
}
pub fn directory() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = TEST_DIR.with(|slot| slot.borrow().clone()) {
        return path;
    }
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        crate::settings::Settings::load()
            .thumbnail_cache_directory
            .filter(|p| p.is_absolute())
            .unwrap_or_else(default_directory)
    })
    .clone()
}
fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn db_path(root: &Path) -> PathBuf {
    root.join(DB_NAME)
}

fn schema(db: &Connection) -> Result<()> {
    db.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA cache_size=-4096;
        PRAGMA wal_autocheckpoint=256;
        CREATE TABLE IF NOT EXISTS thumbnails (
            key TEXT PRIMARY KEY NOT NULL, image_hash TEXT, presentation TEXT NOT NULL,
            byte_count INTEGER NOT NULL CHECK(byte_count>=0),
            missing_until INTEGER NOT NULL DEFAULT 0, last_access INTEGER NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS images(hash TEXT PRIMARY KEY NOT NULL,png BLOB NOT NULL,byte_count INTEGER NOT NULL) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS thumbnail_image ON thumbnails(image_hash);
        CREATE INDEX IF NOT EXISTS thumbnail_lru ON thumbnails(last_access);
        CREATE TABLE IF NOT EXISTS thumbnail_totals(id INTEGER PRIMARY KEY CHECK(id=1), rows INTEGER NOT NULL, bytes INTEGER NOT NULL);
        INSERT OR IGNORE INTO thumbnail_totals SELECT 1,count(*),coalesce(sum(byte_count),0) FROM thumbnails;

        CREATE TABLE IF NOT EXISTS icons (
            key TEXT PRIMARY KEY NOT NULL, image_hash TEXT NOT NULL,
            byte_count INTEGER NOT NULL CHECK(byte_count>=0), last_access INTEGER NOT NULL
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS icon_lru ON icons(last_access);
        CREATE TABLE IF NOT EXISTS icon_totals(id INTEGER PRIMARY KEY CHECK(id=1), rows INTEGER NOT NULL, bytes INTEGER NOT NULL);
        INSERT OR IGNORE INTO icon_totals SELECT 1,count(*),coalesce(sum(byte_count),0) FROM icons;

        CREATE TABLE IF NOT EXISTS previews (
            key TEXT PRIMARY KEY NOT NULL, image_hash TEXT NOT NULL, pages INTEGER NOT NULL,
            byte_count INTEGER NOT NULL CHECK(byte_count>=0), last_access INTEGER NOT NULL
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS preview_lru ON previews(last_access);
        CREATE TABLE IF NOT EXISTS preview_totals(id INTEGER PRIMARY KEY CHECK(id=1), rows INTEGER NOT NULL, bytes INTEGER NOT NULL);
        INSERT OR IGNORE INTO preview_totals SELECT 1,count(*),coalesce(sum(byte_count),0) FROM previews;

        CREATE TRIGGER IF NOT EXISTS thumbnail_insert AFTER INSERT ON thumbnails BEGIN UPDATE thumbnail_totals SET rows=rows+1,bytes=bytes+new.byte_count WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS thumbnail_delete AFTER DELETE ON thumbnails BEGIN UPDATE thumbnail_totals SET rows=rows-1,bytes=bytes-old.byte_count WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS thumbnail_update AFTER UPDATE OF byte_count ON thumbnails BEGIN UPDATE thumbnail_totals SET bytes=bytes+new.byte_count-old.byte_count WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS icon_insert AFTER INSERT ON icons BEGIN UPDATE icon_totals SET rows=rows+1,bytes=bytes+new.byte_count WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS icon_delete AFTER DELETE ON icons BEGIN UPDATE icon_totals SET rows=rows-1,bytes=bytes-old.byte_count WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS icon_update AFTER UPDATE OF byte_count ON icons BEGIN UPDATE icon_totals SET bytes=bytes+new.byte_count-old.byte_count WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS preview_insert AFTER INSERT ON previews BEGIN UPDATE preview_totals SET rows=rows+1,bytes=bytes+new.byte_count WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS preview_delete AFTER DELETE ON previews BEGIN UPDATE preview_totals SET rows=rows-1,bytes=bytes-old.byte_count WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS preview_update AFTER UPDATE OF byte_count ON previews BEGIN UPDATE preview_totals SET bytes=bytes+new.byte_count-old.byte_count WHERE id=1; END;

        DROP TRIGGER IF EXISTS image_delete;
        DROP TRIGGER IF EXISTS image_update;
        CREATE TRIGGER IF NOT EXISTS image_cleanup_thumbnail AFTER DELETE ON thumbnails WHEN old.image_hash IS NOT NULL BEGIN
            DELETE FROM images WHERE hash=old.image_hash AND NOT EXISTS(SELECT 1 FROM thumbnails WHERE image_hash=old.image_hash)
              AND NOT EXISTS(SELECT 1 FROM icons WHERE image_hash=old.image_hash)
              AND NOT EXISTS(SELECT 1 FROM previews WHERE image_hash=old.image_hash);
        END;
        CREATE TRIGGER IF NOT EXISTS image_cleanup_icon AFTER DELETE ON icons WHEN old.image_hash IS NOT NULL BEGIN
            DELETE FROM images WHERE hash=old.image_hash AND NOT EXISTS(SELECT 1 FROM thumbnails WHERE image_hash=old.image_hash)
              AND NOT EXISTS(SELECT 1 FROM icons WHERE image_hash=old.image_hash)
              AND NOT EXISTS(SELECT 1 FROM previews WHERE image_hash=old.image_hash);
        END;
        CREATE TRIGGER IF NOT EXISTS image_cleanup_preview AFTER DELETE ON previews WHEN old.image_hash IS NOT NULL BEGIN
            DELETE FROM images WHERE hash=old.image_hash AND NOT EXISTS(SELECT 1 FROM thumbnails WHERE image_hash=old.image_hash)
              AND NOT EXISTS(SELECT 1 FROM icons WHERE image_hash=old.image_hash)
              AND NOT EXISTS(SELECT 1 FROM previews WHERE image_hash=old.image_hash);
        END;
        CREATE TRIGGER IF NOT EXISTS image_cleanup_thumbnail_update AFTER UPDATE OF image_hash ON thumbnails WHEN old.image_hash IS NOT new.image_hash BEGIN
            DELETE FROM images WHERE old.image_hash IS NOT NULL AND hash=old.image_hash AND NOT EXISTS(SELECT 1 FROM thumbnails WHERE image_hash=old.image_hash)
              AND NOT EXISTS(SELECT 1 FROM icons WHERE image_hash=old.image_hash)
              AND NOT EXISTS(SELECT 1 FROM previews WHERE image_hash=old.image_hash);
        END;
        CREATE TRIGGER IF NOT EXISTS image_cleanup_icon_update AFTER UPDATE OF image_hash ON icons WHEN old.image_hash IS NOT new.image_hash BEGIN
            DELETE FROM images WHERE old.image_hash IS NOT NULL AND hash=old.image_hash AND NOT EXISTS(SELECT 1 FROM thumbnails WHERE image_hash=old.image_hash)
              AND NOT EXISTS(SELECT 1 FROM icons WHERE image_hash=old.image_hash)
              AND NOT EXISTS(SELECT 1 FROM previews WHERE image_hash=old.image_hash);
        END;
        CREATE TRIGGER IF NOT EXISTS image_cleanup_preview_update AFTER UPDATE OF image_hash ON previews WHEN old.image_hash IS NOT new.image_hash BEGIN
            DELETE FROM images WHERE old.image_hash IS NOT NULL AND hash=old.image_hash AND NOT EXISTS(SELECT 1 FROM thumbnails WHERE image_hash=old.image_hash)
              AND NOT EXISTS(SELECT 1 FROM icons WHERE image_hash=old.image_hash)
              AND NOT EXISTS(SELECT 1 FROM previews WHERE image_hash=old.image_hash);
        END;
        PRAGMA user_version=3;",
    )?;
    Ok(())
}

fn open(root: &Path) -> Result<Connection> {
    std::fs::create_dir_all(root)?;
    let path = db_path(root);
    if !path.exists() {
        // A deleted main database must not inherit an old WAL/SHM pair.
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }
    let result = (|| -> Result<Connection> {
        let db = Connection::open(&path)?;
        db.busy_timeout(Duration::from_millis(1000))?;
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version < 3 {
            schema(&db)?;
        } else {
            db.execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
                 PRAGMA cache_size=-4096; PRAGMA wal_autocheckpoint=256;",
            )?;
        }
        Ok(db)
    })();
    match result {
        Ok(db) => Ok(db),
        Err(error) => {
            let text = error.to_string();
            if !text.contains("file is not a database")
                && !text.contains("database disk image is malformed")
            {
                return Err(error);
            }
            // A half-copied/deleted database should not permanently disable the
            // cache. Move the damaged file aside and initialize a fresh one.
            let _ = std::fs::rename(
                &path,
                path.with_extension(format!("corrupt-{}", std::process::id())),
            );
            let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
            let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
            let db = Connection::open(&path)?;
            db.busy_timeout(Duration::from_millis(1000))?;
            schema(&db).map_err(|_| error)?;
            Ok(db)
        }
    }
}

fn with_connection<T>(root: &Path, mut f: impl FnMut(&Connection) -> Result<T>) -> Result<T> {
    DB.with(|slot| {
        let mut slot = slot.borrow_mut();
        let cache_is_current = slot
            .as_ref()
            .is_some_and(|(cached_root, _)| cached_root == root && db_path(root).is_file());
        if !cache_is_current {
            *slot = None;
            *slot = Some((root.to_path_buf(), open(root)?));
        }
        let result = f(&slot.as_ref().unwrap().1);
        if result.as_ref().is_err_and(|error| {
            let text = error.to_string();
            text.contains("database disk image is malformed")
                || text.contains("file is not a database")
                || text.contains("disk I/O error")
        }) {
            *slot = None;
            *slot = Some((root.to_path_buf(), open(root)?));
            return f(&slot.as_ref().unwrap().1);
        }
        result
    })
}

fn with_db<T>(index: &Path, mut f: impl FnMut(&Connection, &str) -> Result<T>) -> Result<T> {
    let root = index
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid cache directory"))?;
    let key = index
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("Invalid cache key"))?;
    with_connection(root, |db| f(db, key))
}
fn with_key<T>(_key: &str, mut f: impl FnMut(&Connection) -> Result<T>) -> Result<T> {
    with_connection(&directory(), |db| f(db))
}

fn update_access(db: &Connection, table: &str, key: &str) -> Result<()> {
    let now = now();
    let sql = format!("UPDATE {table} SET last_access=?2 WHERE key=?1 AND last_access<?2-60");
    db.execute(&sql, params![key, now])?;
    Ok(())
}

fn prune(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    totals: &str,
    key: &str,
    max_rows: i64,
    max_bytes: i64,
) -> Result<()> {
    let (mut count, mut bytes): (i64, i64) = tx.query_row(
        &format!("SELECT rows,bytes FROM {totals} WHERE id=1"),
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    while count > max_rows || bytes > max_bytes {
        let victims = tx
            .prepare(&format!(
                "SELECT key,byte_count FROM {table} WHERE key<>?1 ORDER BY last_access LIMIT 128"
            ))?
            .query_map([key], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if victims.is_empty() {
            break;
        }
        for (victim, size) in victims {
            tx.execute(&format!("DELETE FROM {table} WHERE key=?1"), [victim])?;
            count -= 1;
            bytes -= size;
            if count <= max_rows && bytes <= max_bytes {
                break;
            }
        }
    }
    Ok(())
}

pub fn load_thumbnail(index: &Path) -> Result<Option<(Vec<u8>, Presentation)>> {
    with_db(index, |db, key| {
        let row: Option<(Vec<u8>, String)> = db
            .query_row(
                "SELECT images.png,thumbnails.presentation FROM thumbnails JOIN images ON images.hash=thumbnails.image_hash WHERE key=?1 AND thumbnails.byte_count<=?2",
                params![key, MAX_PNG as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((png, presentation)) = row else {
            return Ok(None);
        };
        ensure!(
            png.len() <= MAX_PNG && png.starts_with(b"\x89PNG\r\n\x1a\n"),
            "Invalid cached thumbnail"
        );
        update_access(db, "thumbnails", key)?;
        Ok(Some((png, serde_json::from_str(&presentation)?)))
    })
}

pub fn save_thumbnail(index: &Path, png: &[u8], presentation: &Presentation) -> Result<()> {
    ensure!(png.len() <= MAX_PNG, "Thumbnail too large");
    with_db(index, |db, key| {
        let tx = db.unchecked_transaction()?;
        let hash = digest(png);
        tx.execute(
            "INSERT OR IGNORE INTO images(hash,png,byte_count) VALUES(?1,?2,?3)",
            params![hash, png, png.len() as i64],
        )?;
        tx.execute(
            "INSERT INTO thumbnails(key,image_hash,presentation,byte_count,last_access)
             VALUES(?1,?2,?3,?4,?5) ON CONFLICT(key) DO UPDATE SET image_hash=excluded.image_hash,
             presentation=excluded.presentation,byte_count=excluded.byte_count,missing_until=0,last_access=excluded.last_access",
            params![key,hash,serde_json::to_string(presentation)?,png.len() as i64,now()],
        )?;
        prune(
            &tx,
            "thumbnails",
            "thumbnail_totals",
            key,
            MAX_THUMBNAIL_ROWS,
            MAX_THUMBNAIL_BYTES,
        )?;
        tx.commit()?;
        Ok(())
    })
}

pub fn load_icon(key: &str) -> Result<Option<Vec<u8>>> {
    with_key(key, |db| {
        let png: Option<Vec<u8>> = db
            .query_row(
                "SELECT images.png FROM icons JOIN images ON images.hash=icons.image_hash WHERE icons.key=?1 AND icons.byte_count<=?2",
                params![key, MAX_PNG as i64],
                |r| r.get(0),
            )
            .optional()?;
        if png.is_some() {
            update_access(db, "icons", key)?;
        }
        ensure!(
            png.as_ref()
                .is_none_or(|png| png.starts_with(b"\x89PNG\r\n\x1a\n")),
            "Invalid cached icon"
        );
        Ok(png)
    })
}

/// Persist a whole icon batch in one transaction. The helper resolves up to 32
/// icons at a time; committing each one separately dominated cold-folder writes.
pub fn save_icons(entries: &[(String, Vec<u8>)]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    for (_, png) in entries {
        ensure!(png.len() <= MAX_PNG, "Icon too large");
    }
    with_key("icons-batch", |db| {
        let tx = db.unchecked_transaction()?;
        let accessed = now();
        for (key, png) in entries {
            let hash = digest(png);
            tx.execute(
                "INSERT OR IGNORE INTO images(hash,png,byte_count) VALUES(?1,?2,?3)",
                params![hash, png, png.len() as i64],
            )?;
            tx.execute(
                "INSERT INTO icons(key,image_hash,byte_count,last_access) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(key) DO UPDATE SET image_hash=excluded.image_hash,byte_count=excluded.byte_count,last_access=excluded.last_access",
                params![key, hash, png.len() as i64, accessed],
            )?;
        }
        let last = entries.last().unwrap().0.as_str();
        prune(
            &tx,
            "icons",
            "icon_totals",
            last,
            MAX_ICON_ROWS,
            MAX_ICON_BYTES,
        )?;
        tx.commit()?;
        Ok(())
    })
}

pub fn save_icon(key: &str, png: &[u8]) -> Result<()> {
    save_icons(&[(key.to_string(), png.to_vec())])
}

pub fn load_preview(key: &str) -> Result<Option<(Vec<u8>, u32)>> {
    with_key(key, |db| {
        let row: Option<(Vec<u8>, u32)> = db
            .query_row(
                "SELECT images.png,previews.pages FROM previews JOIN images ON images.hash=previews.image_hash WHERE previews.key=?1 AND previews.byte_count<=?2",
                params![key, MAX_PNG as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if row.is_some() {
            update_access(db, "previews", key)?;
        }
        ensure!(
            row.as_ref()
                .is_none_or(|(png, _)| png.starts_with(b"\x89PNG\r\n\x1a\n")),
            "Invalid cached preview"
        );
        Ok(row)
    })
}

pub fn save_preview(key: &str, png: &[u8], pages: u32) -> Result<()> {
    ensure!(png.len() <= MAX_PNG, "Preview too large");
    with_key(key, |db| {
        let tx = db.unchecked_transaction()?;
        let hash = digest(png);
        tx.execute(
            "INSERT OR IGNORE INTO images(hash,png,byte_count) VALUES(?1,?2,?3)",
            params![hash, png, png.len() as i64],
        )?;
        tx.execute(
            "INSERT INTO previews(key,image_hash,pages,byte_count,last_access) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(key) DO UPDATE SET image_hash=excluded.image_hash,pages=excluded.pages,byte_count=excluded.byte_count,last_access=excluded.last_access",
            params![key,hash,pages,png.len() as i64,now()],
        )?;
        prune(
            &tx,
            "previews",
            "preview_totals",
            key,
            MAX_PREVIEW_ROWS,
            MAX_PREVIEW_BYTES,
        )?;
        tx.commit()?;
        Ok(())
    })
}

pub fn missing(index: &Path) -> bool {
    with_db(index, |db, key| {
        Ok(db
            .query_row(
                "SELECT missing_until FROM thumbnails WHERE key=?1",
                [key],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .is_some_and(|until| until > now()))
    })
    .unwrap_or(false)
}

pub fn remember_missing(index: &Path) {
    let _ = with_db(index, |db, key| {
        let tx = db.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO thumbnails(key,presentation,byte_count,missing_until,last_access) VALUES(?1,'{}',0,?2,?3)
             ON CONFLICT(key) DO UPDATE SET missing_until=excluded.missing_until,last_access=excluded.last_access",
            params![key, now() + 7 * 24 * 3600, now()],
        )?;
        prune(
            &tx,
            "thumbnails",
            "thumbnail_totals",
            key,
            MAX_THUMBNAIL_ROWS,
            MAX_THUMBNAIL_BYTES,
        )?;
        tx.commit()?;
        Ok(())
    });
}

pub fn clear() -> Result<()> {
    let index = directory().join("clear.cache");
    with_db(&index, |db, _| {
        db.execute("DELETE FROM thumbnails", [])?;
        db.execute("DELETE FROM icons", [])?;
        db.execute("DELETE FROM previews", [])?;
        db.execute("DELETE FROM images", [])?;
        db.execute_batch(
            "UPDATE thumbnail_totals SET rows=0,bytes=0;
             UPDATE icon_totals SET rows=0,bytes=0;
             UPDATE preview_totals SET rows=0,bytes=0;
             PRAGMA wal_checkpoint(TRUNCATE);",
        )?;
        Ok(())
    })
}

#[cfg(test)]
pub(crate) fn close_thread_connection() {
    DB.with(|slot| *slot.borrow_mut() = None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_and_previews_round_trip_without_files() {
        let root =
            std::env::temp_dir().join(format!("re-sqlite-assets-{}-{}", std::process::id(), now()));
        std::fs::create_dir_all(&root).unwrap();
        set_test_directory(root.clone());
        let png = b"\x89PNG\r\n\x1a\nfixture";
        save_icon("test-icon-key", png).unwrap();
        assert_eq!(
            load_icon("test-icon-key").unwrap().as_deref(),
            Some(png.as_slice())
        );
        assert!(
            !root.read_dir().unwrap().any(|e| e
                .unwrap()
                .path()
                .extension()
                .is_some_and(|x| x == "png"))
        );
        save_preview("test-preview-key", png, 3).unwrap();
        let (loaded, pages) = load_preview("test-preview-key").unwrap().unwrap();
        assert_eq!(loaded, png);
        assert_eq!(pages, 3);
        clear().unwrap();
        close_thread_connection();
        reset_test_directory();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn duplicate_bitmaps_share_one_blob_and_eviction_removes_orphans() {
        let root =
            std::env::temp_dir().join(format!("re-sqlite-dedup-{}-{}", std::process::id(), now()));
        std::fs::create_dir_all(&root).unwrap();
        let png = b"\x89PNG\r\n\x1a\nfixture";
        let asset = crate::thumbnails::Presentation::default();
        let first = root.join("first.json");
        let second = root.join("second.json");
        save_thumbnail(&first, png, &asset).unwrap();
        save_thumbnail(&second, png, &asset).unwrap();
        with_db(&first, |db, _| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM images", [], |r| r.get::<_, i64>(0))?,
                1
            );
            db.execute("DELETE FROM thumbnails WHERE key='first'", [])?;
            assert_eq!(
                db.query_row("SELECT count(*) FROM images", [], |r| r.get::<_, i64>(0))?,
                1
            );
            db.execute("DELETE FROM thumbnails", [])?;
            assert_eq!(
                db.query_row("SELECT count(*) FROM images", [], |r| r.get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
        close_thread_connection();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn deleted_database_is_recreated_on_the_next_access() {
        let root = std::env::temp_dir().join(format!(
            "re-sqlite-recreate-{}-{}",
            std::process::id(),
            now()
        ));
        std::fs::create_dir_all(&root).unwrap();
        set_test_directory(root.clone());
        let png = b"\x89PNG\r\n\x1a\nfixture";
        save_icon("recreate-key", png).unwrap();
        close_thread_connection();
        let db = db_path(&root);
        assert!(db.is_file());
        for suffix in ["", "-wal", "-shm"] {
            let path = if suffix.is_empty() {
                db.clone()
            } else {
                PathBuf::from(format!("{}{suffix}", db.display()))
            };
            let _ = std::fs::remove_file(path);
        }
        assert!(load_icon("recreate-key").unwrap().is_none());
        assert!(db.is_file());
        save_icon("recreate-key", png).unwrap();
        assert_eq!(
            load_icon("recreate-key").unwrap().as_deref(),
            Some(png.as_slice())
        );
        close_thread_connection();
        reset_test_directory();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn blobs_round_trip_and_missing_keys_stay_separate() {
        let root =
            std::env::temp_dir().join(format!("re-sqlite-test-{}-{}", std::process::id(), now()));
        let index = root.join("test-key.json");
        std::fs::create_dir_all(&root).unwrap();
        let png = b"\x89PNG\r\n\x1a\nfixture";
        let presentation = Presentation::default();
        save_thumbnail(&index, png, &presentation).unwrap();
        assert_eq!(load_thumbnail(&index).unwrap().unwrap().0, png);
        let miss_index = root.join("missing.json");
        remember_missing(&miss_index);
        assert!(missing(&miss_index));
        assert!(!missing(&index));
        close_thread_connection();
        let _ = std::fs::remove_dir_all(root);
    }
}
