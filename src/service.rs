use anyhow::Result;
use std::{ffi::OsString,path::PathBuf,sync::{atomic::{AtomicBool,Ordering},Arc,OnceLock},time::Duration};
use windows_service::{define_windows_service,service::{ServiceControl,ServiceControlAccept,ServiceExitCode,ServiceState,ServiceStatus,ServiceType},service_control_handler::{self,ServiceControlHandlerResult},service_dispatcher};

static ROOT:OnceLock<PathBuf>=OnceLock::new();
const NAME:&str="AgentWatcher";
define_windows_service!(ffi_main, svc_main);

pub fn dispatch(root:PathBuf)->Result<()>{
    ROOT.set(root).map_err(|_|anyhow::anyhow!("service already initialized"))?;
    service_dispatcher::start(NAME,ffi_main)?;
    Ok(())
}

fn svc_main(_:Vec<OsString>){
    if let Err(e)=serve(){eprintln!("service error: {e}");}
}

fn serve()->Result<()>{
    let stop=Arc::new(AtomicBool::new(false));let signal=stop.clone();
    let handle=service_control_handler::register(NAME,move |control|match control{
        ServiceControl::Stop|ServiceControl::Shutdown=>{signal.store(true,Ordering::Relaxed);ServiceControlHandlerResult::NoError}
        ServiceControl::Interrogate=>ServiceControlHandlerResult::NoError,
        _=>ServiceControlHandlerResult::NotImplemented
    })?;
    let status=|state,code,accept| ServiceStatus{service_type:ServiceType::OWN_PROCESS,current_state:state,controls_accepted:accept,exit_code:ServiceExitCode::Win32(code),checkpoint:0,wait_hint:Duration::default(),process_id:None};
    handle.set_service_status(status(ServiceState::Running,0,ServiceControlAccept::STOP|ServiceControlAccept::SHUTDOWN))?;
    let r=crate::collector::run(ROOT.get().expect("registered path"),&stop,false);
    handle.set_service_status(status(ServiceState::Stopped,if r.is_ok(){0}else{1},ServiceControlAccept::empty()))?;
    r
}
