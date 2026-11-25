use crate::{model::{id, Event}, FRAME_LIMIT};
use anyhow::{ensure, Context, Result};
use chrono::Utc;
use fs2::FileExt;
use std::{fs::{self, File, OpenOptions}, io::{Read, Write}, path::{Path, PathBuf}};
use tempfile::NamedTempFile;

const OUTBOX_LIMIT: u64 = 32 * 1024 * 1024;

pub fn inbox(root: &Path) -> PathBuf { root.join("inbox") }

/// Producers publish closed, flushed files. The collector removes one only after commit.
pub fn send(root: &Path, mut event: Event) -> Result<()> {
    let dir = inbox(root);
    let result = enqueue(&dir, &mut event);
    if result.is_err() {
        // Small independent marker survives the configured queue cap.
        let _ = OpenOptions::new().create_new(true).write(true).open(dir.join("capture-loss.flag"));
    }
    result
}

fn enqueue(dir: &Path, event: &mut Event) -> Result<()> {
    event.validate()?;
    event.sanitize();
    let bytes = serde_json::to_vec(&event)?;
    ensure!(bytes.len() + 64 <= FRAME_LIMIT, "event exceeds frame budget");
    fs::create_dir_all(dir)?;
    let guard = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(dir.join("queue.lock"))?;
    guard.lock_exclusive()?;
    let mut usage = 0u64;
    for entry in fs::read_dir(&dir)? {
        let p = entry?.path();
        if p.extension().is_some_and(|v| v == "evt") {
            if let Ok(m) = p.symlink_metadata() { usage = usage.saturating_add(m.len()); }
        }
    }
    ensure!(usage.saturating_add(bytes.len() as u64) <= OUTBOX_LIMIT, "capture queue full");
    let mut tmp = NamedTempFile::new_in(&dir)?;
    tmp.write_all(&bytes)?;
    tmp.as_file().sync_all()?;
    let order = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    tmp.persist_noclobber(dir.join(format!("{order:020}-{}.evt",id())))?;
    Ok(())
}

pub fn read_event(path: &Path) -> Result<Event> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)] {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let f: File = options.open(path)?;
    let meta = f.metadata()?;
    #[cfg(windows)] {
        use std::os::windows::fs::MetadataExt;
        ensure!(meta.file_attributes() & 0x400 == 0, "reparse points are forbidden");
    }
    ensure!(meta.is_file() && meta.len() <= FRAME_LIMIT as u64, "invalid queue entry");
    let mut bytes=Vec::new();
    f.take(FRAME_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len()<=FRAME_LIMIT,"oversized queue entry");
    let event: Event = serde_json::from_slice(&bytes).context("malformed event")?;
    event.validate()?;
    Ok(event)
}
