//! Persistent thumbnail blobs. Only codec helpers / thumbnail workers call this
//! module; GUI rendering never opens SQLite or touches the filesystem.
use crate::native_assets::ThumbnailAsset;
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: i64 = 128 * 1024 * 1024;
const MAX_ROWS: i64 = 8192;
const MAX_PNG: usize = 8 * 1024 * 1024;
thread_local! { static DB: RefCell<Option<(PathBuf, Connection)>> = const { RefCell::new(None) }; }

pub fn default_directory() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("RustExplorer/cache/thumbnails")
}
pub fn directory() -> PathBuf {
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
fn open(root: &Path) -> Result<Connection> {
    std::fs::create_dir_all(root)?;
    let db = Connection::open(root.join("thumbnails-v2.sqlite3"))?;
    db.busy_timeout(Duration::from_millis(250))?;
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
        CREATE TRIGGER IF NOT EXISTS thumbnail_insert AFTER INSERT ON thumbnails BEGIN UPDATE thumbnail_totals SET rows=rows+1,bytes=bytes+new.byte_count WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS thumbnail_delete AFTER DELETE ON thumbnails BEGIN UPDATE thumbnail_totals SET rows=rows-1,bytes=bytes-old.byte_count WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS thumbnail_update AFTER UPDATE OF byte_count ON thumbnails BEGIN UPDATE thumbnail_totals SET bytes=bytes+new.byte_count-old.byte_count WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS image_delete AFTER DELETE ON thumbnails WHEN old.image_hash IS NOT NULL BEGIN DELETE FROM images WHERE hash=old.image_hash AND NOT EXISTS(SELECT 1 FROM thumbnails WHERE image_hash=old.image_hash); END;
        CREATE TRIGGER IF NOT EXISTS image_update AFTER UPDATE OF image_hash ON thumbnails WHEN old.image_hash IS NOT new.image_hash BEGIN DELETE FROM images WHERE hash=old.image_hash AND NOT EXISTS(SELECT 1 FROM thumbnails WHERE image_hash=old.image_hash); END;
        PRAGMA user_version=2;",
    )?;
    Ok(db)
}
fn with_db<T>(index: &Path, f: impl FnOnce(&Connection, &str) -> Result<T>) -> Result<T> {
    let root = index
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid thumbnail directory"))?;
    let key = index
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("Invalid thumbnail key"))?;
    DB.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_none_or(|(path, _)| path != root) {
            *slot = Some((root.into(), open(root)?));
        }
        f(&slot.as_ref().unwrap().1, key)
    })
}
pub fn load(index: &Path, runtime: &Path) -> Result<Option<ThumbnailAsset>> {
    with_db(index, |db, key| {
        let row: Option<(Vec<u8>, String)> = db.query_row(
            "SELECT images.png,thumbnails.presentation FROM thumbnails JOIN images ON images.hash=thumbnails.image_hash WHERE key=?1 AND thumbnails.byte_count<=?2",
            params![key, MAX_PNG as i64], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((png, presentation)) = row else {
            return Ok(None);
        };
        ensure!(
            png.len() <= MAX_PNG && png.starts_with(b"\x89PNG\r\n\x1a\n"),
            "Invalid cached thumbnail"
        );
        let presentation = serde_json::from_str(&presentation)?;
        std::fs::create_dir_all(runtime)?;
        let image = runtime.join(format!("{}.png", digest(&png)));
        if !image.is_file() {
            let temp = image.with_extension(format!("{}.tmp", std::process::id()));
            std::fs::write(&temp, png)?;
            if std::fs::rename(&temp, &image).is_err() {
                let _ = std::fs::remove_file(temp);
            }
        }
        let time = now();
        // Avoid a write transaction for every cache hit during fast scrolling.
        let _ = db.execute(
            "UPDATE thumbnails SET last_access=?2 WHERE key=?1 AND last_access<?2-60",
            params![key, time],
        );
        Ok(Some(ThumbnailAsset {
            image,
            presentation,
        }))
    })
}
pub fn save(index: &Path, asset: &ThumbnailAsset) -> Result<()> {
    let png = std::fs::read(&asset.image)?;
    ensure!(png.len() <= MAX_PNG, "Thumbnail too large");
    with_db(index, |db, key| {
        let transaction = db.unchecked_transaction()?;
        let hash = digest(&png);
        transaction.execute(
            "INSERT OR IGNORE INTO images(hash,png,byte_count) VALUES(?1,?2,?3)",
            params![hash, png, png.len() as i64],
        )?;
        transaction.execute("INSERT INTO thumbnails(key,image_hash,presentation,byte_count,last_access)
            VALUES(?1,?2,?3,?4,?5) ON CONFLICT(key) DO UPDATE SET image_hash=excluded.image_hash,
            presentation=excluded.presentation,byte_count=excluded.byte_count,missing_until=0,last_access=excluded.last_access",
            params![key,hash,serde_json::to_string(&asset.presentation)?,png.len() as i64,now()])?;
        let (mut count, mut bytes): (i64, i64) = transaction.query_row(
            "SELECT rows,bytes FROM thumbnail_totals WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        while count > MAX_ROWS || bytes > MAX_BYTES {
            let victims=transaction.prepare("SELECT key,byte_count FROM thumbnails WHERE key<>?1 ORDER BY last_access LIMIT 128")?
                .query_map([key],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            if victims.is_empty() {
                break;
            }
            for (victim, size) in victims {
                transaction.execute("DELETE FROM thumbnails WHERE key=?1", [victim])?;
                count -= 1;
                bytes -= size;
                if count <= MAX_ROWS && bytes <= MAX_BYTES {
                    break;
                }
            }
        }
        transaction.commit()?;
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
        db.execute("INSERT INTO thumbnails(key,presentation,byte_count,missing_until,last_access) VALUES(?1,'{}',0,?2,?3)
        ON CONFLICT(key) DO UPDATE SET missing_until=excluded.missing_until,last_access=excluded.last_access",params![key,now()+7*24*3600,now()])?;
        db.execute("DELETE FROM thumbnails WHERE key IN (SELECT key FROM thumbnails ORDER BY last_access LIMIT max(0,(SELECT rows FROM thumbnail_totals WHERE id=1)-8192))",[])?;
        Ok(())
    });
}
pub fn clear() -> Result<()> {
    let index = directory().join("clear.json");
    with_db(&index, |db, _| {
        db.execute("DELETE FROM thumbnails", [])?;
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    })
}
#[cfg(test)]
pub(crate) fn close_thread_connection() {
    DB.with(|slot| *slot.borrow_mut() = None);
}

#[cfg(test)]
mod tests {
    #[test]
    fn duplicate_bitmaps_share_one_blob_and_eviction_removes_orphans() {
        let root = std::env::temp_dir().join(format!(
            "re-sqlite-dedup-{}-{}",
            std::process::id(),
            super::now()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let image = root.join("input.png");
        std::fs::write(&image, b"\x89PNG\r\n\x1a\nfixture").unwrap();
        let asset = crate::native_assets::ThumbnailAsset {
            image,
            presentation: Default::default(),
        };
        let first = root.join("first.json");
        let second = root.join("second.json");
        super::save(&first, &asset).unwrap();
        super::save(&second, &asset).unwrap();
        super::with_db(&first, |db, _| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM images", [], |r| r.get::<_, i64>(0))?,
                1
            );
            assert_eq!(
                db.query_row("SELECT rows FROM thumbnail_totals", [], |r| r
                    .get::<_, i64>(0))?,
                2
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
            assert_eq!(
                db.query_row("SELECT bytes FROM thumbnail_totals", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
        super::close_thread_connection();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn blobs_round_trip_and_missing_keys_stay_separate() {
        let root = std::env::temp_dir().join(format!(
            "re-sqlite-test-{}-{}",
            std::process::id(),
            super::now()
        ));
        let index = root.join("test-key.json");
        let runtime = root.join("runtime");
        std::fs::create_dir_all(&root).unwrap();
        let image = root.join("input.png");
        std::fs::write(&image, b"\x89PNG\r\n\x1a\nfixture").unwrap();
        let asset = crate::native_assets::ThumbnailAsset {
            image,
            presentation: Default::default(),
        };
        super::save(&index, &asset).unwrap();
        assert!(super::load(&index, &runtime).unwrap().is_some());
        let missing = root.join("missing.json");
        super::remember_missing(&missing);
        assert!(super::missing(&missing));
        assert!(!super::missing(&index));
        super::with_db(&missing, |db, key| {
            db.execute(
                "UPDATE thumbnails SET missing_until=?2 WHERE key=?1",
                rusqlite::params![key, super::now() - 1],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(!super::missing(&missing));
        super::DB.with(|slot| *slot.borrow_mut() = None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
