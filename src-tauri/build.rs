use std::path::Path;

fn main() {
    // On Windows the app needs version 6 of comctl32.dll (tao and the dialog plugin import
    // `TaskDialogIndirect`, which only version 6 exports), and Windows loads version 6 only for
    // a program whose embedded manifest asks for it. tauri-build embeds that manifest with
    // `rustc-link-arg-bins`, which reaches the app binary but not the test binaries, so
    // `cargo test` on Windows used to exit with STATUS_ENTRYPOINT_NOT_FOUND before any test ran.
    // `rustc-link-arg` reaches every binary the crate links, tests included.
    let mut windows = tauri_build::WindowsAttributes::new();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        windows = tauri_build::WindowsAttributes::new_without_app_manifest();
        let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
        let manifest = Path::new(&dir).join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed=windows-app-manifest.xml");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
    // tauri-build compiles icons/icon.ico into the Windows resource that Explorer shows as the
    // .exe's icon, but does not tell Cargo to watch it, so a build after `npm run icons` kept the
    // old icon until something else made this script run again.
    println!("cargo:rerun-if-changed=icons/icon.ico");
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("tauri-build failed");
}
