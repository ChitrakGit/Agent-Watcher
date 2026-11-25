mod common;
use agent_watcher::{collector,model::{Body,Stream},query,spool,storage::Store};
use tempfile::tempdir;

#[test]
fn concurrent_producers_keep_identity_and_output_separate(){
    let root=tempdir().unwrap();
    std::thread::scope(|scope|{
        for app in ["A","B"]{
            let dir=root.path();
            scope.spawn(move||{
                let mut origin=common::origin(app);origin.app=app.into();
                spool::send(dir,origin.event(Body::CommandStarted{command:"same command".into()})).unwrap();
                for sequence in 0..8{
                    spool::send(dir,origin.event(Body::CommandOutput{sequence,stream:Stream::Stdout,text:app.into()})).unwrap();
                }
                let mut end=origin.event(Body::CommandFinished{outcome:"succeeded".into(),exit_code:Some(0),output_status:"complete".into()});
                end.last_output_sequence=Some(7);spool::send(dir,end).unwrap();
            });
        }
    });
    let mut store=Store::open(root.path()).unwrap();assert_eq!(collector::drain(root.path(),&mut store).unwrap(),20);
    for app in ["A","B"]{
        let f=query::Filter{agent:vec![app.into()],output:app.repeat(8),..Default::default()};
        let rows=query::search(&store.conn,None,&f,i64::MAX,5).unwrap();
        assert_eq!(rows.len(),1);assert_eq!(rows[0].app,app);assert_eq!(rows[0].capture,"complete");
    }
}

#[test]
fn finish_before_output_recovers_when_missing_sequence_arrives(){
    let root=tempdir().unwrap();let mut store=Store::open(root.path()).unwrap();let day=common::day("2026-11-24");
    store.append(common::start("a","same"),day).unwrap();
    let mut finish=common::finish("a",0);finish.last_output_sequence=Some(1);
    store.append(finish,day).unwrap();store.append(common::output("a",1,Stream::Stdout,"second"),day).unwrap();
    assert_eq!(query::summary(&store.conn,"a",0).unwrap().capture,"partial");
    store.append(common::output("a",0,Stream::Stdout,"first"),day).unwrap();
    assert_eq!(query::summary(&store.conn,"a",0).unwrap().capture,"complete");
}

#[test]
fn open_command_reports_gap_when_native_stream_ends_early(){
    let root=tempdir().unwrap();
    let mut feed=agent_watcher::adapters::native::CodexFeed::default();
    for line in include_str!("fixtures/codex.jsonl").lines().take(2){
        feed.handle(root.path(),&serde_json::from_str(line).unwrap()).unwrap();
    }
    feed.close(root.path()).unwrap();
    let mut s=Store::open(root.path()).unwrap();collector::drain(root.path(),&mut s).unwrap();
    let row=query::search(&s.conn,None,&Default::default(),i64::MAX,1).unwrap().pop().unwrap();
    assert_eq!(row.capture,"partial");assert_eq!(row.outcome,"unknown");
}
