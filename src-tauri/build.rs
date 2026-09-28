fn main() {
    // Windows: tauri-build embeds the application manifest (Common Controls v6, DPI awareness)
    // into the *binaries* only. The integration-test executables link the same tauri / rfd code,
    // which imports comctl32 v6 entry points (TaskDialogIndirect); without the manifest the
    // loader binds comctl32 v5 and every `tests/*.rs` binary died before `main` with
    // STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) — the first Windows test run, v0.2.0-rc1.
    // The same dependency is declared for tests and examples here.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        const COMMON_CONTROLS_V6: &str = "/MANIFESTDEPENDENCY:type='win32' \
            name='Microsoft.Windows.Common-Controls' version='6.0.0.0' \
            processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'";
        for kind in ["tests", "examples", "benches"] {
            println!("cargo:rustc-link-arg-{kind}=/MANIFEST:EMBED");
            println!("cargo:rustc-link-arg-{kind}={COMMON_CONTROLS_V6}");
        }
    }
    tauri_build::build()
}
