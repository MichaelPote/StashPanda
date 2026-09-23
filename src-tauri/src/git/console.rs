//! Records every command as a pair of console events for one repository.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_specta::Event;

use crate::error::GitError;
use crate::transport::{display_command, Category, Cmd, Output};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ConsolePhase {
    Started,
    Finished,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleEvent {
    pub repo_id: String,
    pub seq: u64,
    pub phase: ConsolePhase,
    pub category: Category,
    pub label: String,
    /// The host command line exactly as executed (wsl.exe or ssh wrapper included).
    pub argv: Vec<String>,
    /// Shell-quoted single line for display.
    pub display: String,
    pub started_at_ms: u64,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub error: Option<GitError>,
}

pub struct ConsoleRecorder {
    /// None in tests: events are dropped.
    app: Option<AppHandle>,
    repo_id: String,
    seq: AtomicU64,
    cap: usize,
}

pub struct ConsoleTicket {
    seq: u64,
    started: Instant,
    started_at_ms: u64,
    category: Category,
    label: String,
    argv: Vec<String>,
    display: String,
}

fn lossy_capped(bytes: &[u8], cap: usize) -> (String, bool) {
    if bytes.len() <= cap {
        (String::from_utf8_lossy(bytes).into_owned(), false)
    } else {
        (String::from_utf8_lossy(&bytes[..cap]).into_owned(), true)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl ConsoleRecorder {
    pub fn new(app: Option<AppHandle>, repo_id: String, cap: usize) -> ConsoleRecorder {
        ConsoleRecorder {
            app,
            repo_id,
            seq: AtomicU64::new(0),
            cap,
        }
    }

    pub fn start(&self, cmd: &Cmd, argv: Vec<String>) -> ConsoleTicket {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let display = display_command(&argv);
        let ticket = ConsoleTicket {
            seq,
            started: Instant::now(),
            started_at_ms: now_ms(),
            category: cmd.category,
            label: cmd.label.to_string(),
            argv,
            display,
        };
        let ev = ConsoleEvent {
            repo_id: self.repo_id.clone(),
            seq,
            phase: ConsolePhase::Started,
            category: ticket.category,
            label: ticket.label.clone(),
            argv: ticket.argv.clone(),
            display: ticket.display.clone(),
            started_at_ms: ticket.started_at_ms,
            duration_ms: None,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            stdout_bytes: 0,
            stderr_bytes: 0,
            stdout_truncated: false,
            stderr_truncated: false,
            error: None,
        };
        if let Some(app) = &self.app {
            let _ = ev.emit(app);
        }
        ticket
    }

    pub fn finish(&self, ticket: ConsoleTicket, output: Option<&Output>, error: Option<&GitError>) {
        let (stdout, stdout_truncated) = output
            .map(|o| lossy_capped(&o.stdout, self.cap))
            .unwrap_or_default();
        let (stderr, stderr_truncated) = output
            .map(|o| lossy_capped(&o.stderr, self.cap))
            .unwrap_or_default();
        let ev = ConsoleEvent {
            repo_id: self.repo_id.clone(),
            seq: ticket.seq,
            phase: ConsolePhase::Finished,
            category: ticket.category,
            label: ticket.label,
            argv: ticket.argv,
            display: ticket.display,
            started_at_ms: ticket.started_at_ms,
            duration_ms: Some(ticket.started.elapsed().as_millis() as u64),
            exit_code: output.and_then(|o| o.exit_code),
            stdout,
            stderr,
            stdout_bytes: output.map(|o| o.stdout.len() as u64).unwrap_or(0),
            stderr_bytes: output.map(|o| o.stderr.len() as u64).unwrap_or(0),
            stdout_truncated,
            stderr_truncated,
            error: error.cloned(),
        };
        if let Some(app) = &self.app {
            let _ = ev.emit(app);
        }
    }
}
