mod common;
use agent_watcher::{model::Stream,query::{self,Filter},storage::Store};
use common::*;
use tempfile::tempdir;

#[test]
fn filters_cross_chunks_and_fragments_but_never_streams_or_commands(){
    let tmp=tempdir().unwrap();let mut s=Store::open(tmp.path()).unwrap();let d=day("2026-11-24");
    let mut a=start("first","Build Test");a.time_utc="2026-11-24T12:00:00.000Z".into();
    s.append(a,d).unwrap();s.append(output("first",0,Stream::Stdout,"pré"),d).unwrap();s.seal().unwrap();
    s.append(output("first",1,Stream::Stderr,"error"),d).unwrap();
    s.append(output("first",2,Stream::Stdout,"FIX"),d).unwrap();s.append(finish("first",7),d).unwrap();
    let mut b=start("other","Build Test");b.agent_app="Other Agent".into();s.append(b,d).unwrap();
    let f=Filter{agent:vec!["example agent".into()],command:"build".into(),output:"PRÉFIX".into(),outcome:"failed".into(),capture:"complete".into(),
        from:"2026-11-24T17:30:00+05:30".into(),until:"2026-11-24T12:00:01Z".into(),exit_code:Some(7)};
    let files=query::files(&s.conn).unwrap();
    let result=query::search(&s.conn,Some(files[1].id),&f,i64::MAX,10).unwrap();assert_eq!(result.len(),1);assert_eq!(result[0].id,"first");
    let f=Filter{output:"préerror".into(),..Default::default()};
    assert!(query::search(&s.conn,None,&f,i64::MAX,10).unwrap().is_empty());
    let details=query::detail(&s.conn,"first",0).unwrap();assert_eq!(details.files.len(),2);assert_eq!(details.lines.len(),3);
}

#[test]
fn cursors_are_stable_and_bad_time_ranges_fail(){
    let tmp=tempdir().unwrap();let mut s=Store::open(tmp.path()).unwrap();let d=day("2026-11-24");
    for i in 0..4{s.append(start(&i.to_string(),"same"),d).unwrap();}
    let first=query::search(&s.conn,None,&Filter::default(),i64::MAX,2).unwrap();
    s.append(start("later","same"),d).unwrap();
    let next=query::search(&s.conn,None,&Filter::default(),first[1].cursor,2).unwrap();
    assert_eq!(first[0].id,"3");assert_eq!(next[0].id,"1");assert_eq!(next[1].id,"0");
    let f=Filter{from:"yesterday".into(),..Default::default()};assert!(query::search(&s.conn,None,&f,i64::MAX,5).is_err());
}
