use crate::{model::{stable, Body, Event, Origin}, spool};
use anyhow::{Context, Result};
use std::{fs, path::Path, sync::atomic::{AtomicBool, Ordering}, thread, time::Duration};

const SHELL_EXES: &[&str] = &["cmd.exe", "powershell.exe", "pwsh.exe", "bash.exe", "sh.exe", "wsl.exe"];

#[derive(Debug, Default, PartialEq, Eq)]
pub struct AuditRecord {
    pub record_id: u64,
    pub computer: String,
    pub time_utc: String,
    pub subject_user_sid: String,
    pub target_user_sid: String,
    pub new_process_id: Option<u32>,
    pub new_process_name: String,
    pub command_line: String,
    pub parent_process_name: String,
}

fn exe_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

fn unescape_xml(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_owned();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        if let Some(semi) = after.find(';') {
            let entity = &after[..semi];
            let ch = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                    u32::from_str_radix(&entity[2..], 16).ok().and_then(char::from_u32)
                }
                _ if entity.starts_with('#') => entity[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            if let Some(c) = ch {
                out.push(c);
                rest = &after[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn extract_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(&xml[start..end])
}

fn extract_attr<'a>(xml: &'a str, attr: &str) -> Option<&'a str> {
    for quote in ['\'', '"'] {
        let needle = format!("{attr}={quote}");
        if let Some(pos) = xml.find(&needle) {
            let start = pos + needle.len();
            if let Some(end) = xml[start..].find(quote) {
                return Some(&xml[start..start + end]);
            }
        }
    }
    None
}

pub fn parse_event_xml(xml: &str) -> Option<AuditRecord> {
    let record_id = extract_tag(xml, "EventRecordID")?.trim().parse::<u64>().ok()?;
    let time_utc = extract_attr(xml, "SystemTime")?.trim().to_owned();
    let computer = unescape_xml(extract_tag(xml, "Computer").unwrap_or("").trim());
    let mut rec = AuditRecord {
        record_id,
        computer,
        time_utc,
        ..Default::default()
    };

    let mut pos = 0;
    while let Some(idx) = xml[pos..].find("<Data") {
        let tag_start = pos + idx;
        let Some(gt) = xml[tag_start..].find('>') else { break };
        let header = &xml[tag_start..tag_start + gt];
        let val_start = tag_start + gt + 1;
        if header.ends_with('/') {
            pos = val_start;
            continue;
        }
        let Some(end) = xml[val_start..].find("</Data>") else { break };
        let value = unescape_xml(&xml[val_start..val_start + end]);
        match extract_attr(header, "Name") {
            Some("SubjectUserSid") => rec.subject_user_sid = value,
            Some("TargetUserSid") => rec.target_user_sid = value,
            Some("NewProcessId") => {
                let hex = value.trim().trim_start_matches("0x").trim_start_matches("0X");
                rec.new_process_id = u32::from_str_radix(hex, 16).ok();
            }
            Some("NewProcessName") => rec.new_process_name = value,
            Some("CommandLine") => rec.command_line = value,
            Some("ParentProcessName") => rec.parent_process_name = value,
            _ => {}
        }
        pos = val_start + end + "</Data>".len();
    }
    Some(rec)
}

fn friendly_app(parent_path: &str) -> String {
    let lower_path = parent_path.to_ascii_lowercase();
    let base = exe_name(parent_path);
    let lower_base = base.to_ascii_lowercase();
    if lower_path.contains("antigravity")
        || (lower_base.starts_with("language_server") && lower_path.contains(".gemini"))
    {
        return "Antigravity".into();
    }
    match lower_base.as_str() {
        "claude.exe" => "Claude Code".into(),
        "codex.exe" => "Codex CLI".into(),
        "cursor.exe" => "Cursor".into(),
        "windsurf.exe" => "Windsurf".into(),
        "code.exe" => "VS Code".into(),
        "" => "Windows shell".into(),
        _ => base.strip_suffix(".exe").or_else(|| base.strip_suffix(".EXE")).unwrap_or(base).to_owned(),
    }
}

fn is_self_process(path: &str) -> bool {
    matches!(
        exe_name(path).to_ascii_lowercase().as_str(),
        "agent-watcher.exe" | "agent-watcher-view.exe"
    )
}

fn has_command_args(cmd: &str, exe_path: &str) -> bool {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return false;
    }
    let rest = if let Some(after_q) = trimmed.strip_prefix('"') {
        after_q.split_once('"').map_or("", |(_, r)| r.trim())
    } else {
        trimmed.split_once(char::is_whitespace).map_or("", |(_, r)| r.trim())
    };
    !rest.is_empty() && !trimmed.eq_ignore_ascii_case(exe_path.trim())
}

