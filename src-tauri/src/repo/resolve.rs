//! Turn a library entry into a concrete transport and native path.

use std::sync::Arc;

use crate::error::{GitError, Result};
use crate::platform::Host;
use crate::settings::{Connection, RepoEntry, ResolvedTransport, Settings};
use crate::transport::discover::{find_local_git, find_windows_git_from_linux};
use crate::transport::local::LocalTransport;
use crate::transport::paths::{
    classify, wsl_drive_to_unc, HostPath, IdentityMapper, NativePath, PathKind, PathMapper, WindowsFromLinuxMapper, WslMapper,
};
use crate::transport::ssh::SshTransport;
use crate::transport::ssh_pool::{resolve_target, SshContext};
use crate::transport::wsl::WslTransport;
use crate::transport::SharedTransport;

pub struct Resolution {
    pub transport: SharedTransport,
    pub resolved: ResolvedTransport,
    pub host_path: HostPath,
    pub native_path: NativePath,
    pub warnings: Vec<String>,
}

/// The transport that `Connection::Auto` picks for `path` on this host.
pub fn auto_connection(path: &str, host: &Host, settings: &Settings) -> Result<Connection> {
    match (classify(path), host) {
        (PathKind::Invalid { reason }, _) => Err(GitError::InvalidPath {
            path: path.to_string(),
            reason,
        }),
        (PathKind::Ssh(spec), _) => Ok(Connection::Ssh {
            host: spec.host,
            user: spec.user,
            port: spec.port,
            identity_file: None,
            remote_git: None,
        }),
        (PathKind::WslUnc { distro, .. }, Host::Windows) => Ok(Connection::Wsl { distro }),
        (PathKind::WslUnc { distro, .. }, Host::LinuxWsl { distro: mine }) if distro.eq_ignore_ascii_case(mine) => {
            Ok(Connection::Local)
        }
        (PathKind::WslUnc { distro, .. }, _) => Err(GitError::InvalidPath {
            path: path.to_string(),
            reason: format!("a path inside WSL distro {distro} can only be opened from Windows or from that distro"),
        }),
        (PathKind::WindowsDrive { .. }, Host::Windows) => Ok(Connection::Local),
        (PathKind::WindowsDrive { .. }, Host::LinuxWsl { .. }) => Ok(Connection::Local),
        (PathKind::WindowsDrive { .. }, _) => Err(GitError::InvalidPath {
            path: path.to_string(),
            reason: "Windows drive paths cannot be opened from this machine".into(),
        }),
        (PathKind::LinuxMnt { .. }, Host::LinuxWsl { .. }) => Ok(Connection::Local),
        (PathKind::LinuxAbsolute { .. }, Host::Windows) => Ok(Connection::Wsl {
            distro: settings
                .git
                .wsl_default_distro
                .clone()
                .unwrap_or_else(|| "Ubuntu".to_string()),
        }),
        (PathKind::LinuxAbsolute { .. } | PathKind::LinuxMnt { .. }, _) => Ok(Connection::Local),
    }
}

