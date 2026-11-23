//! What a decoded pass leaves behind: a directory with the images, their thumbnails and
//! `pass.json`, the index the API's gallery reads (`docs/plugins-weather.md`).

use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// Widest thumbnail (pixels).
const THUMB_WIDTH: u32 = 320;
/// Tallest thumbnail; taller previews are cut to the middle.
const THUMB_MAX_HEIGHT: u32 = 480;
/// Images larger than this many pixels get no thumbnail (a decoder bug, not a pass).
const MAX_PIXELS: u64 = 150_000_000;
/// Most images kept per pass.
pub const MAX_IMAGES: usize = 40;

/// One image of a pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// File name inside the pass directory (`[A-Za-z0-9._-]`, ends in `.png`).
    pub name: String,
    /// Human-readable name.
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub size: u64,
    /// File name of the thumbnail, when one could be made.
    pub thumb: Option<String>,
}

/// A string made of `[A-Za-z0-9._-]` only (other characters become `_`), at most 80 long.
#[must_use]
pub fn safe_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    cleaned.trim_start_matches('.').chars().take(80).collect()
}

/// "msu_mr_rgb_221" -> "msu mr rgb 221".
#[must_use]
pub fn label_of(file_stem: &str) -> String {
    file_stem.replace(['_', '-'], " ").trim().to_owned()
}

/// Every `.png` of `dir` and its sub-directories (two levels), sorted by path; composites
/// ("rgb", "false colour", "msa") first, then the channels.
#[must_use]
pub fn find_pngs(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() && depth < 2 {
                walk(&path, depth + 1, out);
            } else if kind.is_file() && path.extension().is_some_and(|e| e == "png") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, 0, &mut out);
    let rank = |p: &PathBuf| {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        // Single channels ("msu-mr-1") after everything composed of several.
        let channel = name
            .trim_end_matches(".png")
            .rsplit(['_', '-'])
            .next()
            .is_some_and(|last| last.len() == 1 && last.chars().all(|c| c.is_ascii_digit()));
        u8::from(channel)
    };
    out.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
    out
}

/// Size of a PNG file without decoding it.
fn dimensions(path: &Path) -> Option<(u32, u32)> {
    let decoder = png::Decoder::new(BufReader::new(File::open(path).ok()?));
    let reader = decoder.read_info().ok()?;
    let info = reader.info();
    Some((info.width, info.height))
}

/// Write a thumbnail of `src` to `dest`: scaled by averaging to [`THUMB_WIDTH`] pixels wide
/// (never enlarged), cut to the middle if taller than [`THUMB_MAX_HEIGHT`], as 8-bit RGB.
///
/// # Errors
/// A short reason (not a PNG, too large, unsupported).
pub fn make_thumbnail(src: &Path, dest: &Path) -> Result<(), String> {
    let file = File::open(src).map_err(|e| e.to_string())?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let (w, h) = {
        let i = reader.info();
        (i.width, i.height)
    };
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err("image size not supported".into());
    }
    let mut buf = vec![0u8; reader.output_buffer_size().ok_or("image too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err("indexed colour not expanded".into()),
    };
    if info.bit_depth != png::BitDepth::Eight {
        return Err("bit depth not supported".into());
    }
    let (w, h) = (w as usize, h as usize);
    let stride = info.line_size;
    let out_w = w.min(THUMB_WIDTH as usize);
    let scale = w as f64 / out_w as f64;
    let full_h = ((h as f64 / scale).round() as usize).max(1);
    let out_h = full_h.min(THUMB_MAX_HEIGHT as usize);
    let skip = (full_h - out_h) / 2;
    let mut rgb = Vec::with_capacity(out_w * out_h * 3);
    for oy in 0..out_h {
        let y0 = (((oy + skip) as f64) * scale) as usize;
        let y1 = ((((oy + skip + 1) as f64) * scale).ceil() as usize).clamp(y0 + 1, h);
        for ox in 0..out_w {
            let x0 = ((ox as f64) * scale) as usize;
            let x1 = ((((ox + 1) as f64) * scale).ceil() as usize).clamp(x0 + 1, w);
            let mut sum = [0u32; 3];
            let mut n = 0u32;
            for y in y0..y1.min(h) {
                let row = &buf[y * stride..y * stride + w * channels];
                for x in x0..x1.min(w) {
                    let px = &row[x * channels..(x + 1) * channels];
                    let (r, g, b) = match channels {
                        1 | 2 => (px[0], px[0], px[0]),
                        _ => (px[0], px[1], px[2]),
                    };
                    sum[0] += u32::from(r);
                    sum[1] += u32::from(g);
                    sum[2] += u32::from(b);
                    n += 1;
                }
            }
            let n = n.max(1);
            rgb.extend(sum.iter().map(|s| (s / n) as u8));
        }
    }
    let out = File::create(dest).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(
        BufWriter::new(out),
        u32::try_from(out_w).map_err(|e| e.to_string())?,
        u32::try_from(out_h).map_err(|e| e.to_string())?,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&rgb).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())
}

