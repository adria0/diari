//! Pictures for the preview. They're decoded off the UI thread, turned
//! upright (phone photos store their rotation in EXIF), and scaled down.
//! Pictures from the web are downloaded once into a cache folder.

use image::{DynamicImage, ImageDecoder, ImageReader};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

/// Longest side of a decoded picture: plenty for a phone or a preview pane.
pub const MAX_SIDE: u32 = 1600;
/// Biggest picture downloaded from the web.
const MAX_DOWNLOAD: u64 = 25 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    Local(PathBuf),
    Web(String),
}

/// Where a web picture is cached: named after a hash of its URL.
pub fn cache_path(cache_dir: &Path, url: &str) -> PathBuf {
    // FNV-1a: stable across runs and Rust versions.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    cache_dir.join(format!("{h:016x}"))
}

/// Read (or download) and decode a picture. Slow: call it off the UI thread.
pub fn load(src: &Source, cache_dir: &Path) -> Result<SharedPixelBuffer<Rgba8Pixel>, String> {
    let bytes = match src {
        Source::Local(p) => std::fs::read(p).map_err(|e| e.to_string())?,
        Source::Web(url) => fetch_cached(url, cache_dir)?,
    };
    decode(&bytes)
}

fn fetch_cached(url: &str, cache_dir: &Path) -> Result<Vec<u8>, String> {
    let path = cache_path(cache_dir, url);
    if let Ok(b) = std::fs::read(&path) {
        return Ok(b);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .into();
    let resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    resp.into_body()
        .into_reader()
        .take(MAX_DOWNLOAD + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_DOWNLOAD {
        return Err("picture too big".into());
    }
    // Cache only what decodes, so a broken download is retried next time.
    decode(&bytes)?;
    std::fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
    let _ = crate::notes::atomic_write(&path, &bytes);
    Ok(bytes)
}

/// Decode, turn upright and scale down to `MAX_SIDE`.
pub fn decode(bytes: &[u8]) -> Result<SharedPixelBuffer<Rgba8Pixel>, String> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    if img.width() > MAX_SIDE || img.height() > MAX_SIDE {
        img = img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle);
    }
    let rgba = img.to_rgba8();
    Ok(SharedPixelBuffer::clone_from_slice(rgba.as_raw(), rgba.width(), rgba.height()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(img).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn decodes_and_scales_down() {
        let b = decode(&png(20, 10)).unwrap();
        assert_eq!((b.width(), b.height()), (20, 10));
        let b = decode(&png(4000, 1000)).unwrap();
        assert_eq!((b.width(), b.height()), (1600, 400));
        assert!(decode(b"not a picture").is_err());
    }

    /// `PICTURE_URL=https://… cargo test --lib images::tests::live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live() {
        let url = std::env::var("PICTURE_URL").expect("set PICTURE_URL");
        let dir = std::env::temp_dir().join("diari-picture-test");
        let _ = std::fs::remove_dir_all(&dir);
        let b = load(&Source::Web(url.clone()), &dir).unwrap();
        println!("{}x{}", b.width(), b.height());
        assert!(cache_path(&dir, &url).exists(), "not cached");
        load(&Source::Web(url), &dir).unwrap(); // from the cache
    }

    /// `PICTURE_FILE=… cargo test --lib images::tests::file -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn file() {
        let p = std::env::var("PICTURE_FILE").expect("set PICTURE_FILE");
        let b = decode(&std::fs::read(p).unwrap()).unwrap();
        println!("{}x{}", b.width(), b.height());
    }

    #[test]
    fn cache_names_are_stable() {
        let d = Path::new("/c");
        assert_eq!(cache_path(d, "https://x.y/a.png"), cache_path(d, "https://x.y/a.png"));
        assert_ne!(cache_path(d, "https://x.y/a.png"), cache_path(d, "https://x.y/b.png"));
    }
}