pub fn resolve_observation(rec: &AuditRecord, owner_sid: &str, agent_exes: &[String]) -> Option<Event> {
    let identity = if !rec.target_user_sid.is_empty() && rec.target_user_sid != "S-1-0-0" {
        &rec.target_user_sid
    } else {
        &rec.subject_user_sid
    };
    if !identity.eq_ignore_ascii_case(owner_sid) {
        return None;
    }
    if is_self_process(&rec.new_process_name)
        || is_self_process(&rec.parent_process_name)
        || rec.command_line.to_ascii_lowercase().contains("agent-watcher.exe")
    {
        return None;
    }

    let new_base = exe_name(&rec.new_process_name).to_ascii_lowercase();
    let is_shell = SHELL_EXES.contains(&new_base.as_str());
    let matches_agent = agent_exes
        .iter()
        .any(|a| !a.is_empty() && rec.parent_process_name.eq_ignore_ascii_case(a));

    if !is_shell && !matches_agent {
        return None;
    }
    if is_shell
        && !matches_agent
        && exe_name(&rec.parent_process_name).eq_ignore_ascii_case("explorer.exe")
        && !has_command_args(&rec.command_line, &rec.new_process_name)
    {
        return None;
    }

    let command = if rec.command_line.trim().is_empty() {
        rec.new_process_name.clone()
    } else {
        rec.command_line.clone()
    };
    if command.trim().is_empty() {
        return None;
    }

    let key = format!("{}|{}|{}", rec.computer, rec.record_id, rec.time_utc);
    let origin = Origin {
        command_id: stable(&["windows-audit", &key]),
        app: friendly_app(&rec.parent_process_name),
        source: "windows-audit".into(),
        attribution: "inferred".into(),
    };
    let mut event = origin.event(Body::ProcessObserved {
        command,
        pid: rec.new_process_id,
    });
    event.event_id = stable(&[&key, "observation"]);
    if chrono::DateTime::parse_from_rfc3339(&rec.time_utc).is_ok() {
        event.time_utc = rec.time_utc.clone();
    }
    Some(event)
}

fn save_bookmark(path: &Path, record_id: u64) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("txt.tmp");
    fs::write(&tmp, record_id.to_string())?;
    fs::rename(&tmp, path).or_else(|_| {
        let _ = fs::remove_file(path);
        fs::rename(&tmp, path)
    })?;
    Ok(())
}

#[cfg(windows)]
mod winlog {
    use super::parse_event_xml;
    use anyhow::{ensure, Result};
    use std::ffi::c_void;

    type EvtHandle = isize;
    const EVT_QUERY_CHANNEL_PATH: u32 = 0x1;
    const EVT_QUERY_FORWARD_DIRECTION: u32 = 0x100;
    const EVT_QUERY_REVERSE_DIRECTION: u32 = 0x200;
    const EVT_RENDER_EVENT_XML: u32 = 1;
    const ERROR_INSUFFICIENT_BUFFER: i32 = 122;
    const ERROR_NO_MORE_ITEMS: i32 = 259;

