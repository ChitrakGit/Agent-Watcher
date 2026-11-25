use crate::{model::now, spool, storage::Store};
use anyhow::Result;
use chrono::Local;
use std::{fs, path::Path, sync::atomic::{AtomicBool,Ordering}, thread, time::{Duration,Instant}};

pub fn drain(root: &Path, store: &mut Store) -> Result<usize> {
    let dir=spool::inbox(root);
    fs::create_dir_all(&dir)?;
    let marker = dir.join("capture-loss.flag");
    if marker.exists() {
        store.status("lastQueueLoss", &now())?;
        fs::remove_file(marker)?;
    }
    let mut files=Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry=entry?;
        if !entry.file_type()?.is_file(){continue;}
        let p=entry.path();
        if p.extension().is_some_and(|v| v=="evt") { files.push(p); }
    }
    files.sort();
    let mut count=0;
    for p in files {
        let event=match spool::read_event(&p) {
            Ok(e)=>e,
            Err(error) if error.downcast_ref::<std::io::Error>().is_some() => {
                store.status("lastReadFailure",&now())?;
                continue;
            }
            Err(_) => {
                store.status("lastRejected",&now())?;
                // Remove malformed input without retaining its possibly sensitive payload.
                fs::remove_file(&p)?;
                continue;
            }
        };
        store.ingest(event)?;
        fs::remove_file(&p)?;
        count+=1;
    }
    Ok(count)
}

pub fn run(root: &Path, stop: &AtomicBool, once: bool) -> Result<()> {
    let mut store=Store::open(root)?;
    let retention=match fs::read_to_string(root.join("retention-bytes.txt")){
        Ok(value)=>value.trim().parse::<u64>()?,
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>0,
        Err(e)=>return Err(e.into())
    };
    store.status("state","running")?;
    let mut last_activity=Instant::now();
    let mut last_tick=Instant::now()-Duration::from_secs(10);
    loop {
        let pass = (|| -> Result<()> {
            let count=drain(root,&mut store)?;
            if count > 0 { last_activity=Instant::now(); }
            store.rotate_date(Local::now().date_naive())?;
            if last_activity.elapsed()>=Duration::from_secs(60) {store.seal()?; last_activity=Instant::now();}
            if last_tick.elapsed() >= Duration::from_secs(5) {
                store.retain(retention)?;
                store.status("heartbeat",&now())?;
                store.status("state","running")?;
                last_tick=Instant::now();
            }
            Ok(())
        })();
        if let Err(e)=pass {
            let _=store.status("state","collection error: check storage and queue permissions");
            if once {return Err(e);}
            eprintln!("Agent Watcher collector: {e:#}");
            thread::sleep(Duration::from_secs(1));
        }
        if once || stop.load(Ordering::Relaxed) {break;}
        thread::sleep(Duration::from_millis(150));
    }
    store.seal()?;
    store.retain(retention)?;
    store.status("state","stopped")?;
    store.status("heartbeat",&now())?;
    Ok(())
}
