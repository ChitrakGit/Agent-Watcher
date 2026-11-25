#![allow(dead_code)]
use agent_watcher::model::{Body,Event,Origin,Stream};
use chrono::NaiveDate;
pub fn day(s:&str)->NaiveDate{NaiveDate::parse_from_str(s,"%Y-%m-%d").unwrap()}
pub fn origin(cid:&str)->Origin{Origin{command_id:cid.into(),app:"Example Agent".into(),source:"executor".into(),attribution:"reported".into()}}
pub fn start(cid:&str,cmd:&str)->Event{origin(cid).event(Body::CommandStarted{command:cmd.into()})}
pub fn output(cid:&str,n:u64,stream:Stream,text:&str)->Event{origin(cid).event(Body::CommandOutput{sequence:n,stream,text:text.into()})}
pub fn finish(cid:&str,code:i32)->Event{origin(cid).event(Body::CommandFinished{outcome:if code==0{"succeeded"}else{"failed"}.into(),exit_code:Some(code),output_status:"complete".into()})}
