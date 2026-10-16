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
    /// Just the git arguments the app asked for, shell-quoted (`checkout master`), without the
    /// transport wrapper or the flags every command carries. The console lists this first.
    pub command: String,
    pub started_at_ms: u64,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// True when the command wrote output that this event leaves out: a successful internal or
    /// watcher command keeps only its byte counts (see `kept_output`).
    pub output_omitted: bool,
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
    command: String,
}

fn lossy_capped(bytes: &[u8], cap: usize) -> (String, bool) {
    if bytes.len() <= cap {
        (String::from_utf8_lossy(bytes).into_owned(), false)
    } else {
        (String::from_utf8_lossy(&bytes[..cap]).into_owned(), true)
    }
}

/// The output text a finished command's console event carries: `(stdout, stdout_truncated,
/// stderr, stderr_truncated, omitted)`. A successful internal or watcher command (graph loads,
/// status polls) carries none. Nothing reads the console's copy except the Console panel; the
/// caller has the real output from `GitClient::exec`. Keeping it would cost up to twice `cap` per
/// entry, in the event sent to the WebView and in the Console's buffer of `console.maxLines`
/// entries, for rows that are almost never opened. User commands and failures keep it.
fn kept_output(
    category: Category,
    output: Option<&Output>,
    failed: bool,
    cap: usize,
) -> (String, bool, String, bool, bool) {
    let Some(out) = output else {
        return (String::new(), false, String::new(), false, false);
    };
    if category != Category::UserAction && !failed && out.success() {
        let omitted = !out.stdout.is_empty() || !out.stderr.is_empty();
        return (String::new(), false, String::new(), false, omitted);
    }
    let (stdout, stdout_truncated) = lossy_capped(&out.stdout, cap);
    let (stderr, stderr_truncated) = lossy_capped(&out.stderr, cap);
    // Git writes terminal codes such as `\x1b[K` (clear to the end of the line) even into a pipe
    // in places; the console shows text, where they would come out as stray characters.
    let clean = |t: String| if t.contains('\x1b') { crate::transport::strip_ansi(&t) } else { t };
    (clean(stdout), stdout_truncated, clean(stderr), stderr_truncated, false)
}

