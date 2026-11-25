use crate::{model::{Body, Event},storage::filename};
use anyhow::{ensure,Result};
use chrono::{DateTime,SecondsFormat,Utc};
use rusqlite::{params,Connection,OptionalExtension};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize)]
pub struct Chunk {pub id:i64,pub name:String,pub bytes:u64,pub state:String}

pub fn files(db:&Connection)->Result<Vec<Chunk>>{
    let mut st=db.prepare("SELECT id,day,version,bytes,state FROM chunks WHERE id NOT IN (SELECT chunk FROM retired) ORDER BY day DESC,version DESC")?;
    let rows=st.query_map([],|r|Ok(Chunk{id:r.get(0)?,name:filename(&r.get::<_,String>(1)?,r.get(2)?),bytes:r.get(3)?,state:r.get(4)?}))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[derive(Default, Clone, Debug, clap::Args)]
pub struct Filter {
    /// Agent name. May be supplied more than once.
    #[arg(long)] pub agent:Vec<String>,
    #[arg(long,default_value="")] pub command:String,
    #[arg(long,default_value="")] pub output:String,
    #[arg(long,default_value="")] pub outcome:String,
    #[arg(long)] pub exit_code:Option<i32>,
    #[arg(long,default_value="")] pub capture:String,
    /// Inclusive RFC 3339 timestamp with UTC offset.
    #[arg(long,default_value="")] pub from:String,
    /// Exclusive RFC 3339 timestamp with UTC offset.
    #[arg(long,default_value="")] pub until:String,
}

#[derive(Debug,Clone,Serialize)]
pub struct Summary {
    pub cursor:i64,pub id:String,pub app:String,pub command:String,pub time:String,
    pub outcome:String,pub capture:String,pub exit_code:Option<i32>,pub attribution:String,pub source:String,
}

pub fn summary(db:&Connection,cid:&str,cursor:i64)->Result<Summary>{
    let mut q=db.prepare("SELECT payload FROM events WHERE cid=?1 AND json_extract(payload,'$.event')<>'commandOutput' ORDER BY rowid")?;
    let mut rows=q.query([cid])?;
    let mut result=Summary{cursor,id:cid.into(),app:String::new(),command:String::new(),time:String::new(),outcome:"unknown".into(),capture:"unknown".into(),exit_code:None,attribution:"unknown".into(),source:String::new()};
    let mut finished=false;
    let mut last_sequence=None;
    let mut gap=false;
    while let Some(row)=rows.next()? {
        let event:Event=serde_json::from_str(&row.get::<_,String>(0)?)?;
        if result.time.is_empty(){result.time=event.time_utc.clone();}
        result.app=event.agent_app;result.attribution=event.attribution;result.source=event.source;
        match event.body{
            Body::CommandRequested{command} => {if result.command.is_empty(){result.command=command;if !finished{result.outcome="requested".into();}}}
            Body::CommandStarted{command} => {result.command=command;result.time=event.time_utc;if !finished{result.outcome="running".into();}}
            Body::CommandFinished{outcome,exit_code,output_status} => {finished=true;result.outcome=outcome;result.exit_code=exit_code;result.capture=output_status;last_sequence=event.last_output_sequence;}
            Body::ProcessObserved{command,..}=>{result.command=command;result.outcome="observed".into();result.capture="unavailable".into();}
            Body::CaptureGap{..}=>gap=true,
            Body::CommandOutput{..}=>{}
        }
    }
    if result.app.is_empty(){
        let raw:String=db.query_row("SELECT payload FROM events WHERE cid=?1 LIMIT 1",[cid],|r|r.get(0))?;
        let e:Event=serde_json::from_str(&raw)?;
        result.app=e.agent_app;result.time=e.time_utc;result.source=e.source;result.attribution=e.attribution;
        result.command=e.command_preview.unwrap_or_default();
    }
    let expired=db.query_row("SELECT 1 FROM expired_events WHERE cid=?1 LIMIT 1",[cid],|_|Ok(())).optional()?.is_some();
    if let Some(last)=last_sequence{
        let count:u64=db.query_row("SELECT COUNT(DISTINCT json_extract(payload,'$.sequence')) FROM events WHERE cid=?1 AND json_extract(payload,'$.event')='commandOutput' AND json_extract(payload,'$.sequence') BETWEEN 0 AND ?2",params![cid,last],|r|r.get(0))?;
        gap |= count != last.saturating_add(1);
    }
    if gap || expired {result.capture="partial".into();if !finished {result.outcome="unknown".into();}}
    Ok(result)
}

fn utc(text:&str)->Result<String>{
    if text.trim().is_empty(){return Ok(String::new());}
    Ok(DateTime::parse_from_rfc3339(text.trim())?.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Millis,true))
}

fn stream_match(db:&Connection,cid:&str,needle:&str)->Result<bool>{
    let mut st=db.prepare("SELECT json_extract(payload,'$.stream'),json_extract(payload,'$.text') FROM events WHERE cid=?1 AND json_extract(payload,'$.event')='commandOutput' ORDER BY json_extract(payload,'$.sequence'),rowid")?;
    let mut rows=st.query([cid])?;
    let mut tails=BTreeMap::<String,String>::new();
    while let Some(row)=rows.next()?{
        let tail=tails.entry(row.get(0)?).or_default();
        tail.push_str(&row.get::<_,String>(1)?.to_lowercase());
        if tail.contains(needle){return Ok(true);}
        let mut keep=tail.len().saturating_sub(needle.len().saturating_sub(1));
        while !tail.is_char_boundary(keep){keep+=1;}
        tail.drain(..keep);
    }
    Ok(false)
}

