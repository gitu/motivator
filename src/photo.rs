//! Photo pipeline: background cut-out (border flood fill against corner
//! reference colors) and mouth-line estimation for the talking flap.
//! Direct port of the design's canvas pipeline, extended with explicit
//! processing modes (auto / pre-cut / raw) and animated-avatar decoding.

use std::path::{Path, PathBuf};

use image::{AnimationDecoder, RgbaImage};

use crate::config::PhotoMode;

/// SeetaFace frontal detection model (BSD-2, VIPL group / rustface project)
static FACE_MODEL: &[u8] = include_bytes!("../assets/seeta_fd_frontal_v1.0.bin");

/// animated avatars are bounded so 48 × 256² RGBA stays ~12 MB worst case
const MAX_FRAMES: usize = 48;
const ANIM_EDGE: u32 = 256;
/// cap size for stills: plenty for a <=96px avatar, keeps flood fill cheap
const STILL_EDGE: u32 = 512;
/// raw files above this become one resized PNG — a texture has to fit in VRAM
const RAW_EDGE: u32 = 2048;

/// Facial geometry extracted from a photo, all values as fractions of the
/// image dimensions. Drives the talking warp and the blink overlay.
#[derive(Clone, Copy)]
pub struct Face {
    /// mouth line — the talking flap/jaw hinge
    pub split: f32,
    /// eye band (center y, height); None when no face (or no eyes) was found
    pub eyes: Option<(f32, f32)>,
    /// bottom of the jaw — the lower bound of the mouth-warp slice
    pub chin: f32,
    /// horizontal face extent, used to bound the blink overlay
    pub face_x: (f32, f32),
}

pub struct Processed {
    pub path: PathBuf,
    /// detected facial geometry; None when detection was skipped (raw mode)
    pub face: Option<Face>,
    /// animated frames (path, delay ms); empty for a still
    pub frames: Vec<(PathBuf, u32)>,
}

/// `stem` names the files on disk: `{stem}.png` for a still,
/// `{stem}.f{n}.png` for animation frames (the app passes the friend id, or
/// `{id}.talk` for the talking still).
pub fn process_and_store(src: &Path, stem: &str, mode: PhotoMode) -> Result<Processed, String> {
    let bytes = std::fs::read(src).map_err(|e| format!("could not read image: {e}"))?;
    let ext = src.extension().and_then(|e| e.to_str());
    let dir = crate::config::photos_dir();
    process_bytes(&bytes, ext, &dir, stem, mode)
}

/// Read an image file as PNG bytes — pass-through when it already is a PNG,
/// re-encode otherwise. Raw-mode photos keep their original format on disk,
/// but the image-edits API is sent `image/png`.
pub fn png_bytes_of(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Ok(bytes);
    }
    let img = image::load_from_memory(&bytes).map_err(|e| format!("could not read image: {e}"))?;
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Same pipeline for in-memory bytes (e.g. an AI-generated talking frame).
pub fn process_and_store_bytes(
    bytes: &[u8],
    ext: Option<&str>,
    stem: &str,
    mode: PhotoMode,
) -> Result<Processed, String> {
    let dir = crate::config::photos_dir();
    process_bytes(bytes, ext, &dir, stem, mode)
}

fn process_bytes(
    bytes: &[u8],
    src_ext: Option<&str>,
    dir: &Path,
    stem: &str,
    mode: PhotoMode,
) -> Result<Processed, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    remove_stale(dir, stem);

    let frames = decode_frames(bytes);
    if frames.len() > 1 {
        return store_animation(frames, dir, stem, mode);
    }
    if mode == PhotoMode::Raw {
        return store_raw(bytes, src_ext, dir, stem);
    }

    let img = image::load_from_memory(bytes).map_err(|e| format!("could not read image: {e}"))?;
    let img = img.resize(
        STILL_EDGE,
        STILL_EDGE,
        image::imageops::FilterType::Triangle,
    );
    let mut rgba = img.to_rgba8();

    // read the face off the pristine photo before the cut-out touches it;
    // the silhouette heuristic is only the no-face-found fallback
    let detected = detect_face(&rgba);

    // pre-cut images (real transparency) skip the flood fill — the alpha
    // channel already is the cut-out; "already cut out" mode forces that
    let transparent = rgba.pixels().filter(|p| p[3] < 128).count();
    let heuristic_split = if mode == PhotoMode::Precut
        || transparent > (rgba.width() * rgba.height()) as usize / 50
    {
        split_heuristic(&rgba)
    } else {
        cutout(&mut rgba).or_else(|| split_heuristic(&rgba))
    };
    let face = detected.unwrap_or_else(|| silhouette_face(&rgba, heuristic_split));

    let path = dir.join(format!("{stem}.png"));
    rgba.save(&path).map_err(|e| e.to_string())?;
    Ok(Processed {
        path,
        face: Some(face),
        frames: Vec::new(),
    })
}

