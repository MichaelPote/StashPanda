//! Read-only probe over real key files. Run with
//! `cargo test -- --ignored probe_real_keys --nocapture`.
#[cfg(test)]
mod probe {
    #[test]
    #[ignore]
    fn probe_real_keys() {
        let dirs: Vec<std::path::PathBuf> = ["/home/me/.ssh"].iter().map(Into::into).collect();
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
