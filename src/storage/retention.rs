use super::{filename,Store};
use anyhow::{Context,Result};
use rusqlite::OptionalExtension;
use std::fs;

impl Store {
    /// The budget applies to published JSON. Preserve the version ledger and replay keys.
    pub fn retain(&mut self, budget:u64)->Result<()>{
        self.finish_retirement()?;
        if budget==0{return Ok(());}
        loop {
            let total:u64=self.conn.query_row(
                "SELECT COALESCE(SUM(bytes),0) FROM chunks WHERE state='sealed' AND id NOT IN (SELECT chunk FROM retired)",[],|r|r.get(0))?;
            if total<=budget{break;}
            let oldest:Option<i64>=self.conn.query_row(
                "SELECT id FROM chunks WHERE state='sealed' AND id NOT IN (SELECT chunk FROM retired) ORDER BY day,version LIMIT 1",[],|r|r.get(0)).optional()?;
            let Some(id)=oldest else {break};
            self.conn.execute("INSERT INTO retired(chunk,cleaned) VALUES(?1,0)",[id])?;
            self.finish_retirement()?;
        }
        Ok(())
    }

    pub(super) fn finish_retirement(&mut self)->Result<()>{
        let pending={
            let mut stmt=self.conn.prepare("SELECT c.id,c.day,c.version FROM chunks c JOIN retired t ON t.chunk=c.id WHERE t.cleaned=0")?;
            let rows=stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,u64>(2)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for(id,day,version) in pending{
            let path=self.root.join("logs").join(filename(&day,version));
            match fs::remove_file(path){
                Ok(())=>{},
                Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
                Err(e)=>return Err(e).context("remove expired log"),
            }
            let tx=self.conn.transaction()?;
            tx.execute("INSERT OR IGNORE INTO expired_events(eid,cid) SELECT eid,cid FROM events WHERE chunk=?1",[id])?;
            tx.execute("DELETE FROM events WHERE chunk=?1",[id])?;
            tx.execute("UPDATE retired SET cleaned=1 WHERE chunk=?1",[id])?;
            tx.commit()?;
            self.conn.execute_batch("PRAGMA incremental_vacuum(256)")?;
        }
        Ok(())
    }
}
