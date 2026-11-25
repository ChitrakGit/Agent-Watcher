mod common;
use agent_watcher::{model::{now,Body,Event,Stream},query,storage::{parse_filename,Store},FILE_LIMIT};
use common::*;
use std::fs;
use tempfile::tempdir;

#[test]
fn exact_fit_and_one_byte_overflow_measure_serialized_utf8(){
    let date=day("2026-11-24");
    let mut a=start("c","Write-Output \"हैलो\"");a.observed_utc=Some(now());
    let mut b=output("c",0,Stream::Stdout,"💡\n\\\"");b.observed_utc=Some(now());
    b.command_preview=Some("Write-Output \"हैलो\"".into());
    let exact=3+serde_json::to_vec(&a).unwrap().len()+serde_json::to_vec(&b).unwrap().len();
    for (limit,expected) in [(exact,1),(exact-1,2)]{
        let tmp=tempdir().unwrap();let mut s=Store::with_limit(tmp.path(),limit).unwrap();
        s.append(a.clone(),date).unwrap();s.append(b.clone(),date).unwrap();s.seal().unwrap();
        let files=query::files(&s.conn).unwrap();assert_eq!(files.len(),expected);
        for f in files{
            let bytes=fs::read(tmp.path().join("logs").join(&f.name)).unwrap();
            assert!(bytes.len()<=limit);assert_eq!(bytes.len() as u64,f.bytes);
            assert!(!serde_json::from_slice::<Vec<Event>>(&bytes).unwrap().is_empty());
        }
    }
}

#[test]
fn daily_versions_restart_replay_and_clock_rollback(){
    let tmp=tempdir().unwrap();let d=day("2026-11-24");let next=day("2026-11-25");
    let e=start("a","x");let mut store=Store::open(tmp.path()).unwrap();
    assert!(store.append(e.clone(),d).unwrap());drop(store);
    let mut store=Store::open(tmp.path()).unwrap();
    assert!(!store.append(e,next).unwrap());
    store.append(start("b","y"),next).unwrap();
    store.append(start("c","z"),d).unwrap();store.seal().unwrap();
    let names:Vec<_>=query::files(&store.conn).unwrap().into_iter().map(|x|x.name).collect();
    assert_eq!(names,["2026_11_25_0.json","2026_11_24_1.json","2026_11_24_0.json"]);
    assert!(parse_filename("2026_11_24_01.json").is_none());
    assert!(parse_filename("../2026_11_24_0.json").is_none());
}

#[test]
fn single_writer_and_interrupted_publication(){
    let tmp=tempdir().unwrap();let mut store=Store::open(tmp.path()).unwrap();
    assert!(Store::open(tmp.path()).is_err());
    store.append(start("c","a"),day("2026-11-24")).unwrap();
    store.conn.execute("UPDATE chunks SET state='sealing'",[]).unwrap();
    drop(store);
    let store=Store::open(tmp.path()).unwrap();
    let path=tmp.path().join("logs/2026_11_24_0.json");
    let bytes=fs::read(&path).unwrap();
    store.conn.execute("UPDATE chunks SET state='sealing'",[]).unwrap();drop(store);
    let s=Store::open(tmp.path()).unwrap();
    assert_eq!(bytes,fs::read(&path).unwrap());
    s.conn.execute("UPDATE chunks SET state='sealing'",[]).unwrap();drop(s);
    fs::write(&path,b"[]").unwrap();assert!(Store::open(tmp.path()).is_err());
    assert_eq!(fs::read(&path).unwrap(),b"[]");
}

#[test]
fn numeric_version_ten_sorts_after_nine_and_empty_dates_stay_empty(){
    let root=tempdir().unwrap();
    let mut store=Store::open(root.path()).unwrap();
    store.rotate_date(day("2026-11-23")).unwrap();
    assert!(query::files(&store.conn).unwrap().is_empty());
    for n in 0..11{
        store.append(start(&format!("cmd-{n}"),"same"),day("2026-11-24")).unwrap();
        store.seal().unwrap();
    }
    let list=query::files(&store.conn).unwrap();
    assert_eq!(list[0].name,"2026_11_24_10.json");
    assert_eq!(list[1].name,"2026_11_24_9.json");
}

#[test]
fn restore_sealed_files_and_legacy_records_without_modifying_them(){
    let tmp=tempdir().unwrap();let logs=tmp.path().join("logs");fs::create_dir(&logs).unwrap();
    let raw=include_str!("fixtures/legacy.json");
    let p=logs.join("2026_11_24_0.json");fs::write(&p,raw).unwrap();
    let mut store=Store::open(tmp.path()).unwrap();assert_eq!(store.restore().unwrap(),1);
    assert_eq!(store.restore().unwrap(),0);
    let row=query::search(&store.conn,None,&Default::default(),i64::MAX,2).unwrap().pop().unwrap();
    assert_eq!(row.app,"Legacy agent");assert_eq!(row.capture,"complete");assert_eq!(row.exit_code,Some(0));
    assert_eq!(fs::read_to_string(p).unwrap(),raw);
}

#[test]
#[ignore="writes 120 MB; run with cargo test --test storage large_output -- --ignored"]
fn large_output_crosses_real_50mb_chunks_without_loss(){
    let tmp=tempdir().unwrap();let mut s=Store::open(tmp.path()).unwrap();let date=day("2026-11-24");
    s.append(start("large","fixture"),date).unwrap();
    let part="x".repeat(120_000);
    for seq in 0..1000 {s.append(output("large",seq,Stream::Stdout,&part),date).unwrap();}
    s.seal().unwrap();
    let files=query::files(&s.conn).unwrap();assert!(files.len()>=3);
    let mut total=0;let mut expected=0;
    for f in files.into_iter().rev(){
        assert!(f.bytes<=FILE_LIMIT as u64);
        let values:Vec<Event>=serde_json::from_reader(fs::File::open(tmp.path().join("logs").join(f.name)).unwrap()).unwrap();
        for e in values{if let Body::CommandOutput{sequence,text,..}=e.body{assert_eq!(sequence,expected);assert_eq!(text,part);total+=text.len();expected+=1;}}
    }
    assert_eq!(total,120_000_000);assert_eq!(expected,1000);
}
