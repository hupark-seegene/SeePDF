//! The `seepdf://` custom protocol — `IPC_CONTRACT.md` §9, `ARCHITECTURE.md` §4.
//!
//! Origin is `seepdf://localhost/` on macOS/Linux and `http://seepdf.localhost/` on Windows;
//! the frontend builds every URL with `convertFileSrc('', 'seepdf')` and never sniffs the OS.
//!
//! The handler runs on the webview's scheme thread (the **main** thread on macOS), so it
//! only parses the URL, checks the document mirror and the encoded-tile cache, and submits.
//! It **always** responds — a dropped `UriSchemeResponder` hangs the `<img>` until the
//! webview times out.

use crate::engine::render::cache::{Night, RenderKind, TileKey};
use crate::engine::render::tiles::RenderRequest;
use crate::engine::types::{CmdStatus, Lane};
use crate::engine::{EngineHandle, Submit};
use crate::ipc::{EngineError, ErrorCode};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{Manager, UriSchemeContext, UriSchemeResponder, Wry};

pub const SCHEME: &str = "seepdf";

/// Tile URLs carry `gen`, so the bytes at a URL never change: the webview may cache them
/// forever. The spike-verified fallback (`max-age=0, must-revalidate` + a 304 answered from
/// the encoded cache) is still implemented below for any webview that ignores `immutable`.
const CACHE_CONTROL: &str = "private, max-age=31536000, immutable";

/// Anything that can deliver one response, once. The Tauri handler passes its
/// `UriSchemeResponder`; tests pass a channel.
pub type Responder = Box<dyn FnOnce(Response<Vec<u8>>) + Send + 'static>;

pub fn register(builder: tauri::Builder<Wry>) -> tauri::Builder<Wry> {
    builder.register_asynchronous_uri_scheme_protocol(SCHEME, tauri_handler)
}

fn tauri_handler(
    ctx: UriSchemeContext<'_, Wry>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    let Some(engine) = app.try_state::<EngineHandle>().map(|s| s.inner().clone()) else {
        return responder.respond(text(
            StatusCode::SERVICE_UNAVAILABLE,
            "engine is not running",
        ));
    };
    let uri = request.uri().to_string();
    let if_none_match = request
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let thumbs_dir = crate::app::store::thumbs_dir(&app);
    serve(
        &engine,
        &uri,
        if_none_match.as_deref(),
        range.as_deref(),
        thumbs_dir,
        Box::new(move |response| responder.respond(response)),
    );
}

/// Route dispatch, independent of Tauri so it can be driven from tests.
pub fn serve(
    engine: &EngineHandle,
    uri: &str,
    if_none_match: Option<&str>,
    range: Option<&str>,
    thumbs_dir: Option<PathBuf>,
    respond: Responder,
) {
    let url = match tauri::Url::parse(uri) {
        Ok(u) => u,
        Err(e) => return respond(text(StatusCode::BAD_REQUEST, format!("bad url: {e}"))),
    };
    let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
    let path = url.path();
    tracing::trace!(%uri, path, "seepdf:// request");

    match path {
        "/tile" | "/page" | "/thumb" | "/ocr" => {
            let key = match parse_image_key(path, &q) {
                Ok(key) => key,
                Err(e) => return respond(error_response(&e)),
            };
            serve_image(engine, key, if_none_match, respond)
        }
        "/recent-thumb" => {
            let Some(id) = q.get("id").cloned() else {
                return respond(text(StatusCode::BAD_REQUEST, "id is required"));
            };
            let Some(dir) = thumbs_dir else {
                return respond(text(StatusCode::NOT_FOUND, "no thumbnail directory"));
            };
            std::thread::spawn(move || respond(serve_recent_thumb(&dir, &id)));
        }
        "/raw" => {
            let Some(doc) = q.get("doc").cloned() else {
                return respond(text(StatusCode::BAD_REQUEST, "doc is required"));
            };
            serve_raw(engine, doc, range.map(str::to_owned), respond)
        }
        "/ping" => respond(text(StatusCode::OK, "pong")),
        other => respond(text(StatusCode::NOT_FOUND, format!("no route {other}"))),
    }
}

