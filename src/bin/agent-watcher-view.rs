#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    #[cfg(feature="desktop")]
    {
        let result=agent_watcher::paths::default_root().and_then(agent_watcher::viewer::open);
        if let Err(error)=result {eprintln!("Agent Watcher viewer: {error:#}");std::process::exit(1);}
    }
    #[cfg(not(feature="desktop"))]
    {eprintln!("build with the desktop feature");std::process::exit(1);}
}
