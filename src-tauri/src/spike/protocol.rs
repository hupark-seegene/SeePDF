//! `seepdf://` custom URI scheme serving page bitmaps as PNG.
//!
//! URL shape (build it with `convertFileSrc('', 'seepdf')` in JS, which yields the
//! platform-correct origin):
//!   macOS / Linux / iOS : `seepdf://localhost/page?doc=<id>&page=<n>&scale=<f>[&tx=&ty=&tile=]`
//!   Windows / Android   : `http://seepdf.localhost/page?...`  (`https://` if `useHttpsScheme`)
//!
//! Routes:
//!   /ping                -> 200 text/plain (sync)
//!   /page                -> 200 image/png (rendered on the engine thread, async respond),
//!                           304 on matching If-None-Match, 400 bad params, 404 unknown doc,
//!                           500 render error, 503 engine down
//!   /raw?doc=<id>        -> 200/206 application/pdf, honours `Range: bytes=a-b`
//!   /slow?ms=<n>         -> sleeps on the engine thread then answers (main thread never blocks)
//!   anything else        -> 404
//!
//! The handler itself runs on the webview's protocol thread (main thread on macOS), so it
//! must return immediately: all work is handed to the engine thread or a worker thread and
//! the `UriSchemeResponder` (which is `Send`) is invoked from there.

use super::engine::{EngineHandle, EngineRequest, RenderedPng, Reply, Tile};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::time::Instant;
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{Manager, UriSchemeContext, UriSchemeResponder, Wry};

pub const SCHEME: &str = "seepdf";

pub fn register(builder: tauri::Builder<Wry>) -> tauri::Builder<Wry> {
    builder.register_asynchronous_uri_scheme_protocol(SCHEME, handle)
}