// ---------------------------------------------------------------------------------------
// image routes
// ---------------------------------------------------------------------------------------

/// Parses `/tile`, `/page`, `/thumb` and `/ocr` into the cache key they share.
pub fn parse_image_key(path: &str, q: &HashMap<String, String>) -> Result<TileKey, EngineError> {
    let doc = q
        .get("doc")
        .cloned()
        .ok_or_else(|| EngineError::invalid("doc is required"))?;
    let generation: u32 = num(q, "gen")?;
    let page: u16 = num(q, "page")?;
    let rotation = q
        .get("rot")
        .map(|s| s.parse::<u16>().map_err(|_| EngineError::invalid("rot must be an integer")))
        .transpose()?
        .unwrap_or(0);
    if !matches!(rotation % 360, 0 | 90 | 180 | 270) {
        return Err(EngineError::invalid("rot must be 0, 90, 180 or 270"));
    }
    let night = Night::parse(q.get("night").map(String::as_str));
    let hl = matches!(q.get("hl").map(String::as_str), Some("1") | Some("true"));
    // `forms` defaults to 1: only the 양식 overlay asks for 0, and only while it is mounted.
    let forms = !matches!(q.get("forms").map(String::as_str), Some("0") | Some("false"));

    let (kind, scale_key, tx, ty) = match path {
        "/tile" => ("tile", num::<u32>(q, "sk")?, num::<u32>(q, "tx")?, num::<u32>(q, "ty")?),
        "/page" => ("page", num::<u32>(q, "sk")?, 0, 0),
        "/thumb" => ("thumb", num::<u32>(q, "w")?, 0, 0),
        _ => (
            "ocr",
            q.get("dpi")
                .map(|s| {
                    s.parse::<u32>()
                        .map_err(|_| EngineError::invalid("dpi must be an integer"))
                })
                .transpose()?
                .unwrap_or(300),
            0,
            0,
        ),
    };
    let kind = match kind {
        "tile" => RenderKind::Tile,
        "page" => RenderKind::Page,
        "thumb" => RenderKind::Thumb,
        _ => RenderKind::Ocr,
    };
    if matches!(kind, RenderKind::Tile | RenderKind::Page)
        && scale_key > crate::engine::render::geometry::MAX_SCALE_KEY
    {
        return Err(EngineError::invalid(format!(
            "sk {scale_key} exceeds the ceiling of {}",
            crate::engine::render::geometry::MAX_SCALE_KEY
        )));
    }
    Ok(TileKey {
        doc,
        generation,
        page,
        kind,
        scale_key,
        rotation: rotation % 360,
        tx,
        ty,
        night,
        hl,
        forms,
    })
}

