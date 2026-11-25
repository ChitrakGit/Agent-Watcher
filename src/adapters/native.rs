use super::{chunks, output};
use crate::{model::{id, mask, stable, Body, Event, Origin, Stream},spool};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{collections::HashMap,io::{BufRead,BufReader,Read,Write},path::Path,process::{Command,Stdio}};

fn text<'a>(v:&'a Value,key:&str)->Option<&'a str>{v.get(key)?.as_str()}
fn emit_native(root:&Path,origin:&Origin,body:Body,slot:&str) -> Result<()> {
    let mut event=origin.event(body);
    event.event_id=stable(&[&origin.command_id,slot]);
    spool::send(root,event)
}

fn result_parts(root:&Path,origin:&Origin,result:&str,stream:Stream,seq:&mut u64) -> Result<()> {
    output(origin,result,stream,seq,|mut e:Event|{
        if let Body::CommandOutput{sequence,..}=&e.body {e.event_id=stable(&[&origin.command_id,"output",&sequence.to_string()]);}
        spool::send(root,e)
    })
}

fn command(origin:&Origin,root:&Path,cmd:&str,requested:bool) -> Result<()> {
    let clean=mask(cmd);
    // Normal Windows command lines fit comfortably in a frame.
    let preview=chunks(&clean,32*1024).next().unwrap_or("");
    let value=if preview.len()<clean.len(){format!("{preview} … [command truncated]")}else{clean};
    let body=if requested {Body::CommandRequested{command:value}} else {Body::CommandStarted{command:value}};
    emit_native(root,origin,body,if requested {"requested"}else{"start"})
}

pub fn claude(root:&Path, v:&Value) -> Result<()> {
    if !matches!(text(v,"tool_name"),Some("Bash"|"PowerShell")) {return Ok(());}
    let session=text(v,"session_id").context("missing session_id")?;
    let tool=text(v,"tool_use_id").context("missing tool_use_id")?;
    let origin=Origin{command_id:stable(&["claude",session,tool]),app:"Claude Code".into(),source:"claude".into(),attribution:"reported".into()};
    let kind=text(v,"hook_event_name").unwrap_or("");
    let cmd=v.pointer("/tool_input/command").and_then(Value::as_str).unwrap_or("");
    if kind=="PreToolUse" {return command(&origin,root,cmd,true);}
    if kind=="PermissionDenied" {
        return emit_native(root,&origin,Body::CommandFinished{outcome:"denied".into(),exit_code:None,output_status:"unavailable".into()},"finish");
    }
    if !matches!(kind,"PostToolUse"|"PostToolUseFailure") {return Ok(());}
    if !cmd.is_empty(){command(&origin,root,cmd,false)?;}
    let response=&v["tool_response"];
    let mut seq=0;
    if let Some(s)=response.as_str().or_else(||text(v,"error")){
        result_parts(root,&origin,s,Stream::Combined,&mut seq)?;
    }else{
        for (field,stream) in [("stdout",Stream::Stdout),("stderr",Stream::Stderr)] {
            if let Some(s)=text(response,field){result_parts(root,&origin,s,stream,&mut seq)?;}
        }
    }
    let code=response.get("exitCode").or_else(||response.get("exit_code")).and_then(Value::as_i64).and_then(|i|i32::try_from(i).ok());
    let outcome=if v["is_interrupt"]==true {"cancelled"} else if kind=="PostToolUseFailure" || code.is_some_and(|c|c!=0) {"failed"} else if code==Some(0) {"succeeded"}else{"unknown"};
    let status=if response["truncated"]==true {"partial"} else if seq>0 {"unknown"} else {"unavailable"};
    emit_native(root,&origin,Body::CommandFinished{outcome:outcome.into(),exit_code:code,output_status:status.into()},"finish")
}

pub struct CodexFeed { run:String, turn:u64, session:Option<String>, running:HashMap<String,Origin> }

impl Default for CodexFeed {
    fn default()->Self { Self::with_run(id()) }
}

