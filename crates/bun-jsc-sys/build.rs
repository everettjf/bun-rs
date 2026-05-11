// Tell rustc to link against the JavaScriptCore framework on macOS.
// On other platforms, we'll error out for now (Linux/Windows comes in P4).

fn main() {
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target.as_str() {
        "macos" | "ios" => {
            println!("cargo:rustc-link-lib=framework=JavaScriptCore");
        }
        "linux" => {
            // Future: pkg-config javascriptcoregtk-4.1
            println!(
                "cargo:warning=bun-jsc-sys: Linux JSC linkage not implemented yet (planned for P4)."
            );
        }
        other => {
            println!(
                "cargo:warning=bun-jsc-sys: target_os '{}' not supported.",
                other
            );
        }
    }
}