fn serve_image(
    engine: &EngineHandle,
    key: TileKey,
    if_none_match: Option<&str>,
    respond: Responder,
) {
    let etag = key.etag();

    // 404 / 410 are decided against the mirror, with no pdfium call at all.
    let Some(summary) = engine.shared.doc(&key.doc) else {
        return respond(text(
            StatusCode::NOT_FOUND,
            format!("unknown document '{}'", key.doc),
        ));
    };
    if key.generation != summary.generation {
        return respond(gone(&etag, summary.generation));
    }
    if key.page >= summary.page_count {
        return respond(text(
            StatusCode::BAD_REQUEST,
            format!("page {} of {}", key.page, summary.page_count),
        ));
    }

    // A 304 is only legal as the answer to the webview's own revalidation; hand-crafting one
    // for a `fetch()` makes WebKit throw `TypeError: Load failed` (tauri spike §6).
    if if_none_match == Some(etag.as_str()) {
        return respond(not_modified(&etag));
    }

    if let Some(hit) = engine.shared.tiles.get(&key) {
        engine.shared.stats.note_cache(true);
        return respond(image_response(&key, &etag, &hit, true));
    }
    engine.shared.stats.note_cache(false);

    let (lane, priority) = schedule(engine, &key);
    let viewport_gen = engine.shared.viewport_gen();
    let cache = engine.shared.tiles.clone();
    let stats = engine.shared.stats.clone();
    let submit = Submit::new(lane, route_label(key.kind))
        .priority(priority)
        .page(key.page)
        .viewport(viewport_gen);

    let request = RenderRequest::new(key.clone());
    let dispatched = engine.dispatch(submit, move |st, status| match status {
        CmdStatus::Stale => respond(text(StatusCode::CONFLICT, "viewport moved on")),
        CmdStatus::Cancelled => respond(text(StatusCode::CONFLICT, "cancelled")),
        CmdStatus::Run => match crate::engine::render::tiles::render(st, &request) {
            Err(e) => respond(error_response(&e)),
            Ok(raw) => {
                let key = request.key.clone();
                let etag = key.etag();
                st.encode.submit(
                    Some(key.clone()),
                    raw,
                    Some(cache),
                    Some(stats),
                    move |encoded| match encoded {
                        Ok(image) => respond(image_response(&key, &etag, &image, false)),
                        Err(e) => respond(error_response(&e)),
                    },
                );
            }
        },
    });
    if let Err(e) = dispatched {
        // `respond` was moved into the closure, which was consumed by `dispatch`; the only
        // thing left to do is log. This happens when the engine thread is gone.
        tracing::error!("seepdf:// dispatch failed: {e}");
    }
}

fn route_label(kind: RenderKind) -> &'static str {
    match kind {
        RenderKind::Tile => "protocol/tile",
        RenderKind::Page => "protocol/page",
        RenderKind::Thumb => "protocol/thumb",
        RenderKind::Ocr => "protocol/ocr",
    }
}

/// Lane and within-lane priority: visible tiles centre-out, thumbnails last.
fn schedule(engine: &EngineHandle, key: &TileKey) -> (Lane, u32) {
    let viewport = engine.shared.viewport.lock().clone();
    let page_distance = key.page.abs_diff(viewport.centre_page) as u32;
    match key.kind {
        RenderKind::Tile => (
            Lane::Interactive,
            page_distance * 64 + key.tx.max(key.ty).min(63),
        ),
        RenderKind::Page => {
            if viewport.keeps(key.page) {
                (Lane::Interactive, page_distance * 64)
            } else {
                (Lane::Prefetch, page_distance)
            }
        }
        RenderKind::Thumb => (Lane::Thumb, page_distance),
        RenderKind::Ocr => (Lane::Background, page_distance),
    }
}

fn image_response(
    key: &TileKey,
    etag: &str,
    image: &Arc<crate::engine::render::cache::EncodedImage>,
    cache_hit: bool,
) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/png")
        .header(header::CONTENT_LENGTH, image.png.len())
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, CACHE_CONTROL)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_EXPOSE_HEADERS, "*")
        .header("X-Image-Width", image.width)
        .header("X-Image-Height", image.height)
        .header("X-Render-Ms", format!("{:.2}", image.render_ms))
        .header("X-Encode-Ms", format!("{:.2}", image.encode_ms))
        .header("X-Cache", if cache_hit { "hit" } else { "miss" })
        .header("X-Doc-Generation", key.generation)
        .body(image.png.clone())
        .expect("static header set is valid")
}

fn not_modified(etag: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, CACHE_CONTROL)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Vec::new())
        .expect("static header set is valid")
}

/// 410 Gone: a lagging `<img>` must never paint pixels from an older generation.
fn gone(etag: &str, current: u32) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::GONE)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_EXPOSE_HEADERS, "*")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ETAG, etag)
        .header("X-Doc-Generation", current)
        .body(format!("stale generation; current is {current}").into_bytes())
        .expect("static header set is valid")
}

