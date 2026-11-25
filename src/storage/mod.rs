pub mod recovery;
mod retention;
use crate::{model::{now, Event}, FILE_LIMIT, FRAME_LIMIT};
use anyhow::{bail, ensure, Context, Result};
use chrono::{Local, NaiveDate};
use fs2::FileExt;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use std::{fs::{self, File, OpenOptions}, io::{self, BufWriter, Write}, path::{Path, PathBuf}, time::Duration};
use tempfile::Builder;

pub const DATABASE: &str = "watcher.sqlite3";

pub struct Store {
    pub conn: Connection,
    root: PathBuf,
    limit: usize,
    _writer: File,
}

pub fn reader(root: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(root.join(DATABASE), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(conn)
}

pub fn filename(day: &str, version: u64) -> String {
    format!("{}_{}.json", day.replace('-', "_"), version)
}

pub fn parse_filename(name: &str) -> Option<(NaiveDate, u64)> {
    let (day, v) = name.strip_suffix(".json")?.rsplit_once('_')?;
    let day = NaiveDate::parse_from_str(day, "%Y_%m_%d").ok()?;
    let version = v.parse().ok()?;
    (filename(&day.to_string(),version) == name).then_some((day,version))
}

impl Store {
    pub fn open(root: &Path) -> Result<Self> { Self::with_limit(root, FILE_LIMIT) }

    pub fn with_limit(root: &Path, limit: usize) -> Result<Self> {
        ensure!((3..=FILE_LIMIT).contains(&limit), "invalid chunk size");
        fs::create_dir_all(root.join("logs"))?;
        let writer = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(root.join("writer.lock"))?;
        writer.try_lock_exclusive().context("another collector owns this store")?;
        for entry in fs::read_dir(root.join("logs"))? {
            let entry=entry?;
            let name=entry.file_name();
            let name=name.to_string_lossy();
            if entry.file_type()?.is_file() && name.starts_with(".aw-seal-") && name.ends_with(".tmp") {
                fs::remove_file(entry.path())?;
            }
        }
        let conn = Connection::open(root.join(DATABASE))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        ensure!(version <= 1, "database requires a newer Agent Watcher");
        if version == 0 { conn.execute_batch("PRAGMA auto_vacuum=INCREMENTAL")?; }
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
        conn.execute_batch(include_str!("schema.sql"))?;
        let mut store = Self { conn, root: root.to_owned(), limit, _writer: writer };
        store.finish_retirement()?;
        store.publish_pending()?;
        Ok(store)
    }

    pub fn ingest(&mut self, event: Event) -> Result<bool> {
        self.append(event, Local::now().date_naive())
    }

    pub fn append(&mut self, mut event: Event, day: NaiveDate) -> Result<bool> {
        event.validate()?;
        event.sanitize();
        event.observed_utc = Some(now());
        if matches!(&event.body, crate::model::Body::CommandOutput{..}) {
            event.command_preview = self.conn.query_row(
                "SELECT substr(json_extract(payload,'$.command'),1,1024) FROM events WHERE cid=?1 AND json_extract(payload,'$.event') IN ('commandStarted','commandRequested') ORDER BY rowid DESC LIMIT 1",
                [&event.command_id], |r|r.get::<_,String>(0)).optional()?;
        }
        let payload = serde_json::to_string(&event)?;
        // Allow escaped preview text and the collector timestamp added after ingress validation.
        ensure!(payload.len() <= FRAME_LIMIT + 8 * 1024 && payload.len() + 2 <= self.limit, "event too large");
        let day = day.to_string();
        let tx = self.conn.transaction()?;
        if tx.query_row("SELECT 1 FROM events WHERE eid=?1 UNION ALL SELECT 1 FROM expired_events WHERE eid=?1 LIMIT 1", [&event.event_id], |_| Ok(())).optional()?.is_some() {
            return Ok(false);
        }
        let active: Option<(i64, String, usize)> = tx.query_row(
            "SELECT id,day,bytes FROM chunks WHERE state='active'", [],
            |r| Ok((r.get(0)?,r.get(1)?,r.get::<_,i64>(2)? as usize))
        ).optional()?;
        let current = match active {
            Some((id, ref d, bytes)) if d == &day && bytes + 1 + payload.len() <= self.limit => Some(id),
            Some((id, _, _)) => {
                tx.execute("UPDATE chunks SET state='sealing' WHERE id=?1",[id])?;
                None
            }
            None => None,
        };
        let chunk = if let Some(id) = current { id } else {
            let mut v: u64 = tx.query_row("SELECT COALESCE(MAX(version)+1,0) FROM chunks WHERE day=?1",[&day],|r|r.get(0))?;
            while self.root.join("logs").join(filename(&day, v)).exists() { v += 1; }
            tx.execute("INSERT INTO chunks(day,version) VALUES(?1,?2)",params![day,v])?;
            tx.last_insert_rowid()
        };
        tx.execute("INSERT INTO events(eid,cid,chunk,payload) VALUES(?1,?2,?3,?4)",
            params![event.event_id,event.command_id,chunk,payload])?;
        let extra = payload.len() + usize::from(current.is_some());
        tx.execute("UPDATE chunks SET bytes=bytes+?2 WHERE id=?1",params![chunk,extra as i64])?;
        tx.commit()?;
        self.publish_pending()?;
        Ok(true)
    }

    pub fn rotate_date(&self, today: NaiveDate) -> Result<()> {
        self.conn.execute("UPDATE chunks SET state='sealing' WHERE state='active' AND day<>?1",[today.to_string()])?;
        self.publish_pending()
    }

    pub fn seal(&self) -> Result<()> {
        self.conn.execute("UPDATE chunks SET state='sealing' WHERE state='active'",[])?;
        self.publish_pending()
    }

    pub fn publish_pending(&self) -> Result<()> {
        let mut pending = self.conn.prepare("SELECT id,day,version,bytes FROM chunks WHERE state='sealing' ORDER BY id")?;
        let rows = pending.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,u64>(2)?,r.get::<_,u64>(3)?)))?;
        for row in rows {
            let (id,day,v,expected)=row?;
            let dir=self.root.join("logs");
            let path=dir.join(filename(&day,v));
            let mut temp=Builder::new().prefix(".aw-seal-").suffix(".tmp").tempfile_in(&dir)?;
            {
                let mut out=BufWriter::new(temp.as_file_mut());
                out.write_all(b"[")?;
                let mut stmt=self.conn.prepare("SELECT payload FROM events WHERE chunk=?1 ORDER BY rowid")?;
                let mut events=stmt.query([id])?;
                let mut first=true;
                while let Some(row)=events.next()? {
                    if !first { out.write_all(b",")?; }
                    let raw:String=row.get(0)?;
                    out.write_all(raw.as_bytes())?;
                    first=false;
                }
                out.write_all(b"]")?;
                out.flush()?;
            }
            temp.as_file().sync_all()?;
            ensure!(temp.as_file().metadata()?.len()==expected && expected<=self.limit as u64,"chunk byte count mismatch");
            if path.exists() {
                if digest(temp.path())? != digest(&path)? { bail!("existing chunk conflicts with journal: {}",path.display()); }
            } else {
                temp.persist_noclobber(&path).context("publish chunk")?;
            }
            self.conn.execute("UPDATE chunks SET state='sealed' WHERE id=?1",[id])?;
        }
        Ok(())
    }

    pub fn status(&self, name: &str, value: &str) -> Result<()> {
        self.conn.execute("INSERT INTO health VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET value=excluded.value",params![name,value])?;
        Ok(())
    }
}

fn digest(path: &Path) -> Result<Vec<u8>> {
    let mut hasher=Sha256::new();
    io::copy(&mut File::open(path)?, &mut hasher)?;
    Ok(hasher.finalize().to_vec())
}