/// No face found: build coarse geometry from the silhouette split — enough
/// for the jaw warp, but no eyes (no blinking on unknown geometry).
fn silhouette_face(img: &RgbaImage, split: Option<f32>) -> Face {
    let split = split.unwrap_or(0.52);
    let (w, h) = (img.width() as usize, img.height() as usize);
    // face width ≈ opaque extent on the mouth row
    let y = ((split * h as f32) as usize).min(h - 1);
    let px = img.as_raw();
    let opaque: Vec<usize> = (0..w).filter(|&x| px[(y * w + x) * 4 + 3] > 128).collect();
    let face_x = match (opaque.first(), opaque.last()) {
        (Some(&a), Some(&b)) if b > a => (a as f32 / w as f32, b as f32 / w as f32),
        _ => (0.25, 0.75),
    };
    Face {
        split,
        eyes: None,
        chin: (split + 0.17).min(0.98),
        face_x,
    }
}

/// Raw mode: the file lands on disk byte-identical (any format the renderer
/// can open), unless it is too large to be a texture — then it becomes one
/// resized PNG, pixels otherwise untouched.
fn store_raw(
    bytes: &[u8],
    src_ext: Option<&str>,
    dir: &Path,
    stem: &str,
) -> Result<Processed, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("could not read image: {e}"))?;
    let path = if img.width() <= RAW_EDGE && img.height() <= RAW_EDGE {
        let ext = src_ext
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_else(|| "png".into());
        let path = dir.join(format!("{stem}.{ext}"));
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        path
    } else {
        let img = img.resize(RAW_EDGE, RAW_EDGE, image::imageops::FilterType::Triangle);
        let path = dir.join(format!("{stem}.png"));
        img.to_rgba8().save(&path).map_err(|e| e.to_string())?;
        path
    };
    Ok(Processed {
        path,
        face: None,
        frames: Vec::new(),
    })
}

/// Decode an animated GIF / APNG / WebP into composited RGBA frames with
/// per-frame delays. Returns an empty vec for stills and unknown formats.
fn decode_frames(bytes: &[u8]) -> Vec<(RgbaImage, u32)> {
    fn collect(frames: image::Frames) -> Vec<(RgbaImage, u32)> {
        frames
            .take(MAX_FRAMES)
            .filter_map(|f| f.ok())
            .map(|f| {
                let (num, den) = f.delay().numer_denom_ms();
                let ms = num / den.max(1);
                let ms = if ms == 0 { 100 } else { ms.clamp(20, 1000) };
                (f.into_buffer(), ms)
            })
            .collect()
    }
    if bytes.starts_with(b"GIF8") {
        if let Ok(d) = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)) {
            return collect(d.into_frames());
        }
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        if let Ok(d) = image::codecs::png::PngDecoder::new(std::io::Cursor::new(bytes)) {
            if d.is_apng().unwrap_or(false) {
                if let Ok(a) = d.apng() {
                    return collect(a.into_frames());
                }
            }
        }
    }
    if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        if let Ok(d) = image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(bytes)) {
            if d.has_animation() {
                return collect(d.into_frames());
            }
        }
    }
    Vec::new()
}

/// Store animation frames as `{stem}.f{n}.png`. The cut-out (auto mode)
/// samples its background reference colors once, from frame 0, so the cut
/// stays stable across frames; implausible frames are left un-cut rather
/// than flickering. Face detection runs on frame 0 only.
fn store_animation(
    frames: Vec<(RgbaImage, u32)>,
    dir: &Path,
    stem: &str,
    mode: PhotoMode,
) -> Result<Processed, String> {
    let mut out = Vec::new();
    let mut refs: Option<Vec<[i32; 3]>> = None;
    let mut face = None;
    for (n, (frame, ms)) in frames.into_iter().enumerate() {
        let img = image::DynamicImage::ImageRgba8(frame).resize(
            ANIM_EDGE,
            ANIM_EDGE,
            image::imageops::FilterType::Triangle,
        );
        let mut rgba = img.to_rgba8();
        if n == 0 && mode != PhotoMode::Raw {
            face = Some(
                detect_face(&rgba)
                    .unwrap_or_else(|| silhouette_face(&rgba, split_heuristic(&rgba))),
            );
        }
        if mode == PhotoMode::Auto {
            let transparent = rgba.pixels().filter(|p| p[3] < 128).count();
            if transparent <= (rgba.width() * rgba.height()) as usize / 50 {
                let r = refs.get_or_insert_with(|| sample_refs(&rgba));
                cutout_with_refs(&mut rgba, r);
            }
        }
        let path = dir.join(format!("{stem}.f{n}.png"));
        rgba.save(&path).map_err(|e| e.to_string())?;
        out.push((path, ms));
    }
    Ok(Processed {
        path: out[0].0.clone(),
        face,
        frames: out,
    })
}

