use super::{emit,stream::drain};
use crate::model::{id, Body, Origin, Stream};
use anyhow::{ensure, Context, Result};
use std::{ffi::OsString, io, path::Path, process::{Command,Stdio}, sync::atomic::AtomicU64, thread};

pub fn run(root:&Path, app:&str, args:&[OsString]) -> Result<i32> {
    ensure!(!args.is_empty(),"missing executable");
    let origin=Origin{command_id:id(),app:app.into(),source:"executor".into(),attribution:"reported".into()};
    let argv:Vec<_>=args.iter().map(|a|a.to_string_lossy().into_owned()).collect();
    // The console also signals the child. Stay alive to drain its final bytes.
    let _ = ctrlc::set_handler(|| {});
    // A JSON argv array preserves the executable and exact argument boundaries.
    let display=serde_json::to_string(&crate::redact::argv(&argv))?;
    let mut lost=emit(root,&origin,Body::CommandRequested{command:display.clone()}).is_err();
    let child=Command::new(&args[0]).args(&args[1..]).stdin(Stdio::inherit()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let mut child=match child {
        Ok(c)=>c,
        Err(e)=>{
            let _=emit(root,&origin,Body::CommandFinished{outcome:"launchFailed".into(),exit_code:None,output_status:"unavailable".into()});
            return Err(e).context("launch command");
        }
    };
    lost |= emit(root,&origin,Body::CommandStarted{command:display}).is_err();
    let stdout=child.stdout.take().context("missing stdout")?;
    let stderr=child.stderr.take().context("missing stderr")?;
    let seq=AtomicU64::new(0);
    let (out,err,status)=thread::scope(|s| -> Result<_> {
        let a=s.spawn(|| drain(root,&origin,stdout,io::stdout().lock(),Stream::Stdout,&seq));
        let b=s.spawn(|| drain(root,&origin,stderr,io::stderr().lock(),Stream::Stderr,&seq));
        let status=child.wait()?;
        Ok((a.join().map_err(|_|anyhow::anyhow!("stdout reader stopped"))??,
            b.join().map_err(|_|anyhow::anyhow!("stderr reader stopped"))??,status))
    })?;
    lost|=out.1 || err.1;
    let state=if lost {"partial"} else if out.0+err.0==0 {"empty"} else {"complete"};
    let outcome=if status.success() {"succeeded"} else if matches!(status.code(),None|Some(130)|Some(-1073741510)) {"cancelled"} else {"failed"};
    let mut finished=origin.event(Body::CommandFinished{outcome:outcome.into(),exit_code:status.code(),output_status:state.into()});
    finished.last_output_sequence=seq.load(std::sync::atomic::Ordering::Relaxed).checked_sub(1);
    if let Err(e)=crate::spool::send(root,finished) {
        eprintln!("Agent Watcher could not record completion: {e}");
    }
    Ok(status.code().unwrap_or(1))
}

