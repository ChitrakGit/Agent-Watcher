use anyhow::{ensure, Result};
use chrono::{SecondsFormat, Utc};
pub use crate::redact::mask;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn id() -> String {
    Uuid::new_v4().to_string()
}

pub fn stable(parts: &[&str]) -> String {
    let name = serde_json::to_vec(parts).expect("strings serialize");
    Uuid::new_v5(&Uuid::NAMESPACE_OID, &name).to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub schema_version: u8,
    pub event_id: String,
    pub command_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_output_sequence: Option<u64>,
    pub time_utc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_utc: Option<String>,
    pub agent_app: String,
    pub source: String,
    pub attribution: String,
    #[serde(flatten)]
    pub body: Body,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Body {
    CommandRequested { command: String },
    CommandStarted { command: String },
    CommandOutput { sequence: u64, stream: Stream, text: String },
    CommandFinished { outcome: String, exit_code: Option<i32>, output_status: String },
    ProcessObserved { command: String, pid: Option<u32> },
    CaptureGap { reason: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream { Stdout, Stderr, Combined, Terminal }

impl Stream {
    pub fn name(self) -> &'static str {
        match self {
            Self::Stdout => "stdout", Self::Stderr => "stderr",
            Self::Combined => "combined", Self::Terminal => "terminal",
        }
    }
}

#[derive(Clone)]
pub struct Origin {
    pub command_id: String,
    pub app: String,
    pub source: String,
    pub attribution: String,
}

impl Origin {
    pub fn event(&self, body: Body) -> Event {
        Event {
            schema_version: 2, event_id: id(), command_id: self.command_id.clone(), command_preview: None,
            last_output_sequence: None,
            time_utc: now(), observed_utc: None, agent_app: self.app.clone(),
            source: self.source.clone(), attribution: self.attribution.clone(), body,
        }
    }
}

impl Event {
    pub fn sanitize(&mut self) {
        self.command_preview = self.command_preview.as_deref().map(mask);
        self.agent_app = mask(&self.agent_app);
        match &mut self.body {
            Body::CommandStarted { command } | Body::CommandRequested { command } => {
                *command = if self.source == "executor" {
                    match serde_json::from_str::<Vec<String>>(command) {
                        Ok(argv) => serde_json::to_string(&crate::redact::argv(&argv)).expect("argv serializes"),
                        Err(_) => mask(command),
                    }
                } else { mask(command) };
            }
            Body::ProcessObserved { command, .. } => *command = mask(command),
            Body::CommandOutput { text, .. } => *text = mask(text),
            Body::CaptureGap { reason } => *reason = mask(reason),
            Body::CommandFinished { .. } => {}
        }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema_version == 2, "unsupported schema version");
        for value in [&self.event_id, &self.command_id] {
            ensure!(!value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control), "invalid identity");
        }
        ensure!(!self.agent_app.trim().is_empty() && self.agent_app.len() <= 256 && !self.agent_app.chars().any(char::is_control), "invalid app name");
        ensure!(matches!(self.source.as_str(), "executor" | "codex" | "claude" | "antigravity" | "windows-audit" | "collector"), "unknown source");
        ensure!(matches!(self.attribution.as_str(), "reported" | "inferred" | "unknown"), "unknown attribution");
        chrono::DateTime::parse_from_rfc3339(&self.time_utc)?;
        ensure!(self.last_output_sequence.is_none_or(|n| n <= i64::MAX as u64),"invalid final sequence");
        if let Body::CommandFinished { outcome, output_status, .. } = &self.body {
            ensure!(matches!(outcome.as_str(), "succeeded" | "failed" | "denied" | "cancelled" | "launchFailed" | "unknown"), "unknown outcome");
            ensure!(matches!(output_status.as_str(), "complete" | "empty" | "partial" | "unavailable" | "unknown"), "unknown output status");
        }
        if let Body::CommandOutput { sequence, .. } = &self.body {
            ensure!(*sequence <= i64::MAX as u64, "invalid sequence");
        }
        Ok(())
    }
}
