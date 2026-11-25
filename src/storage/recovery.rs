use super::{parse_filename,Store,DATABASE};
use crate::{model::{stable,Body,Event,Origin},FILE_LIMIT};
use anyhow::{bail,ensure,Result};
use rusqlite::{params,Connection,OptionalExtension};
use serde_json::Value;
use std::{collections::HashMap,fs,path::Path};

fn legacy(raw:&Value, apps:&mut HashMap<String,String>,name:&str,index:usize)->Result<Event>{
    let cid=raw["commandId"].as_str().unwrap_or("unknown");
    let app=raw["agentApp"].as_str().map(str::to_string)
        .or_else(||apps.get(cid).cloned()).unwrap_or_else(||"Unknown agent".into());
    apps.insert(cid.into(),app.clone());
    let origin=Origin{command_id:cid.into(),app,source:"executor".into(),attribution:"unknown".into()};
    let get=|k:&str|raw[k].as_str().unwrap_or("").to_owned();
    let body=match raw["event"].as_str(){
        Some("commandStarted")=>Body::CommandStarted{command:get("command")},
        Some("commandOutput")=>Body::CommandOutput{sequence:raw["sequence"].as_u64().unwrap_or(index as u64),
            stream:serde_json::from_value(raw["stream"].clone())?,text:get("text")},
        Some("commandFinished")=>{
            let code=raw["exitCode"].as_i64().and_then(|x|i32::try_from(x).ok());
            let status=match raw["outputStatus"].as_str(){Some("captured")=>"complete",Some("empty")=>"empty",Some("partial")=>"partial",Some("unavailable")=>"unavailable",_=>"unknown"};
            Body::CommandFinished{outcome:match code{Some(0)=>"succeeded",Some(_)=>"failed",None=>"unknown"}.into(),exit_code:code,output_status:status.into()}
        }
        _=>bail!("unsupported legacy event"),
    };
    let mut event=origin.event(body);
    event.event_id=stable(&[name,&index.to_string()]);event.time_utc=get("timeUtc");
    Ok(event)
}

pub fn import(db:&mut Connection,log_dir:&Path)->Result<usize>{
    if !log_dir.exists(){return Ok(0);}
    let mut chunks=Vec::new();
    for entry in fs::read_dir(log_dir)?{
        let entry=entry?;
        if !entry.file_type()?.is_file(){continue;}
        let name=entry.file_name().to_string_lossy().into_owned();
        if let Some((day,version))=parse_filename(&name){chunks.push((day,version,name,entry.path()));}
    }
    chunks.sort_by_key(|(day,v,_,_)|(*day,*v));
    let mut apps=HashMap::new();let mut count=0;
    for (day,v,name,path) in chunks{
        let tx=db.transaction()?;
        let day=day.to_string();
        if tx.query_row("SELECT id FROM chunks WHERE day=?1 AND version=?2",params![day,v],|r|r.get::<_,i64>(0)).optional()?.is_some(){continue;}
        let bytes=fs::metadata(&path)?.len();
        ensure!(bytes<=FILE_LIMIT as u64,"oversized chunk {name}");
        let values:Vec<Value>=serde_json::from_reader(fs::File::open(path)?)?;
        tx.execute("INSERT INTO chunks(day,version,state,bytes) VALUES(?1,?2,'sealed',?3)",params![day,v,bytes])?;
        let chunk=tx.last_insert_rowid();
        for (i,value) in values.iter().enumerate(){
            let mut event=match value["schemaVersion"].as_u64(){
                Some(1)=>legacy(value,&mut apps,&name,i)?,
                Some(2)=>serde_json::from_value::<Event>(value.clone())?,
                _=>bail!("unsupported event schema in {name}"),
            };
            event.validate()?;event.sanitize();
            tx.execute("INSERT INTO events(eid,cid,chunk,payload) VALUES(?1,?2,?3,?4)",params![event.event_id,event.command_id,chunk,serde_json::to_string(&event)?])?;
        }
        tx.commit()?;count+=1;
    }
    Ok(count)
}

impl Store {
    pub fn restore(&mut self)->Result<usize>{import(&mut self.conn,&self.root.join("logs"))}
}

/// A standalone logs folder is readable even without the collector's catalog.
pub fn viewer_catalog(root:&Path)->Result<Connection>{
    if root.join(DATABASE).exists(){
        let candidate=super::reader(root).and_then(|db|{
            db.query_row("SELECT COUNT(*) FROM chunks",[],|r|r.get::<_,i64>(0))?;
            Ok(db)
        });
        if candidate.is_ok(){return candidate;}
        if !root.join("logs").is_dir(){return candidate;}
    }
    let mut db=Connection::open_in_memory()?;
    db.execute_batch(include_str!("schema.sql"))?;
    let count=import(&mut db,&root.join("logs"))?;
    if count>0 || root.join(DATABASE).exists(){
        db.execute("INSERT INTO health(name,value) VALUES('state','offline · reading sealed JSON')",[])?;
    }
    Ok(db)
}
