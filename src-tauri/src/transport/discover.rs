//! Find Git executables on the host, honouring overrides first.

use std::path::{Path, PathBuf};

use crate::error::{GitError, Result};
use crate::platform::Host;

fn exists(p: &Path) -> bool {
    p.is_file()
}

/// The git this build runs directly.
pub fn find_local_git(override_path: Option<&str>, host: &Host) -> Result<PathBuf> {
    let mut searched = Vec::new();
    if let Some(o) = override_path.filter(|s| !s.trim().is_empty()) {
        let p = PathBuf::from(o);
        if exists(&p) {
            return Ok(p);
        }
        searched.push(o.to_string());
    }
    // macOS: Homebrew's Git before anything on PATH. Started from Finder, the app's PATH starts
    // with /usr/bin, whose git is Apple's stand-in: without the Command Line Tools it opens a
    // dialog offering to install them instead of running Git, so it is only taken when
    // `xcode-select -p` says they are installed.
    if *host == Host::MacOs {
        for p in ["/opt/homebrew/bin/git", "/usr/local/bin/git"] {
            if exists(Path::new(p)) {
                return Ok(PathBuf::from(p));
            }
            searched.push(p.to_string());
        }
        if let Ok(p) = which::which("git") {
            if p != Path::new("/usr/bin/git") || apple_command_line_tools_installed() {
                return Ok(p);
            }
        }
        searched.push("/usr/bin/git (needs the Command Line Tools: xcode-select --install)".into());
        return Err(GitError::GitNotFound { searched });
    }
    if let Ok(p) = which::which("git") {
        return Ok(p);
    }
    searched.push("git on PATH".into());
    let candidates: Vec<PathBuf> = match host {
        Host::Windows => {
            let mut v = Vec::new();
            for var in ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"] {
                if let Ok(base) = std::env::var(var) {
                    v.push(PathBuf::from(&base).join(r"Git\bin\git.exe"));
                    v.push(PathBuf::from(&base).join(r"Programs\Git\bin\git.exe"));
                }
            }
            v
        }
        _ => vec![PathBuf::from("/usr/bin/git"), PathBuf::from("/usr/local/bin/git")],
    };
    for c in candidates {
        if exists(&c) {
            return Ok(c);
        }
        searched.push(c.to_string_lossy().into_owned());
    }
    Err(GitError::GitNotFound { searched })
}

/// True when Apple's Command Line Tools are installed, so `/usr/bin/git` runs Git rather than
/// offering to install them: `xcode-select -p` prints their folder and exits 0.
fn apple_command_line_tools_installed() -> bool {
    std::process::Command::new("/usr/bin/xcode-select")
        .arg("-p")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Windows `git.exe` reachable from inside WSL, for `/mnt/<d>/` repositories.
pub fn find_windows_git_from_linux(override_path: Option<&str>, automount_root: &str) -> Option<PathBuf> {
    if let Some(o) = override_path.filter(|s| !s.trim().is_empty()) {
        let p = PathBuf::from(o);
        if exists(&p) {
            return Some(p);
        }
    }
    let root = automount_root.trim_end_matches('/');
    for drive in ['c', 'd'] {
        for rel in ["Program Files/Git/bin/git.exe", "Program Files (x86)/Git/bin/git.exe"] {
            let p = PathBuf::from(format!("{root}/{drive}/{rel}"));
            if exists(&p) {
                return Some(p);
            }
        }
    }
    None
}
