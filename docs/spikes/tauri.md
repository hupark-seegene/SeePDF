# Tauri v2 integration spike

Status: **compiles (`cargo check` clean, `tsc --noEmit` clean)**; live run: see "Live verification" below.
Versions pinned in `Cargo.lock` at the time of the spike: tauri 2.11.5, tauri-runtime-wry 2.11.4, wry 0.55.1,
tauri-utils 2.9.3, tauri-plugin-dialog 2.7.3, tauri-plugin-fs 2.5.2, tauri-plugin-window-state 2.4.1,
tauri-plugin-store 2.4.4, pdfium-render 0.9.4 (`pdfium_latest` = `pdfium_7881` bindings; runtime lib chromium/8057),
image 0.25.10, tokio 1.53.1 (transitive), `@tauri-apps/api` 2.11.1, `@tauri-apps/cli` 2.11.4.

Code: `src-tauri/src/spike/{mod,engine,protocol,ipc,jobs,files,pdfium_path}.rs`, `src-tauri/src/lib.rs`,
`src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`, `src/App.tsx`, `src/spike/{seepdfUrl,bench}.ts`,
`src/spike/spike.css`. The `spike` module is throw-away; copy the patterns, then delete it.

---

## 1. Custom URI scheme `seepdf://` (page pixels as PNG)

### API (verified in tauri 2.11.5 `src/app.rs`)

```rust
// asynchronous variant — handler must return immediately and answer later from any thread
pub fn register_asynchronous_uri_scheme_protocol<N: Into<String>, H>(self, uri_scheme: N, handler: H) -> Self
where H: Fn(UriSchemeContext<'_, R>, http::Request<Vec<u8>>, UriSchemeResponder) + Send + Sync + 'static;

impl UriSchemeResponder { pub fn respond<T: Into<Cow<'static, [u8]>>>(self, response: http::Response<T>) }
// UriSchemeResponderFn = Box<dyn FnOnce(http::Response<Cow<'static,[u8]>>) + Send>  => the responder IS Send
impl UriSchemeContext<'_, R> { pub fn app_handle(&self) -> &AppHandle<R>; pub fn webview_label(&self) -> &str }

// synchronous variant (blocks the protocol thread; do not use for rendering)
pub fn register_uri_scheme_protocol<N, T: Into<Cow<'static,[u8]>>, H>(self, uri_scheme: N, handler: H) -> Self
where H: Fn(UriSchemeContext<'_, R>, http::Request<Vec<u8>>) -> http::Response<T> + Send + Sync + 'static;
```

`tauri::http` re-exports the `http` crate (`Request`, `Response`, `StatusCode`, `header::*`); `tauri::Url` re-exports
`url::Url` — use it to parse the request URI and `query_pairs()`.

### Threading

wry's `with_asynchronous_custom_protocol` calls the handler on the webview's scheme-handler thread (main thread on
macOS/WKWebView). The handler in `spike/protocol.rs` only parses the URL and then hands the `UriSchemeResponder` to the
engine thread inside a `Reply::from_fn(move |png| responder.respond(..))` callback; the engine thread renders, encodes
PNG and answers the webview directly. The spike's `/slow?ms=2000` route sleeps 2 s on the engine thread while a
100 ms UI ticker keeps running (see numbers below), proving nothing blocks.

### URL shape (verified in tauri's injected `scripts/core.js`, `convertFileSrc`)

| platform | URL |
|---|---|
| macOS, Linux, iOS | `seepdf://localhost/page?doc=tracemonkey&page=0&scale=1.5` |
| Windows, Android | `http://seepdf.localhost/page?doc=…` (`https://seepdf.localhost` when the window has `useHttpsScheme: true`) |

Do not sniff the OS in JS: `convertFileSrc('', 'seepdf')` (from `@tauri-apps/api/core`) returns the platform-correct
origin (`seepdf://localhost/` or `http://seepdf.localhost/`) because Tauri fills it from `osName`/`protocolScheme`.
`src/spike/seepdfUrl.ts` wraps this: `seepdfUrl('/page', {doc, page, scale, tx, ty, tile})`.

### Routes implemented