/// `arg` with the password of every URL in it (`https://alice:secret@host/…`) replaced by `***`,
/// as Git leaves it out of its own messages. The rest, the user name included, stays as it is.
pub(crate) fn hide_passwords(arg: &str) -> String {
    let mut out = String::with_capacity(arg.len());
    let mut rest = arg;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        let authority = &tail[..tail.find(['/', '?', '#']).unwrap_or(tail.len())];
        match authority.rfind('@').and_then(|at| authority[..at].find(':').map(|colon| (colon, at))) {
            Some((colon, at)) => {
                out.push_str(&authority[..=colon]);
                out.push_str("***");
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = &tail[authority.len()..];
    }
    out.push_str(rest);
    out
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
        // A URL the user typed into the clone dialog can carry a password; the console is on screen.
        let argv: Vec<String> = argv.iter().map(|a| hide_passwords(a)).collect();
        let display = display_command(&argv);
        let summary: Vec<String> = cmd.summary.as_deref().unwrap_or(&cmd.args).iter().map(|a| hide_passwords(a)).collect();
        let command = display_command(&summary);
        let ticket = ConsoleTicket {
            seq,
            started: Instant::now(),
            started_at_ms: now_ms(),
            category: cmd.category,
            label: cmd.label.to_string(),
            argv,
            display,
            command,
        };
        let ev = ConsoleEvent {
            repo_id: self.repo_id.clone(),
            seq,
            phase: ConsolePhase::Started,
            category: ticket.category,
            label: ticket.label.clone(),
            argv: ticket.argv.clone(),
            display: ticket.display.clone(),
            command: ticket.command.clone(),
            started_at_ms: ticket.started_at_ms,
            duration_ms: None,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            stdout_bytes: 0,
            stderr_bytes: 0,
            stdout_truncated: false,
            stderr_truncated: false,
            output_omitted: false,
            error: None,
        };
        if let Some(app) = &self.app {
            let _ = ev.emit(app);
        }
        ticket
    }

    pub fn finish(&self, ticket: ConsoleTicket, output: Option<&Output>, error: Option<&GitError>) {
        self.finish_counted(ticket, output, None, error);
    }

    /// `finish` for a command whose standard output went to the caller piece by piece as it
    /// arrived (`GitClient::exec_streaming`): `output` holds no standard output, and
    /// `stdout_bytes` says how much there was.
    pub fn finish_streamed(&self, ticket: ConsoleTicket, output: Option<&Output>, stdout_bytes: u64, error: Option<&GitError>) {
        self.finish_counted(ticket, output, Some(stdout_bytes), error);
    }

    fn finish_counted(&self, ticket: ConsoleTicket, output: Option<&Output>, streamed_bytes: Option<u64>, error: Option<&GitError>) {
        let (stdout, stdout_truncated, stderr, stderr_truncated, mut output_omitted) =
            kept_output(ticket.category, output, error.is_some(), self.cap);
        let stdout_bytes = streamed_bytes.unwrap_or_else(|| output.map(|o| o.stdout.len() as u64).unwrap_or(0));
        // Streamed output is never kept, so any of it counts as left out.
        if streamed_bytes.is_some_and(|n| n > 0) {
            output_omitted = true;
        }
        let ev = ConsoleEvent {
            repo_id: self.repo_id.clone(),
            seq: ticket.seq,
            phase: ConsolePhase::Finished,
            category: ticket.category,
            label: ticket.label,
            argv: ticket.argv,
            display: ticket.display,
            command: ticket.command,
            started_at_ms: ticket.started_at_ms,
            duration_ms: Some(ticket.started.elapsed().as_millis() as u64),
            exit_code: output.and_then(|o| o.exit_code),
            stdout,
            stderr,
            stdout_bytes,
            stderr_bytes: output.map(|o| o.stderr.len() as u64).unwrap_or(0),
            stdout_truncated,
            stderr_truncated,
            output_omitted,
            error: error.cloned(),
        };
        if let Some(app) = &self.app {
            let _ = ev.emit(app);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(code: i32, stdout: &str, stderr: &str) -> Output {
        Output { exit_code: Some(code), stdout: stdout.as_bytes().to_vec(), stderr: stderr.as_bytes().to_vec() }
    }

    #[test]
    fn passwords_in_urls_are_hidden() {
        assert_eq!(hide_passwords("https://alice:s3cret@example.com/r.git"), "https://alice:***@example.com/r.git");
        assert_eq!(hide_passwords("remote.origin.url=http://a:b@h:8080/x?y=1"), "remote.origin.url=http://a:***@h:8080/x?y=1");
        // No password, a user name only, a port, or no URL at all: left as it is.
        for same in ["ssh://git@example.com:2222/r.git", "https://example.com:8443/a@b", "git@example.com:r.git", "/srv/r.git", ""] {
            assert_eq!(hide_passwords(same), same);
        }
        // What the console shows of a clone with a password in its URL.
        let recorder = ConsoleRecorder::new(None, "t".into(), 100);
        let cmd = Cmd::git("clone", ["clone", "--progress", "--", "https://alice:s3cret@example.com/r.git", "/tmp/r"].map(String::from).to_vec());
        let ticket = recorder.start(&cmd, cmd.args.clone());
        for shown in [&ticket.display, &ticket.command, &ticket.argv.join(" ")] {
            assert!(!shown.contains("s3cret") && shown.contains("alice:***@example.com"), "{shown}");
        }
    }

    #[test]
    fn terminal_codes_are_removed_from_kept_output() {
        let (stdout, _, stderr, _, _) = kept_output(Category::UserAction, Some(&out(0, "a\x1b[Kb", "\x1b[KSuccessfully rebased.")), false, 1024);
        assert_eq!((stdout.as_str(), stderr.as_str()), ("ab", "Successfully rebased."));
    }

    #[test]
    fn successful_internal_and_watcher_output_is_left_out() {
        for category in [Category::Internal, Category::Watcher] {
            let (stdout, _, stderr, _, omitted) = kept_output(category, Some(&out(0, "abc", "warn")), false, 100);
            assert_eq!((stdout.as_str(), stderr.as_str(), omitted), ("", "", true));
        }
        // Nothing written, so nothing was left out.
        assert!(!kept_output(Category::Internal, Some(&out(0, "", "")), false, 100).4);
    }

    #[test]
    fn user_commands_and_failures_keep_their_output() {
        let (stdout, _, _, _, omitted) = kept_output(Category::UserAction, Some(&out(0, "abc", "")), false, 100);
        assert_eq!((stdout.as_str(), omitted), ("abc", false));
        let (_, _, stderr, _, omitted) = kept_output(Category::Internal, Some(&out(128, "", "fatal: x")), false, 100);
        assert_eq!((stderr.as_str(), omitted), ("fatal: x", false));
        // Exit code 0 but classified as an error by the caller.
        let (stdout, _, _, _, omitted) = kept_output(Category::Watcher, Some(&out(0, "abc", "")), true, 100);
        assert_eq!((stdout.as_str(), omitted), ("abc", false));
    }
}