pub fn search(db:&Connection,file:Option<i64>,f:&Filter,before:i64,limit:usize)->Result<Vec<Summary>>{
    ensure!((1..=500).contains(&limit),"page size must be 1–500");
    let from=utc(&f.from)?;let until=utc(&f.until)?;
    ensure!(from.is_empty()||until.is_empty()||from<until,"start must be before end");
    let needle=f.output.to_lowercase();let command=f.command.to_lowercase();
    let mut st=db.prepare("SELECT cid,MIN(rowid) AS first_id FROM events WHERE (?1 IS NULL OR cid IN (SELECT cid FROM events WHERE chunk=?1)) GROUP BY cid HAVING first_id < ?2 ORDER BY first_id DESC")?;
    let mut rows=st.query(params![file,before])?;
    let mut result=Vec::new();
    while let Some(row)=rows.next()?{
        let s=summary(db,&row.get::<_,String>(0)?,row.get(1)?)?;
        if !f.agent.is_empty() && !f.agent.iter().any(|a|a.eq_ignore_ascii_case(&s.app)) {continue;}
        if f.exit_code.is_some() && f.exit_code != s.exit_code {continue;}
        if !s.command.to_lowercase().contains(&command) || (!f.outcome.is_empty()&&s.outcome!=f.outcome) || (!f.capture.is_empty()&&s.capture!=f.capture){continue;}
        let time=utc(&s.time)?;
        if (!from.is_empty() && time<from) || (!until.is_empty() && time>=until){continue;}
        if !needle.is_empty() && !stream_match(db,&s.id,&needle)? {continue;}
        result.push(s);if result.len()==limit {break;}
    }
    Ok(result)
}

#[derive(Debug,Clone,Serialize)]
pub struct Detail {pub files:Vec<String>,pub lines:Vec<String>,pub has_more:bool}
pub fn detail(db:&Connection,cid:&str,page:usize)->Result<Detail>{
    let mut st=db.prepare("SELECT DISTINCT c.day,c.version FROM events e JOIN chunks c ON c.id=e.chunk WHERE e.cid=?1 ORDER BY c.day,c.version")?;
    let files=st.query_map([cid],|r|Ok(filename(&r.get::<_,String>(0)?,r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut st=db.prepare("SELECT payload FROM events WHERE cid=?1 AND json_extract(payload,'$.event') IN ('commandOutput','captureGap') ORDER BY CASE WHEN json_extract(payload,'$.sequence') IS NULL THEN 1 ELSE 0 END,json_extract(payload,'$.sequence'),rowid LIMIT 33 OFFSET ?2")?;
    let mut rows=st.query(params![cid,page.saturating_mul(32) as i64])?;
    let mut lines=Vec::new();
    while let Some(row)=rows.next()?{
        let e:Event=serde_json::from_str(&row.get::<_,String>(0)?)?;
        match e.body {
            Body::CommandOutput{stream,text,..}=>lines.push(format!("[{}] {}",stream.name(),visible(&text))),
            Body::CaptureGap{reason}=>lines.push(format!("[capture gap] {}",visible(&reason))),
            _=>{}
        }
    }
    let has_more=lines.len()>32;lines.truncate(32);
    Ok(Detail{files,lines,has_more})
}

pub fn visible(text:&str)->String{
    text.chars().map(|c|if c.is_control() && !matches!(c,'\n'|'\r'|'\t') {'�'}else{c}).collect()
}

pub fn health(db:&Connection)->Result<String>{
    let get=|key:&str|->rusqlite::Result<Option<String>>{db.query_row("SELECT value FROM health WHERE name=?1",[key],|r|r.get(0)).optional()};
    let mut state=get("state")?.unwrap_or_else(||"waiting for collector".into());
    let beat=get("heartbeat")?.unwrap_or_default();
    if state=="running" && DateTime::parse_from_rfc3339(&beat).map_or(true,|t|(Utc::now()-t.with_timezone(&Utc)).num_seconds()>20) {state="offline (stale heartbeat)".into();}
    if let Some(bad)=get("lastRejected")? {state.push_str(&format!(" · last invalid event {bad}"));}
    if let Some(loss)=get("lastQueueLoss")? {state.push_str(&format!(" · queue loss reported {loss}"));}
    if let Some(fail)=get("lastReadFailure")? {state.push_str(&format!(" · last unreadable queue entry {fail}"));}
    Ok(format!("{state}  {beat}"))
}

pub fn sources(db:&Connection)->Result<Vec<(String,String,String,i64)>> {
    let mut stmt=db.prepare("SELECT json_extract(payload,'$.agentApp'),json_extract(payload,'$.source'),MAX(COALESCE(json_extract(payload,'$.observedUtc'),json_extract(payload,'$.timeUtc'))),COUNT(DISTINCT cid) FROM events GROUP BY 1,2 ORDER BY 3 DESC")?;
    let rows=stmt.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}