| route | behaviour |
|---|---|
| `/page?doc&page&scale[&tx&ty&tile]` | 200 `image/png`; `ETag`, `Cache-Control: private, max-age=0, must-revalidate`, `Access-Control-Allow-Origin: *`, `Access-Control-Expose-Headers: *`, `X-Render-Ms`, `X-Encode-Ms`, `X-Image-Width/Height`. `If-None-Match` hit → 304 without touching the engine. Bad params → 400, unknown doc → 404, render error (bad page/tile) → 500, engine thread gone → the request is dropped (log). |
| `/raw?doc` | 200/206 `application/pdf`, honours `Range: bytes=a-b` (`Accept-Ranges`, `Content-Range`, 416). Pattern for streaming the original file to JS. |
| `/slow?ms` | sleeps on the engine thread, answers text. |
| `/ping` | 200 text (sync). |
| other | 404 |

Errors: build any `http::Response` with the right `StatusCode` and call `responder.respond(r)` — there is no error
channel; a dropped responder without `respond` makes the `<img>` fire `onerror` after the webview times out, so always
respond.

### CSP (production only)

`tauri.conf.json → app.security.csp` (map form; tauri adds nonces/hashes for its own injected scripts). Tauri v2
delivers it as the `Content-Security-Policy` **HTTP header** of the `tauri://localhost` response
(`tauri/src/protocol/tauri.rs`), not as a `<meta>` tag, so `document.querySelector('meta[http-equiv=…]')` is
empty in production; the only observable is a `securitypolicyviolation` event.

```json
"csp": {
  "default-src": "'self' ipc: http://ipc.localhost",
  "connect-src": "'self' ipc: http://ipc.localhost seepdf: http://seepdf.localhost https://seepdf.localhost",
  "img-src":     "'self' data: blob: seepdf: http://seepdf.localhost https://seepdf.localhost",
  "style-src":   "'self' 'unsafe-inline'",
  "font-src":    "'self' data:"
}
```

`img-src` needs `seepdf:` (macOS/Linux) **and** `http://seepdf.localhost` (Windows). `connect-src` is needed if JS
`fetch()`es the scheme (the spike does, to read headers / test 304 and 206). In `tauri dev` the HTML comes from Vite,
so this CSP is **not applied**; test CSP with `tauri build --debug`.

`fetch()` from the page origin (`http://localhost:1420` in dev, `tauri://localhost` in prod) to `seepdf://` is
cross-origin: responses must carry `Access-Control-Allow-Origin: *` (and `Access-Control-Expose-Headers` for custom
headers) or `fetch` rejects; plain `<img>` loads do not need CORS.

---

## 2. Binary IPC

### API (verified in tauri 2.11.5 `src/ipc/mod.rs`, `src/ipc/protocol.rs`, `scripts/ipc-protocol.js`)

```rust
// Rust -> JS raw bytes
#[tauri::command] fn ipc_get_bytes(n: usize) -> tauri::ipc::Response { tauri::ipc::Response::new(vec![..]) }
//   Response::new(body: impl Into<InvokeResponseBody>)  — From<Vec<u8>> => InvokeResponseBody::Raw
//   the reply has Content-Type application/octet-stream and the JS side does response.arrayBuffer()

// JS -> Rust raw bytes
#[tauri::command] fn ipc_put_bytes(request: tauri::ipc::Request<'_>) -> Result<PutInfo, String> {
    match request.body() { InvokeBody::Raw(bytes) => .., InvokeBody::Json(_) => Err(..) }
    // request.headers(): &HeaderMap — extra metadata goes in headers because the body is the payload
}
```

JS:

```ts
const buf = await invoke<ArrayBuffer>('ipc_get_bytes', { n });           // ArrayBuffer, zero JSON
await invoke<PutInfo>('ipc_put_bytes', new Uint8Array(buf), { headers: { 'x-doc': 'id' } });
// processIpcMessage: ArrayBuffer / ArrayBuffer.isView(args) => body sent as-is with application/octet-stream
// anything else => JSON.stringify with a replacer that walks EVERY value (Uint8Array inside an object becomes a number[])
```

