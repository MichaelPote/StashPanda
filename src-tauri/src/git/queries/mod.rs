//! One module per Git query: the command line, and a parser tested on
//! captured bytes. Every query takes the repository's `GitClient`.

pub mod blame;
pub mod blob;
pub mod commit_files;
pub mod conflict_file;
pub mod diff;
pub mod history;
pub mod in_progress;
pub mod log;
pub mod refs;
pub mod status;

use crate::error::Result;
use crate::git::GitClient;

/// The status, with the merge, rebase, cherry-pick or revert in progress read from `git_dir`
/// alongside it.
pub async fn full_status(client: &GitClient, git_dir: &str) -> Result<status::StatusSnapshot> {
    let (status, op) = tokio::join!(status::get_status(client), in_progress::get_in_progress(client, git_dir));
    status.map(|s| status::StatusSnapshot { in_progress: op, ..s })
}

/// The refs and the status together, as every refresh needs them: in one batch where each
/// process start is costly (`Transport::batches`, over WSL), side by side otherwise. The
/// operation in progress is read from `git_dir` at the same time, through the transport's file
/// functions rather than a Git command.
pub async fn refs_and_status(client: &GitClient, git_dir: &str) -> (Result<refs::RefsSnapshot>, Result<status::StatusSnapshot>) {
    if !client.transport.batches() {
        return tokio::join!(refs::get_refs(client), full_status(client, git_dir));
    }
    let mut reqs = refs::refs_requests(client);
    reqs.push(status::status_request(client));
    let (batch, op) = tokio::join!(client.exec_batch(reqs), in_progress::get_in_progress(client, git_dir));
    match batch {
        Ok(outs) => {
            let (refs_outs, status_out) = outs.split_at(outs.len() - 1);
            let status = &status_out[0];
            let status = if status.success() {
                Ok(status::StatusSnapshot { in_progress: op, ..status::parse_status(&status.stdout) })
            } else {
                Err(crate::error::classify_git_failure("status", vec![], status.exit_code, &status.stderr, &client.transport.kind()))
            };
            (refs::refs_from(refs_outs), status)
        }
        Err(e) => (Err(e.clone()), Err(e)),
    }
}

/// End-to-end checks against a throwaway repository through the real local
/// transport. Skipped when `git` is not on PATH.
#[cfg(test)]
mod e2e {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Arc;

    use crate::git::console::ConsoleRecorder;
    use crate::git::versions::Capabilities;
    use crate::git::GitClient;
    use crate::transport::local::LocalTransport;
    use crate::transport::paths::{IdentityMapper, NativePath};
    use crate::transport::SharedTransport;

    fn git(dir: &Path, args: &[&str]) {
        let st = Command::new("git")
            .arg("-C").arg(dir)
            .args(["-c", "user.email=t@t", "-c", "user.name=T", "-c", "commit.gpgsign=false"])
            .args(args)
            .status()
            .expect("git runs");
        assert!(st.success(), "git {:?} failed", args);
    }

