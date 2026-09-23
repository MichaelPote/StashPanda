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
