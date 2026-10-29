//! Records every command as a pair of console events for one repository.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_specta::Event;

use crate::error::GitError;
use crate::transport::{display_command, Category, Cmd, Output};

/// `console.logInternal`, for every repository at once: whether the commands the app runs by
/// itself are logged. Set from the settings when they load and each time they are saved.
static LOG_INTERNAL: AtomicBool = AtomicBool::new(false);

pub fn set_log_internal(on: bool) {
    LOG_INTERNAL.store(on, Ordering::Relaxed);
}

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
    /// What the command does, in a few words ("Fetch", "Abort rebase", "Delete remote branch"),
    /// worked out from its arguments by `command_title`. The console shows it before `command`.
    pub title: String,
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
    /// False for an internal command while `console.logInternal` is off: neither of its events is
    /// sent. Decided when it starts, so a command is never left half logged.
    logged: bool,
    seq: u64,
    started: Instant,
    started_at_ms: u64,
    category: Category,
    label: String,
    argv: Vec<String>,
    display: String,
    command: String,
    title: String,
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
/// as Git leaves it out of its own messages. An `http` or `https` URL with a user name and no
/// password (`https://ghp_token@host/…`) is how an access token is usually given, so that user
/// name is hidden too. The rest, an SSH user name such as `git@` included, stays as it is.
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
            None => match authority.rfind('@') {
                Some(at) if is_http(head) => {
                    out.push_str("***");
                    out.push_str(&authority[at..]);
                }
                _ => out.push_str(authority),
            },
        }
        rest = &tail[authority.len()..];
    }
    out.push_str(rest);
    out
}