    fn fixture() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stashpanda-e2e-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "hello\nworld\n").unwrap();
        std::fs::write(dir.join("bin.dat"), [0u8, 1, 2, 3, 0, 5]).unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "first"]);
        git(&dir, &["mv", "a.txt", "renamed.txt"]);
        std::fs::write(dir.join("renamed.txt"), "hello\nworld\nmore\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "second"]);
        git(&dir, &["tag", "-a", "v1", "-m", "release"]);
        git(&dir, &["checkout", "-qb", "side"]);
        std::fs::write(dir.join("side.txt"), "s\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "side"]);
        git(&dir, &["checkout", "-q", "main"]);
        git(&dir, &["merge", "-q", "--no-ff", "side", "-m", "merge side"]);
        std::fs::write(dir.join("renamed.txt"), "hello\nworld\nmore\nwip\n").unwrap();
        std::fs::write(dir.join("untracked.txt"), "u\n").unwrap();
        dir
    }

    async fn client_for(dir: &Path) -> GitClient {
        let transport: SharedTransport = Arc::new(LocalTransport {
            git: which::which("git").unwrap(),
            mapper: Box::new(IdentityMapper),
            windows_git_from_linux: false,
        });
        let info = transport.probe().await.expect("probe");
        GitClient {
            transport,
            repo: NativePath(dir.to_string_lossy().into_owned()),
            caps: Capabilities::for_version(info.git_version),
            console: Arc::new(ConsoleRecorder::new(None, "test".into(), 4096)),
        }
    }

    /// The refresh's refs and status come out the same from one WSL batch as from separate local
    /// commands. Only inside WSL with interop.
    #[tokio::test]
    async fn refs_and_status_agree_between_batch_and_separate_runs() {
        use crate::transport::paths::WslMapper;
        use crate::transport::wsl::WslTransport;
        let Ok(distro) = std::env::var("WSL_DISTRO_NAME") else { return };
        if which::which("git").is_err() || !Path::new("/mnt/c/Windows/System32/wsl.exe").exists() {
            return;
        }
        let dir = fixture();
        let local = client_for(&dir).await;
        let wsl = GitClient {
            transport: Arc::new(WslTransport { distro: distro.clone(), git: "git".into(), mapper: WslMapper { distro, automount_root: "/mnt/".into() } }),
            ..local.clone()
        };
        assert!(wsl.transport.batches() && !local.transport.batches());
        let git_dir = dir.join(".git").to_string_lossy().into_owned();
        let (lr, ls) = super::refs_and_status(&local, &git_dir).await;
        let (wr, ws) = super::refs_and_status(&wsl, &git_dir).await;
        assert_eq!(lr.unwrap(), wr.unwrap());
        assert_eq!(ls.unwrap(), ws.unwrap());
    }

    #[tokio::test]
    async fn modified_times_through_the_local_transport() {
        if which::which("git").is_err() {
            return;
        }
        let dir = fixture();
        let client = client_for(&dir).await;
        let paths = ["untracked.txt", "renamed.txt", "gone.txt"];
        let full: Vec<NativePath> = paths.iter().map(|p| super::diff::join_repo_path(&client.repo, p)).collect();
        let times = client.transport.modified_times(&full).await.unwrap();
        let on_disk = |name: &str| {
            let t = std::fs::metadata(dir.join(name)).unwrap().modified().unwrap();
            t.duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
        };
        assert_eq!(times, vec![Some(on_disk("untracked.txt")), Some(on_disk("renamed.txt")), None]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stashpanda-{name}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        dir
    }

    /// The `diff --git` lines of a diff, which say which files it is about.
    fn diff_files(text: &str) -> Vec<&str> {
        text.lines().filter(|l| l.starts_with("diff --git ")).collect()
    }

    /// Names that look like pathspec magic or patterns name only that file, in a diff and in the
    /// file history.
    #[tokio::test]
    async fn diff_and_history_take_a_name_literally() {
        use super::diff::{get_diff, DiffOptions, DiffTarget};
        if which::which("git").is_err() {
            return;
        }
        let dir = scratch("literal");
        let names = [":x.txt", "x.txt", "a[1].txt", "a1.txt", "*"];
        for n in names {
            std::fs::write(dir.join(n), "one\n").unwrap();
        }
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "first"]);
        for n in names {
            std::fs::write(dir.join(n), "one\ntwo\n").unwrap();
        }
        let client = client_for(&dir).await;
        let opts = DiffOptions::default();
        for n in [":x.txt", "a[1].txt", "*"] {
            let d = get_diff(&client, &DiffTarget::Unstaged, n, None, &opts).await.unwrap();
            assert_eq!(diff_files(&d.text), [format!("diff --git a/{n} b/{n}")], "{n}");
        }
        git(&dir, &["commit", "-qam", "second"]);
        for n in [":x.txt", "a[1].txt"] {
            let d = get_diff(&client, &DiffTarget::Commit { commit: "HEAD".into(), parent: None }, n, None, &opts).await.unwrap();
            assert_eq!(diff_files(&d.text), [format!("diff --git a/{n} b/{n}")], "{n}");
            let history = super::history::get_file_history(&client, n, None, 10).await.unwrap();
            assert_eq!(history.entries.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), [n, n], "{n}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A copy's diff is the copy's alone, not its source's changes too.
    #[tokio::test]
    async fn diff_of_a_copy_leaves_out_the_source() {
        use super::commit_files::{get_commit_files, ChangeStatus};
        use super::diff::{get_diff, DiffOptions, DiffTarget};
        if which::which("git").is_err() {
            return;
        }
        let dir = scratch("copy");
        let lines: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join("src.txt"), &lines).unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "first"]);
        std::fs::write(dir.join("copy.txt"), format!("{lines}copied\n")).unwrap();
        std::fs::write(dir.join("src.txt"), format!("{lines}extra\n")).unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "copy"]);
        let client = client_for(&dir).await;
        let files = get_commit_files(&client, "HEAD", None).await.unwrap();
        let copy = files.iter().find(|f| f.path == "copy.txt").unwrap();
        assert_eq!((copy.status, copy.old_path.as_deref()), (ChangeStatus::Copied, Some("src.txt")));
        let target = DiffTarget::Commit { commit: "HEAD".into(), parent: None };
        let d = get_diff(&client, &target, "copy.txt", Some("src.txt"), &DiffOptions::default()).await.unwrap();
        assert_eq!(diff_files(&d.text), ["diff --git a/src.txt b/copy.txt"], "{}", d.text);
        assert!(d.text.contains("copy from src.txt\n") && d.text.contains("+copied\n") && !d.text.contains("+extra"), "{}", d.text);
        // A rename keeps its diff, from the old name to the new.
        git(&dir, &["mv", "src.txt", "moved.txt"]);
        let d = get_diff(&client, &DiffTarget::Staged, "moved.txt", Some("src.txt"), &DiffOptions::default()).await.unwrap();
        assert_eq!(diff_files(&d.text), ["diff --git a/src.txt b/moved.txt"], "{}", d.text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Untracked files show as Git would add them, and a nested repository says what it is.
    #[cfg(unix)]
    #[tokio::test]
    async fn untracked_links_executables_and_nested_repositories() {
        use super::diff::{get_diff, DiffNote, DiffOptions, DiffTarget};
        use std::os::unix::fs::PermissionsExt;
        if which::which("git").is_err() {
            return;
        }
        let dir = scratch("untracked");
        std::fs::write(dir.join("keep.txt"), "k\n").unwrap();
        std::os::unix::fs::symlink("keep.txt", dir.join("link.txt")).unwrap();
        std::fs::write(dir.join("run.sh"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(dir.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("text.txt"), "a\r\nBinary files are fine\r\nb").unwrap();
        std::fs::create_dir(dir.join("nested")).unwrap();
        git(&dir.join("nested"), &["init", "-q", "-b", "main"]);
        let client = client_for(&dir).await;
        let status = super::status::get_status(&client).await.unwrap();
        let mut untracked: Vec<&str> = status.entries.iter().filter(|e| e.untracked).map(|e| e.path.as_str()).collect();
        untracked.sort_unstable();
        assert_eq!(untracked, ["keep.txt", "link.txt", "nested/", "run.sh", "text.txt"]);
        let diff = |path: &'static str| {
            let client = &client;
            async move { get_diff(client, &DiffTarget::Untracked, path, None, &DiffOptions::default()).await.unwrap() }
        };
        let link = diff("link.txt").await;
        assert!(link.text.contains("new file mode 120000\n") && link.text.contains("+keep.txt\n\\ No newline at end of file\n"), "{}", link.text);
        assert!(diff("run.sh").await.text.contains("new file mode 100755\n"));
        let text = diff("text.txt").await;
        assert!(!text.is_binary && text.text.contains("@@ -0,0 +1,3 @@\n+a\r\n+Binary files are fine\r\n+b\n\\ No newline at end of file\n"), "{}", text.text);
        assert_eq!(text.new_eol, super::blob::Eol::CrLf);
        let nested = diff("nested/").await;
        assert_eq!((nested.note, nested.text.as_str()), (Some(DiffNote::NestedRepository), ""));
        // A file that went away meanwhile is an error, not an empty diff.
        assert!(get_diff(&client, &DiffTarget::Untracked, "gone.txt", None, &DiffOptions::default()).await.is_err());
        // A name that was not UTF-8 is refused rather than taken for another file.
        let lossy = crate::git::parse::lossy(b"caf\xe9.txt");
        assert!(matches!(get_diff(&client, &DiffTarget::Unstaged, &lossy, None, &DiffOptions::default()).await, Err(crate::error::GitError::InvalidPath { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Settings in the user's Git config that would change what the queries read: none of them
    /// reaches the result.
    #[tokio::test]
    async fn user_config_does_not_change_what_queries_read() {
        use super::diff::{get_diff, DiffNote, DiffOptions, DiffTarget};
        if which::which("git").is_err() {
            return;
        }
        let sub = scratch("config-sub");
        git(&sub, &["commit", "-q", "--allow-empty", "-m", "s1"]);
        let dir = scratch("config");
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        std::fs::write(dir.join(".gitattributes"), "*.txt diff=upper\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["-c", "protocol.file.allow=always", "submodule", "-q", "add", sub.to_str().unwrap(), "sub"]);
        git(&dir, &["commit", "-qm", "first"]);
        git(&dir, &["config", "diff.upper.textconv", "tr a-z A-Z"]);
        git(&dir, &["config", "diff.submodule", "log"]);
        git(&dir, &["config", "blame.ignoreRevsFile", ".git-blame-ignore-revs"]);
        git(&dir, &["config", "log.showSignature", "true"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        let client = client_for(&dir).await;
        let opts = DiffOptions::default();

        // A textconv driver would show `+TWO`, which line staging could not find in the file.
        let d = get_diff(&client, &DiffTarget::Unstaged, "a.txt", None, &opts).await.unwrap();
        assert!(d.text.contains("\n+two\n"), "{}", d.text);
        // The submodule has untracked files only: listed as changed, with nothing to diff.
        std::fs::write(dir.join("sub/new.txt"), "n\n").unwrap();
        let d = get_diff(&client, &DiffTarget::Unstaged, "sub", None, &opts).await.unwrap();
        assert_eq!((d.note, d.text.as_str()), (Some(DiffNote::SubmoduleUntracked), ""));
        // `diff.submodule=log` would print a commit list instead of the pointer change.
        git(&dir.join("sub"), &["commit", "-q", "--allow-empty", "-m", "s2"]);
        let d = get_diff(&client, &DiffTarget::Unstaged, "sub", None, &opts).await.unwrap();
        assert!(d.text.contains("-Subproject commit ") && d.text.contains("+Subproject commit "), "{}", d.text);
        assert_eq!(d.note, None);
        // Blame refuses, as Git does, and says which setting is the cause.
        let err = super::blame::get_blame(&client, None, "a.txt", None).await.unwrap_err().to_string();
        assert!(err.contains("blame.ignoreRevsFile"), "{err}");

        // A signed commit, whose check `log.showSignature` would print before each record.
        if which::which("ssh-keygen").is_ok() {
            let key = dir.with_extension("key");
            let st = Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-C", "t@t", "-f"]).arg(&key).status().unwrap();
            assert!(st.success());
            let signing_key = format!("user.signingkey={}", key.display());
            git(&dir, &["-c", "gpg.format=ssh", "-c", &signing_key, "commit", "-q", "-S", "-am", "signed"]);
            let head = String::from_utf8(Command::new("git").arg("-C").arg(&dir).args(["rev-parse", "HEAD"]).output().unwrap().stdout).unwrap().trim().to_string();
            assert_eq!(super::log::get_commit_body(&client, &head).await.unwrap(), "signed");
            let between = super::log::get_log_between(&client, std::slice::from_ref(&head), &[], 10).await.unwrap();
            assert_eq!(between.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["signed", "first"]);
            assert_eq!(between[0].oid, head);
            let parents = super::log::get_unchanged_parents(&client, &head).await.unwrap();
            assert_eq!(parents, [(between[1].oid.clone(), false)]);
            let history = super::history::get_file_history(&client, "a.txt", None, 10).await.unwrap();
            assert_eq!(history.entries.iter().map(|e| e.subject.as_str()).collect::<Vec<_>>(), ["signed", "first"]);
            let _ = std::fs::remove_file(&key);
            let _ = std::fs::remove_file(key.with_extension("key.pub"));
        }
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&sub);
    }

    /// File history follows a rename and lists a merge that changed the file; blame covers the
    /// working tree, a commit, the commit before a change, and the files it cannot blame.
    #[tokio::test]
    async fn file_history_and_blame_against_a_real_repository() {
        use super::blame::{get_blame, BlameContent};
        use super::commit_files::ChangeStatus;
        use super::history::get_file_history;
        if which::which("git").is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("stashpanda-blame-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let commit = |msg: &str| git(&dir, &["commit", "-qm", msg]);
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(dir.join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
        git(&dir, &["add", "-A"]);
        commit("add a");
        std::fs::write(dir.join("a.txt"), "one\ntwo\tTAB\nthree\n").unwrap();
        git(&dir, &["add", "-A"]);
        commit("tab");
        git(&dir, &["mv", "a.txt", "b ü.txt"]);
        std::fs::write(dir.join("b ü.txt"), "one\ntwo\tTAB\nthree\nfünf\n").unwrap();
        git(&dir, &["add", "-A"]);
        commit("rename");
        // Both sides change the file, so the merge has to combine them and the history lists it.
        git(&dir, &["checkout", "-qb", "side"]);
        std::fs::write(dir.join("b ü.txt"), "ONE\ntwo\tTAB\nthree\nfünf\n").unwrap();
        git(&dir, &["commit", "-qam", "side change"]);
        git(&dir, &["checkout", "-q", "main"]);
        std::fs::write(dir.join("b ü.txt"), "one\ntwo\tTAB\nthree\nfünf\nsix\n").unwrap();
        git(&dir, &["commit", "-qam", "main change"]);
        git(&dir, &["merge", "-q", "--no-ff", "side", "-m", "merge side"]);
        // Two merges that do not change the file, which the history leaves out: one of a branch
        // that never touched it, and one that takes it unchanged from the branch that changed it.
        git(&dir, &["checkout", "-qb", "docs"]);
        std::fs::write(dir.join("notes.txt"), "n\n").unwrap();
        git(&dir, &["add", "notes.txt"]);
        commit("notes");
        git(&dir, &["checkout", "-q", "main"]);
        git(&dir, &["merge", "-q", "--no-ff", "docs", "-m", "merge docs"]);
        git(&dir, &["checkout", "-qb", "side2"]);
        std::fs::write(dir.join("b ü.txt"), "ONE\ntwo\tTAB\nthree\nfünf\nsix\nseven\n").unwrap();
        git(&dir, &["commit", "-qam", "side2 change"]);
        git(&dir, &["checkout", "-q", "main"]);
        git(&dir, &["merge", "-q", "--no-ff", "side2", "-m", "merge side2"]);
        git(&dir, &["rm", "-q", "bin.dat"]);
        commit("drop the binary");
        // Uncommitted: a changed line, a staged new file, an untracked file.
        std::fs::write(dir.join("b ü.txt"), "ONE\ntwo\tTAB\nthree\nfünf\nsix\nseven\nwip\n").unwrap();
        std::fs::write(dir.join("staged.txt"), "s\n").unwrap();
        git(&dir, &["add", "staged.txt"]);
        std::fs::write(dir.join("untracked.txt"), "u\n").unwrap();
        let client = client_for(&dir).await;

        let history = get_file_history(&client, "b ü.txt", None, 100).await.unwrap();
        // The commits share one second, so their order is not certain.
        let mut subjects: Vec<&str> = history.entries.iter().map(|e| e.subject.as_str()).collect();
        subjects.sort_unstable();
        assert_eq!(subjects, ["add a", "main change", "merge side", "rename", "side change", "side2 change", "tab"], "{history:?}");
        assert!(!history.more);
        let merge = history.entries.iter().find(|e| e.subject == "merge side").unwrap();
        assert_eq!((merge.parents.len(), merge.path.as_str()), (2, "b ü.txt"));
        let rename = history.entries.iter().find(|e| e.subject == "rename").unwrap();
        assert_eq!((rename.status, rename.old_path.as_deref()), (ChangeStatus::Renamed, Some("a.txt")));
        let first = history.entries.last().unwrap();
        assert_eq!((first.status, first.path.as_str()), (ChangeStatus::Added, "a.txt"));
        // Each entry's diff of the file, as the history view asks for it.
        let rename_diff = super::diff::get_diff(
            &client,
            &super::diff::DiffTarget::Commit { commit: rename.oid.clone(), parent: None },
            &rename.path,
            rename.old_path.as_deref(),
            &super::diff::DiffOptions::default(),
        )
        .await
        .unwrap();
        assert!(rename_diff.text.contains("+fünf"), "{}", rename_diff.text);
        let merge_diff = super::diff::get_diff(
            &client,
            &super::diff::DiffTarget::Commit { commit: merge.oid.clone(), parent: Some(merge.parents[0].clone()) },
            &merge.path,
            None,
            &super::diff::DiffOptions::default(),
        )
        .await
        .unwrap();
        assert!(merge_diff.text.contains("+ONE"), "against its first parent the merge brings in the side change: {}", merge_diff.text);

        // A cap that stops short says there is more; asking with a larger one reads further.
        let capped = get_file_history(&client, "b ü.txt", None, 2).await.unwrap();
        assert_eq!((capped.entries.len(), capped.more), (2, true));
        // From an older commit, by the name the file had there.
        let tab = history.entries.iter().find(|e| e.subject == "tab").unwrap();
        let older = get_file_history(&client, "a.txt", Some(&tab.oid), 100).await.unwrap();
        assert_eq!(older.entries.iter().map(|e| e.subject.as_str()).collect::<Vec<_>>(), ["tab", "add a"]);
        // A deleted file still has a history, ending with its deletion.
        let deleted = get_file_history(&client, "bin.dat", None, 100).await.unwrap();
        assert_eq!(deleted.entries.first().map(|e| e.status), Some(ChangeStatus::Deleted));

        let blame = get_blame(&client, None, "b ü.txt", None).await.unwrap();
        assert_eq!(blame.content, BlameContent::Lines);
        assert_eq!(blame.lines, ["ONE", "two\tTAB", "three", "fünf", "six", "seven", "wip"]);
        let summary_of = |b: &super::blame::FileBlame, line: u32| {
            let g = b.groups.iter().find(|g| g.start <= line && line < g.start + g.count).unwrap();
            let o = &b.origins[g.origin as usize];
            (b.commits[o.commit as usize].clone(), o.clone())
        };
        assert_eq!(summary_of(&blame, 0).0.summary, "side change");
        let (tab_commit, tab_origin) = summary_of(&blame, 1);
        assert_eq!((tab_commit.summary.as_str(), tab_origin.path.as_str()), ("tab", "a.txt"), "blame follows the rename");
        assert_eq!(summary_of(&blame, 5).0.summary, "side2 change", "not the merge that took it unchanged");
        assert!(summary_of(&blame, 6).0.uncommitted);
        let (root, root_origin) = summary_of(&blame, 2);
        assert!(root.boundary && root_origin.previous.is_none(), "{root:?}");

        // Blame before the "tab" change: its previous version, at the root commit, under the old name.
        let previous = tab_origin.previous.clone().unwrap();
        assert_eq!(previous.path, "a.txt");
        let before = get_blame(&client, None, &previous.path, Some(&previous.oid)).await.unwrap();
        assert_eq!(before.lines, ["one", "two", "three"]);
        assert_eq!(before.commits.len(), 1);

        let at_commit = get_blame(&client, None, "b ü.txt", Some("HEAD")).await.unwrap();
        assert!(!at_commit.commits.iter().any(|c| c.uncommitted));
        assert_eq!(get_blame(&client, None, "bin.dat", Some(&tab.oid)).await.unwrap().content, BlameContent::Binary);
        assert_eq!(get_blame(&client, None, "nowhere.txt", Some("HEAD")).await.unwrap().content, BlameContent::Missing);
        assert_eq!(get_blame(&client, None, "bin.dat", None).await.unwrap().content, BlameContent::Missing, "deleted from the working tree");
        assert_eq!(get_blame(&client, None, "untracked.txt", None).await.unwrap().content, BlameContent::NotCommitted);
        let staged = get_blame(&client, None, "staged.txt", None).await.unwrap();
        assert!(staged.commits.iter().all(|c| c.uncommitted) && staged.lines == ["s"]);
        assert!(get_blame(&client, None, "b ü.txt", Some("--output=x")).await.is_err(), "an option is never taken for a revision");
        // The same through the long-lived `cat-file`, as the app reads them.
        let catfile = Arc::new(crate::git::catfile::CatFile::new(false));
        let cancel = tokio_util::sync::CancellationToken::new();
        let helper = Some((&catfile, &cancel));
        assert_eq!(get_blame(&client, helper, "bin.dat", Some(&tab.oid)).await.unwrap().content, BlameContent::Binary);
        assert_eq!(get_blame(&client, helper, "nowhere.txt", Some("HEAD")).await.unwrap().content, BlameContent::Missing);
        assert_eq!(get_blame(&client, helper, &previous.path, Some(&previous.oid)).await.unwrap().lines, before.lines);

        // A repository with no commits: nothing is committed and there is no history.
        let empty = std::env::temp_dir().join(format!("stashpanda-blame-empty-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&empty).unwrap();
        git(&empty, &["init", "-q", "-b", "main"]);
        std::fs::write(empty.join("new.txt"), "n\n").unwrap();
        let empty_client = client_for(&empty).await;
        assert_eq!(get_blame(&empty_client, None, "new.txt", None).await.unwrap().content, BlameContent::NotCommitted);
        assert!(get_file_history(&empty_client, "new.txt", None, 10).await.unwrap().entries.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    /// The merge editor's file, read from real `git merge` conflicts in each of Git's three marker
    /// styles; then saved with the hash check, and put back to Git's merge with `diff3` markers.
    #[tokio::test]
    async fn conflict_file_from_real_merges_in_every_marker_style() {
        use super::conflict_file::{content_hash, get_conflict_file, ConflictBlock, ConflictFile, ConflictRefusal};
        use super::blob::Eol;
        use crate::error::GitError;
        if which::which("git").is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("stashpanda-conflict-file-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "core.autocrlf", "false"]);
        let large = |last: &str| format!("{}{last}\n", format!("{}\n", "a".repeat(999)).repeat(6000));
        // Each file's base, main and feature versions. feature deletes gone.txt and adds
        // both.txt, which main adds too.
        type Versions<'a> = (&'a str, &'a [u8], &'a [u8], &'a [u8]);
        let files: [Versions; 10] = [
            ("plain.txt", b"1\n2\n3\n", b"1\nX\nY\nZ\n3\n", b"1\nX\nQ\nZ\n3\n"),
            ("crlf.txt", b"a\r\nb\r\nc\r\n", b"a\r\nB-main\r\nc\r\n", b"a\r\nB-feature\r\nc\r\n"),
            ("bom.txt", "\u{feff}top\nmid\nend\n".as_bytes(), "\u{feff}top\nmid-main\nend\n".as_bytes(), "\u{feff}top\nmid-feature\nend\n".as_bytes()),
            ("nonl.txt", b"one\ntwo\ntail", b"one\ntwo-main\ntail", b"one\ntwo-feature\ntail"),
            ("big.txt", b"<<<<<<< not a marker\nkeep\nx\n", b"<<<<<<< not a marker\nkeep\nx-main\n", b"<<<<<<< not a marker\nkeep\nx-feature\n"),
            ("lone.txt", b"<<<<<<< stray\nkeep\nv\n=======\nend\n", b"<<<<<<< stray\nkeep\nv-main\n=======\nend\n", b"<<<<<<< stray\nkeep\nv-feature\n=======\nend\n"),
            ("gone.txt", b"g\n", b"g-main\n", b""),
            ("bin.dat", &[0, 1, 2], &[0, 1, 3], &[0, 1, 4]),
            ("latin.txt", b"caf\xe9\n", b"caf\xe9 main\n", b"caf\xe9 feature\n"),
            ("clean.txt", b"same\n", b"same\n", b"same\n"),
        ];
        for (name, base, _, _) in &files {
            std::fs::write(dir.join(name), base).unwrap();
        }
        std::fs::write(dir.join("large.txt"), large("base")).unwrap();
        std::fs::write(dir.join(".gitattributes"), "big.txt conflict-marker-size=10\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "base"]);
        git(&dir, &["checkout", "-qb", "feature"]);
        for (name, _, _, feature) in &files {
            std::fs::write(dir.join(name), feature).unwrap();
        }
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        std::fs::write(dir.join("both.txt"), "feature added\n").unwrap();
        std::fs::write(dir.join("large.txt"), large("feature")).unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "feature"]);
        git(&dir, &["checkout", "-q", "main"]);
        for (name, _, main, _) in &files {
            std::fs::write(dir.join(name), main).unwrap();
        }
        std::fs::write(dir.join("both.txt"), "main added\n").unwrap();
        std::fs::write(dir.join("large.txt"), large("main")).unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "main"]);
        let merge = |style: &str| {
            let out = Command::new("git").arg("-C").arg(&dir).args(["-c", &format!("merge.conflictStyle={style}"), "merge", "-q", "feature"]).output().unwrap();
            assert!(!out.status.success(), "the merge stops on conflicts");
        };
        let client = client_for(&dir).await;
        let catfile = Arc::new(crate::git::catfile::CatFile::new(false));
        let cancel = tokio_util::sync::CancellationToken::new();
        let open = |path: &'static str| {
            let (client, catfile, cancel) = (&client, &catfile, &cancel);
            async move { get_conflict_file(client, Some((catfile, cancel)), path).await.unwrap() }
        };
        // The result with every block replaced by one side's text: ours everywhere must give our
        // version back, and theirs theirs.
        let resolve = |f: &ConflictFile, side: fn(&ConflictBlock) -> &str| {
            let lines: Vec<&str> = f.result.split_inclusive('\n').collect();
            let mut out = String::new();
            let mut at = 0;
            for b in &f.blocks {
                out.extend(lines[at..b.start_line as usize].iter().copied());
                out.push_str(side(b));
                at = b.end_line as usize;
            }
            out.extend(lines[at..].iter().copied());
            out
        };
        let check_sides = |f: &ConflictFile| {
            assert_eq!(f.refused, None, "{}", f.path);
            assert_eq!(Some(resolve(f, |b| &b.ours)), f.ours, "{}", f.path);
            assert_eq!(Some(resolve(f, |b| &b.theirs)), f.theirs, "{}", f.path);
            for text in [&f.result, f.ours.as_ref().unwrap(), f.theirs.as_ref().unwrap()] {
                assert!(!text.contains('\r') && !text.contains('\u{feff}'), "{}: {text:?}", f.path);
            }
            assert_eq!(f.hash, content_hash(&std::fs::read(dir.join(&f.path)).unwrap()));
        };

        for style in ["merge", "diff3", "zdiff3"] {
            merge(style);
            let plain = open("plain.txt").await;
            check_sides(&plain);
            assert_eq!((plain.eol, plain.bom, plain.final_newline), (Eol::Lf, false, true));
            assert_eq!(plain.base.as_deref(), Some("1\n2\n3\n"));
            assert_eq!(plain.blocks.len(), 1, "{style}");
            let block = &plain.blocks[0];
            assert_eq!((block.ours_label.as_str(), block.theirs_label.as_str()), ("HEAD", "feature"));
            match style {
                // No base section, so the block takes no lines in the result.
                "merge" => {
                    assert_eq!((block.ours.as_str(), block.base.as_deref(), block.theirs.as_str()), ("Y\n", None, "Q\n"));
                    assert_eq!((block.start_line, block.end_line, block.base_label.as_deref()), (2, 2, None));
                    assert_eq!(plain.result, "1\nX\nZ\n3\n");
                }
                // The lines both sides share stay in the block.
                "diff3" => {
                    assert_eq!((block.ours.as_str(), block.base.as_deref(), block.theirs.as_str()), ("X\nY\nZ\n", Some("2\n"), "X\nQ\nZ\n"));
                    assert_eq!((block.start_line, block.end_line), (1, 2));
                    assert!(block.base_label.as_ref().is_some_and(|l| !l.is_empty()), "{block:?}");
                    assert_eq!(plain.result, "1\n2\n3\n");
                }
                // zdiff3 moves them out.
                _ => {
                    assert_eq!((block.ours.as_str(), block.base.as_deref(), block.theirs.as_str()), ("Y\n", Some("2\n"), "Q\n"));
                    assert_eq!((block.start_line, block.end_line), (2, 3));
                    assert_eq!(plain.result, "1\nX\n2\nZ\n3\n");
                }
            }

            let crlf = open("crlf.txt").await;
            check_sides(&crlf);
            assert_eq!((crlf.eol, crlf.blocks.len()), (Eol::CrLf, 1));
            assert_eq!(crlf.blocks[0].ours, "B-main\n");

            let bom = open("bom.txt").await;
            check_sides(&bom);
            assert!(bom.bom && bom.result.starts_with("top\n"), "{bom:?}");

            let nonl = open("nonl.txt").await;
            check_sides(&nonl);
            assert!(!nonl.final_newline && nonl.result.ends_with("\ntail"), "{nonl:?}");

            // Markers ten characters long, as the attribute asks; the seven-character line is text.
            let big = open("big.txt").await;
            check_sides(&big);
            assert!(std::fs::read_to_string(dir.join("big.txt")).unwrap().contains("<<<<<<<<<< HEAD\n"));
            assert_eq!(big.blocks.len(), 1);
            assert!(big.result.starts_with("<<<<<<< not a marker\nkeep\n"), "{big:?}");

            // A stray `<<<<<<<` before the real block and a stray `=======` after it stay as text.
            let lone = open("lone.txt").await;
            check_sides(&lone);
            assert_eq!(lone.blocks.len(), 1);
            assert!(lone.result.starts_with("<<<<<<< stray\nkeep\n") && lone.result.ends_with("=======\nend\n"), "{lone:?}");

            // Both sides added the file, so there is no base version. The diff3 styles still
            // write a base section, with nothing in it.
            let both = open("both.txt").await;
            check_sides(&both);
            assert_eq!(both.base, None);
            let expected_base = if style == "merge" { None } else { Some("") };
            assert_eq!(both.blocks[0].base.as_deref(), expected_base);

            git(&dir, &["merge", "--abort"]);
        }

        merge("merge");
        let refused = |f: ConflictFile| {
            assert!(f.result.is_empty() && f.blocks.is_empty() && f.ours.is_none() && f.theirs.is_none(), "{f:?}");
            f.refused
        };
        assert_eq!(refused(open("gone.txt").await), Some(ConflictRefusal::DeletedOnOneSide));
        assert_eq!(refused(open("bin.dat").await), Some(ConflictRefusal::Binary));
        assert_eq!(refused(open("latin.txt").await), Some(ConflictRefusal::NotUtf8));
        assert_eq!(refused(open("large.txt").await), Some(ConflictRefusal::TooLarge));
        assert_eq!(refused(open("clean.txt").await), Some(ConflictRefusal::NotConflicted));
        assert_eq!(refused(open("nowhere.txt").await), Some(ConflictRefusal::NotConflicted));
        assert!(matches!(get_conflict_file(&client, None, "../plain.txt").await, Err(GitError::InvalidPath { .. })));
        // The same without the long-lived `cat-file`.
        let plain = open("plain.txt").await;
        assert_eq!(get_conflict_file(&client, None, "plain.txt").await.unwrap(), plain);

        // Saving checks the hash from opening: a save over a file changed since is refused and
        // writes nothing, and the hash a save returns is good for the next one.
        use crate::git::ops::{restore_conflict, write_worktree_file};
        let saved = write_worktree_file(&client, "plain.txt", "resolved\n", &plain.hash).await.unwrap();
        assert_eq!((std::fs::read_to_string(dir.join("plain.txt")).unwrap(), saved.clone()), ("resolved\n".to_string(), content_hash(b"resolved\n")));
        let stale = write_worktree_file(&client, "plain.txt", "again\n", &plain.hash).await;
        assert!(matches!(&stale, Err(GitError::ChangedOnDisk { path }) if path == "plain.txt"), "{stale:?}");
        assert_eq!(std::fs::read_to_string(dir.join("plain.txt")).unwrap(), "resolved\n");
        write_worktree_file(&client, "plain.txt", "again\n", &saved).await.unwrap();
        assert!(matches!(write_worktree_file(&client, "../x.txt", "x", &saved).await, Err(GitError::InvalidPath { .. })));
        // Still conflicted in the index, with no markers left in the file.
        let edited = open("plain.txt").await;
        assert_eq!((edited.result.as_str(), edited.blocks.len()), ("again\n", 0));

        // Back to Git's merge, now with diff3 markers and their base sections.
        let restored = restore_conflict(&client, "plain.txt").await.unwrap();
        assert!(restored.ok, "{restored:?}");
        let text = std::fs::read_to_string(dir.join("plain.txt")).unwrap();
        assert!(text.contains("<<<<<<< ours\n") && text.contains("||||||| base\n2\n=======\n"), "{text}");
        let reopened = open("plain.txt").await;
        check_sides(&reopened);
        assert_eq!(reopened.blocks.len(), 1);
        assert_eq!((reopened.blocks[0].base.as_deref(), reopened.blocks[0].base_label.as_deref()), (Some("2\n"), Some("base")));
        assert!(matches!(restore_conflict(&client, "clean.txt").await, Err(GitError::InvalidPath { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn conflict_file_is_current_after_the_merge_is_done_again() {
        // The long-lived `cat-file` keeps the index it first read. A conflict read through it,
        // then aborted and replaced by another merge's conflict, must give the new versions.
        use super::conflict_file::get_conflict_file;
        if which::which("git").is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("stashpanda-conflict-again-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("f.txt"), "base\n").unwrap();
        git(&dir, &["add", "f.txt"]);
        git(&dir, &["commit", "-qm", "base"]);
        for (branch, text) in [("one", "one\n"), ("two", "two\n")] {
            git(&dir, &["checkout", "-qb", branch, "main"]);
            std::fs::write(dir.join("f.txt"), text).unwrap();
            git(&dir, &["commit", "-qam", branch]);
        }
        git(&dir, &["checkout", "-q", "main"]);
        std::fs::write(dir.join("f.txt"), "main\n").unwrap();
        git(&dir, &["commit", "-qam", "main"]);
        let merge = |branch: &str| {
            let out = Command::new("git").arg("-C").arg(&dir).args(["-c", "user.email=t@t", "-c", "user.name=T", "merge", branch]).output().unwrap();
            assert!(!out.status.success(), "the merge of {branch} stops on a conflict");
        };
        let client = client_for(&dir).await;
        let catfile = Arc::new(crate::git::catfile::CatFile::new(false));
        let cancel = tokio_util::sync::CancellationToken::new();

        merge("one");
        let first = get_conflict_file(&client, Some((&catfile, &cancel)), "f.txt").await.unwrap();
        assert_eq!(first.theirs.as_deref(), Some("one\n"));
        git(&dir, &["merge", "--abort"]);
        merge("two");
        let second = get_conflict_file(&client, Some((&catfile, &cancel)), "f.txt").await.unwrap();
        assert_eq!((second.ours.as_deref(), second.theirs.as_deref()), (Some("main\n"), Some("two\n")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn every_query_runs_against_a_real_repository() {
        if which::which("git").is_err() {
            eprintln!("git not on PATH, skipping");
            return;
        }
        let dir = fixture();
        let client = client_for(&dir).await;

        let status = super::status::get_status(&client).await.unwrap();
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.unstaged_count, 1);
        assert_eq!(status.untracked_count, 1);
        assert!(status.is_dirty());

        let log_head_oid = String::from_utf8(
            Command::new("git").arg("-C").arg(&dir).args(["rev-parse", "HEAD"]).output().unwrap().stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        let refs = super::refs::get_refs(&client).await.unwrap();
        let names: Vec<&str> = refs.refs.iter().map(|r| r.short.as_str()).collect();
        assert!(names.contains(&"main") && names.contains(&"side") && names.contains(&"v1"), "{names:?}");
        assert_eq!(refs.head_ref.as_deref(), Some("refs/heads/main"));
        assert_eq!(refs.head.as_deref(), Some(log_head_oid.as_str()), "HEAD oid comes from rev-parse");
        let tag = refs.refs.iter().find(|r| r.short == "v1").unwrap();
        assert!(matches!(tag.kind, super::refs::RefKind::Tag { annotated: true }));
        let second = refs.refs.iter().find(|r| r.short == "side").unwrap();
        assert!(tag.target != second.target);

        let log = super::log::get_log(&client, 1000).await.unwrap();
        assert_eq!(log.len(), 4);
        let merge = &log[0];
        assert_eq!(merge.parents.len(), 2);
        assert_eq!(merge.subject, "merge side");

        let second_commit = log.iter().find(|c| c.subject == "second").unwrap();
        let files = super::commit_files::get_commit_files(&client, &second_commit.oid, None).await.unwrap();
        let renamed = files.iter().find(|f| f.path == "renamed.txt").expect("rename detected");
        assert_eq!(renamed.status, super::commit_files::ChangeStatus::Renamed);
        assert_eq!(renamed.old_path.as_deref(), Some("a.txt"));
        assert_eq!(renamed.added, Some(1));

        let root = log.iter().find(|c| c.subject == "first").unwrap();
        let root_files = super::commit_files::get_commit_files(&client, &root.oid, None).await.unwrap();
        assert_eq!(root_files.len(), 2, "--root diffs the first commit against the empty tree");
        assert!(root_files.iter().any(|f| f.path == "bin.dat" && f.binary));

        let diff = super::diff::get_diff(
            &client,
            &super::diff::DiffTarget::Unstaged,
            "renamed.txt",
            None,
            &super::diff::DiffOptions::default(),
        )
        .await
        .unwrap();
        assert!(diff.text.contains("+wip"), "{}", diff.text);
        assert!(!diff.is_binary);

        let commit_diff = super::diff::get_diff(
            &client,
            &super::diff::DiffTarget::Commit { commit: second_commit.oid.clone(), parent: None },
            "renamed.txt",
            Some("a.txt"),
            &super::diff::DiffOptions::default(),
        )
        .await
        .unwrap();
        assert!(commit_diff.text.contains("+more"), "{}", commit_diff.text);

        let untracked = super::diff::get_diff(
            &client,
            &super::diff::DiffTarget::Untracked,
            "untracked.txt",
            None,
            &super::diff::DiffOptions::default(),
        )
        .await
        .unwrap();
        assert!(untracked.text.contains("+u\n"));

        let blob = super::blob::get_blob(&client, None, &format!("{}:renamed.txt", second_commit.oid)).await.unwrap();
        assert_eq!(blob.text, "hello\nworld\nmore\n");
        let wt = super::blob::get_worktree_file(&client, "renamed.txt").await.unwrap();
        assert!(wt.text.ends_with("wip\n"));
        let body = super::log::get_commit_body(&client, &merge.oid).await.unwrap();
        assert_eq!(body, "merge side");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
