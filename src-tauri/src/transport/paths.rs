//! Path classification and translation between what the app sees (the host
//! path) and what `git -C` must receive inside a transport (the native path).
//! Pure string logic; never spawns `wslpath`.

use crate::error::{GitError, Result};

/// A path as the user typed it or as the OS running StashPanda can open it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HostPath(pub String);

/// A path as `git -C` receives it inside the transport's environment.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NativePath(pub String);

impl HostPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl NativePath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub trait PathMapper: Send + Sync {
    fn to_native(&self, host: &HostPath) -> Result<NativePath>;
    fn to_host(&self, native: &NativePath) -> Result<HostPath>;
}

/// What kind of location a user-entered path denotes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathKind {
    /// `C:\foo\bar` (any separator style).
    WindowsDrive { normalized: String },
    /// `\\wsl.localhost\<distro>\...` or `\\wsl$\<distro>\...`.
    WslUnc { distro: String, linux_path: String },
    /// `/mnt/<d>/...` seen from inside WSL.
    LinuxMnt { drive: char, rest: String, normalized: String },
    /// Any other absolute POSIX path.
    LinuxAbsolute { normalized: String },
    /// `user@host:/path`, `host:/path`, `ssh://user@host:port/path`.
    Ssh(SshSpec),
    Invalid { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSpec {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
    pub path: String,
}

fn strip_trailing_seps(s: &str) -> &str {
    let t = s.trim_end_matches(['/', '\\']);
    if t.is_empty() {
        s
    } else {
        t
    }
}

/// Normalise a Windows path: strip `\\?\`, forward slashes to backslashes,
/// collapse duplicate separators, uppercase the drive letter, trim trailing.
pub fn normalize_windows(p: &str) -> String {
    let p = p.strip_prefix(r"\\?\").unwrap_or(p);
    let mut s: String = p.replace('/', "\\");
    while s.contains("\\\\") && !s.starts_with("\\\\") {
        s = s.replace("\\\\", "\\");
    }
    // Collapse doubles after the UNC prefix too.
    if s.starts_with("\\\\") {
        let body = s[2..].to_string();
        let mut b = body;
        while b.contains("\\\\") {
            b = b.replace("\\\\", "\\");
        }
        s = format!("\\\\{b}");
    }
    let mut chars: Vec<char> = s.chars().collect();
    if chars.len() >= 2 && chars[1] == ':' {
        chars[0] = chars[0].to_ascii_uppercase();
    }
    let s: String = chars.into_iter().collect();
    let t = strip_trailing_seps(&s);
    // Keep the root form `C:\`.
    if t.len() == 2 && t.ends_with(':') {
        format!("{t}\\")
    } else {
        t.to_string()
    }
}

/// Normalise a POSIX path: collapse duplicate slashes, trim trailing slash.
pub fn normalize_posix(p: &str) -> String {
    let mut s = p.to_string();
    while s.contains("//") {
        s = s.replace("//", "/");
    }
    strip_trailing_seps(&s).to_string()
}

pub fn classify(input: &str) -> PathKind {
    let raw = input.trim();
    if raw.is_empty() {
        return PathKind::Invalid {
            reason: "empty path".into(),
        };
    }

    // ssh://user@host:port/path
    if let Some(rest) = raw.strip_prefix("ssh://") {
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => {
                return PathKind::Invalid {
                    reason: "ssh:// URL needs a path".into(),
                }
            }
        };
        let (user, hostport) = match authority.rfind('@') {
            Some(i) => (Some(authority[..i].to_string()), &authority[i + 1..]),
            None => (None, authority),
        };
        let (host, port) = match hostport.rfind(':') {
            Some(i) => match hostport[i + 1..].parse::<u16>() {
                Ok(p) => (hostport[..i].to_string(), Some(p)),
                Err(_) => (hostport.to_string(), None),
            },
            None => (hostport.to_string(), None),
        };
        if host.is_empty() {
            return PathKind::Invalid {
                reason: "ssh:// URL has no host".into(),
            };
        }
        return PathKind::Ssh(SshSpec {
            user,
            host,
            port,
            path: normalize_posix(path),
        });
    }

    // UNC WSL path
    let unc = raw.replace('/', "\\");
    for prefix in [r"\\wsl.localhost\", r"\\wsl$\"] {
        if let Some(rest) = unc.strip_prefix(prefix) {
            let (distro, tail) = match rest.find('\\') {
                Some(i) => (&rest[..i], &rest[i..]),
                None => (rest, ""),
            };
            if distro.is_empty() {
                return PathKind::Invalid {
                    reason: "WSL UNC path has no distro".into(),
                };
            }
            let linux = if tail.is_empty() {
                "/".to_string()
            } else {
                normalize_posix(&tail.replace('\\', "/"))
            };
            return PathKind::WslUnc {
                distro: distro.to_string(),
                linux_path: linux,
            };
        }
    }

    // Windows drive path
    let chars: Vec<char> = raw.chars().collect();
    if chars.len() >= 2 && chars[0].is_ascii_alphabetic() && chars[1] == ':' {
        if chars.len() > 2 && chars[2] != '\\' && chars[2] != '/' {
            return PathKind::Invalid {
                reason: "drive-relative paths are not supported".into(),
            };
        }
        return PathKind::WindowsDrive {
            normalized: normalize_windows(raw),
        };
    }

    // POSIX absolute
    if raw.starts_with('/') {
        let normalized = normalize_posix(raw);
        if let Some(rest) = normalized.strip_prefix("/mnt/") {
            let mut it = rest.chars();
            if let Some(d) = it.next() {
                let after: String = it.collect();
                if d.is_ascii_alphabetic() && (after.is_empty() || after.starts_with('/')) {
                    return PathKind::LinuxMnt {
                        drive: d.to_ascii_lowercase(),
                        rest: after,
                        normalized,
                    };
                }
            }
        }
        return PathKind::LinuxAbsolute { normalized };
    }

    // scp-style: [user@]host:/path  (a colon followed by a slash marks it)
    if let Some(i) = raw.find(":/") {
        let authority = &raw[..i];
        let path = &raw[i + 1..];
        if !authority.is_empty() && !authority.contains('\\') && !authority.contains('/') {
            let (user, host) = match authority.rfind('@') {
                Some(j) => (Some(authority[..j].to_string()), authority[j + 1..].to_string()),
                None => (None, authority.to_string()),
            };
            return PathKind::Ssh(SshSpec {
                user,
                host,
                port: None,
                path: normalize_posix(path),
            });
        }
    }

    PathKind::Invalid {
        reason: "not an absolute Windows path, a WSL UNC path, a POSIX path or an SSH location".into(),
    }
}

/// `C:\foo\bar` -> `/mnt/c/foo/bar`.
pub fn windows_to_wsl(win: &str, automount_root: &str) -> Result<String> {
    let n = normalize_windows(win);
    let chars: Vec<char> = n.chars().collect();
    if chars.len() < 2 || chars[1] != ':' {
        return Err(GitError::InvalidPath {
            path: win.to_string(),
            reason: "not a drive path".into(),
        });
    }
    let drive = chars[0].to_ascii_lowercase();
    let rest: String = n[2..].replace('\\', "/");
    let root = strip_trailing_seps(automount_root);
    Ok(normalize_posix(&format!("{root}/{drive}{rest}")))
}

/// `/mnt/c/foo/bar` -> `C:\foo\bar`.
pub fn wsl_to_windows(posix: &str, automount_root: &str) -> Result<String> {
    let n = normalize_posix(posix);
    let root = format!("{}/", strip_trailing_seps(automount_root));
    let rest = n.strip_prefix(&root).ok_or_else(|| GitError::InvalidPath {
        path: posix.to_string(),
        reason: format!("not under {root}"),
    })?;
    let mut it = rest.chars();
    let d = it.next().filter(|c| c.is_ascii_alphabetic()).ok_or_else(|| {
        GitError::InvalidPath {
            path: posix.to_string(),
            reason: "no drive letter after the automount root".into(),
        }
    })?;
    let tail: String = it.collect();
    if !(tail.is_empty() || tail.starts_with('/')) {
        return Err(GitError::InvalidPath {
            path: posix.to_string(),
            reason: "malformed drive segment".into(),
        });
    }
    let win_tail = tail.replace('/', "\\");
    Ok(if win_tail.is_empty() {
        format!("{}:\\", d.to_ascii_uppercase())
    } else {
        format!("{}:{}", d.to_ascii_uppercase(), win_tail)
    })
}

/// `/home/me/code/x` -> `\\wsl.localhost\<distro>\home\me\code\x`.
pub fn linux_to_unc(linux: &str, distro: &str) -> String {
    let n = normalize_posix(linux);
    format!(r"\\wsl.localhost\{distro}{}", n.replace('/', "\\"))
}

/// `W:\home\me\code\x` -> `\\wsl$\Ubuntu\home\me\code\x` when `remote_of('W')` says the drive is mapped to
/// a WSL share. None for any other path or drive. Without this, a mapped drive gets a different
/// library key from the same folder written as a UNC path, and runs Windows git over the share.
pub fn wsl_drive_to_unc(path: &str, remote_of: impl FnOnce(char) -> Option<String>) -> Option<String> {
    let PathKind::WindowsDrive { normalized } = classify(path) else { return None };
    let remote = remote_of(normalized.chars().next()?)?;
    let remote = normalize_windows(&remote);
    if !matches!(classify(&remote), PathKind::WslUnc { .. }) {
        return None;
    }
    Some(format!("{remote}{}", normalized[2..].trim_end_matches('\\')))
}

/// Same-path mapper for Local transports.
pub struct IdentityMapper;
impl PathMapper for IdentityMapper {
    fn to_native(&self, host: &HostPath) -> Result<NativePath> {
        Ok(NativePath(host.0.clone()))
    }
    fn to_host(&self, native: &NativePath) -> Result<HostPath> {
        Ok(HostPath(native.0.clone()))
    }
}

/// App on Windows, git inside a WSL distro.
pub struct WslMapper {
    pub distro: String,
    pub automount_root: String,
}
impl PathMapper for WslMapper {
    fn to_native(&self, host: &HostPath) -> Result<NativePath> {
        match classify(&host.0) {
            PathKind::WslUnc { distro, linux_path } => {
                if !distro.eq_ignore_ascii_case(&self.distro) {
                    return Err(GitError::InvalidPath {
                        path: host.0.clone(),
                        reason: format!(
                            "path is inside distro {distro} but the repository is configured for {}",
                            self.distro
                        ),
                    });
                }
                Ok(NativePath(linux_path))
            }
            PathKind::WindowsDrive { normalized } => {
                Ok(NativePath(windows_to_wsl(&normalized, &self.automount_root)?))
            }
            PathKind::LinuxAbsolute { normalized } => Ok(NativePath(normalized)),
            _ => Err(GitError::InvalidPath {
                path: host.0.clone(),
                reason: "cannot map this path into WSL".into(),
            }),
        }
    }
    fn to_host(&self, native: &NativePath) -> Result<HostPath> {
        let root = format!("{}/", strip_trailing_seps(&self.automount_root));
        if native.0.starts_with(&root) {
            return Ok(HostPath(wsl_to_windows(&native.0, &self.automount_root)?));
        }
        Ok(HostPath(linux_to_unc(&native.0, &self.distro)))
    }
}

/// App on Linux (WSL), running Windows `git.exe` against `/mnt/<d>/...`.
pub struct WindowsFromLinuxMapper {
    pub automount_root: String,
    pub distro: String,
}
impl PathMapper for WindowsFromLinuxMapper {
    fn to_native(&self, host: &HostPath) -> Result<NativePath> {
        match classify(&host.0) {
            PathKind::LinuxMnt { normalized, .. } => {
                Ok(NativePath(wsl_to_windows(&normalized, &self.automount_root)?))
            }
            PathKind::LinuxAbsolute { normalized } => {
                Ok(NativePath(linux_to_unc(&normalized, &self.distro)))
            }
            PathKind::WindowsDrive { normalized } => Ok(NativePath(normalized)),
            _ => Err(GitError::InvalidPath {
                path: host.0.clone(),
                reason: "cannot map this path for Windows git".into(),
            }),
        }
    }
    fn to_host(&self, native: &NativePath) -> Result<HostPath> {
        match classify(&native.0) {
            PathKind::WindowsDrive { normalized } => {
                Ok(HostPath(windows_to_wsl(&normalized, &self.automount_root)?))
            }
            PathKind::WslUnc { linux_path, .. } => Ok(HostPath(linux_path)),
            _ => Err(GitError::InvalidPath {
                path: native.0.clone(),
                reason: "cannot map this Windows path back to Linux".into(),
            }),
        }
    }
}

/// SSH: native is the remote POSIX path; there is no host view.
pub struct SshMapper;
impl PathMapper for SshMapper {
    fn to_native(&self, host: &HostPath) -> Result<NativePath> {
        match classify(&host.0) {
            PathKind::Ssh(spec) => Ok(NativePath(spec.path)),
            PathKind::LinuxAbsolute { normalized } => Ok(NativePath(normalized)),
            _ => Err(GitError::InvalidPath {
                path: host.0.clone(),
                reason: "SSH repositories need a POSIX path".into(),
            }),
        }
    }
    fn to_host(&self, native: &NativePath) -> Result<HostPath> {
        Err(GitError::InvalidPath {
            path: native.0.clone(),
            reason: "remote repositories have no local path".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_every_input_shape() {
        assert_eq!(
            classify(r"C:\src\stashpanda\"),
            PathKind::WindowsDrive { normalized: r"C:\src\stashpanda".into() }
        );
        assert_eq!(
            classify("c:/src//x"),
            PathKind::WindowsDrive { normalized: r"C:\src\x".into() }
        );
        assert_eq!(
            classify(r"\\wsl.localhost\Ubuntu\home\me\code\x\"),
            PathKind::WslUnc { distro: "Ubuntu".into(), linux_path: "/home/me/code/x".into() }
        );
        assert_eq!(
            classify(r"\\wsl$\Ubuntu\home\brave"),
            PathKind::WslUnc { distro: "Ubuntu".into(), linux_path: "/home/brave".into() }
        );
        assert_eq!(
            classify("//wsl.localhost/Ubuntu/home/me/code"),
            PathKind::WslUnc { distro: "Ubuntu".into(), linux_path: "/home/me/code".into() }
        );
        assert_eq!(
            classify("/mnt/c/Users/Michael/src/"),
            PathKind::LinuxMnt { drive: 'c', rest: "/Users/Michael/src".into(), normalized: "/mnt/c/Users/Michael/src".into() }
        );
        assert_eq!(
            classify("/home/me/code//stashpanda"),
            PathKind::LinuxAbsolute { normalized: "/home/me/code/stashpanda".into() }
        );
        assert_eq!(
            classify("/mnt/data/x"),
            PathKind::LinuxAbsolute { normalized: "/mnt/data/x".into() }
        );
        assert_eq!(
            classify("brave@example.com:/srv/repo"),
            PathKind::Ssh(SshSpec { user: Some("brave".into()), host: "example.com".into(), port: None, path: "/srv/repo".into() })
        );
        assert_eq!(
            classify("ssh://brave@example.com:2222/srv/repo/"),
            PathKind::Ssh(SshSpec { user: Some("brave".into()), host: "example.com".into(), port: Some(2222), path: "/srv/repo".into() })
        );
        assert_eq!(
            classify("staging:/home/me/code/app"),
            PathKind::Ssh(SshSpec { user: None, host: "staging".into(), port: None, path: "/home/me/code/app".into() })
        );
        assert!(matches!(classify("relative/path"), PathKind::Invalid { .. }));
        assert!(matches!(classify("C:relative"), PathKind::Invalid { .. }));
        assert!(matches!(classify(""), PathKind::Invalid { .. }));
    }

    #[test]
    fn windows_and_wsl_round_trip() {
        assert_eq!(windows_to_wsl(r"C:\Users\Michael\src", "/mnt").unwrap(), "/mnt/c/Users/Michael/src");
        assert_eq!(windows_to_wsl(r"d:/x", "/mnt").unwrap(), "/mnt/d/x");
        assert_eq!(windows_to_wsl(r"C:\", "/mnt").unwrap(), "/mnt/c");
        assert_eq!(wsl_to_windows("/mnt/c/Users/Michael/src", "/mnt").unwrap(), r"C:\Users\Michael\src");
        assert_eq!(wsl_to_windows("/mnt/c", "/mnt").unwrap(), r"C:\");
        assert_eq!(windows_to_wsl(r"C:\x", "/custom/").unwrap(), "/custom/c/x");
        assert!(wsl_to_windows("/home/me/code", "/mnt").is_err());
        assert_eq!(linux_to_unc("/home/me/code/x", "Ubuntu"), r"\\wsl.localhost\Ubuntu\home\me\code\x");
    }

    #[test]
    fn mapped_wsl_drive_becomes_unc() {
        let wsl = |d: char| (d == 'W').then(|| r"\\wsl$\Ubuntu".to_string());
        assert_eq!(wsl_drive_to_unc(r"W:\home\me\code\x\", wsl).as_deref(), Some(r"\\wsl$\Ubuntu\home\me\code\x"));
        assert_eq!(wsl_drive_to_unc("w:/home/me/code/x", wsl).as_deref(), Some(r"\\wsl$\Ubuntu\home\me\code\x"));
        assert_eq!(wsl_drive_to_unc(r"W:\", wsl).as_deref(), Some(r"\\wsl$\Ubuntu"));
        assert_eq!(wsl_drive_to_unc(r"C:\src\x", wsl), None);
        assert_eq!(wsl_drive_to_unc(r"\\wsl.localhost\Ubuntu\x", wsl), None);
        assert_eq!(wsl_drive_to_unc(r"S:\x", |_| Some(r"\\fileserver\share\".to_string())), None);
        // The same folder through the drive and through the share is one path once classified.
        assert_eq!(
            classify(&wsl_drive_to_unc(r"W:\home\me\code\x", wsl).unwrap()),
            classify(r"\\wsl.localhost\Ubuntu\home\me\code\x")
        );
    }

    #[test]
    fn wsl_mapper_from_windows() {
        let m = WslMapper { distro: "Ubuntu".into(), automount_root: "/mnt".into() };
        assert_eq!(m.to_native(&HostPath(r"\\wsl.localhost\Ubuntu\home\me\code\x".into())).unwrap(), NativePath("/home/me/code/x".into()));
        assert_eq!(m.to_native(&HostPath(r"C:\src\x".into())).unwrap(), NativePath("/mnt/c/src/x".into()));
        assert!(m.to_native(&HostPath(r"\\wsl.localhost\Debian\home\me\code\x".into())).is_err());
        assert_eq!(m.to_host(&NativePath("/home/me/code/x".into())).unwrap(), HostPath(r"\\wsl.localhost\Ubuntu\home\me\code\x".into()));
        assert_eq!(m.to_host(&NativePath("/mnt/c/src/x".into())).unwrap(), HostPath(r"C:\src\x".into()));
    }

    #[test]
    fn windows_git_from_linux_mapper() {
        let m = WindowsFromLinuxMapper { automount_root: "/mnt".into(), distro: "Ubuntu".into() };
        assert_eq!(m.to_native(&HostPath("/mnt/c/src/x".into())).unwrap(), NativePath(r"C:\src\x".into()));
        assert_eq!(m.to_native(&HostPath("/home/me/code/x".into())).unwrap(), NativePath(r"\\wsl.localhost\Ubuntu\home\me\code\x".into()));
        assert_eq!(m.to_host(&NativePath(r"C:\src\x".into())).unwrap(), HostPath("/mnt/c/src/x".into()));
    }

    /// Cross-check against the real `wslpath` when it exists (WSL only).
    #[test]
    #[ignore]
    fn agrees_with_wslpath() {
        let out = std::process::Command::new("wslpath").args(["-u", r"C:\Users\Michael"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), windows_to_wsl(r"C:\Users\Michael", "/mnt").unwrap());
        let out = std::process::Command::new("wslpath").args(["-w", "/mnt/c/Users/Michael"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), wsl_to_windows("/mnt/c/Users/Michael", "/mnt").unwrap());
    }
}