pub fn resolve(entry: &RepoEntry, settings: &Settings, host: &Host, ssh: Option<&Arc<SshContext>>) -> Result<Resolution> {
    let mut warnings = Vec::new();
    // A drive letter mapped to a WSL share is that share, so `W:\x` gets the same transport and
    // key as `\\wsl.localhost\Ubuntu\x`. A Local connection chosen by hand keeps Windows git.
    let unmapped = match (&entry.connection, host) {
        (Connection::Auto | Connection::Wsl { .. }, Host::Windows) => {
            wsl_drive_to_unc(&entry.path, crate::platform::mapped_drive_remote)
        }
        _ => None,
    };
    let path = unmapped.as_deref().unwrap_or(&entry.path);
    let kind = classify(path);
    if let PathKind::Invalid { reason } = &kind {
        return Err(GitError::InvalidPath {
            path: entry.path.clone(),
            reason: reason.clone(),
        });
    }
    let connection = match &entry.connection {
        Connection::Auto => auto_connection(path, host, settings)?,
        other => other.clone(),
    };
    let automount = settings.git.wsl_automount_root.clone();
    let automount = if automount.trim().is_empty() { "/mnt".to_string() } else { automount };

    match connection {
        Connection::Auto => unreachable!("resolved above"),
        Connection::Local => {
            let host_path = HostPath(match &kind {
                PathKind::WindowsDrive { normalized } => normalized.clone(),
                PathKind::LinuxAbsolute { normalized } | PathKind::LinuxMnt { normalized, .. } => normalized.clone(),
                PathKind::WslUnc { linux_path, .. } => linux_path.clone(),
                PathKind::Ssh(_) | PathKind::Invalid { .. } => {
                    return Err(GitError::InvalidPath {
                        path: entry.path.clone(),
                        reason: "an SSH location cannot use the Local connection".into(),
                    })
                }
            });
            // Windows git for /mnt/<d>/ paths from inside WSL, Linux git otherwise.
            let use_windows_git = matches!(&kind, PathKind::LinuxMnt { .. } | PathKind::WindowsDrive { .. })
                && matches!(host, Host::LinuxWsl { .. });
            if use_windows_git {
                let distro = match host {
                    Host::LinuxWsl { distro } => distro.clone(),
                    _ => "Ubuntu".into(),
                };
                match find_windows_git_from_linux(
                    entry.git_executable.as_deref().or(settings.git.windows_executable_from_linux.as_deref()),
                    &automount,
                ) {
                    Some(git) => {
                        let mapper = WindowsFromLinuxMapper { automount_root: automount.clone(), distro };
                        let native_path = mapper.to_native(&host_path)?;
                        let git_s = git.to_string_lossy().into_owned();
                        let transport: SharedTransport = Arc::new(LocalTransport {
                            git,
                            mapper: Box::new(mapper),
                            windows_git_from_linux: true,
                        });
                        return Ok(Resolution {
                            transport,
                            resolved: ResolvedTransport::Local { git_executable: git_s },
                            host_path,
                            native_path,
                            warnings,
                        });
                    }
                    None => warnings.push(
                        "No Windows git.exe was found under the automount root, so Linux git is used for this Windows folder. Line endings and file modes may be reported differently.".into(),
                    ),
                }
            }
            if matches!(&kind, PathKind::WslUnc { .. }) && host.is_windows() {
                warnings.push("Windows git over a \\\\wsl.localhost path is slow and usually hits 'dubious ownership'. Prefer the WSL connection.".into());
            }
            let override_exe = entry.git_executable.as_deref().or(match host {
                Host::Windows => settings.git.windows_executable.as_deref(),
                _ => settings.git.linux_executable.as_deref(),
            });
            let git = find_local_git(override_exe, host)?;
            let git_s = git.to_string_lossy().into_owned();
            let transport: SharedTransport = Arc::new(LocalTransport {
                git,
                mapper: Box::new(IdentityMapper),
                windows_git_from_linux: false,
            });
            Ok(Resolution {
                transport,
                resolved: ResolvedTransport::Local { git_executable: git_s },
                native_path: NativePath(host_path.0.clone()),
                host_path,
                warnings,
            })
        }
        Connection::Wsl { distro } => {
            if !host.is_windows() {
                return Err(GitError::InvalidPath {
                    path: entry.path.clone(),
                    reason: "the WSL connection is only available from the Windows build; inside WSL use Local".into(),
                });
            }
            let mapper = WslMapper { distro: distro.clone(), automount_root: automount };
            let host_path = HostPath(match &kind {
                PathKind::WindowsDrive { normalized } => normalized.clone(),
                PathKind::WslUnc { .. } => path.trim().to_string(),
                PathKind::LinuxAbsolute { normalized } => crate::transport::paths::linux_to_unc(normalized, &distro),
                _ => {
                    return Err(GitError::InvalidPath {
                        path: entry.path.clone(),
                        reason: "cannot map this path into WSL".into(),
                    })
                }
            });
            let native_path = mapper.to_native(&host_path)?;
            let git = entry.git_executable.clone().unwrap_or_else(|| "git".to_string());
            let transport: SharedTransport = Arc::new(WslTransport { distro: distro.clone(), git, mapper });
            Ok(Resolution {
                transport,
                resolved: ResolvedTransport::Wsl { distro },
                host_path,
                native_path,
                warnings,
            })
        }
        Connection::Ssh { ref remote_git, .. } => {
            let Some(ctx) = ssh else {
                return Err(GitError::SshUnsupportedFeature { feature: "SSH is not initialised in this context".into() });
            };
            let spec = match &kind {
                PathKind::Ssh(spec) => spec.clone(),
                PathKind::LinuxAbsolute { normalized } => crate::transport::paths::SshSpec {
                    user: None,
                    host: String::new(),
                    port: None,
                    path: normalized.clone(),
                },
                _ => {
                    return Err(GitError::InvalidPath {
                        path: entry.path.clone(),
                        reason: "an SSH repository needs `user@host:/path` or a POSIX path with an SSH connection".into(),
                    })
                }
            };
            if spec.host.is_empty() && !matches!(&connection, Connection::Ssh { host, .. } if !host.trim().is_empty()) {
                return Err(GitError::InvalidPath { path: entry.path.clone(), reason: "no SSH host given".into() });
            }
            let target = resolve_target(&spec, &connection)?;
            let native_path = NativePath(spec.path.clone());
            let resolved = ResolvedTransport::Ssh { host: target.host.clone(), user: Some(target.user.clone()), port: Some(target.port) };
            let transport: SharedTransport = Arc::new(SshTransport::new(target, remote_git.clone(), ctx.clone()));
            Ok(Resolution {
                transport,
                resolved,
                host_path: HostPath(entry.path.trim().to_string()),
                native_path,
                warnings,
            })
        }
    }
}

/// Stable library key: UUID v5 of the resolved transport and native path.
pub fn repo_key(resolved: &ResolvedTransport, native_path: &NativePath) -> String {
    let tag = match resolved {
        ResolvedTransport::Local { .. } => "local".to_string(),
        ResolvedTransport::Wsl { distro } => format!("wsl:{}", distro.to_ascii_lowercase()),
        ResolvedTransport::Ssh { host, user, port } => {
            format!("ssh:{}@{}:{}", user.clone().unwrap_or_default(), host.to_ascii_lowercase(), port.unwrap_or(22))
        }
    };
    let material = format!("{tag}|{}", native_path.0);
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, material.as_bytes())
        .simple()
        .to_string()
}