impl CodexFeed {
    pub fn with_run(run:String)->Self{
        Self{run,turn:0,session:None,running:HashMap::new()}
    }
    fn record_gap(&self,root:&Path,reason:&str){
        let gap=||Body::CaptureGap{reason:reason.into()};
        if self.running.is_empty(){
            let origin=Origin{command_id:id(),app:"Codex CLI".into(),source:"codex".into(),attribution:"reported".into()};
            let _=spool::send(root,origin.event(gap()));
        }else{
            for origin in self.running.values(){let _=spool::send(root,origin.event(gap()));}
        }
    }
    pub fn handle(&mut self,root:&Path,v:&Value) -> Result<()> {
        if text(v,"type")==Some("thread.started") {self.session=text(v,"thread_id").map(str::to_owned);return Ok(());}
        if text(v,"type")==Some("turn.started"){self.turn+=1;return Ok(());}
        let item=&v["item"];
        if text(item,"type")!=Some("command_execution") {return Ok(());}
        let sid=self.session.as_deref().context("command before thread ID")?;
        let item_id=text(item,"id").context("missing item ID")?;
        let origin=Origin{command_id:stable(&["codex",&self.run,sid,&self.turn.to_string(),item_id]),app:"Codex CLI".into(),source:"codex".into(),attribution:"reported".into()};
        match text(v,"type"){
            Some("item.started")=>{
                command(&origin,root,text(item,"command").unwrap_or(""),false)?;
                self.running.insert(item_id.to_owned(),origin);
            }
            Some("item.completed")=>{
                if let Some(cmd)=text(item,"command"){command(&origin,root,cmd,false)?;}
                let mut seq=0;
                if let Some(s)=text(item,"aggregated_output"){result_parts(root,&origin,s,Stream::Combined,&mut seq)?;}
                let code=item["exit_code"].as_i64().and_then(|x|i32::try_from(x).ok());
                let outcome=match code{Some(0)=>"succeeded",Some(_)=>"failed",None=>"unknown"};
                let status=if item["truncated"]==true {"partial"}else if item["aggregated_output"].is_string() {"unknown"}else{"unavailable"};
                emit_native(root,&origin,Body::CommandFinished{outcome:outcome.into(),exit_code:code,output_status:status.into()},"finish")?;
                self.running.remove(item_id);
            }
            _=>{}
        }
        Ok(())
    }

    pub fn close(self,root:&Path) -> Result<()> {
        for origin in self.running.values(){
            emit_native(root,origin,Body::CaptureGap{reason:"source ended before command completion".into()},"gap")?;
        }
        Ok(())
    }
}

pub fn codex(root:&Path,args:&[String]) -> Result<i32> {
    let executable=std::env::var_os("AGENT_WATCHER_CODEX_EXE").unwrap_or_else(||"codex".into());
    let mut child=Command::new(executable).args(["exec","--json"]).args(args).stdout(Stdio::piped()).spawn().context("launch native codex exec --json")?;
    let mut input=BufReader::new(child.stdout.take().context("missing Codex output")?);
    let mut feed=CodexFeed::default();
    let mut relay=std::io::stdout().lock();
    let mut line=Vec::new();
    let mut overflow=false;
    loop {
        let buf=input.fill_buf()?;
        if buf.is_empty(){break;}
        let newline=buf.iter().position(|b|*b==b'\n');
        let n=newline.map_or(buf.len(),|p|p+1);
        let _=relay.write_all(&buf[..n]).and_then(|_|relay.flush());
        if !overflow && line.len()+n<=8*1024*1024 {line.extend_from_slice(&buf[..n]);}
        else {overflow=true;line.clear();}
        input.consume(n);
        if newline.is_some(){
            if overflow {feed.record_gap(root,"Codex event exceeded 8 MiB");}
            else if let Ok(v)=serde_json::from_slice(&line) {
                if let Err(e)=feed.handle(root,&v){eprintln!("Agent Watcher capture: {e}");}
            }else{feed.record_gap(root,"Malformed Codex event");}
            line.clear();overflow=false;
        }
    }
    if overflow{feed.record_gap(root,"Final Codex event exceeded 8 MiB");}
    else if !line.is_empty(){
        match serde_json::from_slice(&line){
            Ok(v)=>{if feed.handle(root,&v).is_err(){feed.record_gap(root,"Final Codex event was incomplete");}}
            Err(_)=>feed.record_gap(root,"Malformed final Codex event"),
        }
    }
    let _=feed.close(root);
    Ok(child.wait()?.code().unwrap_or(1))
}

