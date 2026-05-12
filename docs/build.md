# Build prerequisites

## macOS

The system [`JavaScriptCore.framework`](https://developer.apple.com/documentation/javascriptcore)
ships with every macOS install. No extra steps; `cargo build` just works.

Tested: macOS 15.x (Darwin 25.x) on Apple Silicon. Intel macs should also work.

## Linux (Debian / Ubuntu)

```sh
sudo apt-get install libjavascriptcoregtk-4.1-dev pkg-config
cargo build --workspace
```

`bun-jsc-sys/build.rs` probes for `javascriptcoregtk-4.1`, then 4.0, then 6.0
via pkg-config and falls back to a naive `-ljavascriptcoregtk-4.1` link if
none of those answer.

Tested: not yet — the project author's daily driver is macOS. CI
([`.github/workflows/ci.yml`](../.github/workflows/ci.yml)) runs the test
suite on Ubuntu 24.04 with `libjavascriptcoregtk-4.1-dev`; that's the
authoritative signal for "Linux still works."

Known risks:
- JSC's public C API has been stable for a decade, but Linux distros
  package different upstream commits. If a symbol is missing on your
  distro's build, please file an issue with `apt list libjavascriptcoregtk-4.1`.

## Linux (Fedora)

```sh
sudo dnf install webkitgtk4.1-devel pkg-config
cargo build --workspace
```

## Linux (Arch)

```sh
sudo pacman -S webkit2gtk-4.1 pkg-config
cargo build --workspace
```

## Windows

Not supported. WebKit/JSC has no official Windows build.

Future options if Windows demand grows:
- Build JSC from upstream source via `bun/scripts/build/deps/webkit.ts`
  (5–10 min build; produces a static `.lib`).
- Switch the engine for the Windows target only (rusty_v8, QuickJS).
