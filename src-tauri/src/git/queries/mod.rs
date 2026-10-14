//! One module per Git query: the command line, and a parser tested on
//! captured bytes. Every query takes the repository's `GitClient`.

pub mod blame;
pub mod blob;
pub mod commit_files;
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
        git(&dir, &["rm", "-q", "bin.dat"]);
        commit("drop the binary");
        // Uncommitted: a changed line, a staged new file, an untracked file.
        std::fs::write(dir.join("b ü.txt"), "ONE\ntwo\tTAB\nthree\nfünf\nsix\nwip\n").unwrap();
        std::fs::write(dir.join("staged.txt"), "s\n").unwrap();
        git(&dir, &["add", "staged.txt"]);
        std::fs::write(dir.join("untracked.txt"), "u\n").unwrap();
        let client = client_for(&dir).await;

        let history = get_file_history(&client, "b ü.txt", None, 100).await.unwrap();
        // The commits share one second, so only the merge's place at the top is certain.
        let mut subjects: Vec<&str> = history.entries.iter().map(|e| e.subject.as_str()).collect();
        assert_eq!(subjects[0], "merge side", "{history:?}");
        subjects.sort_unstable();
        assert_eq!(subjects, ["add a", "main change", "merge side", "rename", "side change", "tab"]);
        assert!(!history.more);
        let merge = &history.entries[0];
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
        assert_eq!(blame.lines, ["ONE", "two\tTAB", "three", "fünf", "six", "wip"]);
        let summary_of = |b: &super::blame::FileBlame, line: u32| {
            let g = b.groups.iter().find(|g| g.start <= line && line < g.start + g.count).unwrap();
            let o = &b.origins[g.origin as usize];
            (b.commits[o.commit as usize].clone(), o.clone())
        };
        assert_eq!(summary_of(&blame, 0).0.summary, "side change");
        let (tab_commit, tab_origin) = summary_of(&blame, 1);
        assert_eq!((tab_commit.summary.as_str(), tab_origin.path.as_str()), ("tab", "a.txt"), "blame follows the rename");
        assert!(summary_of(&blame, 5).0.uncommitted);
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
