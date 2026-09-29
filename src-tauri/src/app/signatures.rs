//! 서명 → PNG (P1-9).
//!
//! A **typed** signature is rendered by the webview (a canvas, with the system's own script
//! fonts — nothing is downloaded) and handed over as PNG bytes. Writing it to a file lets the
//! 서명 tool place it through the `StampImage::Path` spec that already exists, so the engine
//! reads it exactly like a picked image and no new `AnnotSpec` variant is needed.
//!
//! Files live in `$APPDATA/SeePDF/signatures/`, named after a hash of their bytes, so placing
//! the same saved signature twice reuses one file. The directory keeps the [`KEEP_FILES`]
//! most recently used ones; the PNG is only an intermediate (the saved signature itself is the
//! text + style in `Settings.signatures`), so pruning never loses user data.

use crate::ipc::EngineError;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A typed signature is a few tens of KB; anything this large is not one.
pub const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;
/// Longest side, px. The dialog renders at ~4× the placed size, far below this.
pub const MAX_SIDE_PX: u32 = 4096;
/// How many signature PNGs the directory keeps.
pub const KEEP_FILES: usize = 32;

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Validates `bytes` as a PNG and writes it into `dir` (created if missing). Returns the path.
pub fn write_png(dir: &Path, bytes: &[u8]) -> Result<PathBuf, EngineError> {
    if bytes.len() > MAX_PNG_BYTES {
        return Err(EngineError::invalid(format!(
            "signature image is {} bytes (limit {MAX_PNG_BYTES})",
            bytes.len()
        )));
    }
    if !bytes.starts_with(PNG_MAGIC) {
        return Err(EngineError::invalid("signature image is not a PNG"));
    }
    let (w, h) =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png)
            .into_dimensions()
            .map_err(|e| EngineError::invalid(format!("signature image does not decode: {e}")))?;
    if w == 0 || h == 0 || w > MAX_SIDE_PX || h > MAX_SIDE_PX {
        return Err(EngineError::invalid(format!(
            "signature image is {w}×{h} px (limit {MAX_SIDE_PX})"
        )));
    }

    std::fs::create_dir_all(dir)
        .map_err(|e| EngineError::io(format!("create {}: {e}", dir.display())))?;
    let name = format!("sig-{:016x}.png", fnv1a64(bytes));
    let path = dir.join(&name);
    if path.is_file() {
        // Same bytes already on disk: mark it as recently used so pruning keeps it.
        if let Ok(file) = std::fs::File::options().append(true).open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
    } else {
        // Write-then-rename, so the engine never reads a half-written file.
        let tmp = dir.join(format!("{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
        std::fs::write(&tmp, bytes)
            .map_err(|e| EngineError::io(format!("write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            EngineError::io(format!("rename {}: {e}", path.display()))
        })?;
    }
    prune(dir, KEEP_FILES, &path);
    Ok(path)
}

/// Biggest picked image the library copies (v0.3 T2 / T3).
pub const MAX_LIBRARY_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// v0.3 T2 / T3: copies a picked PNG / JPEG into `dir` (created if missing) for 내 도장 or
/// 저장된 서명 and returns `(path, width, height)`. Named `img-<hash>.<ext>` — content-addressed
/// like [`write_png`], so saving the same image twice keeps one file — and **never pruned**
/// (the saved entry in `Settings` points at it; [`remove_library_image`] deletes it).
pub fn copy_library_image(dir: &Path, src: &Path) -> Result<(PathBuf, u32, u32), EngineError> {
    let meta = std::fs::metadata(src)
        .map_err(|e| EngineError::io(format!("read {}: {e}", src.display())))?;
    if meta.len() > MAX_LIBRARY_IMAGE_BYTES {
        return Err(EngineError::invalid(format!(
            "image is {} bytes (limit {MAX_LIBRARY_IMAGE_BYTES})",
            meta.len()
        )));
    }
    let bytes =
        std::fs::read(src).map_err(|e| EngineError::io(format!("read {}: {e}", src.display())))?;
    let format = image::guess_format(&bytes)
        .map_err(|e| EngineError::invalid(format!("not a PNG or JPEG image: {e}")))?;
    let ext = match format {
        image::ImageFormat::Png => "png",
        image::ImageFormat::Jpeg => "jpg",
        other => {
            return Err(EngineError::invalid(format!(
                "{other:?} images are not supported (PNG or JPEG)"
            )))
        }
    };
    let (w, h) = image::ImageReader::with_format(std::io::Cursor::new(&bytes), format)
        .into_dimensions()
        .map_err(|e| EngineError::invalid(format!("image does not decode: {e}")))?;
    if w == 0 || h == 0 {
        return Err(EngineError::invalid("image has no pixels"));
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| EngineError::io(format!("create {}: {e}", dir.display())))?;
    let path = dir.join(format!("img-{:016x}.{ext}", fnv1a64(&bytes)));
    if !path.is_file() {
        let tmp = dir.join(format!("img.{}.tmp", uuid::Uuid::new_v4().simple()));
        std::fs::write(&tmp, &bytes)
            .map_err(|e| EngineError::io(format!("write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            EngineError::io(format!("rename {}: {e}", path.display()))
        })?;
    }
    Ok((path, w, h))
}

/// Deletes a library image written by [`copy_library_image`] — only an `img-*` file directly
/// inside `dir`, so a settings entry can never make SeePDF delete anything else.
pub fn remove_library_image(dir: &Path, path: &Path) -> Result<bool, EngineError> {
    let inside = path.parent().map(|p| p == dir).unwrap_or(false)
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("img-"));
    if !inside {
        return Ok(false);
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(EngineError::io(format!("remove {}: {e}", path.display()))),
    }
}

/// Deletes all but the `keep` most recently modified `sig-*.png` files, never `current`.
fn prune(dir: &Path, keep: usize, current: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("sig-") && n.ends_with(".png"))
        })
        .map(|p| {
            let modified = std::fs::metadata(&p)
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            (modified, p)
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (_, path) in files.into_iter().skip(keep) {
        if path != current {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// FNV-1a, 64-bit: stable across runs and Rust versions (unlike `DefaultHasher`).
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, shade: u8) -> Vec<u8> {
        let mut out = Vec::new();
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([shade, 0, 0, 255]));
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .expect("encode test png");
        out
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "seepdf-sig-test-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_a_png_once_per_content() {
        let dir = temp_dir("once");
        let bytes = png(40, 16, 10);
        let a = write_png(&dir, &bytes).expect("write");
        let b = write_png(&dir, &bytes).expect("write again");
        assert_eq!(a, b, "same bytes, same file");
        assert_eq!(std::fs::read(&a).unwrap(), bytes);
        let c = write_png(&dir, &png(40, 16, 11)).expect("write other");
        assert_ne!(a, c);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_what_is_not_a_signature_png() {
        let dir = temp_dir("reject");
        assert!(write_png(&dir, b"GIF89a....").is_err());
        let mut truncated = png(8, 8, 1);
        truncated.truncate(20);
        assert!(write_png(&dir, &truncated).is_err());
        assert!(write_png(&dir, &png(MAX_SIDE_PX + 1, 1, 1)).is_err());
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "nothing written"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn library_images_are_copied_once_and_never_pruned() {
        let dir = temp_dir("library");
        let src = dir.join("picked.png");
        std::fs::write(&src, png(400, 100, 3)).unwrap();
        let lib = dir.join("stamps");
        let (a, w, h) = copy_library_image(&lib, &src).expect("copy");
        assert_eq!((w, h), (400, 100));
        let (b, _, _) = copy_library_image(&lib, &src).expect("copy again");
        assert_eq!(a, b, "content-addressed");
        // Signature pruning never touches library images.
        for i in 0..(KEEP_FILES + 2) {
            write_png(&lib, &png(4, 4, i as u8)).expect("write");
        }
        assert!(a.is_file());
        // Only an img-* file inside the directory can be removed.
        assert!(!remove_library_image(&lib, &src).unwrap());
        assert!(remove_library_image(&lib, &a).unwrap());
        assert!(!a.exists());
        std::fs::write(dir.join("x.gif"), b"GIF89a....").unwrap();
        assert!(copy_library_image(&lib, &dir.join("x.gif")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn keeps_at_most_keep_files() {
        let dir = temp_dir("prune");
        let mut last = PathBuf::new();
        for i in 0..(KEEP_FILES + 4) {
            last = write_png(&dir, &png(4, 4, i as u8)).expect("write");
        }
        let count = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(count, KEEP_FILES);
        assert!(last.is_file(), "the file just written survives pruning");
        let _ = std::fs::remove_dir_all(dir);
    }
}
