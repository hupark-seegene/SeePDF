//! Binary IPC benchmarks: raw bytes (`tauri::ipc::Response` / `InvokeBody::Raw`) versus
//! JSON number arrays and base64 strings, plus a JS -> tracing log bridge.

use tauri::ipc::{InvokeBody, Request, Response};

/// Rust -> JS raw bytes. JS receives an `ArrayBuffer` (content-type application/octet-stream).
#[tauri::command]
pub fn ipc_get_bytes(n: usize) -> Response {
    let mut v = vec![0u8; n];
    for (i, b) in v.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
    Response::new(v)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PutInfo {
    pub len: usize,
    pub checksum: u64,
    /// Any extra metadata travels in request headers when the body is raw.
    pub doc_header: Option<String>,
}

/// Plain byte sum (fits in a JS Number for 14 MB) so the webview can verify integrity.
fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().map(|&b| b as u64).sum()
}

/// JS -> Rust raw bytes: `invoke('ipc_put_bytes', new Uint8Array(...), { headers: { 'x-doc': 'id' } })`.
#[tauri::command]
pub fn ipc_put_bytes(request: Request<'_>) -> Result<PutInfo, String> {
    match request.body() {
        InvokeBody::Raw(bytes) => Ok(PutInfo {
            len: bytes.len(),
            checksum: checksum(bytes),
            doc_header: request
                .headers()
                .get("x-doc")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
        }),
        InvokeBody::Json(_) => Err("expected a raw body (pass an ArrayBuffer/Uint8Array as the args)".into()),
    }
}

/// Rust -> JS as a JSON array of numbers (what a plain `Vec<u8>` return does).
#[tauri::command]
pub fn ipc_get_json_array(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

#[tauri::command]
pub fn ipc_put_json_array(data: Vec<u8>) -> PutInfo {
    PutInfo { len: data.len(), checksum: checksum(&data), doc_header: None }
}

/// Rust -> JS as a base64 JSON string.
#[tauri::command]
pub fn ipc_get_base64(n: usize) -> String {
    let v: Vec<u8> = (0..n).map(|i| (i % 251) as u8).collect();
    b64_encode(&v)
}

#[tauri::command]
pub fn ipc_put_base64(data: String) -> Result<PutInfo, String> {
    let bytes = b64_decode(&data)?;
    Ok(PutInfo { len: bytes.len(), checksum: checksum(&bytes), doc_header: None })
}

/// Let the webview write into the Rust log so benchmark numbers land in the terminal.
#[tauri::command]
pub fn spike_log(level: String, msg: String) {
    match level.as_str() {
        "error" => tracing::error!(target: "webview", "{msg}"),
        "warn" => tracing::warn!(target: "webview", "{msg}"),
        _ => tracing::info!(target: "webview", "{msg}"),
    }
}

// --- tiny base64 (standard alphabet, padded); no crate needed for a spike ---
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(B64[(n >> 18 & 63) as usize] as char);
        out.push(B64[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn b64_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut table = [255u8; 256];
    for (i, &c) in B64.iter().enumerate() {
        table[c as usize] = i as u8;
    }
    let bytes = input.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &c in bytes {
        let v = table[c as usize];
        if v == 255 {
            return Err(format!("invalid base64 byte {c}"));
        }
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}
