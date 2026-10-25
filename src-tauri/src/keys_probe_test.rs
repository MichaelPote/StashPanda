//! Read-only probe over real key files. Run with
//! `STASHPANDA_KEY_DIRS=<folder>:<folder> cargo test -- --ignored probe_real_keys --nocapture`
//! (folders separated as in `PATH`); without it, `~/.ssh` is read.
#[cfg(test)]
mod probe {
    #[test]
    #[ignore]
    fn probe_real_keys() {
        let dirs: Vec<std::path::PathBuf> = match std::env::var_os("STASHPANDA_KEY_DIRS") {
            Some(list) => std::env::split_paths(&list).collect(),
            None => vec![std::path::Path::new(&std::env::var_os("HOME").expect("HOME is set")).join(".ssh")],
        };
        for info in crate::keys::scan(&dirs) {
            println!(
                "{:28} | {:<9} | enc={:<5} | {:<12} | {:<4} | {} | {}",
                info.file_name,
                format!("{:?}", info.format),
                info.encrypted,
                info.algorithm.clone().unwrap_or_default(),
                info.bits.map(|b| b.to_string()).unwrap_or_default(),
                info.fingerprint.clone().unwrap_or_else(|| "-".into()),
                info.error.clone().unwrap_or_default()
            );
        }
    }
}