/// Move the PNGs found in `from` into `to`, with thumbnails; returns the images in gallery order.
/// Name clashes between sub-directories get a numeric prefix.
///
/// # Errors
/// Fails when `to` cannot be written.
pub fn collect_images(from: &Path, to: &Path) -> Result<Vec<Image>, String> {
    fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let mut images = Vec::new();
    let mut taken = std::collections::HashSet::new();
    for src in find_pngs(from).into_iter().take(MAX_IMAGES) {
        let stem = src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut base = safe_name(&stem);
        if base.is_empty() {
            base = "image".into();
        }
        let mut name = format!("{base}.png");
        let mut n = 2;
        while !taken.insert(name.clone()) {
            name = format!("{base}-{n}.png");
            n += 1;
        }
        let dest = to.join(&name);
        if fs::rename(&src, &dest).is_err() {
            fs::copy(&src, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        }
        let Some((width, height)) = dimensions(&dest) else {
            // Not a readable PNG: drop it rather than show a broken tile.
            let _ = fs::remove_file(&dest);
            continue;
        };
        let size = fs::metadata(&dest).map_or(0, |m| m.len());
        let thumb_name = format!("thumb-{name}");
        let thumb = make_thumbnail(&dest, &to.join(&thumb_name))
            .is_ok()
            .then_some(thumb_name);
        images.push(Image {
            label: label_of(&stem),
            name,
            width,
            height,
            size,
            thumb,
        });
    }
    Ok(images)
}

/// What `pass.json` says about a pass (schema 1).
#[derive(Debug, Clone)]
pub struct PassMeta {
    pub id: String,
    pub satellite: Option<String>,
    /// `lrpt` or `apt`.
    pub pipeline: String,
    pub started_ms: i64,
    pub ended_ms: i64,
    pub frequency_hz: Option<u64>,
    pub device: Option<String>,
    pub instance: Option<String>,
    /// Highest level over the noise floor during the pass, dB.
    pub peak_db: Option<f64>,
    pub images: Vec<Image>,
}

impl PassMeta {
    /// The JSON document stored next to the images. `max_elevation_deg` is filled in by the API
    /// (it knows the element sets and the receiver position), `keep` by the user.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "v": 1,
            "id": self.id,
            "satellite": self.satellite,
            "pipeline": self.pipeline,
            "started_ms": self.started_ms,
            "ended_ms": self.ended_ms,
            "duration_s": (self.ended_ms - self.started_ms).max(0) / 1000,
            "frequency_hz": self.frequency_hz,
            "device": self.device,
            "instance": self.instance,
            "peak_db": self.peak_db.map(|d| (d * 10.0).round() / 10.0),
            "max_elevation_deg": Value::Null,
            "keep": false,
            "images": self.images.iter().map(|i| json!({
                "name": i.name, "label": i.label, "width": i.width, "height": i.height,
                "size": i.size, "thumb": i.thumb,
            })).collect::<Vec<_>>(),
        })
    }

    /// Write `pass.json` into `dir`, through a temporary file.
    ///
    /// # Errors
    /// Fails when the file cannot be written.
    pub fn write(&self, dir: &Path) -> Result<(), String> {
        let tmp = dir.join("pass.json.part");
        let text = serde_json::to_vec_pretty(&self.to_json()).map_err(|e| e.to_string())?;
        fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        fs::rename(&tmp, dir.join("pass.json")).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Write a w x h RGB gradient PNG.
    pub fn write_png(path: &Path, w: u32, h: u32, color: png::ColorType) {
        let channels = match color {
            png::ColorType::Grayscale => 1,
            png::ColorType::Rgba => 4,
            _ => 3,
        };
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                for c in 0..channels {
                    data.push(((x + y * 2 + c) % 256) as u8);
                }
            }
        }
        let mut enc = png::Encoder::new(BufWriter::new(File::create(path).expect("create")), w, h);
        enc.set_color(color);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .expect("header")
            .write_image_data(&data)
            .expect("data");
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("superhet-weather-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn names_are_made_safe() {
        assert_eq!(safe_name("MSU-MR/rgb 221.png"), "MSU-MR_rgb_221.png");
        assert_eq!(safe_name("../../etc"), "_.._etc");
        assert_eq!(safe_name(".hidden"), "hidden");
        assert_eq!(safe_name(&"a".repeat(200)).len(), 80);
        assert_eq!(label_of("msu_mr_rgb-221"), "msu mr rgb 221");
    }

    #[test]
    fn thumbnails_are_scaled_cut_and_never_enlarged() {
        let dir = scratch("thumb");
        // A wide-and-tall strip like a long pass.
        write_png(&dir.join("strip.png"), 1568, 6000, png::ColorType::Rgb);
        make_thumbnail(&dir.join("strip.png"), &dir.join("t.png")).expect("thumb");
        assert_eq!(dimensions(&dir.join("t.png")), Some((320, 480)));
        // Small and grey stays as large as it is.
        write_png(&dir.join("small.png"), 100, 50, png::ColorType::Grayscale);
        make_thumbnail(&dir.join("small.png"), &dir.join("s.png")).expect("thumb");
        assert_eq!(dimensions(&dir.join("s.png")), Some((100, 50)));
        // RGBA is fine; a text file is not a PNG.
        write_png(&dir.join("a.png"), 640, 100, png::ColorType::Rgba);
        make_thumbnail(&dir.join("a.png"), &dir.join("a-t.png")).expect("thumb");
        assert_eq!(dimensions(&dir.join("a-t.png")), Some((320, 50)));
        fs::write(dir.join("x.png"), b"not a png").expect("write");
        assert!(make_thumbnail(&dir.join("x.png"), &dir.join("x-t.png")).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn images_are_collected_ordered_and_clashes_resolved() {
        let from = scratch("collect-from");
        let to = scratch("collect-to").join("pass");
        fs::create_dir_all(from.join("MSU-MR")).expect("mkdir");
        fs::create_dir_all(from.join("Other")).expect("mkdir");
        write_png(
            &from.join("MSU-MR/msu_mr_1.png"),
            64,
            64,
            png::ColorType::Grayscale,
        );
        write_png(
            &from.join("MSU-MR/msu_mr_rgb_124.png"),
            64,
            64,
            png::ColorType::Rgb,
        );
        write_png(
            &from.join("Other/msu_mr_rgb_124.png"),
            64,
            64,
            png::ColorType::Rgb,
        );
        fs::write(from.join("MSU-MR/broken.png"), b"junk").expect("write");
        fs::write(from.join("meteor.cadu"), b"data").expect("write");
        let images = collect_images(&from, &to).expect("collect");
        let names: Vec<&str> = images.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(
            names,
            ["msu_mr_rgb_124.png", "msu_mr_rgb_124-2.png", "msu_mr_1.png"],
            "{images:?}"
        );
        assert!(images.iter().all(|i| i.thumb.is_some() && i.width == 64));
        assert!(to.join("thumb-msu_mr_1.png").is_file());
        assert!(!from.join("MSU-MR/msu_mr_1.png").exists());
        let _ = fs::remove_dir_all(&from);
        let _ = fs::remove_dir_all(to.parent().expect("parent"));
    }

    #[test]
    fn pass_json_has_the_documented_shape() {
        let dir = scratch("meta");
        let meta = PassMeta {
            id: "1795436348-ab12".into(),
            satellite: Some("METEOR-M2 4".into()),
            pipeline: "lrpt".into(),
            started_ms: 1_795_436_348_000,
            ended_ms: 1_795_436_948_000,
            frequency_hz: Some(137_900_000),
            device: Some("dev0".into()),
            instance: Some("i-1".into()),
            peak_db: Some(7.349),
            images: vec![],
        };
        meta.write(&dir).expect("write");
        let doc: Value =
            serde_json::from_slice(&fs::read(dir.join("pass.json")).expect("read")).expect("json");
        assert_eq!(doc["v"], 1);
        assert_eq!(doc["duration_s"], 600);
        assert_eq!(doc["peak_db"], 7.3);
        assert_eq!(doc["keep"], false);
        assert!(doc["max_elevation_deg"].is_null());
        assert!(!dir.join("pass.json.part").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