pub fn error_response(e: &EngineError) -> Response<Vec<u8>> {
    let status = match e.code {
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::InvalidArgument => StatusCode::BAD_REQUEST,
        ErrorCode::Stale => StatusCode::CONFLICT,
        ErrorCode::Cancelled => StatusCode::CONFLICT,
        ErrorCode::Busy => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_EXPOSE_HEADERS, "*");
    if status == StatusCode::SERVICE_UNAVAILABLE {
        builder = builder.header(header::RETRY_AFTER, "0");
    }
    builder
        .body(e.message.clone().into_bytes())
        .expect("static header set is valid")
}

// ---------------------------------------------------------------------------------------
// /raw and /recent-thumb
// ---------------------------------------------------------------------------------------

fn serve_raw(engine: &EngineHandle, doc_id: String, range: Option<String>, respond: Responder) {
    let Some(summary) = engine.shared.doc(&doc_id) else {
        return respond(text(
            StatusCode::NOT_FOUND,
            format!("unknown document '{doc_id}'"),
        ));
    };
    if let Some(path) = summary.path {
        // File IO off the protocol thread.
        std::thread::spawn(move || respond(serve_file_range(&path, range.as_deref())));
        return;
    }
    // Untitled / merged documents only exist in memory.
    let submit = Submit::new(Lane::Background, "protocol/raw");
    let dispatched = engine.dispatch(submit, move |st, status| {
        if status != CmdStatus::Run {
            return respond(text(StatusCode::CONFLICT, "cancelled"));
        }
        match st.doc(&doc_id) {
            Err(e) => respond(error_response(&e)),
            Ok(doc) => {
                let bytes = doc.bytes.to_vec();
                respond(bytes_range(bytes, range.as_deref(), "application/pdf"))
            }
        }
    });
    if let Err(e) = dispatched {
        tracing::error!("/raw dispatch failed: {e}");
    }
}

fn serve_recent_thumb(dir: &std::path::Path, id: &str) -> Response<Vec<u8>> {
    // The id comes from our own store, but never let it escape the directory.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return text(StatusCode::BAD_REQUEST, "bad thumbnail id");
    }
    match std::fs::read(dir.join(format!("{id}.png"))) {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/png")
            .header(header::CONTENT_LENGTH, bytes.len())
            .header(header::CACHE_CONTROL, "private, max-age=60")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(bytes)
            .expect("static header set is valid"),
        Err(e) => text(StatusCode::NOT_FOUND, format!("{id}: {e}")),
    }
}

fn serve_file_range(path: &std::path::Path, range: Option<&str>) -> Response<Vec<u8>> {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return text(StatusCode::NOT_FOUND, format!("{}: {e}", path.display())),
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let (start, end) = match parse_range(range, len) {
        Ok(v) => v,
        Err(response) => return response,
    };
    let n = (end - start + 1) as usize;
    let mut buffer = vec![0u8; n];
    if file
        .seek(SeekFrom::Start(start))
        .and_then(|_| file.read_exact(&mut buffer))
        .is_err()
    {
        return text(StatusCode::INTERNAL_SERVER_ERROR, "read failed");
    }
    range_response(buffer, range.is_some(), start, end, len, "application/pdf")
}

fn bytes_range(
    bytes: Vec<u8>,
    range: Option<&str>,
    content_type: &'static str,
) -> Response<Vec<u8>> {
    let len = bytes.len() as u64;
    let (start, end) = match parse_range(range, len) {
        Ok(v) => v,
        Err(response) => return response,
    };
    let slice = bytes[start as usize..=(end as usize)].to_vec();
    range_response(slice, range.is_some(), start, end, len, content_type)
}

fn parse_range(range: Option<&str>, len: u64) -> Result<(u64, u64), Response<Vec<u8>>> {
    let Some(spec) = range.and_then(|r| r.strip_prefix("bytes=")) else {
        return Ok((0, len.saturating_sub(1)));
    };
    let (a, b) = spec.split_once('-').unwrap_or((spec, ""));
    let start: u64 = a.parse().unwrap_or(0);
    let end: u64 = b
        .parse()
        .unwrap_or(len.saturating_sub(1))
        .min(len.saturating_sub(1));
    if start > end || start >= len {
        return Err(Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{len}"))
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(Vec::new())
            .expect("static header set is valid"));
    }
    Ok((start, end))
}