    #[link(name = "wevtapi")]
    extern "system" {
        fn EvtQuery(session: EvtHandle, path: *const u16, query: *const u16, flags: u32) -> EvtHandle;
        fn EvtNext(result_set: EvtHandle, events_size: u32, events: *mut EvtHandle, timeout: u32, flags: u32, returned: *mut u32) -> i32;
        fn EvtRender(context: EvtHandle, fragment: EvtHandle, flags: u32, buffer_size: u32, buffer: *mut c_void, buffer_used: *mut u32, property_count: *mut u32) -> i32;
        fn EvtClose(object: EvtHandle) -> i32;
    }

    struct Handle(EvtHandle);
    impl Drop for Handle {
        fn drop(&mut self) {
            if self.0 != 0 {
                unsafe { EvtClose(self.0); }
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn render_xml(event: EvtHandle) -> Result<String> {
        let mut buf = vec![0u16; 4096];
        let mut used = 0u32;
        let mut props = 0u32;
        let ok = unsafe {
            EvtRender(0, event, EVT_RENDER_EVENT_XML, (buf.len() * 2) as u32, buf.as_mut_ptr() as *mut c_void, &mut used, &mut props)
        };
        if ok == 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER) {
                return Err(err.into());
            }
            buf.resize((used as usize + 1) / 2, 0);
            let ok2 = unsafe {
                EvtRender(0, event, EVT_RENDER_EVENT_XML, (buf.len() * 2) as u32, buf.as_mut_ptr() as *mut c_void, &mut used, &mut props)
            };
            ensure!(ok2 != 0, "EvtRender failed: {}", std::io::Error::last_os_error());
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Ok(String::from_utf16_lossy(&buf[..len]))
    }

    pub fn latest_record_id() -> Result<u64> {
        let channel = wide("Security");
        let query = wide("*");
        let raw = unsafe { EvtQuery(0, channel.as_ptr(), query.as_ptr(), EVT_QUERY_CHANNEL_PATH | EVT_QUERY_REVERSE_DIRECTION) };
        ensure!(raw != 0, "open Security log: {}", std::io::Error::last_os_error());
        let results = Handle(raw);
        let mut event = 0isize;
        let mut returned = 0u32;
        let ok = unsafe { EvtNext(results.0, 1, &mut event, 0, 0, &mut returned) };
        if ok == 0 || returned == 0 {
            return Ok(0);
        }
        let _ev = Handle(event);
        let xml = render_xml(event)?;
        Ok(parse_event_xml(&xml).map_or(0, |r| r.record_id))
    }

    pub fn query_since(after_record_id: u64, max_events: usize) -> Result<Vec<super::AuditRecord>> {
        let channel = wide("Security");
        let xpath = wide(&format!("*[System[(EventID=4688) and (EventRecordID > {after_record_id})]]"));
        let raw = unsafe { EvtQuery(0, channel.as_ptr(), xpath.as_ptr(), EVT_QUERY_CHANNEL_PATH | EVT_QUERY_FORWARD_DIRECTION) };
        ensure!(raw != 0, "query Security 4688: {}", std::io::Error::last_os_error());
        let results = Handle(raw);
        let mut out = Vec::new();
        let mut batch = [0isize; 64];
        while out.len() < max_events {
            let want = batch.len().min(max_events - out.len()) as u32;
            let mut returned = 0u32;
            let ok = unsafe { EvtNext(results.0, want, batch.as_mut_ptr(), 0, 0, &mut returned) };
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(ERROR_NO_MORE_ITEMS) {
                    break;
                }
                return Err(err.into());
            }
            for &h in &batch[..returned as usize] {
                let _ev = Handle(h);
                if let Ok(xml) = render_xml(h) {
                    if let Some(rec) = parse_event_xml(&xml) {
                        out.push(rec);
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(windows)]
pub fn watch(root: &Path, owner_sid: &str, agent_exes: &[String], stop: &AtomicBool) -> Result<()> {
    let bookmark_path = root.join("audit-bookmark.txt");
    let mut last_id = match fs::read_to_string(&bookmark_path) {
        Ok(s) => s.trim().parse::<u64>().unwrap_or(0),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let id = winlog::latest_record_id().context("read initial Security log record")?;
            save_bookmark(&bookmark_path, id)?;
            id
        }
        Err(e) => return Err(e.into()),
    };

    while !stop.load(Ordering::Relaxed) {
        let step = (|| -> Result<usize> {
            let latest = winlog::latest_record_id()?;
            if latest < last_id {
                last_id = 0;
                save_bookmark(&bookmark_path, last_id)?;
            }
            let events = winlog::query_since(last_id, 256)?;
            let count = events.len();
            for rec in events {
                if let Some(event) = resolve_observation(&rec, owner_sid, agent_exes) {
                    spool::send(root, event)?;
                }
                last_id = rec.record_id;
                save_bookmark(&bookmark_path, last_id)?;
            }
            Ok(count)
        })();
        match step {
            Ok(n) if n >= 256 => continue,
            Ok(_) => {}
            Err(e) => eprintln!("Agent Watcher audit: {e:#}"),
        }
        for _ in 0..10 {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_4688_xml_and_attributes_shells() {
        let xml = r#"<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><TimeCreated SystemTime='2026-11-25T10:15:30.1234567Z'/><EventRecordID>42</EventRecordID><Computer>DEV-PC</Computer></System><EventData><Data Name='SubjectUserSid'>S-1-5-21-1001</Data><Data Name='NewProcessId'>0x1a2b</Data><Data Name='NewProcessName'>C:\Windows\System32\cmd.exe</Data><Data Name='CommandLine'>"C:\Windows\System32\cmd.exe" /d /c echo a &amp; echo b</Data><Data Name='TargetUserSid'>S-1-0-0</Data><Data Name='ParentProcessName'>C:\Users\dev\.antigravity\bin\language_server_windows_x64.exe</Data></EventData></Event>"#;
        let rec = parse_event_xml(xml).unwrap();
        assert_eq!(rec.record_id, 42);
        assert_eq!(rec.new_process_id, Some(0x1a2b));
        assert_eq!(rec.command_line, "\"C:\\Windows\\System32\\cmd.exe\" /d /c echo a & echo b");

        let ev = resolve_observation(&rec, "S-1-5-21-1001", &[]).unwrap();
        assert_eq!(ev.agent_app, "Antigravity");
        assert_eq!(ev.source, "windows-audit");
        match ev.body {
            Body::ProcessObserved { command, pid } => {
                assert_eq!(pid, Some(0x1a2b));
                assert!(command.contains("echo a & echo b"));
            }
            _ => panic!("expected ProcessObserved"),
        }
    }

    #[test]
    fn excludes_agent_watcher_and_other_users() {
        let mut rec = AuditRecord {
            record_id: 99,
            computer: "DEV-PC".into(),
            time_utc: "2026-11-25T10:15:30Z".into(),
            subject_user_sid: "S-1-5-21-1001".into(),
            target_user_sid: "S-1-0-0".into(),
            new_process_id: Some(77),
            new_process_name: r"C:\Windows\System32\cmd.exe".into(),
            command_line: r#"cmd /c "C:\Program Files\AgentWatcher\agent-watcher.exe" antigravity-hook"#.into(),
            parent_process_name: r"C:\Users\dev\.gemini\antigravity\language_server.exe".into(),
        };
        assert!(resolve_observation(&rec, "S-1-5-21-1001", &[]).is_none());

        rec.command_line = r#"cmd /c whoami"#.into();
        assert!(resolve_observation(&rec, "S-1-5-21-9999", &[]).is_none());
        assert!(resolve_observation(&rec, "S-1-5-21-1001", &[]).is_some());
    }
}
