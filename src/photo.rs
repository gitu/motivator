//! Photo pipeline: background cut-out (border flood fill against corner
//! reference colors) and mouth-line estimation for the talking flap.
//! Direct port of the design's canvas pipeline.

use std::path::{Path, PathBuf};

use image::RgbaImage;

pub struct Processed {
    pub path: PathBuf,
    pub split: f32,
}

pub fn process_and_store(src: &Path, friend_id: &str) -> Result<Processed, String> {
    let img = image::open(src).map_err(|e| format!("could not read image: {e}"))?;
    // cap size: plenty for a <=96px avatar, keeps flood fill + textures cheap
    let img = img.resize(512, 512, image::imageops::FilterType::Triangle);
    let mut rgba = img.to_rgba8();

    // pre-cut PNGs (real transparency) skip the flood fill — the alpha channel
    // already is the cut-out; we only estimate the mouth line
    let transparent = rgba.pixels().filter(|p| p[3] < 128).count();
    let split = if transparent > (rgba.width() * rgba.height()) as usize / 50 {
        split_heuristic(&rgba).unwrap_or(0.52)
    } else {
        match cutout(&mut rgba) {
            Some(split) => split,
            None => split_heuristic(&rgba).unwrap_or(0.52),
        }
    };

    let dir = crate::config::photos_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{friend_id}.png"));
    rgba.save(&path).map_err(|e| e.to_string())?;
    Ok(Processed { path, split })
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
    fn cutout_keeps_almost_empty_photo() {
        // nearly everything matches the background → implausible, image untouched
        let mut img = RgbaImage::from_pixel(64, 64, image::Rgba([240, 240, 240, 255]));
        img.put_pixel(32, 32, image::Rgba([10, 10, 10, 255]));
        let before = img.clone();
        assert!(cutout(&mut img).is_none());
        assert_eq!(img.as_raw(), before.as_raw());
    }
}
