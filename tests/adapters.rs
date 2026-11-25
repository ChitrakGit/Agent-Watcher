mod common;
use agent_watcher::{adapters::native::{antigravity, claude, CodexFeed},collector,model::{Body,Event},query,redact::{self,Redactor},spool,storage::Store};
use serde_json::{json,Value};
use std::fs;
use tempfile::tempdir;

#[test]
fn secrets_mask_across_every_possible_input_boundary(){
    let raw="before API_key=\"a very\nprivate value\"; Bearer token123 after 🦀";
    for cut in 0..=raw.len(){
        let mut r=Redactor::default();let mut out=Vec::new();
        r.write(&raw.as_bytes()[..cut],&mut out);r.write(&raw.as_bytes()[cut..],&mut out);r.finish(&mut out);
        let s=String::from_utf8(out).unwrap();
        assert_eq!(s,"before API_key=[REDACTED]; Bearer [REDACTED] after 🦀");
    }
    assert_eq!(redact::argv(&["--password".into(),"sensitive".into()])[1],"[REDACTED]");
}

#[test]
fn codex_replayed_items_are_idempotent_and_ignore_conversation(){
    let tmp=tempdir().unwrap();
    for _ in 0..2{
        let mut f=CodexFeed::with_run("replayed-run".into());
        for line in include_str!("fixtures/codex.jsonl").lines(){f.handle(tmp.path(),&serde_json::from_str(line).unwrap()).unwrap();}
        f.close(tmp.path()).unwrap();
    }
    let mut s=Store::open(tmp.path()).unwrap();collector::drain(tmp.path(),&mut s).unwrap();s.seal().unwrap();
    let rows=query::search(&s.conn,None,&Default::default(),i64::MAX,10).unwrap();
    assert_eq!(rows.len(),1);assert_eq!(rows[0].app,"Codex CLI");assert_eq!(rows[0].outcome,"failed");
    assert_eq!(rows[0].exit_code,Some(7));assert_eq!(rows[0].capture,"unknown");
    let bytes=fs::read_dir(tmp.path().join("logs")).unwrap().next().unwrap().unwrap().path();
    let raw=fs::read_to_string(bytes).unwrap();assert!(!raw.contains("PRIVATE_CHAT"));assert!(!raw.contains("FAKE_KEY"));
    let events:Vec<Event>=serde_json::from_str(&raw).unwrap();assert_eq!(events.len(),3);
}

#[test]
fn claude_denied_requested_and_missing_output_have_distinct_states(){
    let tmp=tempdir().unwrap();
    let mut v=json!({"session_id":"a","tool_use_id":"b","tool_name":"Bash","tool_input":{"command":"echo hello"},"hook_event_name":"PreToolUse"});
    claude(tmp.path(),&v).unwrap();v["hook_event_name"]=json!("PermissionDenied");claude(tmp.path(),&v).unwrap();
    let mut s=Store::open(tmp.path()).unwrap();collector::drain(tmp.path(),&mut s).unwrap();
    let row=query::search(&s.conn,None,&Default::default(),i64::MAX,10).unwrap().pop().unwrap();
    assert_eq!(row.outcome,"denied");assert_eq!(row.capture,"unavailable");assert_eq!(row.exit_code,None);
    let count:i64=s.conn.query_row("SELECT COUNT(*) FROM events WHERE json_extract(payload,'$.event')='commandStarted'",[],|r|r.get(0)).unwrap();assert_eq!(count,0);
}

#[test]
fn claude_success_and_invalid_spool_entry(){
    let tmp=tempdir().unwrap();
    let v:Value=serde_json::from_str(include_str!("fixtures/claude.json")).unwrap();claude(tmp.path(),&v).unwrap();
    fs::write(spool::inbox(tmp.path()).join("broken.evt"),"broken").unwrap();
    let mut s=Store::open(tmp.path()).unwrap();collector::drain(tmp.path(),&mut s).unwrap();
    assert!(query::health(&s.conn).unwrap().contains("invalid event"));
    let row=query::search(&s.conn,None,&Default::default(),i64::MAX,10).unwrap().pop().unwrap();
    let detail=query::detail(&s.conn,&row.id,0).unwrap();assert_eq!(detail.lines,["[stdout] ok\n","[stderr] warning\n"]);
    let bad=common::origin("bad").event(Body::CommandFinished{outcome:"invented".into(),exit_code:None,output_status:"complete".into()});
    assert!(spool::send(tmp.path(),bad).is_err());
}

#[test]
fn antigravity_run_command_hook_records_success_and_failure() {
    let tmp=tempdir().unwrap();
    let ok=json!({
        "conversationId":"conv-1",
        "stepIdx":4,
        "hookEventName":"PostToolUse",
        "toolCall":{"id":"call-ok","name":"run_command","args":{"CommandLine":"cmd /c echo hi"}}
    });
    antigravity(tmp.path(),&ok).unwrap();
    let fail=json!({
        "conversationId":"conv-1",
        "stepIdx":5,
        "hookEventName":"PostToolUse",
        "error":"exit status 7",
        "toolCall":{"id":"call-fail","name":"run_command","args":{"CommandLine":"cmd /c exit 7"}}
    });
    antigravity(tmp.path(),&fail).unwrap();
    let mut s=Store::open(tmp.path()).unwrap();collector::drain(tmp.path(),&mut s).unwrap();
    let rows=query::search(&s.conn,None,&Default::default(),i64::MAX,10).unwrap();
    assert_eq!(rows.len(),2);
    assert_eq!(rows[0].app,"Antigravity");assert_eq!(rows[0].source,"antigravity");assert_eq!(rows[0].outcome,"failed");assert_eq!(rows[0].exit_code,Some(7));
    assert_eq!(rows[1].app,"Antigravity");assert_eq!(rows[1].outcome,"succeeded");
}
