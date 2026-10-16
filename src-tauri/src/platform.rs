//! Which operating system StashPanda itself is running on, and whether that
//! Linux is a WSL distro. Decides how paths are classified and which
//! transports make sense.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    Windows,
    /// Linux inside WSL 2; `distro` comes from `$WSL_DISTRO_NAME`.
    LinuxWsl { distro: String },
    Linux,
    MacOs,
}

impl Host {
    pub fn detect() -> Host {
        if cfg!(target_os = "windows") {
            return Host::Windows;
        }
        if cfg!(target_os = "macos") {
            return Host::MacOs;
        }
        if let Ok(d) = std::env::var("WSL_DISTRO_NAME") {
            if !d.is_empty() {
                return Host::LinuxWsl { distro: d };
            }
        }
        if let Ok(v) = std::fs::read_to_string("/proc/version") {
            if v.to_ascii_lowercase().contains("microsoft") {
                return Host::LinuxWsl {
                    distro: "Ubuntu".to_string(),
                };
            }
        }
        Host::Linux
    }

    pub fn is_windows(&self) -> bool {
        matches!(self, Host::Windows)
    }
}

/// macOS: give this process the PATH the user's login shell has. Started from Finder or the Dock,
/// an app gets only `/usr/bin:/bin:/usr/sbin:/sbin`, so Git's hooks and credential helpers could
/// not find what the user installed with Homebrew, nvm and the like (a husky hook that runs
/// `npx`, for one). Asks `$SHELL -l -c` for its PATH, giving up after 3 seconds, and falls back
/// to adding Homebrew's folders. Does nothing when PATH already has them, as when started from a
/// terminal. Call it first thing in `main`, before any other thread starts.
#[cfg(target_os = "macos")]
pub fn adopt_login_shell_path() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const HOMEBREW: [&str; 2] = ["/opt/homebrew/bin", "/usr/local/bin"];
    let current = std::env::var("PATH").unwrap_or_default();
    if current.split(':').any(|d| HOMEBREW.contains(&d)) {
        return;
    }
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".to_string());
    let from_shell = Command::new(&shell)
        .args(["-l", "-c", "printf '%s' \"$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()
        .and_then(|mut child| {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) if status.success() => break child.wait_with_output().ok(),
                    Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
                    _ => {
                        let _ = child.kill();
                        break None;
                    }
                }
            }
        })
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|p| p.contains('/'));
    let mut dirs: Vec<String> = match from_shell {
        Some(p) => p.split(':').map(str::to_string).collect(),
        None => HOMEBREW.iter().map(|d| d.to_string()).collect(),
    };
    // Keep what the app was started with too, after the shell's.
    for d in current.split(':').filter(|d| !d.is_empty()) {
        if !dirs.iter().any(|x| x == d) {
            dirs.push(d.to_string());
        }
    }
    std::env::set_var("PATH", dirs.join(":"));
}