pub fn hook_input(root:&Path, input:impl Read) -> Result<()> {
    let mut bytes=Vec::new();
    input.take(8*1024*1024+1).read_to_end(&mut bytes)?;
    ensure!(bytes.len()<=8*1024*1024,"hook payload too large");
    claude(root,&serde_json::from_slice(&bytes)?)
}

fn parse_exit_status(err:&str)->Option<i32>{
    for marker in ["exit status ","exited with code ","exit code "] {
        if let Some(pos)=err.to_ascii_lowercase().find(marker) {
            let tail=err[pos+marker.len()..].trim_start();
            let num:String=tail.chars().take_while(|c|c.is_ascii_digit() || *c=='-').collect();
            if let Ok(code)=num.parse::<i32>(){return Some(code);}
        }
    }
    None
}

pub fn antigravity(root:&Path, v:&Value) -> Result<()> {
    let call=&v["toolCall"];
    if text(call,"name")!=Some("run_command") {return Ok(());}
    let conv=text(v,"conversationId").unwrap_or("unknown");
    let step=v["stepIdx"].as_i64().map(|n|n.to_string()).unwrap_or_else(||"0".into());
    let tool_id=text(call,"id").unwrap_or(&step);
    let origin=Origin{
        command_id:stable(&["antigravity",conv,tool_id]),
        app:"Antigravity".into(),
        source:"antigravity".into(),
        attribution:"reported".into(),
    };
    let args=&call["args"];
    let cmd=text(args,"CommandLine").or_else(||text(args,"commandLine")).or_else(||text(args,"command")).unwrap_or("");
    let kind=text(v,"hookEventName").unwrap_or("PostToolUse");
    if kind=="PreToolUse" {
        return command(&origin,root,cmd,true);
    }
    if !cmd.is_empty(){command(&origin,root,cmd,false)?;}
    let mut seq=0;
    if let Some(s)=text(v,"stdout") { result_parts(root,&origin,s,Stream::Stdout,&mut seq)?; }
    if let Some(s)=text(v,"stderr") { result_parts(root,&origin,s,Stream::Stderr,&mut seq)?; }
    if let Some(s)=text(v,"output").or_else(||text(&v["toolResult"],"output")) {
        result_parts(root,&origin,s,Stream::Combined,&mut seq)?;
    }
    let err=text(v,"error").filter(|s|!s.trim().is_empty());
    if let Some(msg)=err {
        result_parts(root,&origin,msg,Stream::Stderr,&mut seq)?;
    }
    let code=v.get("exitCode").and_then(Value::as_i64).and_then(|i|i32::try_from(i).ok()).or_else(||err.and_then(parse_exit_status));
    let outcome=if err.is_some() || code.is_some_and(|c|c!=0) {"failed"} else {"succeeded"};
    let status=if seq>0 {"unknown"} else {"unavailable"};
    emit_native(root,&origin,Body::CommandFinished{outcome:outcome.into(),exit_code:code,output_status:status.into()},"finish")
}

pub fn antigravity_hook_input(root:&Path, input:impl Read) -> Result<()> {
    let mut bytes=Vec::new();
    input.take(8*1024*1024+1).read_to_end(&mut bytes)?;
    ensure!(bytes.len()<=8*1024*1024,"hook payload too large");
    antigravity(root,&serde_json::from_slice(&bytes)?)
}