fn handle(ctx: UriSchemeContext<'_, Wry>, request: Request<Vec<u8>>, responder: UriSchemeResponder) {
    let t0 = Instant::now();
    let uri = request.uri().to_string();
    tracing::debug!(
        %uri,
        webview = ctx.webview_label(),
        thread = ?std::thread::current().name(),
        if_none_match = ?request.headers().get(header::IF_NONE_MATCH),
        cache_control = ?request.headers().get(header::CACHE_CONTROL),
        "seepdf:// request"
    );

    let url = match tauri::Url::parse(&uri) {
        Ok(u) => u,
        Err(e) => return responder.respond(text(StatusCode::BAD_REQUEST, format!("bad url: {e}"))),
    };
    let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
    let engine = ctx.app_handle().state::<EngineHandle>().inner().clone();

    match url.path() {
        "/ping" => responder.respond(text(StatusCode::OK, "pong")),

        "/page" => {
            let doc = q.get("doc").cloned().unwrap_or_default();
            let page: u16 = match q.get("page").map(|s| s.parse()) {
                Some(Ok(p)) => p,
                _ => return responder.respond(text(StatusCode::BAD_REQUEST, "page must be an integer")),
            };
            let scale: f32 = q.get("scale").and_then(|s| s.parse().ok()).unwrap_or(1.0);
            let tile = match (q.get("tx"), q.get("ty"), q.get("tile")) {
                (Some(x), Some(y), Some(s)) => match (x.parse(), y.parse(), s.parse()) {
                    (Ok(x), Ok(y), Ok(size)) => Some(Tile { x, y, size }),
                    _ => return responder.respond(text(StatusCode::BAD_REQUEST, "tx/ty/tile must be integers")),
                },
                _ => None,
            };
            // Cheap 304 path before touching the engine. Revision would come from the doc's
            // edit counter in the real app; the spike uses 0.
            let etag = format!("\"{doc}:{page}:{scale}:{:?}:rev0\"", tile.map(|t| (t.x, t.y, t.size)));
            if request
                .headers()
                .get(header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok())
                .map(|v| v == etag)
                .unwrap_or(false)
            {
                tracing::debug!(%etag, "If-None-Match hit -> 304");
                return responder.respond(
                    Response::builder()
                        .status(StatusCode::NOT_MODIFIED)
                        .header(header::ETAG, etag)
                        .body(Vec::new())
                        .unwrap(),
                );
            }
            if !engine.docs.read().contains_key(&doc) {
                return responder.respond(text(StatusCode::NOT_FOUND, format!("unknown document '{doc}'")));
            }
            let req = EngineRequest::RenderPagePng {
                doc,
                page,
                scale,
                tile,
                reply: Reply::from_fn(move |result: Result<RenderedPng, String>| match result {
                    Ok(png) => {
                        let total_ms = t0.elapsed().as_secs_f64() * 1000.0;
                        tracing::debug!(bytes = png.png.len(), render_ms = png.render_ms, encode_ms = png.encode_ms, total_ms, thread = ?std::thread::current().name(), "seepdf:// /page responded");
                        responder.respond(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header(header::CONTENT_TYPE, "image/png")
                                .header(header::CONTENT_LENGTH, png.png.len())
                                .header(header::ETAG, etag)
                                .header(header::CACHE_CONTROL, "private, max-age=0, must-revalidate")
                                .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                                .header(header::ACCESS_CONTROL_EXPOSE_HEADERS, "*")
                                .header("X-Image-Width", png.width)
                                .header("X-Image-Height", png.height)
                                .header("X-Render-Ms", format!("{:.2}", png.render_ms))
                                .header("X-Encode-Ms", format!("{:.2}", png.encode_ms))
                                .body(png.png)
                                .unwrap(),
                        )
                    }
                    Err(e) => {
                        let status = if e.contains("unknown document") { StatusCode::NOT_FOUND } else { StatusCode::INTERNAL_SERVER_ERROR };
                        responder.respond(text(status, e))
                    }
                }),
            };
            if let Err(e) = engine.send(req) {
                // `req` (and the responder inside it) was consumed by send(); nothing else to do.
                tracing::error!("{e}");
            }
        }

        "/raw" => {
            let doc = q.get("doc").cloned().unwrap_or_default();
            let Some(info) = engine.docs.read().get(&doc).cloned() else {
                return responder.respond(text(StatusCode::NOT_FOUND, format!("unknown document '{doc}'")));
            };
            let range = request
                .headers()
                .get(header::RANGE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            // File IO off the protocol thread.
            std::thread::spawn(move || responder.respond(serve_file_range(&info.path, range.as_deref())));
        }

        "/slow" => {
            let ms: u64 = q.get("ms").and_then(|s| s.parse().ok()).unwrap_or(1000);
            let req = EngineRequest::Sleep {
                ms,
                reply: Reply::from_fn(move |()| responder.respond(text(StatusCode::OK, format!("slept {ms} ms on the engine thread")))),
            };
            if let Err(e) = engine.send(req) {
                tracing::error!("{e}");
            }
        }

        other => responder.respond(text(StatusCode::NOT_FOUND, format!("no route {other}"))),
    }
}

fn text(status: StatusCode, body: impl Into<String>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(body.into().into_bytes())
        .unwrap()
}

/// Serve a file with optional single `bytes=a-b` range (206) — the pattern for streaming
/// the original PDF to the webview (e.g. for a JS-side text layer or download).
fn serve_file_range(path: &str, range: Option<&str>) -> Response<Vec<u8>> {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return text(StatusCode::NOT_FOUND, format!("{path}: {e}")),
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let (start, end) = match range.and_then(|r| r.strip_prefix("bytes=")) {
        Some(spec) => {
            let (a, b) = spec.split_once('-').unwrap_or((spec, ""));
            let start: u64 = a.parse().unwrap_or(0);
            let end: u64 = b.parse().unwrap_or(len.saturating_sub(1)).min(len.saturating_sub(1));
            if start > end || start >= len {
                return Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                    .body(Vec::new())
                    .unwrap();
            }
            (start, end)
        }
        None => (0, len.saturating_sub(1)),
    };
    let n = (end - start + 1) as usize;
    let mut buf = vec![0u8; n];
    if file.seek(SeekFrom::Start(start)).and_then(|_| file.read_exact(&mut buf)).is_err() {
        return text(StatusCode::INTERNAL_SERVER_ERROR, "read failed");
    }
    let mut b = Response::builder()
        .status(if range.is_some() { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK })
        .header(header::CONTENT_TYPE, "application/pdf")
        .header(header::CONTENT_LENGTH, n)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_EXPOSE_HEADERS, "*");
    if range.is_some() {
        b = b.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
    }
    b.body(buf).unwrap()
}
