mod stream;
pub mod audit;
pub mod executor;
pub mod native;

use crate::model::{Body, Event, Origin, Stream};
use crate::spool;
use anyhow::Result;
use std::path::Path;

pub(crate) fn emit(root:&Path, origin:&Origin, body:Body) -> Result<()> {
    spool::send(root,origin.event(body))
}

pub(crate) fn chunks(text:&str, max:usize) -> impl Iterator<Item=&str> {
    let mut rest=text;
    std::iter::from_fn(move || {
        if rest.is_empty() {return None;}
        let mut end=rest.len().min(max.max(4));
        while !rest.is_char_boundary(end) {end-=1;}
        let (piece,remaining)=rest.split_at(end);
        rest=remaining;
        Some(piece)
    })
}

pub(crate) fn output(origin:&Origin, text:&str, stream:Stream, seq:&mut u64, mut write:impl FnMut(Event)->Result<()>) -> Result<()> {
    let clean=crate::model::mask(text);
    for piece in chunks(&clean,24*1024) {
        let event=origin.event(Body::CommandOutput{sequence:*seq,stream,text:piece.to_owned()});
        *seq+=1;
        write(event)?;
    }
    Ok(())
}
