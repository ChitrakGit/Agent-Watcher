mod common;
use agent_watcher::{model::Stream,query,storage::Store};
use tempfile::tempdir;
#[test]
fn cleanup_keeps_replay_keys_and_version_numbers(){
    let root=tempdir().unwrap();let mut store=Store::open(root.path()).unwrap();
    let date=common::day("2026-11-24");let start=common::start("cmd","example");
    store.append(start.clone(),date).unwrap();store.seal().unwrap();
    store.append(common::output("cmd",0,Stream::Stdout,"part two"),date).unwrap();store.seal().unwrap();
    let newest=query::files(&store.conn).unwrap()[0].clone();
    store.retain(newest.bytes).unwrap();
    assert_eq!(query::files(&store.conn).unwrap().len(),1);
    assert!(!store.append(start,date).unwrap());
    let row=query::search(&store.conn,None,&Default::default(),i64::MAX,10).unwrap().pop().unwrap();
    assert_eq!(row.command,"example");assert_eq!(row.capture,"partial");
    store.append(common::start("new","new"),date).unwrap();
    assert!(query::files(&store.conn).unwrap()[0].name.ends_with("_2.json"));
}
