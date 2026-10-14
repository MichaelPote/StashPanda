//! Opening a repository across the Windows and WSL boundary, the way the app does: from the
//! Windows build, a repository inside WSL through its `\\wsl.localhost\<distro>\…` path; from the
//! Linux build inside WSL, a Windows folder through `/mnt/<drive>/…`, which runs Windows
//! `git.exe`. Each test skips itself on a machine without the other side.

use std::process::Command;

use crate::git::ops;
use crate::git::queries::refs::get_refs;
use crate::git::queries::status::get_status;
use crate::git::GitReq;
use crate::platform::Host;
use crate::repo::open::open_repo;
use crate::settings::{Connection, RepoEntry, ResolvedTransport, Settings};

fn entry(path: String) -> RepoEntry {
    RepoEntry {
        key: format!("cross-{}", uuid::Uuid::new_v4().simple()),
        path,
        connection: Connection::Auto,
        resolved_transport: None,
        display_name: None,
        git_executable: None,
        group_id: None,
        last_opened_at: None,
    }
}

/// Status, refs, an argument with a space and both quotes, and a stage and commit of a file whose
/// name has a space in it, through the opened repository's own client.
async fn exercise(h: &crate::repo::RepoHandle, file_on_disk: &dyn Fn(&str, &str)) {
    let status = get_status(&h.client).await.unwrap();
    assert_eq!(status.branch.as_deref(), Some("main"), "{status:?}");
    let refs = get_refs(&h.client).await.unwrap();
    assert!(refs.refs.iter().any(|r| r.short == "main"), "{refs:?}");

    // `git rev-parse --sq-quote` quotes its argument for a shell, so a word split or a lost quote
    // anywhere between here and Git shows up in the result.
    let out = h.client.exec(GitReq::new("rev-parse", ["rev-parse", "--sq-quote", "a b'c\"d"])).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), r#"'a b'\''c"d'"#);

    file_on_disk("sp ace.txt", "hello\n");
    let paths = vec!["sp ace.txt".to_string()];
    let r = ops::commit(&h.client, "Add a file with a space", Some(&paths), false, false).await.unwrap();
    assert!(r.ok, "{r:?}");
    let status = get_status(&h.client).await.unwrap();
    assert!(status.entries.is_empty(), "{status:?}");
    let body = h.transport.read_file(&crate::git::queries::diff::join_repo_path(&h.client.repo, "sp ace.txt"), 1024).await.unwrap();
    assert_eq!(body, b"hello\n");
}

/// The Windows build opening a repository that lives inside WSL.
#[cfg(windows)]
#[tokio::test]
async fn windows_opens_a_wsl_repository_through_its_unc_path() {
    use crate::transport::wsl::list_distros;
    let Ok(distros) = list_distros().await else { return };
    // The first distribution that has Git; Docker's helper distributions have none.
    let Some(distro) = distros.into_iter().find(|d| {
        Command::new("wsl.exe").args(["-d", d, "-e", "git", "--version"]).output().is_ok_and(|o| o.status.success())
    }) else {
        return;
    };
    let name = format!("stashpanda-cross-{}", uuid::Uuid::new_v4().simple());
    let linux = format!("/tmp/{name}");
    let wsl = |script: &str| {
        let st = Command::new("wsl.exe").args(["-d", &distro, "-e", "sh", "-c", script]).status().unwrap();
        assert!(st.success(), "{script}");
    };
    wsl(&format!(
        "mkdir -p {linux} && cd {linux} && git init -q -b main && git config user.email t@t && git config user.name T \
         && git config commit.gpgsign false && echo 1 > a.txt && git add a.txt && git commit -qm first"
    ));
    let unc = format!(r"\\wsl.localhost\{distro}\tmp\{name}");
    let h = open_repo(None, &entry(unc), &Settings::default(), &Host::detect(), None).await.unwrap();
    assert!(matches!(&h.info.transport, ResolvedTransport::Wsl { distro: d } if d == &distro), "{:?}", h.info.transport);
    assert_eq!(h.info.toplevel, linux);
    let write = |file: &str, text: &str| wsl(&format!("printf '{text}' > '{linux}/{file}'"));
    exercise(&h, &write).await;
    wsl(&format!("rm -rf {linux}"));
}

/// The Linux build inside WSL opening a Windows folder, which it does with Windows `git.exe`.
#[cfg(unix)]
#[tokio::test]
async fn wsl_opens_a_windows_folder_with_windows_git() {
    if std::env::var("WSL_DISTRO_NAME").is_err() || !std::path::Path::new("/mnt/c/Windows/System32/cmd.exe").exists() {
        return;
    }
    // %TEMP% as Windows sees it, then as /mnt/c/… for the files this test writes itself.
    let Ok(out) = Command::new("/mnt/c/Windows/System32/cmd.exe").args(["/c", "echo %TEMP%"]).current_dir("/mnt/c").output() else { return };
    let temp_win = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let Ok(out) = Command::new("wslpath").args(["-u", &temp_win]).output() else { return };
    let temp = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if temp.is_empty() {
        return;
    }
    let dir = std::path::PathBuf::from(&temp).join(format!("stashpanda-cross-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let git = |args: &[&str]| {
        let st = Command::new("git").arg("-C").arg(&dir).args(args).status().unwrap();
        assert!(st.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    for (k, v) in [("user.email", "t@t"), ("user.name", "T"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
        git(&["config", k, v]);
    }
    std::fs::write(dir.join("a.txt"), "1\n").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-qm", "first"]);

    let opened = open_repo(None, &entry(dir.to_string_lossy().into_owned()), &Settings::default(), &Host::detect(), None).await;
    let h = match opened {
        Ok(h) => h,
        // No Windows Git installed: the resolver falls back to Linux Git and warns; nothing more to test.
        Err(e) => panic!("opening {} failed: {e:?}", dir.display()),
    };
    let ResolvedTransport::Local { git_executable } = &h.info.transport else { panic!("{:?}", h.info.transport) };
    if !git_executable.to_ascii_lowercase().ends_with("git.exe") {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    // Windows Git gets the Windows form of the path.
    let argv = h.transport.render(&h.client.build_cmd(&GitReq::new("status", ["status"])));
    assert!(argv.iter().any(|a| a.starts_with("C:\\") || a.contains(":\\")), "{argv:?}");
    let write = |file: &str, text: &str| std::fs::write(dir.join(file), text).unwrap();
    exercise(&h, &write).await;
    let _ = std::fs::remove_dir_all(&dir);
}