fn range_response(
    body: Vec<u8>,
    partial: bool,
    start: u64,
    end: u64,
    len: u64,
    content_type: &'static str,
) -> Response<Vec<u8>> {
    let mut builder = Response::builder()
        .status(if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, body.len())
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_EXPOSE_HEADERS, "*");
    if partial {
        builder = builder.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
    }
    builder.body(body).expect("static header set is valid")
}

fn text(status: StatusCode, body: impl Into<String>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_EXPOSE_HEADERS, "*")
        .body(body.into().into_bytes())
        .expect("static header set is valid")
}

fn num<T: std::str::FromStr>(q: &HashMap<String, String>, key: &str) -> Result<T, EngineError> {
    q.get(key)
        .ok_or_else(|| EngineError::invalid(format!("{key} is required")))?
        .parse::<T>()
        .map_err(|_| EngineError::invalid(format!("{key} must be an integer")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn parses_every_image_route() {
        let key = parse_image_key(
            "/tile",
            &query(&[
                ("doc", "d1"),
                ("gen", "3"),
                ("page", "2"),
                ("sk", "200"),
                ("rot", "90"),
                ("tx", "1"),
                ("ty", "4"),
                ("night", "1"),
                ("hl", "1"),
            ]),
        )
        .expect("tile");
        assert_eq!(key.kind, RenderKind::Tile);
        assert_eq!((key.tx, key.ty, key.rotation), (1, 4, 90));
        assert_eq!(key.night, Night::Dark);
        assert!(key.hl);
        assert!(key.forms, "forms defaults to on");

        // `forms=0` (F-20): the 양식 overlay is mounted, so PDFium must not draw the widgets.
        let no_forms = parse_image_key(
            "/page",
            &query(&[
                ("doc", "d1"),
                ("gen", "1"),
                ("page", "0"),
                ("sk", "200"),
                ("forms", "0"),
            ]),
        )
        .expect("page");
        assert!(!no_forms.forms);
        assert_ne!(
            no_forms.etag(),
            TileKey { forms: true, ..no_forms.clone() }.etag(),
            "forms is part of the cache key and the ETag"
        );

        let thumb = parse_image_key(
            "/thumb",
            &query(&[("doc", "d1"), ("gen", "1"), ("page", "0"), ("w", "180")]),
        )
        .expect("thumb");
        assert_eq!(thumb.scale_key, 180);

        let ocr = parse_image_key(
            "/ocr",
            &query(&[("doc", "d1"), ("gen", "1"), ("page", "0")]),
        )
        .expect("ocr");
        assert_eq!(ocr.scale_key, 300, "dpi defaults to 300");
    }

    #[test]
    fn rejects_bad_parameters() {
        assert_eq!(
            parse_image_key("/page", &query(&[("gen", "1"), ("page", "0"), ("sk", "100")]))
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            parse_image_key(
                "/page",
                &query(&[("doc", "d1"), ("gen", "1"), ("page", "0"), ("sk", "100"), ("rot", "45")])
            )
            .unwrap_err()
            .code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            parse_image_key(
                "/page",
                &query(&[("doc", "d1"), ("gen", "1"), ("page", "0"), ("sk", "9000")])
            )
            .unwrap_err()
            .code,
            ErrorCode::InvalidArgument
        );
    }

    #[test]
    fn range_headers_follow_rfc() {
        assert_eq!(parse_range(Some("bytes=0-1023"), 4096).unwrap(), (0, 1023));
        assert_eq!(parse_range(Some("bytes=100-"), 4096).unwrap(), (100, 4095));
        assert_eq!(parse_range(None, 4096).unwrap(), (0, 4095));
        assert!(parse_range(Some("bytes=5000-6000"), 4096).is_err());
    }
}
