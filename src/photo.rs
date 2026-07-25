//! Photo pipeline: background cut-out (border flood fill against corner
//! reference colors) and mouth-line estimation for the talking flap.
//! Direct port of the design's canvas pipeline.

use std::path::{Path, PathBuf};

use image::RgbaImage;

/// SeetaFace frontal detection model (BSD-2, VIPL group / rustface project)
static FACE_MODEL: &[u8] = include_bytes!("../assets/seeta_fd_frontal_v1.0.bin");

pub struct Processed {
    pub path: PathBuf,
    pub split: f32,
}

pub fn process_and_store(src: &Path, friend_id: &str) -> Result<Processed, String> {
    let img = image::open(src).map_err(|e| format!("could not read image: {e}"))?;
    // cap size: plenty for a <=96px avatar, keeps flood fill + textures cheap
    let img = img.resize(512, 512, image::imageops::FilterType::Triangle);
    let mut rgba = img.to_rgba8();

    // find the mouth on the pristine photo before the cut-out touches it;
    // the silhouette heuristic is only the no-face-found fallback
    let face_split = mouth_from_face(&rgba);

    // pre-cut PNGs (real transparency) skip the flood fill — the alpha channel
    // already is the cut-out
    let transparent = rgba.pixels().filter(|p| p[3] < 128).count();
    let heuristic_split = if transparent > (rgba.width() * rgba.height()) as usize / 50 {
        split_heuristic(&rgba)
    } else {
        cutout(&mut rgba).or_else(|| split_heuristic(&rgba))
    };
    let split = face_split.or(heuristic_split).unwrap_or(0.52);

    let dir = crate::config::photos_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{friend_id}.png"));
    rgba.save(&path).map_err(|e| e.to_string())?;
    Ok(Processed { path, split })
}

/// Detect the (largest) face and place the mouth line at 74% of the face
/// box height — the lip line's position within SeetaFace's detection box,
/// calibrated on sample portraits. (The original design used 82% of the
/// browser FaceDetector's box, which frames faces differently.)
/// Returns None when no face is found.
fn mouth_from_face(img: &RgbaImage) -> Option<f32> {
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
    let mouth_y = bbox.y() as f32 + bbox.height() as f32 * 0.74;
    let refined = snap_above_teeth(
        img,
        (bbox.x(), bbox.y(), bbox.width(), bbox.height()),
        mouth_y,
    );
    if std::env::var_os("MOTIVATOR_DEBUG_FACE").is_some() {
        eprintln!(
            "face bbox: x={} y={} w={} h={} (image {}x{}) mouth_y={mouth_y:.1} refined={refined:.1}",
            bbox.x(),
            bbox.y(),
            bbox.width(),
            bbox.height(),
            img.width(),
            img.height()
        );
    }
    Some((refined / img.height() as f32).clamp(0.3, 0.85))
}

/// If teeth are visible (a smile), a split through them puts teeth on both
/// flap slices — creepy in motion. Look for the teeth band (bright, low
/// chroma rows in the central strip of the face) around the candidate mouth
/// line and snap the split to just above it, so the whole set of teeth stays
/// on the static bottom slice and the lifting top reads as the upper lip.
fn snap_above_teeth(img: &RgbaImage, bbox: (i32, i32, u32, u32), mouth_y: f32) -> f32 {
    let (bx, _by, bw, bh) = bbox;
    let win = 0.12 * bh as f32;
    let y_lo = (mouth_y - win).max(0.0) as u32;
    let y_hi = ((mouth_y + win) as u32).min(img.height().saturating_sub(1));
    // central half of the face box — teeth live there, collars/ears don't
    let x_lo = (bx + bw as i32 / 4).clamp(0, img.width() as i32 - 1) as u32;
    let x_hi = (bx + (bw as i32 * 3) / 4).clamp(0, img.width() as i32 - 1) as u32;
    if y_lo >= y_hi || x_lo >= x_hi {
        return mouth_y;
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
        return mouth_y; // no teeth visible (closed mouth) — keep the estimate
    }
    if best_len as f32 > 0.08 * bh as f32 {
        return mouth_y; // far too tall for teeth — pale skin, not a mouth
    }
    let band_top = y_lo as f32 + best_start as f32;
    if band_top < mouth_y - 0.06 * bh as f32 || band_top > mouth_y + 0.12 * bh as f32 {
        return mouth_y; // stray highlight away from the mouth line
    }
    (band_top - 0.015 * bh as f32).max(0.0)
}

/// Remove the background by flood-filling from the top/left/right borders,
/// matching any of four reference colors sampled near the corners/edges.
/// Returns the mouth split if the cut-out looks plausible, None if the image
/// was left untouched (removal ratio implausible).
fn cutout(img: &mut RgbaImage) -> Option<f32> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w < 8 || h < 8 {
        return None;
    }
    let px = img.as_mut();
    let refs: Vec<[i32; 3]> = [(1, 1), (w - 2, 1), (1, h / 2), (w - 2, h / 2)]
        .iter()
        .map(|&(x, y)| {
            let i = (y * w + x) * 4;
            [px[i] as i32, px[i + 1] as i32, px[i + 2] as i32]
        })
        .collect();
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
        return None; // not a clean background — keep the photo as-is
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
    split_heuristic(img).or(Some(0.52))
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
        let p = process_and_store(&src, "precut-test").unwrap();
        let out = image::open(&p.path).unwrap().to_rgba8();
        // alpha preserved: corners stay transparent, center stays opaque
        assert_eq!(out.get_pixel(1, 1)[3], 0);
        assert!(out.get_pixel(out.width() / 2, out.height() / 2)[3] > 0);
        assert!((0.3..=0.78).contains(&p.split), "split={}", p.split);
    }

    #[test]
    fn split_snaps_above_visible_teeth() {
        // warm "skin" face with a bright neutral teeth band at y=44..48
        let mut img = RgbaImage::from_pixel(100, 100, image::Rgba([200, 150, 120, 255]));
        for y in 44..48 {
            for x in 35..65 {
                img.put_pixel(x, y, image::Rgba([230, 228, 225, 255]));
            }
        }
        let bbox = (10, 0, 80u32, 90u32);
        // candidate line mid-teeth (y=46) must snap above the band
        let snapped = snap_above_teeth(&img, bbox, 46.0);
        assert!(
            snapped < 44.0,
            "split {snapped} should sit above the teeth band at y=44"
        );
        assert!(snapped > 38.0, "split {snapped} should stay near the mouth");
    }

    #[test]
    fn split_unchanged_without_teeth() {
        // closed mouth: uniform warm skin, no bright neutral band
        let img = RgbaImage::from_pixel(100, 100, image::Rgba([200, 150, 120, 255]));
        assert_eq!(snap_above_teeth(&img, (10, 0, 80, 90), 46.0), 46.0);
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
