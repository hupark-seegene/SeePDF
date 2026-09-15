# SeePDF — project notes for agents

Lightweight, high-performance cross-platform (macOS + Windows) PDF editor with OCR.
Stack: **Tauri v2** (Rust backend) + **React 19 / TypeScript / Vite 8** frontend. PDF engine: **PDFium** via the
`pdfium-render` 0.9 crate (dynamic `libpdfium` shipped in `src-tauri/resources/pdfium/`, per target triple).

## Environment (this Mac, arm64, macOS 26)
- Rust 1.96 lives in `~/.cargo/bin` but is NOT on PATH by default: always `export PATH="$HOME/.cargo/bin:$PATH"`.
- Corporate firewall blocks crates.io; `~/.cargo/config.toml` redirects to the USTC mirror. **Never edit that file.**
  `cargo search` does not work; look up versions at `https://mirrors.ustc.edu.cn/crates.io-index/<prefix>/<crate>`.
  GitHub and npm are reachable. No Homebrew, no Xcode (Command Line Tools only), no system tesseract.
- Node 24 / npm 11. `npm run typecheck` (tsc --noEmit), `npm run build`, `npm run tauri dev`.
- Rust: `cd src-tauri && cargo check` / `cargo test` / `cargo run --example <name>`.
- When working in a git worktree, set `CARGO_TARGET_DIR=/Users/veri/Dev/SeePDF/src-tauri/target` so builds share the
  cache (cargo serialises concurrent builds with a file lock — "Blocking waiting for file lock" is normal, just wait).
  Also run `npm install` in the worktree (node_modules is gitignored; postinstall fetches libpdfium for the host).
- PDFium binaries are gitignored; `npm run fetch:pdfium` (or `-- --all`) downloads them from GitHub releases.
- Sample PDFs for tests/spikes live in `fixtures/`.

## Hard rules
- PDFium is **not thread-safe**. Every pdfium call goes through the single engine thread (`src-tauri/src/engine/`);
  never call pdfium from a Tauri command thread directly.
- Never block the UI: long operations (render, OCR, save) are async with progress + cancellation.
- Do not commit binaries (`*.dylib`, `*.dll`, `*.so`) or `fixtures/` outputs.
- UI strings go through i18n (Korean default, English). The primary user is Korean.
- Keep the app light: no Electron, no heavyweight UI kits; prefer small, tree-shakeable deps.
