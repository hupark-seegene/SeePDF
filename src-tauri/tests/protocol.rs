//! `seepdf://` routes: status codes, headers and the caching contract
//! (`IPC_CONTRACT.md` §9).

mod common;

use common::*;
use seepdf_lib::protocol;
use std::sync::mpsc;
use std::time::Duration;
use tauri::http::{header, Response, StatusCode};

/// Drives the protocol handler synchronously. The real handler hands its
/// `UriSchemeResponder` to the engine and the encode pool; here the sink is a channel.
fn get(url: &str) -> Response<Vec<u8>> {
    get_with(url, None, None)
}

fn get_with(url: &str, if_none_match: Option<&str>, range: Option<&str>) -> Response<Vec<u8>> {
    let (tx, rx) = mpsc::channel::<Response<Vec<u8>>>();
    protocol::serve(
        engine(),
        url,
        if_none_match,
        range,
        None,
        Box::new(move |response| {
            let _ = tx.send(response);
        }),
    );
    rx.recv_timeout(Duration::from_secs(30))
        .expect("the handler always responds")
}

fn etag(response: &Response<Vec<u8>>) -> String {
    response
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn head(response: &Response<Vec<u8>>, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

#[test]
fn protocol_routes() {
    let doc = open("tracemonkey.pdf");
    let (id, generation) = (doc.doc_id.clone(), doc.info.doc_generation);
    let base = "seepdf://localhost";

    // --- /page: 200 image/png with the documented headers ---
    let url = format!("{base}/page?doc={id}&gen={generation}&page=0&sk=100&rot=0");
    let page = get(&url);
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(head(&page, "content-type"), "image/png");
    assert_eq!(&page.body()[1..4], b"PNG");
    assert_eq!(
        head(&page, "cache-control"),
        "private, max-age=31536000, immutable"
    );
    assert_eq!(head(&page, "access-control-allow-origin"), "*");
    assert_eq!(head(&page, "access-control-expose-headers"), "*");
    assert_eq!(head(&page, "x-image-width"), "612");
    assert_eq!(head(&page, "x-image-height"), "792");
    assert_eq!(head(&page, "x-cache"), "miss");
    assert!(!head(&page, "x-render-ms").is_empty());
    assert!(!head(&page, "x-encode-ms").is_empty());
    assert_eq!(
        etag(&page),
        format!("\"{id}:{generation}:0:page:100:0:0:0:0:0\"")
    );

    // --- the encoded LRU answers the second request without pdfium ---
    let cached = get(&url);
    assert_eq!(cached.status(), StatusCode::OK);
    assert_eq!(head(&cached, "x-cache"), "hit");
    assert_eq!(cached.body(), page.body());

    // --- 304, but only when the request actually carries If-None-Match ---
    let revalidated = get_with(&url, Some(&etag(&page)), None);
    assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
    assert!(revalidated.body().is_empty());
    let mismatched = get_with(&url, Some("\"something-else\""), None);
    assert_eq!(mismatched.status(), StatusCode::OK);

    // --- /tile ---
    let tile = get(&format!(
        "{base}/tile?doc={id}&gen={generation}&page=0&sk=400&rot=0&tx=1&ty=1"
    ));
    assert_eq!(tile.status(), StatusCode::OK);
    assert_eq!(head(&tile, "x-image-width"), "512");
    assert_eq!(head(&tile, "x-image-height"), "512");

    // --- /thumb keeps the aspect ratio ---
    let thumb = get(&format!(
        "{base}/thumb?doc={id}&gen={generation}&page=0&w=180"
    ));
    assert_eq!(thumb.status(), StatusCode::OK);
    assert_eq!(head(&thumb, "x-image-width"), "180");

    // --- /ocr is a gray8 PNG ---
    let ocr = get(&format!(
        "{base}/ocr?doc={id}&gen={generation}&page=0&dpi=150"
    ));
    assert_eq!(ocr.status(), StatusCode::OK);
    assert_eq!(head(&ocr, "content-type"), "image/png");
    assert_eq!(ocr.body()[25], 0, "PNG IHDR colour type 0 = grayscale");

    // --- /raw, with and without a Range ---
    let raw = get(&format!("{base}/raw?doc={id}"));
    assert_eq!(raw.status(), StatusCode::OK);
    assert_eq!(head(&raw, "content-type"), "application/pdf");
    assert_eq!(head(&raw, "accept-ranges"), "bytes");
    assert_eq!(&raw.body()[0..5], b"%PDF-");
    let partial = get_with(&format!("{base}/raw?doc={id}"), None, Some("bytes=0-1023"));
    assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(partial.body().len(), 1024);
    assert!(head(&partial, "content-range").starts_with("bytes 0-1023/"));

    // --- 404 unknown document ---
    assert_eq!(
        get(&format!("{base}/page?doc=nope&gen=1&page=0&sk=100")).status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&format!("{base}/raw?doc=nope")).status(),
        StatusCode::NOT_FOUND
    );

    // --- 410 stale generation: a lagging <img> must never paint old pixels ---
    let stale = get(&format!(
        "{base}/page?doc={id}&gen={}&page=0&sk=100",
        generation.saturating_sub(1)
    ));
    assert_eq!(stale.status(), StatusCode::GONE);
    assert_eq!(head(&stale, "x-doc-generation"), generation.to_string());

    // --- 400 bad parameters ---
    for bad in [
        format!("{base}/page?gen=1&page=0&sk=100"),
        format!("{base}/page?doc={id}&gen={generation}&page=0&sk=100&rot=45"),
        format!("{base}/page?doc={id}&gen={generation}&page=0&sk=9000"),
        format!("{base}/page?doc={id}&gen={generation}&page=9999&sk=100"),
        format!("{base}/tile?doc={id}&gen={generation}&page=0&sk=100&tx=99&ty=0"),
        format!("{base}/recent-thumb"),
    ] {
        assert_eq!(
            get(&bad).status(),
            StatusCode::BAD_REQUEST,
            "expected 400 for {bad}"
        );
    }

    // --- unknown route ---
    assert_eq!(get(&format!("{base}/nope")).status(), StatusCode::NOT_FOUND);
    assert_eq!(get(&format!("{base}/ping")).status(), StatusCode::OK);

    // --- every response says no-store or immutable, never nothing ---
    for response in [&stale, &page] {
        assert!(!head(response, "cache-control").is_empty());
    }
}

/// After a mutation the old generation is 410 and the new one renders.
#[test]
fn protocol_generation_bump_invalidates() {
    use seepdf_lib::engine::registry::{self, MutateOpts};
    use seepdf_lib::ipc::types::ChangeReason;

    let doc = open("rotation.pdf");
    let id = doc.doc_id.clone();
    let base = "seepdf://localhost";
    let first = get(&format!("{base}/page?doc={id}&gen=1&page=0&sk=100"));
    assert_eq!(first.status(), StatusCode::OK);

    let doc_id = id.clone();
    with_state(move |st| {
        registry::mutate(
            st,
            &doc_id,
            MutateOpts::new("undo.pageRotate", ChangeReason::Pages).page(0),
            |d| {
                let page = d.page(0)?;
                page.set_rotation(pdfium_render::prelude::PdfPageRenderRotation::Degrees90);
                Ok(())
            },
        )
    })
    .expect("mutate");

    assert_eq!(
        get(&format!("{base}/page?doc={id}&gen=1&page=0&sk=100")).status(),
        StatusCode::GONE
    );
    let after = get(&format!("{base}/page?doc={id}&gen=2&page=0&sk=100"));
    assert_eq!(after.status(), StatusCode::OK);
    assert_eq!(head(&after, "x-cache"), "miss", "the old tile was swept");
    assert_eq!(head(&after, "x-image-width"), "792", "rotated by the edit");
}
