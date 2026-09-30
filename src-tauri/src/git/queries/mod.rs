//! One module per Git query: the command line, and a parser tested on
//! captured bytes. Every query takes the repository's `GitClient`.

pub mod blob;
pub mod commit_files;
pub mod diff;
pub mod log;
pub mod refs;
pub mod status;

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

        let blob = super::blob::get_blob(&client, &format!("{}:renamed.txt", second_commit.oid)).await.unwrap();
        assert_eq!(blob.text, "hello\nworld\nmore\n");
        let wt = super::blob::get_worktree_file(&client, "renamed.txt").await.unwrap();
        assert!(wt.text.ends_with("wip\n"));
        let body = super::log::get_commit_body(&client, &merge.oid).await.unwrap();
        assert_eq!(body, "merge side");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
