use serde_json::Value;
use std::{path::Path,process::{Command,Output}};
use tempfile::tempdir;

fn call(root:&Path,args:&[&str])->Output{
    Command::new(env!("CARGO_BIN_EXE_agent-watcher")).arg("--home").arg(root).args(args).output().unwrap()
}

#[test]
fn command_exit_streams_and_offline_file_are_visible_end_to_end(){
    let tmp=tempdir().unwrap();
    #[cfg(windows)]
    let args=["run","--agent","Fixture agent","--","cmd.exe","/d","/c","echo hello & echo problem 1>&2 & exit /b 7"];
    #[cfg(not(windows))]
    let args=["run","--agent","Fixture agent","--","sh","-c","echo hello; echo problem >&2; exit 7"];
    let child=call(tmp.path(),&args);
    assert_eq!(child.status.code(),Some(7));assert!(String::from_utf8_lossy(&child.stdout).contains("hello"));
    assert!(String::from_utf8_lossy(&child.stderr).contains("problem"));
    let collected=call(tmp.path(),&["collect","--once"]);assert!(collected.status.success(),"{:?}",collected.stderr);
    let list:Value=serde_json::from_slice(&call(tmp.path(),&["files"]).stdout).unwrap();assert_eq!(list.as_array().unwrap().len(),1);
    let name=list[0]["name"].as_str().unwrap();assert!(tmp.path().join("logs").join(name).is_file());
    let result=call(tmp.path(),&["search","--file",name,"--outcome","failed","--agent","Fixture agent","--output","PROBLEM"]);
    let rows:Value=serde_json::from_slice(&result.stdout).unwrap();assert_eq!(rows.as_array().unwrap().len(),1);assert_eq!(rows[0]["exit_code"],7);
    let view=call(tmp.path(),&["show",rows[0]["id"].as_str().unwrap()]);let text=String::from_utf8(view.stdout).unwrap();
    assert!(text.contains("[stderr]"));assert!(text.contains("[stdout]"));
}

#[test]
fn failed_launch_has_no_output_or_exit_code(){
    let tmp=tempdir().unwrap();
    assert!(!call(tmp.path(),&["run","--agent","test","--","nonexistent-agentwatcher-fixture-001"]).status.success());
    assert!(call(tmp.path(),&["collect","--once"]).status.success());
    let result=call(tmp.path(),&["search","--outcome","launchFailed"]);
    let rows:Value=serde_json::from_slice(&result.stdout).unwrap();assert_eq!(rows[0]["capture"],"unavailable");assert!(rows[0]["exit_code"].is_null());
}

#[test]
fn raw_output_reaches_caller_and_durable_files_receive_masked_text(){
    let root=tempdir().unwrap();
    #[cfg(windows)]
    let args=["run","--agent","fixture","--","cmd.exe","/d","/c","echo password=FIXTURE_PRIVATE_VALUE"];
    #[cfg(not(windows))]
    let args=["run","--agent","fixture","--","sh","-c","echo password=FIXTURE_PRIVATE_VALUE"];
    let out=call(root.path(),&args);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("FIXTURE_PRIVATE_VALUE"));
    for e in std::fs::read_dir(root.path().join("inbox")).unwrap(){
        let p=e.unwrap().path();
        if p.extension().is_some_and(|v|v=="evt"){
            assert!(!std::fs::read_to_string(p).unwrap().contains("FIXTURE_PRIVATE_VALUE"));
        }
    }
    assert!(call(root.path(),&["collect","--once"]).status.success());
    for e in std::fs::read_dir(root.path().join("logs")).unwrap(){
        let data=std::fs::read_to_string(e.unwrap().path()).unwrap();
        assert!(!data.contains("FIXTURE_PRIVATE_VALUE"));assert!(data.contains("[REDACTED]"));
    }
}
