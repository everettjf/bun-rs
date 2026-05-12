// Link against JavaScriptCore.
//
//   macOS/iOS: the framework that ships with the OS.
//   Linux:    `javascriptcoregtk-4.1` (`libjavascriptcoregtk-4.1.so`),
//              installed via `webkit2gtk` on Debian/Ubuntu / `webkit2gtk4.1`
//              on Fedora / similar on Arch.
//
// Windows is not supported — JSC isn't shipped publicly on Windows.

fn main() {
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target.as_str() {
        "macos" | "ios" => {
            println!("cargo:rustc-link-lib=framework=JavaScriptCore");
        }
        "linux" | "freebsd" | "netbsd" | "openbsd" | "dragonfly" => {
            // Prefer pkg-config so we pick up the right -L/-l flags
            // regardless of distro layout.
            let candidates = [
                "javascriptcoregtk-4.1",
                "javascriptcoregtk-4.0",
                "javascriptcoregtk-6.0",
            ];
            let mut found = false;
            for c in &candidates {
                if pkg_config::Config::new()
                    .atleast_version("2.20")
                    .probe(c)
                    .is_ok()
                {
                    println!("cargo:warning=bun-jsc-sys: linking {c}");
                    found = true;
                    break;
                }
            }
            if !found {
                println!(
                    "cargo:warning=bun-jsc-sys: pkg-config could not find any \
                     javascriptcoregtk variant. Install libjavascriptcoregtk-4.1-dev \
                     (Debian/Ubuntu) or webkitgtk4.1-devel (Fedora)."
                );
                // Fall back to a naive direct link — many systems have
                // libjavascriptcoregtk-4.1.so on the default lib path.
                println!("cargo:rustc-link-lib=javascriptcoregtk-4.1");
            }
        }
        other => {
            println!(
                "cargo:warning=bun-jsc-sys: target_os '{}' is not supported. \
                 Only macOS and Linux are tested.",
                other
            );
        }
    }
}
