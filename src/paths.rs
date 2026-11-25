use anyhow::Result;
use std::path::PathBuf;

pub fn default_root()->Result<PathBuf>{
    if let Some(home)=std::env::var_os("AGENT_WATCHER_HOME"){return Ok(PathBuf::from(home));}
    #[cfg(windows)]
    if let Some(base)=std::env::var_os("LOCALAPPDATA"){
        let local=PathBuf::from(base).join("AgentWatcher");
        let machine=winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE);
        if let Ok(key)=machine.open_subkey(r"SOFTWARE\AgentWatcher"){
            if let Ok(root)=key.get_value::<String,_>("DataRoot"){return Ok(PathBuf::from(root));}
        }
        return Ok(local);
    }
    Ok(std::env::current_dir()?.join("agent-watcher-data"))
}
