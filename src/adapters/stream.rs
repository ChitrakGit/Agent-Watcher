use crate::{model::{Body, Origin, Stream},redact::Redactor,spool};
use std::{io::{Read,Write},path::Path,sync::atomic::{AtomicU64,Ordering}};
use anyhow::Result;

/// Drain even when relay or audit storage fails. A child cannot deadlock on an abandoned pipe.
pub(super) fn drain(root:&Path,origin:&Origin,mut input:impl Read,mut relay:impl Write,stream:Stream,seq:&AtomicU64)->Result<(u64,bool)>{
    let mut bytes=0;
    let mut lost=false;
    let mut relay_ok=true;
    let mut mask=Redactor::default();
    let mut clean=Vec::with_capacity(24*1024);
    let mut buf=[0;8192];
    loop{
        let n=input.read(&mut buf)?;
        if n==0{mask.finish(&mut clean);}else{
            bytes+=n as u64;
            if relay_ok{relay_ok=relay.write_all(&buf[..n]).and_then(|_|relay.flush()).is_ok();}
            mask.write(&buf[..n],&mut clean);
        }
        if clean.len()>=16*1024 || n==0 || clean.contains(&b'\n'){
            let mut position=0;
            while position<clean.len(){
                let remaining=&clean[position..];
                let (valid,bad)=match std::str::from_utf8(remaining){
                    Ok(s)=>(s.len(),None),
                    Err(e)=>(e.valid_up_to(),e.error_len()),
                };
                if valid>0{
                    let text=std::str::from_utf8(&remaining[..valid])?;
                    for part in super::chunks(text,24*1024){
                        let event=origin.event(Body::CommandOutput{sequence:seq.fetch_add(1,Ordering::Relaxed),stream,text:part.into()});
                        if spool::send(root,event).is_err(){lost=true;}
                    }
                    position+=valid;
                }
                if let Some(bad)=bad {position+=bad;lost=true;}else{break;}
            }
            clean.drain(..position);
        }
        if n==0{lost |= !clean.is_empty();break;}
    }
    if lost{
        let _=super::emit(root,origin,Body::CaptureGap{reason:format!("{} output omitted: invalid UTF-8 or queue write failure",stream.name())});
    }
    Ok((bytes,lost))
}