/// Delete this stem's previous photo files (`{stem}.png` / `{stem}.gif` / …
/// and frames `{stem}.f{n}.png`) so re-uploads never leave orphans. Files of
/// a longer stem like `{stem}.talk.png` have an extra dot and are kept.
fn remove_stale(dir: &Path, stem: &str) {
    let prefix = format!("{}.", stem);
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(rest) = name.to_str().and_then(|n| n.strip_prefix(&prefix)) else {
            continue;
        };
        let is_frame = rest
            .strip_prefix('f')
            .and_then(|r| r.strip_suffix(".png"))
            .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
        if is_frame || !rest.contains('.') {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Detect the (largest) face and read its geometry: the flap/jaw hinge in
/// the middle of the mouth (coarse anchor from the face box → lip-parting
/// shadow → mid-teeth when visible), the eye band for blinking, the chin,
/// and the face's horizontal extent.
/// Returns None when no face is found.
fn detect_face(img: &RgbaImage) -> Option<Face> {
    let model = rustface::model::read_model(std::io::Cursor::new(FACE_MODEL)).ok()?;
    let mut detector = rustface::create_detector_with_model(model);
    detector.set_min_face_size(20);
    detector.set_score_thresh(2.0);
    detector.set_pyramid_scale_factor(0.8);
    detector.set_slide_window_step(4, 4);

    let gray = image::DynamicImage::ImageRgba8(img.clone()).to_luma8();
    let data = rustface::ImageData::new(&gray, gray.width(), gray.height());
    let faces = detector.detect(&data);
    let best = faces
        .iter()
        .max_by_key(|f| f.bbox().width() * f.bbox().height())?;
    let bbox = best.bbox();
    let b = (bbox.x(), bbox.y(), bbox.width(), bbox.height());
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (bx, by, bw, bh) = (b.0 as f32, b.1 as f32, b.2 as f32, b.3 as f32);
    // coarse anchor from the face box (lip parting sits ~76.5% down a
    // SeetaFace box, ±2% across faces), then land exactly on the parting:
    // the darkest row shadow between the lips
    let anchor = by + bh * 0.765;
    let parting = darkest_row_near(img, b, anchor);
    // hinge in the MIDDLE of the mouth so upper teeth lift with the head and
    // the lower lip stays — like a real jaw. Teeth band center when teeth are
    // detectable, otherwise the parting shadow nudged into the mouth.
    let refined = center_on_teeth(img, b, parting).unwrap_or(parting + 0.02 * bh);
    let split = (refined / h).clamp(0.3, 0.85);
    let eyes = eyes_in_box(img, b);
    // a SeetaFace box ends roughly at the chin
    let chin = ((by + 1.02 * bh) / h).clamp((split + 0.04).min(0.97), 0.99);
    let face_x = (
        ((bx + 0.08 * bw) / w).clamp(0.0, 1.0),
        ((bx + 0.92 * bw) / w).clamp(0.0, 1.0),
    );
    if std::env::var_os("MOTIVATOR_DEBUG_FACE").is_some() {
        eprintln!(
            "face bbox: x={bx} y={by} w={bw} h={bh} (image {w}x{h}) anchor={anchor:.1} \
             parting={parting:.1} refined={refined:.1} eyes={eyes:?} chin={chin:.3}"
        );
    }
    Some(Face {
        split,
        eyes,
        chin,
        face_x,
    })
}

/// Locate the eye band inside the face box. Both the brow and the eyes are
/// dark horizontal bands in the upper face — per face half, take the darkest
/// row in the upper window as the brow, then the darkest row clearly below
/// it as the eye; the halves must agree, so hair or glasses edges can't fake
/// an eye line. Returns (center y, band height) as image fractions.
fn eyes_in_box(img: &RgbaImage, bbox: (i32, i32, u32, u32)) -> Option<(f32, f32)> {
    let (bx, by, bw, bh) = (bbox.0 as f32, bbox.1 as f32, bbox.2 as f32, bbox.3 as f32);
    let h = img.height() as f32;
    let half = |x_lo: f32, x_hi: f32| -> Option<f32> {
        let (y_lo, y_hi) = (by + 0.20 * bh, by + 0.55 * bh);
        let brow = darkest_row_in(img, x_lo, x_hi, y_lo, y_hi)?;
        // the eye shadow sits below the brow; when nothing darker shows up
        // down there, the "brow" row most likely was the eye already
        darkest_row_in(img, x_lo, x_hi, brow + 0.06 * bh, y_hi).or(Some(brow))
    };
    let l = half(bx + 0.13 * bw, bx + 0.45 * bw)?;
    let r = half(bx + 0.55 * bw, bx + 0.87 * bw)?;
    if (l - r).abs() > 0.06 * bh {
        return None; // halves disagree — better no blink than a wrong one
    }
    Some(((l + r) / 2.0 / h, 0.11 * bh / h))
}

/// Darkest row (mean luminance over opaque pixels) within the window, or
/// None when the window is degenerate or has no contrast worth trusting.
fn darkest_row_in(img: &RgbaImage, x_lo: f32, x_hi: f32, y_lo: f32, y_hi: f32) -> Option<f32> {
    let x_lo = (x_lo.max(0.0) as u32).min(img.width().saturating_sub(1));
    let x_hi = (x_hi.max(0.0) as u32).min(img.width().saturating_sub(1));
    let y_lo = (y_lo.max(0.0) as u32).min(img.height().saturating_sub(1));
    let y_hi = (y_hi.max(0.0) as u32).min(img.height().saturating_sub(1));
    if x_lo >= x_hi || y_lo >= y_hi {
        return None;
    }
    let mut best: Option<(f32, f32)> = None; // (row, mean luminance)
    let mut sum_all = 0.0;
    let mut rows = 0.0;
    for y in y_lo..=y_hi {
        let mut sum = 0.0;
        let mut n = 0u32;
        for x in x_lo..=x_hi {
            let p = img.get_pixel(x, y);
            if p[3] > 128 {
                sum += 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
                n += 1;
            }
        }
        if n * 3 < x_hi - x_lo {
            continue; // mostly transparent row — outside the subject
        }
        let mean = sum / n as f32;
        sum_all += mean;
        rows += 1.0;
        if best.is_none_or(|(_, m)| mean < m) {
            best = Some((y as f32, mean));
        }
    }
    let (row, min_mean) = best?;
    // flat skin has no eye/brow shadow — demand real contrast
    if rows < 3.0 || min_mean > 0.92 * (sum_all / rows) {
        return None;
    }
    Some(row)
}

/// The lip parting is the darkest horizontal shadow band near the mouth.
/// Return the darkest row (mean luminance over the central half of the face
/// box) within a narrow window around the anchor line.
fn darkest_row_near(img: &RgbaImage, bbox: (i32, i32, u32, u32), anchor: f32) -> f32 {
    let (bx, _by, bw, bh) = bbox;
    let win = 0.06 * bh as f32;
    let y_lo = (anchor - win).max(0.0) as u32;
    let y_hi = ((anchor + win) as u32).min(img.height().saturating_sub(1));
    let x_lo = (bx + bw as i32 / 4).clamp(0, img.width() as i32 - 1) as u32;
    let x_hi = (bx + (bw as i32 * 3) / 4).clamp(0, img.width() as i32 - 1) as u32;
    if y_lo >= y_hi || x_lo >= x_hi {
        return anchor;
    }
    let mut best = (anchor, f32::MAX);
    for y in y_lo..=y_hi {
        let mut sum = 0.0;
        for x in x_lo..=x_hi {
            let p = img.get_pixel(x, y);
            sum += 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
        }
        let mean = sum / (x_hi - x_lo + 1) as f32;
        if mean < best.1 {
            best = (y as f32, mean);
        }
    }
    best.0
}

/// Locate the visible teeth band (bright, low-chroma rows in the central
/// strip of the face) around the candidate mouth line and return its center
/// — splitting mid-teeth sends the upper set with the lifting head and keeps
/// the lower set with the jaw, like a mouth actually opening.
/// Returns None when no plausible band exists (closed mouth, warm-lit teeth
/// that match skin tones, pale-skin false positives).
fn center_on_teeth(img: &RgbaImage, bbox: (i32, i32, u32, u32), mouth_y: f32) -> Option<f32> {
    let (bx, _by, bw, bh) = bbox;
    let win = 0.12 * bh as f32;
    let y_lo = (mouth_y - win).max(0.0) as u32;
    let y_hi = ((mouth_y + win) as u32).min(img.height().saturating_sub(1));
    // central half of the face box — teeth live there, collars/ears don't
    let x_lo = (bx + bw as i32 / 4).clamp(0, img.width() as i32 - 1) as u32;
    let x_hi = (bx + (bw as i32 * 3) / 4).clamp(0, img.width() as i32 - 1) as u32;
    if y_lo >= y_hi || x_lo >= x_hi {
        return None;
    }
    let toothy: Vec<bool> = (y_lo..=y_hi)
        .map(|y| {
            let n = (x_lo..=x_hi)
                .filter(|&x| {
                    let p = img.get_pixel(x, y);
                    let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
                    let mx = r.max(g).max(b);
                    let mn = r.min(g).min(b);
                    // bright and near-neutral — teeth, not (warm) skin or lips
                    p[3] > 128 && mx > 130 && mx - mn < 45
                })
                .count();
            n as u32 * 8 > x_hi - x_lo // ≥ 12.5% of the strip width
        })
        .collect();
    // longest contiguous toothy run; require a couple of rows to avoid
    // snapping to specular highlights
    let (mut best_start, mut best_len) = (0usize, 0usize);
    let (mut run_start, mut run_len) = (0usize, 0usize);
    for (i, &t) in toothy.iter().enumerate() {
        if t {
            if run_len == 0 {
                run_start = i;
            }
            run_len += 1;
            if run_len > best_len {
                best_start = run_start;
                best_len = run_len;
            }
        } else {
            run_len = 0;
        }
    }
    if best_len < 2.max((0.015 * bh as f32) as usize) {
        return None; // no teeth visible (closed mouth)
    }
    if best_len as f32 > 0.08 * bh as f32 {
        return None; // far too tall for teeth — pale skin, not a mouth
    }
    let band_top = y_lo as f32 + best_start as f32;
    if band_top < mouth_y - 0.06 * bh as f32 || band_top > mouth_y + 0.12 * bh as f32 {
        return None; // stray highlight away from the mouth line
    }
    Some(band_top + best_len as f32 / 2.0)
}

/// Remove the background by flood-filling from the top/left/right borders,
/// matching any of four reference colors sampled near the corners/edges.
/// Returns the mouth split if the cut-out looks plausible, None if the image
/// was left untouched (removal ratio implausible).
fn cutout(img: &mut RgbaImage) -> Option<f32> {
    let refs = sample_refs(img);
    if !cutout_with_refs(img, &refs) {
        return None;
    }
    split_heuristic(img).or(Some(0.52))
}

/// Background reference colors sampled near the corners/edges.
fn sample_refs(img: &RgbaImage) -> Vec<[i32; 3]> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w < 8 || h < 8 {
        return Vec::new();
    }
    let px = img.as_raw();
    [(1, 1), (w - 2, 1), (1, h / 2), (w - 2, h / 2)]
        .iter()
        .map(|&(x, y)| {
            let i = (y * w + x) * 4;
            [px[i] as i32, px[i + 1] as i32, px[i + 2] as i32]
        })
        .collect()
}

/// Flood fill from the borders against `refs`; returns false when the
/// removal ratio was implausible (image left untouched).
fn cutout_with_refs(img: &mut RgbaImage, refs: &[[i32; 3]]) -> bool {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w < 8 || h < 8 || refs.is_empty() {
        return false;
    }
    let px = img.as_mut();
    let tol = 48 * 48;
    let is_bg = |px: &[u8], p: usize| {
        let i = p * 4;
        let (r, g, b) = (px[i] as i32, px[i + 1] as i32, px[i + 2] as i32);
        refs.iter().any(|rf| {
            let (dr, dg, db) = (r - rf[0], g - rf[1], b - rf[2]);
            dr * dr + dg * dg + db * db < tol
        })
    };

    let mut removed = vec![false; w * h];
    let mut stack: Vec<usize> = (0..w).collect(); // top edge
    for y in 0..h {
        stack.push(y * w); // left edge
        stack.push(y * w + w - 1); // right edge
    }
    while let Some(p) = stack.pop() {
        if removed[p] || !is_bg(px, p) {
            continue;
        }
        removed[p] = true;
        let (x, y) = (p % w, p / w);
        if x > 0 {
            stack.push(p - 1);
        }
        if x < w - 1 {
            stack.push(p + 1);
        }
        if y > 0 {
            stack.push(p - w);
        }
        if y < h - 1 {
            stack.push(p + w);
        }
    }

    let n = removed.iter().filter(|&&r| r).count();
    if n < w * h / 20 || n > w * h * 9 / 10 {
        return false; // not a clean background — keep the photo as-is
    }
    for p in 0..w * h {
        if removed[p] {
            px[p * 4 + 3] = 0;
        }
    }
    // background leftovers (clouds, props) survive the flood fill as floating
    // islands — keep only the largest connected opaque region, the subject
    keep_largest_component(px, w, h);
    // soften the silhouette edge
    for p in 0..w * h {
        if removed[p] {
            continue;
        }
        let (x, y) = (p % w, p / w);
        let edge = (x > 0 && removed[p - 1])
            || (x < w - 1 && removed[p + 1])
            || (y > 0 && removed[p - w])
            || (y < h - 1 && removed[p + w]);
        if edge {
            px[p * 4 + 3] = px[p * 4 + 3].min(110);
        }
    }
    true
}

/// Zero the alpha of every 4-connected opaque region except the largest one.
fn keep_largest_component(px: &mut [u8], w: usize, h: usize) {
    let mut label = vec![0u32; w * h]; // 0 = unvisited/transparent
    let mut sizes = vec![0usize]; // sizes[label]
    let mut stack = Vec::new();
    for start in 0..w * h {
        if label[start] != 0 || px[start * 4 + 3] == 0 {
            continue;
        }
        let id = sizes.len() as u32;
        sizes.push(0);
        stack.push(start);
        label[start] = id;
        while let Some(p) = stack.pop() {
            sizes[id as usize] += 1;
            let (x, y) = (p % w, p / w);
            for q in [
                (x > 0).then(|| p - 1),
                (x < w - 1).then(|| p + 1),
                (y > 0).then(|| p - w),
                (y < h - 1).then(|| p + w),
            ]
            .into_iter()
            .flatten()
            {
                if label[q] == 0 && px[q * 4 + 3] > 0 {
                    label[q] = id;
                    stack.push(q);
                }
            }
        }
    }
    if sizes.len() <= 2 {
        return; // zero or one region — nothing to prune
    }
    let biggest = (1..sizes.len()).max_by_key(|&i| sizes[i]).unwrap() as u32;
    for p in 0..w * h {
        if label[p] != 0 && label[p] != biggest {
            px[p * 4 + 3] = 0;
        }
    }
}

/// Estimate the mouth line from the opaque silhouette: top of head → widest
/// point (ears/hair) → narrowest below it (neck); mouth sits ~80% of the way
/// from crown to neck. Clamped to [0.3, 0.78] of image height.
pub fn split_heuristic(img: &RgbaImage) -> Option<f32> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let px = img.as_raw();
    let mut widths = vec![0usize; h];
    for y in 0..h {
        widths[y] = (0..w).filter(|&x| px[(y * w + x) * 4 + 3] > 128).count();
    }
    let mut top_y = 0;
    while top_y < h && widths[top_y] < w / 50 {
        top_y += 1;
    }
    if top_y >= h.saturating_sub(10) {
        return None;
    }
    let lim = (top_y + (h - top_y) / 2).min(h - 1);
    let (mut peak_y, mut peak_w) = (top_y, 0usize);
    for (y, &wy) in widths.iter().enumerate().take(lim + 1).skip(top_y) {
        if wy > peak_w {
            peak_w = wy;
            peak_y = y;
        }
    }
    let (mut neck_y, mut min_w) = (peak_y, usize::MAX);
    for (y, &wy) in widths.iter().enumerate().skip(peak_y) {
        if wy * 4 > peak_w * 5 {
            break; // shoulders — wider than the head peak
        }
        if wy < min_w {
            min_w = wy;
            neck_y = y;
        }
    }
    if neck_y <= top_y + 4 {
        return None;
    }
    let split = (top_y as f32 + 0.85 * (neck_y - top_y) as f32) / h as f32;
    Some(split.clamp(0.3, 0.78))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// flat background + head/neck/shoulder silhouette
    fn portrait() -> RgbaImage {
        let (w, h) = (64u32, 64u32);
        let mut img = RgbaImage::from_pixel(w, h, image::Rgba([240, 240, 240, 255]));
        let skin = image::Rgba([170, 120, 90, 255]);
        for y in 6..40 {
            for x in 22..42 {
                img.put_pixel(x, y, skin); // head
            }
        }
        for y in 40..50 {
            for x in 28..36 {
                img.put_pixel(x, y, skin); // neck
            }
        }
        for y in 50..64 {
            for x in 6..58 {
                img.put_pixel(x, y, skin); // shoulders
            }
        }
        img
    }

    #[test]
    fn cutout_removes_background_and_finds_mouth() {
        let mut img = portrait();
        // a floating "cloud" that the flood fill can't reach (color far from
        // the background refs) — must be pruned as a disconnected island
        for y in 2..5 {
            for x in 4..12 {
                img.put_pixel(x, y, image::Rgba([90, 200, 60, 255]));
            }
        }
        let split = cutout(&mut img).expect("plausible cut-out");
        assert_eq!(img.get_pixel(1, 1)[3], 0, "background must be transparent");
        assert_eq!(img.get_pixel(63, 20)[3], 0);
        assert!(img.get_pixel(32, 20)[3] > 0, "head must stay opaque");
        assert_eq!(img.get_pixel(8, 3)[3], 0, "floating island must be pruned");
        // mouth ≈ 80% from crown (y=6) to neck minimum → around y≈0.5–0.6 of height
        assert!((0.3..=0.78).contains(&split), "split={split}");
    }

    #[test]
    fn precut_png_keeps_its_alpha() {
        // head-only silhouette with existing transparency
        let mut img = RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 0]));
        let skin = image::Rgba([170, 120, 90, 255]);
        for y in 4..60 {
            // ellipse-ish head: wide in the middle, narrow at crown and chin
            let half = 6 + ((y as i32 - 32).unsigned_abs() as f32 / 28.0 * -14.0 + 16.0) as u32;
            for x in (32 - half.min(26))..(32 + half.min(26)) {
                img.put_pixel(x, y, skin);
            }
        }
        let dir = std::env::temp_dir().join("motivator-test-photos");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("precut.png");
        img.save(&src).unwrap();

        std::env::set_var("XDG_DATA_HOME", dir.join("data"));
        let p = process_and_store(&src, "precut-test", PhotoMode::Auto).unwrap();
        let out = image::open(&p.path).unwrap().to_rgba8();
        // alpha preserved: corners stay transparent, center stays opaque
        assert_eq!(out.get_pixel(1, 1)[3], 0);
        assert!(out.get_pixel(out.width() / 2, out.height() / 2)[3] > 0);
        let split = p.face.expect("still photos report a face").split;
        assert!((0.3..=0.78).contains(&split), "split={split}");
        assert!(p.frames.is_empty());
    }

    fn png_bytes(img: &RgbaImage) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(img.clone())
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join("motivator-photo-tests")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn precut_mode_never_touches_an_opaque_background() {
        // a portrait on a clean flat background: auto would flood-fill it away,
        // "already cut out" must leave every pixel opaque
        let dir = tmp_dir("precut-mode");
        let p = process_bytes(
            &png_bytes(&portrait()),
            Some("png"),
            &dir,
            "x",
            PhotoMode::Precut,
        )
        .unwrap();
        let out = image::open(&p.path).unwrap().to_rgba8();
        assert!(out.pixels().all(|px| px[3] == 255), "no pixel may be cut");
        assert!(p.face.is_some(), "precut still detects the mouth");
    }

    #[test]
    fn raw_mode_stores_the_file_byte_identical() {
        let dir = tmp_dir("raw-mode");
        let bytes = png_bytes(&portrait());
        let p = process_bytes(&bytes, Some("png"), &dir, "x", PhotoMode::Raw).unwrap();
        assert_eq!(std::fs::read(&p.path).unwrap(), bytes);
        assert!(p.face.is_none(), "raw mode skips detection");
        assert!(p.frames.is_empty());
    }

    #[test]
    fn animated_gif_becomes_frames_with_delays() {
        let mut bytes = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut bytes);
            enc.set_repeat(image::codecs::gif::Repeat::Infinite)
                .unwrap();
            for shade in [60u8, 140, 220] {
                let img = RgbaImage::from_pixel(32, 32, image::Rgba([shade, 40, 40, 255]));
                enc.encode_frame(image::Frame::from_parts(
                    img,
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(120, 1),
                ))
                .unwrap();
            }
        }
        let dir = tmp_dir("animated");
        let p = process_bytes(&bytes, Some("gif"), &dir, "x", PhotoMode::Precut).unwrap();
        assert_eq!(p.frames.len(), 3);
        assert_eq!(p.path, p.frames[0].0, "photo points at frame 0");
        for (n, (path, ms)) in p.frames.iter().enumerate() {
            assert!(path.ends_with(format!("x.f{n}.png")), "frame file {n}");
            assert!(path.exists());
            assert_eq!(*ms, 120);
        }
    }

    #[test]
    fn raw_mode_resizes_oversize_images_to_a_png() {
        // a 2400px-wide upload can't become a texture verbatim — raw mode
        // falls back to one resized PNG instead of a byte copy
        let dir = tmp_dir("raw-oversize");
        let img = RgbaImage::from_pixel(2400, 200, image::Rgba([200, 150, 120, 255]));
        let p = process_bytes(&png_bytes(&img), Some("jpg"), &dir, "x", PhotoMode::Raw).unwrap();
        assert!(p.path.ends_with("x.png"), "fallback is a png, not x.jpg");
        let out = image::open(&p.path).unwrap();
        assert!(out.width() <= RAW_EDGE && out.height() <= RAW_EDGE);
        assert!(p.face.is_none());
    }

    fn gif_bytes(frames: &[(RgbaImage, u32)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut enc = image::codecs::gif::GifEncoder::new(&mut bytes);
        enc.set_repeat(image::codecs::gif::Repeat::Infinite)
            .unwrap();
        for (img, ms) in frames {
            enc.encode_frame(image::Frame::from_parts(
                img.clone(),
                0,
                0,
                image::Delay::from_numer_denom_ms(*ms, 1),
            ))
            .unwrap();
        }
        drop(enc);
        bytes
    }

    #[test]
    fn zero_and_extreme_frame_delays_are_normalized() {
        // 0 ms (unset in many GIFs) → 100 ms default; 5 s clamps to 1 s
        let img = RgbaImage::from_pixel(16, 16, image::Rgba([60, 40, 40, 255]));
        let bytes = gif_bytes(&[(img.clone(), 0), (img, 5000)]);
        let frames = decode_frames(&bytes);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].1, 100);
        assert_eq!(frames[1].1, 1000);
    }

    #[test]
    fn animated_auto_mode_cuts_the_background_in_every_frame() {
        // flat light background, dark subject sliding right by a few px per
        // frame — auto mode must remove the background in each frame using
        // the reference colors sampled from frame 0
        let mut frames = Vec::new();
        for n in 0..3u32 {
            let mut img = RgbaImage::from_pixel(64, 64, image::Rgba([240, 240, 240, 255]));
            for y in 16..56 {
                for x in (20 + n)..(40 + n) {
                    img.put_pixel(x, y, image::Rgba([60, 40, 40, 255]));
                }
            }
            frames.push((img, 80));
        }
        let dir = tmp_dir("anim-auto");
        let p =
            process_bytes(&gif_bytes(&frames), Some("gif"), &dir, "x", PhotoMode::Auto).unwrap();
        assert_eq!(p.frames.len(), 3);
        for (n, (path, _)) in p.frames.iter().enumerate() {
            let out = image::open(path).unwrap().to_rgba8();
            assert_eq!(out.get_pixel(1, 1)[3], 0, "frame {n}: corner transparent");
            let c = out.get_pixel(out.width() / 2, out.height() / 2)[3];
            assert!(c > 0, "frame {n}: subject stays opaque");
        }
    }

    #[test]
    fn png_bytes_of_passes_png_and_reencodes_the_rest() {
        let dir = tmp_dir("png-of");
        let img = RgbaImage::from_pixel(8, 8, image::Rgba([200, 150, 120, 255]));
        let png_path = dir.join("a.png");
        img.save(&png_path).unwrap();
        let png = std::fs::read(&png_path).unwrap();
        assert_eq!(png_bytes_of(&png_path).unwrap(), png, "png passes through");

        let jpg_path = dir.join("a.jpg");
        image::DynamicImage::ImageRgba8(img)
            .to_rgb8()
            .save(&jpg_path)
            .unwrap();
        let out = png_bytes_of(&jpg_path).unwrap();
        assert!(out.starts_with(&[0x89, b'P', b'N', b'G']), "jpg → png");
    }

    #[test]
    fn reupload_removes_stale_frames_but_not_the_talk_still() {
        let dir = tmp_dir("stale");
        for name in ["x.f0.png", "x.f1.png", "x.gif", "x.talk.png", "xy.png"] {
            std::fs::write(dir.join(name), b"z").unwrap();
        }
        remove_stale(&dir, "x");
        assert!(!dir.join("x.f0.png").exists());
        assert!(!dir.join("x.f1.png").exists());
        assert!(!dir.join("x.gif").exists());
        assert!(dir.join("x.talk.png").exists(), "other stem must survive");
        assert!(dir.join("xy.png").exists(), "other friend must survive");
    }

    #[test]
    fn split_centers_on_visible_teeth() {
        // warm "skin" face with a bright neutral teeth band at y=44..48
        let mut img = RgbaImage::from_pixel(100, 100, image::Rgba([200, 150, 120, 255]));
        for y in 44..48 {
            for x in 35..65 {
                img.put_pixel(x, y, image::Rgba([230, 228, 225, 255]));
            }
        }
        let bbox = (10, 0, 80u32, 90u32);
        // the hinge goes mid-band: upper teeth lift, lower part stays
        let center = center_on_teeth(&img, bbox, 43.0).expect("band found");
        assert!(
            (45.0..=47.0).contains(&center),
            "split {center} should sit mid-teeth (band 44..48)"
        );
    }

    #[test]
    fn darkest_row_finds_the_lip_parting() {
        // uniform skin with a dark parting shadow at y=50..52
        let mut img = RgbaImage::from_pixel(100, 100, image::Rgba([200, 150, 120, 255]));
        for y in 50..52 {
            for x in 30..70 {
                img.put_pixel(x, y, image::Rgba([70, 40, 35, 255]));
            }
        }
        let row = darkest_row_near(&img, (10, 0, 80, 90), 46.0);
        assert!((50.0..=51.0).contains(&row), "row={row}");
        // no shadow near the anchor → anchor is kept
        let flat = RgbaImage::from_pixel(100, 100, image::Rgba([200, 150, 120, 255]));
        let kept = darkest_row_near(&flat, (10, 0, 80, 90), 46.0);
        assert!((40.0..=52.0).contains(&kept), "kept={kept}");
    }

    #[test]
    fn eyes_found_below_the_brow() {
        // warm skin, a dark brow band at y=24..26 and a darker eye band at
        // y=34..36 across both halves of the face box
        let mut img = RgbaImage::from_pixel(100, 100, image::Rgba([200, 150, 120, 255]));
        for y in 24..27 {
            for x in 15..85 {
                img.put_pixel(x, y, image::Rgba([120, 90, 70, 255]));
            }
        }
        for y in 34..37 {
            for x in 15..85 {
                img.put_pixel(x, y, image::Rgba([55, 40, 35, 255]));
            }
        }
        let (ey, eh) = eyes_in_box(&img, (10, 0, 80, 90)).expect("eyes found");
        assert!(
            (0.32..=0.38).contains(&ey),
            "eye line {ey} should sit on the darker band"
        );
        assert!(eh > 0.05 && eh < 0.15, "band height {eh}");
    }

    #[test]
    fn no_eyes_on_flat_skin_or_tilted_mismatch() {
        // flat skin — no contrast, no eyes
        let img = RgbaImage::from_pixel(100, 100, image::Rgba([200, 150, 120, 255]));
        assert!(eyes_in_box(&img, (10, 0, 80, 90)).is_none());
        // left "eye" high, right "eye" low — halves disagree, no blink
        let mut img = RgbaImage::from_pixel(100, 100, image::Rgba([200, 150, 120, 255]));
        for x in 22..45 {
            img.put_pixel(x, 22, image::Rgba([50, 40, 35, 255]));
        }
        for x in 55..80 {
            img.put_pixel(x, 46, image::Rgba([50, 40, 35, 255]));
        }
        assert!(eyes_in_box(&img, (10, 0, 80, 90)).is_none());
    }

    #[test]
    fn silhouette_fallback_has_geometry_but_no_eyes() {
        // silhouette geometry is read off the cut-out, not the raw photo
        let mut img = portrait();
        cutout(&mut img).expect("plausible cut-out");
        let face = silhouette_face(&img, Some(0.55));
        assert!(face.eyes.is_none(), "no eyes without a detected face");
        assert!((face.chin - 0.72).abs() < 1e-5);
        // the mouth row (y=35) crosses the head (x 22..42) — face_x must
        // cover exactly that opaque extent, not the full image
        assert!(face.face_x.0 > 0.2 && face.face_x.1 < 0.8);
        assert!(face.face_x.1 > face.face_x.0);
    }

    #[test]
    fn no_teeth_band_on_closed_mouth() {
        // closed mouth: uniform warm skin, no bright neutral band
        let img = RgbaImage::from_pixel(100, 100, image::Rgba([200, 150, 120, 255]));
        assert!(center_on_teeth(&img, (10, 0, 80, 90), 46.0).is_none());
    }

    #[test]
    fn cutout_keeps_almost_empty_photo() {
        // nearly everything matches the background → implausible, image untouched
        let mut img = RgbaImage::from_pixel(64, 64, image::Rgba([240, 240, 240, 255]));
        img.put_pixel(32, 32, image::Rgba([10, 10, 10, 255]));
        let before = img.clone();
        assert!(cutout(&mut img).is_none());
        assert_eq!(img.as_raw(), before.as_raw());
    }
}
