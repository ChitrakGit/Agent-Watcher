use agent_watcher::{adapters,collector,model::{Body,Origin},query,spool,storage::{self,Store}};
use anyhow::{Context,Result};
use clap::{Parser,Subcommand};
use std::{ffi::OsString,path::PathBuf,sync::{Arc,atomic::{AtomicBool,Ordering}}};

#[derive(Parser)]
#[command(version,about="Open the Agent Watcher viewer, or collect local agent commands")]
struct Cli{
    /// Log root. Defaults to the enrolled Windows log root.
    #[arg(long,global=true,env="AGENT_WATCHER_HOME")] home:Option<PathBuf>,
    #[command(subcommand)] action:Option<Action>
}

#[derive(Subcommand)]
enum Action {
    /// Browse files and filter commands in a native desktop window.
    View,
    /// Continuously process queued events. --once drains, seals, and exits.
    Collect{#[arg(long)]once:bool},
    /// Windows Service Control Manager entry point.
    Service,
    /// Run a command, relay stdout/stderr and keep its exit code.
    Run{#[arg(long)]agent:String,#[arg(last=true,required=true)]args:Vec<OsString>},
    /// Run the native Codex CLI in exec --json mode and observe command items.
    Codex{#[arg(trailing_var_arg=true,allow_hyphen_values=true)]args:Vec<String>},
    /// Receive a documented Claude Code hook on stdin.
    ClaudeHook,
    /// Receive an Antigravity lifecycle hook on stdin.
    AntigravityHook,
    /// Ingest one 4688 observation produced by the optional audit reader.
    AuditEvent,
    /// Continuously watch Windows Event 4688 shell and agent child processes.
    AuditWatch{
        #[arg(long)]owner_sid:String,
        #[arg(long)]agent_exe:Vec<String>,
    },
    /// List active and sealed chunks.
    Files,
    /// Search command summaries. Times require an explicit offset.
    Search{#[arg(long)]file:Option<String>,#[command(flatten)]filter:query::Filter,#[arg(long,default_value_t=100)]limit:usize},
    /// Read one page of a command's captured output.
    Show{id:String,#[arg(long,default_value_t=0)]page:usize},
    /// Collector state and heartbeat.
    Status,
    /// Recover a fresh readable catalog from the sealed JSON files in this root.
    Restore,
}

fn app()->Result<i32>{
    let cli=Cli::parse();
    let root=match cli.home{Some(p)=>p,None=>agent_watcher::paths::default_root()?};
    match cli.action.unwrap_or(Action::View){
        Action::View=>{
            #[cfg(feature="desktop")] {agent_watcher::viewer::open(root)?;}
            #[cfg(not(feature="desktop"))] {anyhow::bail!("rebuild with the desktop feature to open the viewer");}
        }
        Action::Collect{once}=>{
            let stop=Arc::new(AtomicBool::new(false));let s=stop.clone();
            ctrlc::set_handler(move||s.store(true,Ordering::Relaxed))?;
            collector::run(&root,&stop,once)?;
        }
        Action::Service=>{
            #[cfg(windows)] {agent_watcher::service::dispatch(root)?;}
            #[cfg(not(windows))] {anyhow::bail!("the Windows service requires Windows");}
        }
        Action::Run{agent,args}=>return adapters::executor::run(&root,&agent,&args),
        Action::Codex{args}=>return adapters::native::codex(&root,&args),
        Action::ClaudeHook=>{
            // Observer hooks must allow the agent to continue on capture failure.
            if let Err(e)=adapters::native::hook_input(&root,std::io::stdin().lock()) {eprintln!("Agent Watcher hook: {e}");}
        }
        Action::AntigravityHook=>{
            if let Err(e)=adapters::native::antigravity_hook_input(&root,std::io::stdin().lock()) {eprintln!("Agent Watcher hook: {e}");}
            println!("{{}}");
        }
        Action::AuditEvent=>{
            let v:serde_json::Value=serde_json::from_reader(std::io::Read::take(std::io::stdin().lock(),agent_watcher::FRAME_LIMIT as u64))?;
            let field=|k:&str|v[k].as_str().with_context(||format!("missing {k}"));
            let key=field("key")?;
            let origin=Origin{command_id:agent_watcher::model::stable(&["windows-audit",key]),app:field("app")?.into(),source:"windows-audit".into(),attribution:"inferred".into()};
            let mut e=origin.event(Body::ProcessObserved{command:field("command")?.into(),pid:v["pid"].as_u64().and_then(|p|u32::try_from(p).ok())});
            e.event_id=agent_watcher::model::stable(&[key,"observation"]);e.time_utc=field("time")?.into();
            spool::send(&root,e)?;
        }
        Action::AuditWatch{owner_sid,agent_exe}=>{
            #[cfg(windows)] {
                let stop=Arc::new(AtomicBool::new(false));let s=stop.clone();
                ctrlc::set_handler(move||s.store(true,Ordering::Relaxed))?;
                adapters::audit::watch(&root,&owner_sid,&agent_exe,&stop)?;
            }
            #[cfg(not(windows))] {let _=(owner_sid,agent_exe); anyhow::bail!("Windows 4688 audit watcher requires Windows");}
        }
        Action::Files=>println!("{}",serde_json::to_string_pretty(&query::files(&storage::reader(&root)?)?)?),
        Action::Search{file,filter,limit}=>{
            let db=storage::reader(&root)?;
            let id=if let Some(name)=file{Some(query::files(&db)?.into_iter().find(|f|f.name==name).context("file not found")?.id)}else{None};
            println!("{}",serde_json::to_string_pretty(&query::search(&db,id,&filter,i64::MAX,limit)?)?);
        }
        Action::Show{id,page}=>println!("{}",serde_json::to_string_pretty(&query::detail(&storage::reader(&root)?,&id,page)?)?),
        Action::Status=>println!("{}",query::health(&storage::reader(&root)?)?),
        Action::Restore=>{let mut store=Store::open(&root)?;let count=store.restore()?;println!("Imported {count} sealed files.");}
    }
    Ok(0)
}
fn main(){match app(){Ok(code)=>std::process::exit(code),Err(e)=>{eprintln!("Agent Watcher: {e:#}");std::process::exit(1);}}}