/// Whether the URL whose `scheme://` ends `head` is `http` or `https`.
fn is_http(head: &str) -> bool {
    let lower = head.to_ascii_lowercase();
    // `git+http://` or `xhttp://` is another scheme that happens to end in http.
    ["http://", "https://"].iter().any(|scheme| {
        lower.ends_with(scheme) && !lower[..lower.len() - scheme.len()].ends_with(|c: char| c.is_ascii_alphanumeric() || "+-.".contains(c))
    })
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
        let title = command_title(&summary);
        let ticket = ConsoleTicket {
            logged: cmd.category == Category::UserAction || LOG_INTERNAL.load(Ordering::Relaxed),
            seq,
            started: Instant::now(),
            started_at_ms: now_ms(),
            category: cmd.category,
            label: cmd.label.to_string(),
            argv,
            display,
            command,
            title,
        };
        if !ticket.logged {
            return ticket;
        }
        let ev = ConsoleEvent {
            repo_id: self.repo_id.clone(),
            seq,
            phase: ConsolePhase::Started,
            category: ticket.category,
            label: ticket.label.clone(),
            argv: ticket.argv.clone(),
            display: ticket.display.clone(),
            command: ticket.command.clone(),
            title: ticket.title.clone(),
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
        if !ticket.logged {
            return;
        }
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
            title: ticket.title,
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

/// What a Git command does, in a few words, from the arguments StashPanda gave it (`fetch
/// --progress origin`, `-c merge.conflictStyle=diff3 merge feature`): "Fetch", "Merge", "Abort
/// rebase", "Delete remote branch". Options before the subcommand (`-c <key=value>`, `-C <dir>`)
/// are skipped. A subcommand with no title of its own gets its name with a capital letter.
pub fn command_title(args: &[String]) -> String {
    let mut rest = args.iter().map(String::as_str);
    let sub = loop {
        match rest.next() {
            Some("-c" | "-C" | "--git-dir" | "--work-tree") => {
                rest.next();
            }
            Some(a) if a.starts_with('-') => {}
            Some(a) => break a,
            None => return "Git".to_string(),
        }
    };
    let after: Vec<&str> = rest.collect();
    let has = |flag: &str| after.iter().any(|a| *a == flag || a.strip_prefix(flag).is_some_and(|r| r.starts_with('=')));
    let first = after.iter().find(|a| !a.starts_with('-')).copied();
    // A `+` refspec forces that one update, as `--force` forces them all.
    let forced = has("--force") || has("-f") || has("--force-with-lease") || after.iter().any(|a| a.starts_with('+') && a.contains(':'));
    // `--abort`, `--continue` and `--skip` for the commands that stop part way.
    let step = |name: &str| {
        if has("--abort") {
            format!("Abort {name}")
        } else if has("--continue") {
            format!("Continue {name}")
        } else if has("--skip") {
            format!("Skip a commit in the {name}")
        } else {
            let mut c = name.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    };
    let title = match sub {
        // `git fetch . a:b` moves local branch b to a: a push from this repository into itself.
        "fetch" if first == Some(".") && forced => "Force-move branch",
        "fetch" if first == Some(".") => "Move branch",
        "fetch" => "Fetch",
        "pull" => "Pull",
        "push" if (has("--delete") || has("-d")) && after.iter().any(|a| a.starts_with("refs/tags/")) => "Delete remote tag",
        "push" if has("--delete") || has("-d") => "Delete remote branch",
        "push" if forced => "Force push",
        "push" => "Push",
        "clone" => "Clone",
        "init" => "Create repository",
        "commit" if has("--amend") => "Amend commit",
        "commit" => "Commit",
        "merge" if has("--ff-only") && !has("--abort") && !has("--continue") => "Fast-forward",
        "rebase" | "merge" | "cherry-pick" | "revert" => return step(sub),
        "checkout" | "switch" => "Checkout",
        "reset" => "Reset",
        "stash" => match first {
            Some("pop") => "Pop stash",
            Some("apply") => "Apply stash",
            Some("drop") => "Drop stash",
            Some("list") => "List stashes",
            Some("show") => "Show stash",
            Some("store") => "Store stash",
            Some("create") => "Create stash",
            _ => "Stash",
        },
        "branch" if has("-d") || has("-D") || has("--delete") => "Delete branch",
        "branch" if has("-m") || has("-M") || has("--move") => "Rename branch",
        "branch" if has("--set-upstream-to") || has("-u") => "Set upstream",
        "branch" if has("--unset-upstream") => "Remove upstream",
        "branch" if first.is_none() || has("--list") || has("-l") || has("-a") || has("-r") => "List branches",
        "branch" => "Create branch",
        "tag" if has("-d") || has("--delete") => "Delete tag",
        "tag" if first.is_none() || has("-l") || has("--list") => "List tags",
        "tag" => "Create tag",
        "add" | "update-index" => "Stage",
        "rm" => "Remove files",
        "restore" | "checkout-index" => "Restore files",
        "apply" if has("--cached") => "Stage lines",
        "apply" => "Apply patch",
        "clean" => "Delete untracked files",
        "status" => "Status",
        "log" => "History",
        "show" => "Show commit",
        "diff" | "diff-tree" | "diff-index" | "diff-files" => "Diff",
        "blame" => "Blame",
        "rev-parse" => "Resolve revision",
        "rev-list" => "List commits",
        "merge-base" if has("--is-ancestor") => "Check ancestry",
        "merge-base" => "Find merge base",
        "for-each-ref" | "show-ref" => "List refs",
        "ls-remote" => "List remote refs",
        "symbolic-ref" if after.iter().filter(|a| !a.starts_with('-')).count() > 1 => "Set HEAD",
        "symbolic-ref" => "Read HEAD",
        "update-ref" if has("-d") => "Delete ref",
        "update-ref" => "Move ref",
        "reflog" => "Reflog",
        "config" if has("--get") || has("--get-all") || has("--get-regexp") || has("--list") || has("-l") => "Read config",
        "config" if has("--unset") || has("--unset-all") || has("--remove-section") => "Remove config",
        "config" => "Change config",
        "remote" => "Remotes",
        "cat-file" => "Read objects",
        "ls-files" => "List files",
        "ls-tree" => "List tree",
        "worktree" => "Worktrees",
        "submodule" => "Submodules",
        "check-attr" => "Check attributes",
        "check-ignore" => "Check ignored files",
        "write-tree" => "Write tree",
        "hash-object" => "Store object",
        "version" => "Version",
        other => return step(other),
    };
    title.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_say_what_each_command_does() {
        let title = |line: &str| command_title(&line.split_whitespace().map(String::from).collect::<Vec<_>>());
        assert_eq!(title("fetch --all --progress"), "Fetch");
        assert_eq!(title("commit -F - --amend"), "Amend commit");
        assert_eq!(title("-c merge.conflictStyle=diff3 merge -q feature"), "Merge");
        assert_eq!(title("merge --no-edit --ff-only FETCH_HEAD"), "Fast-forward");
        assert_eq!(title("rebase --onto FETCH_HEAD abc123"), "Rebase");
        assert_eq!(title("rebase --abort"), "Abort rebase");
        assert_eq!(title("cherry-pick --continue"), "Continue cherry-pick");
        assert_eq!(title("push --progress origin --delete refs/heads/x"), "Delete remote branch");
        assert_eq!(title("push --progress origin --delete refs/tags/v1"), "Delete remote tag");
        assert_eq!(title("push --progress origin refs/heads/main:refs/heads/main"), "Push");
        assert_eq!(title("push --progress --force-with-lease=refs/heads/main:abc origin refs/heads/main:refs/heads/main"), "Force push");
        assert_eq!(title("fetch --no-write-fetch-head . refs/heads/main:refs/heads/release"), "Move branch");
        assert_eq!(title("fetch --no-write-fetch-head . +refs/heads/feature:refs/heads/side"), "Force-move branch");
        assert_eq!(title("fetch --progress origin +refs/heads/*:refs/remotes/origin/*"), "Fetch");
        assert_eq!(title("stash pop --index stash@{0}"), "Pop stash");
        assert_eq!(title("stash push -m wip"), "Stash");
        assert_eq!(title("branch -d topic"), "Delete branch");
        assert_eq!(title("branch --set-upstream-to=refs/remotes/origin/x -- x"), "Set upstream");
        assert_eq!(title("config --get push.default"), "Read config");
        assert_eq!(title("config --local push.default current"), "Change config");
        assert_eq!(title("update-ref -m fast-forward refs/heads/r abc def"), "Move ref");
        assert_eq!(title("symbolic-ref --quiet HEAD"), "Read HEAD");
        assert_eq!(title("frobnicate --now"), "Frobnicate");
        assert_eq!(title(""), "Git");
    }

    fn out(code: i32, stdout: &str, stderr: &str) -> Output {
        Output { exit_code: Some(code), stdout: stdout.as_bytes().to_vec(), stderr: stderr.as_bytes().to_vec() }
    }

    #[test]
    fn passwords_in_urls_are_hidden() {
        assert_eq!(hide_passwords("https://alice:s3cret@example.com/r.git"), "https://alice:***@example.com/r.git");
        assert_eq!(hide_passwords("remote.origin.url=http://a:b@h:8080/x?y=1"), "remote.origin.url=http://a:***@h:8080/x?y=1");
        // A user name with no password in an http(s) URL is usually an access token.
        assert_eq!(hide_passwords("https://ghp_abc123@github.com/o/r.git"), "https://***@github.com/o/r.git");
        assert_eq!(hide_passwords("url=HTTP://tok@h/x"), "url=HTTP://***@h/x");
        // An SSH user name, a port, an @ in the path, or no URL at all: left as it is.
        for same in ["ssh://git@example.com:2222/r.git", "git+ssh://u@h/r", "xhttp://u@h/r", "https://example.com:8443/a@b", "git@example.com:r.git", "/srv/r.git", ""] {
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
    fn internal_commands_are_logged_only_when_switched_on() {
        let recorder = ConsoleRecorder::new(None, "t".into(), 100);
        let internal = Cmd::git("status", vec!["status".into()]);
        let user = Cmd::git("commit", vec!["commit".into()]).category(Category::UserAction);
        assert!(!recorder.start(&internal, internal.args.clone()).logged);
        assert!(recorder.start(&user, user.args.clone()).logged);
        set_log_internal(true);
        assert!(recorder.start(&internal, internal.args.clone()).logged);
        set_log_internal(false);
    }

    #[test]
    fn successful_internal_output_is_left_out() {
        let (stdout, _, stderr, _, omitted) = kept_output(Category::Internal, Some(&out(0, "abc", "warn")), false, 100);
        assert_eq!((stdout.as_str(), stderr.as_str(), omitted), ("", "", true));
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
        let (stdout, _, _, _, omitted) = kept_output(Category::Internal, Some(&out(0, "abc", "")), true, 100);
        assert_eq!((stdout.as_str(), omitted), ("abc", false));
    }
}