Async commands can return `tauri::ipc::Response` too (`render_page_png_ipc` in `lib.rs` does).

### Measured (14 MiB = 14 680 064 bytes, debug build, Apple Silicon, WKWebView) — see "Live verification"

Three consecutive runs gave the same numbers within ±5 %; the run below is the third (`RUST_LOG` on, another
agent's release `cargo build` running on the same machine). Timing is `performance.now()` around `await invoke(...)`
in the webview, including any JS-side encode/decode.

| transport (14 MiB) | ms | throughput | notes |
|---|---:|---:|---|
| Rust → JS raw (`ipc::Response` → `ArrayBuffer`) | **99** | 141 MB/s | zero-copy on the JS side; pattern verified |
| JS → Rust raw (`Uint8Array` → `InvokeBody::Raw`) | **75** | 187 MB/s | header `x-doc` arrived intact; checksum verified |
| Rust → JS base64 string (JSON) + `atob` | 832 | 17 MB/s | invoke alone 806 ms (serde string + JSON parse of 19 MB), atob 26 ms |
| JS → Rust base64 string (`btoa` + JSON) | 347 | 40 MB/s | btoa 41 ms, rest is JSON transport + Rust decode |
| Rust → JS `Vec<u8>` as JSON number array | 1 395 | 10 MB/s | serde emits 14 M numbers (~40 MB text), `JSON.parse`, `Uint8Array.from` |
| JS → Rust `number[]` as JSON arg (`Vec<u8>`) | 2 134 | 7 MB/s | tauri's replacer walks 14 M elements; `Array.from` alone 124 ms |

Raw binary IPC is **8–28× faster** than the JSON/base64 paths and needs no JS decode step. 14 MiB round trip
(Rust→JS then JS→Rust) is ~175 ms in a debug build.

Page pixels, same page (`tracemonkey` p.1 at scale 1.5 → 918×1188, 707 KB PNG), debug build:

| path | wall time in webview | of which engine (render + PNG encode) |
|---|---:|---:|
| `<img src="seepdf://localhost/page?…">` (first paint after `src` set) | 551–613 ms | 262–269 ms (render 2–7 ms, PNG encode ~260 ms) |
| `invoke('render_page_png_ipc')` → `Blob` → `createObjectURL` → `<img>` | 262–273 ms | same 262 ms |
| 4 × 256 px tiles via protocol (`&tx&ty&tile=256`) | ~300 ms total, queued behind the full page on the single engine thread | 4 ms render + 6–14 ms encode each |

Note the `<img>` number includes WebKit's image decode + layout and the tiles sharing the engine queue; the transport
itself is a few ms in both cases — **PNG encoding (debug build) is the cost**, not IPC vs protocol.

---

## 3. Managed state, engine thread, async commands, progress + cancel

* `EngineHandle { tx: crossbeam_channel::Sender<EngineRequest>, docs: Arc<RwLock<..>> }` is `app.manage()`d in
  `setup` and read with `tauri::State<'_, EngineHandle>`.
* Every request carries a `Reply<T>(Box<dyn FnOnce(T) + Send>)`. `Reply::oneshot()` returns
  `(Reply, tauri::async_runtime::Receiver<T>)` built on `tauri::async_runtime::channel(1)` (tokio mpsc). tauri does
  **not** re-export `tokio::sync::oneshot` and `tokio` is not a direct dependency, so mpsc(1) is the zero-dependency
  oneshot; add `tokio = { features = ["sync"] }` if a real `oneshot` is preferred.
* Async commands that take `State` must return `Result<_, _>` (tauri restriction), e.g.
  `async fn engine_ping(engine: State<'_, EngineHandle>) -> Result<String, String> { engine.ping().await }`.
* pdfium lives only on the `seepdf-engine` thread. `Pdfium::bind_to_library(path) -> Result<Box<dyn PdfiumLibraryBindings>, PdfiumError>`
  may be called **once per process** (global `OnceCell`; second call → `PdfiumLibraryBindingsAlreadyInitialized`).
  `Pdfium::new(bindings)` is leaked (`Box::leak`) to get `&'static Pdfium`, so `PdfDocument<'static>` can be kept
  in a `HashMap` on that thread.
* Long job: `start_job(app: AppHandle, jobs: State<Jobs>, steps, step_ms, mode, on_progress: tauri::ipc::Channel<JobEvent>) -> u64`
  spawns a worker thread that polls an `Arc<AtomicBool>` per step; `cancel_job(job_id) -> bool` flips it.
  Progress goes through `on_progress.send(ev)` (`Channel<T: Serialize>`; JS: `const ch = new Channel<JobEvent>(); ch.onmessage = …; invoke('start_job', { …, onProgress: ch })`)
  and/or `app.emit("job-progress", ev)` (`tauri::Emitter` trait; JS `listen('job-progress', …)`).

Measured, 2 000 progress messages with `step_ms = 0` (pure IPC cost), three alternating runs:

| transport | webview receive time for 2 000 msgs | msg/s | Rust-side loop |
|---|---:|---:|---:|
| `tauri::ipc::Channel<JobEvent>` | 48 / 23 / 20 ms | 42 k – 100 k | 18–20 ms |
| `app.emit("job-progress", ev)` + `listen` | 29 / 28 / 33 ms | 60 k – 71 k | 23–32 ms |

Both are far above any realistic progress rate (OCR/page-level progress is <100 msg/s). The Channel is
per-invocation (no global event name, no other windows receive it, ordering guaranteed by index), which is the
reason to prefer it, not raw speed. Cancel: a 400-step × 50 ms job cancelled from JS after 1.5 s stopped at
26–27/400 (`cancel_job` returned `true`, `Cancelled { done: 26 }` delivered on the same Channel).

---

## 4. File open

### Dialog plugin (no npm package needed — call the plugin commands directly)

```ts
const path = await invoke<string | null>('plugin:dialog|open', { options: { title: 'Open PDF', multiple: false, directory: false, filters: [{ name: 'PDF', extensions: ['pdf'] }] } });
const out  = await invoke<string | null>('plugin:dialog|save', { options: { title: 'Save PDF as', defaultPath: 'copy.pdf', filters: [...] } });
```

`OpenDialogOptions { title?, filters: [{name, extensions}], multiple, directory, defaultPath?, recursive, canCreateDirectories? }`
(camelCase). `open` returns `string | null` for a single file, `string[] | null` with `multiple: true`. The dialog plugin
adds every picked path to the fs plugin scope (`window.try_fs_scope().allow_file/allow_directory`), so JS can read a
picked file without a static scope entry.

### Capabilities (`src-tauri/capabilities/default.json`)

```json
"permissions": [
  "core:default",
  "core:window:allow-start-dragging",   // data-tauri-drag-region on the custom title bar
  "core:window:allow-set-title",
  "opener:default",
  "dialog:default",                     // allow-open, allow-save, allow-message
  "fs:default",                         // app dirs only
  { "identifier": "fs:allow-read-file", "allow": [ { "path": "$HOME/**" }, { "path": "$DESKTOP/**" }, { "path": "$DOCUMENT/**" }, { "path": "$DOWNLOAD/**" }, { "path": "$TEMP/**" } ] },
  "fs:allow-stat",
  "store:default",
  "window-state:default"
]
```

`plugin:fs|read_file` returns `tauri::ipc::Response` (raw `ArrayBuffer` in JS):
`await invoke<ArrayBuffer>('plugin:fs|read_file', { path, options: {} })`. In SeePDF the Rust engine reads files,
so the fs scope is only needed for JS-side reads of dropped/argv paths.

### Drag and drop

`tauri.conf.json` window `dragDropEnabled: true` (default). JS:

```ts
const unlisten = await getCurrentWebview().onDragDropEvent((e) => {
  if (e.payload.type === 'drop') openFiles(e.payload.paths);  // 'enter' | 'over' | 'drop' | 'leave'
});
```

Rust mirror: `window.on_window_event(|ev| if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, position }) = ev { .. })`.
Both fire for the same drop.

### CLI argument / Finder double-click

* Windows (and `cargo run -- file.pdf`, `tauri dev -- -- -- file.pdf`): read `std::env::args_os().skip(1)` in `setup`.
* macOS: `app.run(|app, event| if let RunEvent::Opened { urls } = event { … })` — `urls` are `file://` URLs
  (`Url::to_file_path()`), delivered for Finder double-click, `open -a SeePDF x.pdf` and Dock drops. It only works
  from a **bundled** `.app` whose `Info.plist` has `CFBundleDocumentTypes`, which `bundle.fileAssociations` generates:

```json
"fileAssociations": [{ "ext": ["pdf"], "name": "PDF Document", "description": "PDF Document", "mimeType": "application/pdf", "role": "Editor", "rank": "Alternate" }]
```

* Timing: `Opened` can arrive before the webview has registered listeners, so `spike/files.rs` both queues the path
  (`PendingOpens`, drained by the `take_pending_opens` command on mount) and emits `open-file`.

### Window config

```json
{ "label": "main", "title": "SeePDF", "width": 1400, "height": 900, "minWidth": 800, "minHeight": 520,
  "titleBarStyle": "Overlay", "hiddenTitle": true, "dragDropEnabled": true }
```

`titleBarStyle` values are `Visible | Transparent | Overlay` (capitalised). Overlay + hiddenTitle puts the traffic
lights over the web content: reserve ~34 px at the top and mark the strip `data-tauri-drag-region` (needs
`core:window:allow-start-dragging`). On Windows the property is ignored, so the same layout renders a normal title bar.
`tauri_plugin_window_state::Builder::new().build()` restores size/position on launch and saves on close; the saved
state overrides `width/height` from the config (min size still applies).

---

## 5. Resources: shipping libpdfium

`tauri.conf.json`: `"bundle": { "resources": ["resources/pdfium/*"] }` (list form keeps the relative path, so files land
at `<resource_dir>/resources/pdfium/<file>`). `tauri-build` also copies them next to the dev executable
(`target/debug/resources/pdfium/…`) on every platform, so `app.path().resource_dir()` works in **both** dev and
bundle:

| context | `resource_dir()` |
|---|---|
| `tauri dev` / `cargo run` | `src-tauri/target/debug` |
| macOS `.app` | `SeePDF.app/Contents/Resources` |
| Windows | directory of `SeePDF.exe` |

`spike/pdfium_path.rs` tries `<resource_dir>/resources/pdfium`, `<resource_dir>/pdfium`, then (debug builds only)
`CARGO_MANIFEST_DIR/resources/pdfium`, and picks the file by `(std::env::consts::OS, ARCH)`.
A glob that matches nothing makes `build.rs` fail (`GlobPathNotFound`); `LICENSE.pdfium` and `VERSION` are committed,
so the glob is always non-empty even before `npm run fetch:pdfium`.

Bloat warning: the glob ships all four platform binaries (~30 MB). Use platform config overlays
(`src-tauri/tauri.macos.conf.json` with `"resources": ["resources/pdfium/*apple-darwin.dylib", "resources/pdfium/LICENSE.pdfium", "resources/pdfium/VERSION"]`,
`tauri.windows.conf.json` with the dll) — they merge over `tauri.conf.json` automatically.

Verified at startup (dev): `resolved bundled libpdfium lib=…/src-tauri/target/debug/resources/pdfium/libpdfium-aarch64-apple-darwin.dylib`,
`pdfium bound on engine thread pdfium_version=155.0.8057.0 bind_ms=1.6`, `startup smoke test: tracemonkey.pdf opened via pdfium pages=14 ms=0.26`.
`Pdfium::bind_to_library` + `Pdfium::new` cost 1.6–4 ms; opening a 1 MB PDF from disk 0.3–1.5 ms.

---

## 6. Live verification

`npm run tauri dev -- --no-watch -- -- /Users/veri/Dev/SeePDF/fixtures/160F-2019.pdf`, screenshot
`screencapture -x /tmp/seepdf-spike.png` viewed and confirmed (copy in the session scratchpad as `seepdf-spike-dev.png`).
The window (1400×900, overlay title bar with traffic lights over our `data-tauri-drag-region` strip, macOS/aarch64
debug) shows:

* Engine panel: `engine_ping: pong from Some("seepdf-engine") (pdfium 155.0.8057.0, 1 doc(s) open)`, pdfium dir and
  `resource_dir()` paths, argv including the PDF, documents `doc1 (1p)` (from argv) and `tracemonkey (14p)`.
* Protocol panel: the page rendered through `<img src="seepdf://localhost/page?doc=doc1&page=0&scale=1.5">`
  (551 ms first paint), four 256 px tiles, the same page via binary IPC (263 ms, 707 KB), and the probe list:
  `GET /page -> 200 image/png, 723779 B, render 2.06 ms, encode 259.92 ms, 918x1188, etag …`;
  second `fetch` of the same URL → 200 while the Rust log shows `If-None-Match hit -> 304` (WebKit revalidated on
  its own and served its cache); manual `If-None-Match` → `TypeError: Load failed`; `/raw` with
  `Range: bytes=0-1023` → `206 bytes 0-1023/1016315`; `/nope` → 404; unknown doc → 404; page 9999 → 500
  `PageIndexOutOfBounds`; tile out of range → 500.
* File panel: argv file opened through the pending queue (`pending: opened "doc1"`), `plugin:fs|read_file ->
  1016315 bytes in 1.0 ms`, `cancel_job(7) -> true`. Dialog buttons and the drop zone are wired (not exercised by
  automation; `onDragDropEvent` and `on_window_event(DragDrop)` are registered and logged).
* IPC table as above; job panel `job #7: 26/400 — cancelled via Channel`; ticker panel
  `slept 2000 ms on the engine thread; UI ticker advanced 19 times in 2012 ms => main thread not blocked`.
* Rust log: every `seepdf:// request` line is on `thread=Some("main")` and every `/page responded` on
  `thread=Some("seepdf-engine")`.

### Bundled `.app` (macOS) — `npm run tauri build -- --debug --bundles app`

Built `src-tauri/target/debug/bundle/macos/SeePDF.app` (debug profile, ~1 min incremental). Verified:

* `Contents/Resources/resources/pdfium/` contains the six files from `resources/pdfium/*`;
  `Contents/Info.plist` has `CFBundleDocumentTypes[0] = { CFBundleTypeExtensions: [pdf], CFBundleTypeName: "PDF Document",
  CFBundleTypeRole: Editor, LSHandlerRank: Alternate, LSItemContentTypes: [com.adobe.pdf] }` generated from
  `bundle.fileAssociations`.
* Launched the binary directly (`SeePDF.app/Contents/MacOS/seepdf`, stdout captured):
  `resource_dir=…/SeePDF.app/Contents/Resources`, `resolved bundled libpdfium lib=…/Contents/Resources/resources/pdfium/libpdfium-aarch64-apple-darwin.dylib`,
  `pdfium bound on engine thread`, smoke test 14 pages.
* `open -a …/SeePDF.app fixtures/TAMReview.pdf` against the running instance → Rust log
  `open request path=…/TAMReview.pdf source="macos-opened"` (i.e. `RunEvent::Opened { urls: [file://…] }`), engine
  `document opened id=doc1 pages=23 ms=0.8`, webview `macos-opened: opened doc1 23 pages` via the `open-file` event
  (listener already mounted, so the pending queue stayed empty). Screenshot (`seepdf-spike-bundle.png` in the
  scratchpad) shows `doc1 (23p)` selected and its page rendered through `seepdf://localhost/page?doc=doc1…` in 257 ms.
* Same IPC/Channel/cancel/non-blocking numbers as in dev (raw 103 / 78 ms; Channel 23–52 ms and emit 31–35 ms for
  2 000 msgs; cancelled at 27/400; ticker advanced 20× during the 2 s engine sleep).
* Production CSP: a `securitypolicyviolation` listener plus a deliberate `<img src="http://csp-probe.invalid/x.png">`
  and `fetch("http://csp-probe.invalid/")` reported `CSP active — violations reported for: img-src, connect-src`,
  while in the same session 7 `seepdf://localhost/page…` image requests were served and `fetch(seepdf://…/page)`
  returned 200 — i.e. the `img-src seepdf:` / `connect-src seepdf:` entries above are both necessary and sufficient
  on macOS. (In `tauri dev` the same probe reports no violations: Vite serves the HTML without the header.)

---

## Gotchas (all hit during the spike)

1. **`tauri dev` restarts the app on every change under `src-tauri/`** (the CLI's own watcher, not Vite). With other
   agents writing `src-tauri/examples/*.rs` the app restarted every 10–20 s and never finished a benchmark. Use
   `npm run tauri dev -- --no-watch` when you need a stable process; Vite HMR for `src/` still works.
2. **Passing CLI args to the binary in dev**: `npm run tauri dev -- --no-watch -- -- /path/x.pdf` (npm eats the first
   `--`, tauri's first `--` starts cargo args, the second `--` starts binary args). The CLI ends up running
   `cargo run --no-default-features -- /path/x.pdf`.
3. **`fetch()` and 304**: hand-crafting `If-None-Match` from JS and answering 304 makes WebKit throw
   `TypeError: Load failed` (a 304 is only legal as the answer to the browser's own cache revalidation). Only return
   304 when the request actually carries `If-None-Match` from the cache; plain `<img>` loads are unaffected.
4. **pdfium-render 0.9 binds once per process** (`Pdfium::bind_to_library` → `PdfiumLibraryBindingsAlreadyInitialized`
   on the second call). Bind on the engine thread, `Box::leak` the `Pdfium` to hold `PdfDocument<'static>`.
5. **`bitmap.as_image()` requires pdfium-render's `image_api` feature** — on by default (`image_latest` → `image_025`)
   and unified with our `image = "0.25"` (single `image 0.25.10` in `Cargo.lock`), so `DynamicImage` types match.
6. **PNG encoding dominates** in a debug build: rendering a 918×1188 page takes ~2–25 ms in pdfium but
   `PngEncoder(CompressionType::Fast)` takes ~275–315 ms unoptimised. Use `[profile.dev.package."*"] opt-level = 3`
   (or at least for `image`, `png`, `flate2`/`miniz_oxide`) so dev-mode numbers are realistic; in release it is
   roughly an order of magnitude faster.
7. **Async commands that borrow `State` must return `Result`** (tauri macro restriction); sync commands may return
   plain values.
8. **`tauri::async_runtime` re-exports only tokio's `mpsc::{channel, Sender, Receiver}`, `Mutex`, `RwLock`**, not
   `oneshot`. `channel(1)` + `try_send` is the zero-dependency oneshot.
9. **Extra args with a raw body**: when `invoke()`'s args is an `ArrayBuffer`/`Uint8Array` there is no JSON object to
   put other parameters in — send them as request headers (`invoke(cmd, bytes, { headers: {...} })`,
   read with `request.headers()`) or in the URL/command name.
10. **A `Uint8Array` nested inside a JSON args object is NOT sent raw**: tauri's replacer converts it to a
    `number[]` (and walks every element). Only a top-level `ArrayBuffer`/typed array takes the raw path.
11. **CSP is not applied in `tauri dev`** (HTML served by Vite). Verify `img-src seepdf:` with `tauri build --debug`.
12. **`RunEvent::Opened` only fires for a bundled `.app`** with `CFBundleDocumentTypes`; in dev, test with argv.
    Also it can fire before the webview exists → queue + `take_pending_opens`.
13. **Resource glob must match something** or `build.rs` fails with `GlobPathNotFound` — keep `LICENSE.pdfium` and
    `VERSION` committed next to the gitignored binaries.
14. **`titleBarStyle: "Overlay"` + `hiddenTitle: true`** leaves the traffic lights floating over the page: pad the
    top ~34 px and add `data-tauri-drag-region` (needs `core:window:allow-start-dragging`).
15. **`@tauri-apps/plugin-{dialog,fs,window-state,store}` npm packages are not installed** (package.json is frozen for
    this phase). `invoke('plugin:dialog|open', { options })` etc. call the same commands; add the packages later for
    typings.
16. **Engine-thread `Reply` must handle a vanished receiver** (webview reload mid-request): `try_send` errors are
    ignored on purpose.
17. **Concurrent `cargo run` (dev profile) and another agent's `cargo build --release --example …`** share CPU but
    not the cargo lock (`target/debug` vs `target/release` have separate locks), so the numbers below were measured
    with a release build of pdfium examples running in parallel — treat them as upper bounds.

## Recommendations

1. **Page pixels: use the `seepdf://` protocol for `<img>`/`<canvas drawImage>` tiles, binary IPC for data JS
   must process.** Transport cost is equal (a few ms); the protocol gives free WebKit caching + revalidation
   (verified: WebKit sends `If-None-Match` itself), decode off the JS thread, `<img>` lazy loading, and no
   `ArrayBuffer` → `Blob` → object-URL bookkeeping. Use IPC (`tauri::ipc::Response`) when JS needs the bytes
   (thumbnails into an OffscreenCanvas, text-layer data, export). Never send pixels as JSON/base64 (8–28× slower).
2. **Encode cost, not transport, is the bottleneck**: ~260 ms PNG per page in debug. Options in priority order:
   `[profile.dev.package."*"] opt-level = 3` (or for `image`/`png`/`miniz_oxide`/`flate2`), `CompressionType::Fast`
   is already used, consider `NoFilter`, consider serving raw BGRA via the protocol as an uncompressed format
   (e.g. `image/bmp`-like or a custom `application/octet-stream` consumed by `ImageData` in JS) for the visible
   viewport and PNG only for cached tiles, or JPEG for continuous-tone pages. Measure again in release.
3. **ETag = (doc id, page, scale, tile, doc revision)** and `Cache-Control: private, max-age=0, must-revalidate`:
   answer 304 only when the request carries `If-None-Match` (WebKit does this itself); bump the revision on every
   edit so stale tiles are never served. Add `Access-Control-Allow-Origin: *` only if JS will `fetch()` the
   scheme; plain `<img>` does not need it.
4. **Progress: `tauri::ipc::Channel<T>`** for per-job progress (ordered, scoped to the invoker, ≥40 k msg/s);
   `app.emit` only for app-wide broadcasts (document list changed, recent files). Cancellation = `AtomicBool` per
   job checked between pages; the engine thread must poll it inside long pdfium loops.
5. **Engine thread contract**: one `crossbeam` request channel, every request carries a `Reply<T>` callback;
   commands use `Reply::oneshot()` (`tauri::async_runtime::channel(1)`), protocol handlers pass the
   `UriSchemeResponder` straight into the callback. Keep `Pdfium` leaked/`'static` on that thread and never let
   `PdfDocument` escape it. Add a priority lane (viewport tiles before thumbnails) — with one queue the full page
   delayed the tiles by ~30 ms each.
6. **File open**: `bundle.fileAssociations` for `.pdf` + argv scan + `RunEvent::Opened` + a pending queue drained by
   the first `take_pending_opens` call; frontend also listens to `open-file` for later opens. Dialog through
   `plugin:dialog|open` (auto-extends the fs scope), drag-drop via `onDragDropEvent`.
7. **Resources**: keep `bundle.resources` pointing at `resources/pdfium/*` (works in dev via tauri-build's copy and
   in the bundle via `resource_dir()`), but split per platform with `tauri.macos.conf.json` /
   `tauri.windows.conf.json` to avoid shipping 30 MB of foreign binaries; on macOS both dylibs (arm64 + x86_64) are
   needed unless the build is per-arch.
8. **Dev workflow**: `npm run tauri dev -- --no-watch` in this multi-agent tree; `RUST_LOG=seepdf_lib=debug` shows
   per-request thread/timing lines; `spike_log` lets the webview write benchmark numbers into the same log.
9. **npm packages to add when package.json is unfrozen**: `@tauri-apps/plugin-dialog`, `@tauri-apps/plugin-fs`,
   `@tauri-apps/plugin-window-state`, `@tauri-apps/plugin-store` (typings and helpers; the raw `invoke('plugin:…')`
   calls used here keep working). Rust side: `tokio = { version = "1", features = ["sync"] }` only if a real
   `oneshot`/`Notify` is wanted.
