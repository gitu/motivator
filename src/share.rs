//! Friend cards: a friend's whole config steganographically embedded in a PNG.
//!
//! The payload lives in the 2 low bits of each RGB channel (alpha untouched —
//! the card is rendered fully opaque), so it survives any lossless pixel
//! round-trip: clipboard copy/paste, PNG re-save. Metadata chunks would be
//! stripped by clipboards and chat apps; pixel LSBs are not. Lossy paths
//! (screenshots, JPEG re-encoding, resizing) destroy the payload — the card
//! must travel as the PNG itself.

use std::io::Cursor;

use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::config::{self, Accent, Config, Expansion, Friend, Quote};

const MAGIC: &[u8; 4] = b"MOTV";
const VERSION: u8 = 1;
/// payload bits hidden per color channel
const BITS: u8 = 2;
const MASK: u8 = (1 << BITS) - 1;
/// card edge in px — capacity at 2 bits × 3 channels: 512·512·3/4 = 192 KiB
const CARD: u32 = 512;
/// payload photo edge — plenty for a ≤96px on-screen avatar
const SHARE_PHOTO: u32 = 256;

/// Everything that defines a friend except its local id and photo path.
/// The photo travels beside it as raw PNG bytes. The LLM endpoint settings
/// (`Config.api`: base url, token, model) are global config, not part of the
/// friend — they never enter a card, only the friend's own texts and
/// behavior do.
#[derive(Clone, Serialize, Deserialize)]
pub struct SharedFriend {
    pub name: String,
    pub accent: Accent,
    pub split: f32,
    /// both prompt fields arrived after card v1 — older cards default them
    /// to empty, older apps ignore the extra keys
    #[serde(default)]
    pub persona: String,
    #[serde(default)]
    pub chat_prompt: String,
    pub quotes: Vec<Quote>,
    pub pool: Vec<String>,
    pub expansion: Expansion,
    pub nudges: bool,
    pub interval_secs: u64,
}

/// Render the friend's card and embed their config + photo in its pixels.
/// `accent` is the friend's accent color (theme-resolved RGB).
pub fn encode_card(friend: &Friend, accent: [u8; 3]) -> Result<RgbaImage, String> {
    let photo = friend
        .photo
        .as_ref()
        .and_then(|p| image::open(p).ok())
        .map(|i| i.to_rgba8());
    let photo_png = match &photo {
        Some(img) => png_bytes(&downscale(img, SHARE_PHOTO))?,
        None => Vec::new(),
    };
    let shared = SharedFriend {
        name: friend.name.clone(),
        accent: friend.accent,
        split: friend.split,
        persona: friend.persona.clone(),
        chat_prompt: friend.chat_prompt.clone(),
        quotes: friend.quotes.clone(),
        pool: friend.pool.clone(),
        expansion: friend.expansion,
        nudges: friend.nudges,
        interval_secs: friend.interval_secs,
    };
    let json = serde_json::to_vec(&shared).map_err(|e| e.to_string())?;
    let mut card = render_card(photo.as_ref(), accent);
    embed(&mut card, &frame_payload(&json, &photo_png))?;
    Ok(card)
}

/// Extract a shared friend (and their photo as PNG bytes) from a card image.
pub fn decode_card(img: &RgbaImage) -> Result<(SharedFriend, Option<Vec<u8>>), String> {
    let bytes = extract_all(img);
    let missing = || "no friend data found in this image".to_string();
    // magic + version + json_len + photo_len + crc with empty json/photo
    if bytes.len() < 17 || &bytes[..4] != MAGIC {
        return Err(missing());
    }
    if bytes[4] != VERSION {
        return Err(format!(
            "card format v{} — update motivator to import it",
            bytes[4]
        ));
    }
    let json_len = u32::from_le_bytes(bytes[5..9].try_into().unwrap()) as usize;
    let json_end = 9usize
        .checked_add(json_len)
        .filter(|&e| e + 8 <= bytes.len())
        .ok_or_else(missing)?;
    let photo_len = u32::from_le_bytes(bytes[json_end..json_end + 4].try_into().unwrap()) as usize;
    let photo_start = json_end + 4;
    let photo_end = photo_start
        .checked_add(photo_len)
        .filter(|&e| e + 4 <= bytes.len())
        .ok_or_else(missing)?;
    let stored = u32::from_le_bytes(bytes[photo_end..photo_end + 4].try_into().unwrap());
    if crc32(&bytes[..photo_end]) != stored {
        return Err("this image doesn't carry intact friend data (was it re-encoded?)".into());
    }
    let shared: SharedFriend = serde_json::from_slice(&bytes[9..json_end])
        .map_err(|_| "friend data in this card is unreadable".to_string())?;
    let photo = (photo_len > 0).then(|| bytes[photo_start..photo_end].to_vec());
    Ok((shared, photo))
}

/// Add a decoded friend to the config: mint a fresh id, write the photo to the
/// photos dir, sanitize ranges, push + activate. Returns the new id.
pub fn import_into(
    cfg: &mut Config,
    s: SharedFriend,
    photo_png: Option<Vec<u8>>,
) -> Result<String, String> {
    let id = format!(
        "f{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    let photo = match photo_png {
        Some(bytes) => {
            let img = image::load_from_memory(&bytes)
                .map_err(|e| format!("bad photo in card: {e}"))?
                .to_rgba8();
            let dir = config::photos_dir();
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join(format!("{id}.png"));
            img.save(&path).map_err(|e| e.to_string())?;
            Some(path)
        }
        None => None,
    };
    let mut quotes = s.quotes;
    for q in &mut quotes {
        q.w = q.w.min(5);
    }
    cfg.friends.push(Friend {
        id: id.clone(),
        name: if s.name.is_empty() {
            "friend".into()
        } else {
            s.name
        },
        photo,
        split: if s.split.is_finite() {
            s.split.clamp(0.1, 0.9)
        } else {
            0.52
        },
        persona: s.persona,
        chat_prompt: s.chat_prompt,
        accent: s.accent,
        quotes,
        pool: s.pool,
        expansion: s.expansion,
        nudges: s.nudges,
        interval_secs: s.interval_secs.max(5),
    });
    cfg.active = id.clone();
    Ok(id)
}

fn frame_payload(json: &[u8], photo: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(17 + json.len() + photo.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(json);
    out.extend_from_slice(&(photo.len() as u32).to_le_bytes());
    out.extend_from_slice(photo);
    let crc = crc32(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    out
}

/// usable payload bytes in an image at BITS bits per RGB channel
fn capacity(img: &RgbaImage) -> usize {
    (img.width() as usize * img.height() as usize * 3 * BITS as usize) / 8
}

fn embed(img: &mut RgbaImage, payload: &[u8]) -> Result<(), String> {
    let cap = capacity(img);
    if payload.len() > cap {
        return Err(format!(
            "friend too large to share ({} KB > {} KB card capacity)",
            payload.len() / 1024,
            cap / 1024
        ));
    }
    let px = img.as_mut();
    for (i, &b) in payload.iter().enumerate() {
        for (j, shift) in [6u8, 4, 2, 0].into_iter().enumerate() {
            let c = i * 4 + j; // running RGB channel index, alpha skipped
            let flat = (c / 3) * 4 + (c % 3);
            px[flat] = (px[flat] & !MASK) | ((b >> shift) & MASK);
        }
    }
    Ok(())
}

fn extract_all(img: &RgbaImage) -> Vec<u8> {
    let px = img.as_raw();
    let cap = capacity(img);
    let mut out = Vec::with_capacity(cap);
    for i in 0..cap {
        let mut b = 0u8;
        for j in 0..4 {
            let c = i * 4 + j;
            let flat = (c / 3) * 4 + (c % 3);
            b = (b << BITS) | (px[flat] & MASK);
        }
        out.push(b);
    }
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let m = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & m);
        }
    }
    !crc
}

/// Opaque card: accent-tinted background with the cut-out photo anchored
/// bottom-center (contain). Letter-avatar friends get a plain accent card.
fn render_card(photo: Option<&RgbaImage>, accent: [u8; 3]) -> RgbaImage {
    let mix = |a: u8, b: u8, t: f32| (a as f32 + (b as f32 - a as f32) * t) as u8;
    let t = if photo.is_some() { 0.25 } else { 1.0 };
    let bg = image::Rgba([
        mix(24, accent[0], t),
        mix(24, accent[1], t),
        mix(28, accent[2], t),
        255,
    ]);
    let mut card = RgbaImage::from_pixel(CARD, CARD, bg);
    if let Some(p) = photo {
        let p = downscale(p, CARD);
        let (pw, ph) = p.dimensions();
        let x0 = (CARD - pw) / 2;
        let y0 = CARD - ph;
        for y in 0..ph {
            for x in 0..pw {
                let s = p.get_pixel(x, y);
                let a = s[3] as u32;
                if a == 0 {
                    continue;
                }
                let d = card.get_pixel_mut(x0 + x, y0 + y);
                for i in 0..3 {
                    d[i] = ((s[i] as u32 * a + d[i] as u32 * (255 - a)) / 255) as u8;
                }
            }
        }
    }
    card
}

fn downscale(img: &RgbaImage, max: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w <= max && h <= max {
        return img.clone();
    }
    let s = (max as f32 / w as f32).min(max as f32 / h as f32);
    image::imageops::resize(
        img,
        ((w as f32 * s).round() as u32).max(1),
        ((h as f32 * s).round() as u32).max(1),
        image::imageops::FilterType::Triangle,
    )
}

fn png_bytes(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::QuoteSrc;

    fn friend(photo: Option<std::path::PathBuf>) -> Friend {
        Friend {
            id: "t".into(),
            name: "test pal".into(),
            photo,
            split: 0.5,
            persona: "upbeat, terse".into(),
            chat_prompt: "be {name}".into(),
            accent: Accent::Cyan,
            quotes: vec![
                Quote::sample("go"),
                Quote {
                    t: "ship it".into(),
                    src: QuoteSrc::New,
                    w: 4,
                },
            ],
            pool: vec!["small steps".into()],
            expansion: Expansion::Remix,
            nudges: true,
            interval_secs: 600,
        }
    }

    #[test]
    fn card_roundtrip_without_photo() {
        let card = encode_card(&friend(None), [80, 200, 255]).unwrap();
        let (s, photo) = decode_card(&card).unwrap();
        assert_eq!(s.name, "test pal");
        assert!(matches!(s.accent, Accent::Cyan));
        assert_eq!(s.persona, "upbeat, terse");
        assert_eq!(s.chat_prompt, "be {name}");
        assert_eq!(s.quotes.len(), 2);
        assert_eq!(s.quotes[1].w, 4);
        assert_eq!(s.pool, vec!["small steps".to_string()]);
        assert!(matches!(s.expansion, Expansion::Remix));
        assert!(s.nudges);
        assert_eq!(s.interval_secs, 600);
        assert!(photo.is_none());
        // card must be fully opaque so clipboard alpha handling can't hurt it
        assert!(card.pixels().all(|p| p[3] == 255));
    }

    #[test]
    fn card_payload_never_carries_api_config() {
        // the llm endpoint settings (base url, token, model) are global
        // config, not friend data — pin the payload's exact key set so they
        // can't sneak into a card
        let card = encode_card(&friend(None), [80, 200, 255]).unwrap();
        let bytes = extract_all(&card);
        let json_len = u32::from_le_bytes(bytes[5..9].try_into().unwrap()) as usize;
        let payload: serde_json::Value = serde_json::from_slice(&bytes[9..9 + json_len]).unwrap();
        let mut keys: Vec<&str> = payload
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "accent",
                "chat_prompt",
                "expansion",
                "interval_secs",
                "name",
                "nudges",
                "persona",
                "pool",
                "quotes",
                "split"
            ]
        );
        for leak in ["api", "api_key", "base_url", "model"] {
            assert!(payload.get(leak).is_none(), "{leak} must not be in a card");
        }
    }

    #[test]
    fn card_roundtrip_with_photo_through_png_file() {
        let dir = std::env::temp_dir().join("motivator-test-share");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("face.png");
        let mut img = RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 0]));
        for y in 8..56 {
            for x in 20..44 {
                img.put_pixel(x, y, image::Rgba([170, 120, 90, 255]));
            }
        }
        img.save(&src).unwrap();

        let card = encode_card(&friend(Some(src)), [80, 200, 255]).unwrap();
        // survive an actual PNG write/read round-trip, like a shared file would
        let card_path = dir.join("card.png");
        card.save_with_format(&card_path, image::ImageFormat::Png)
            .unwrap();
        let back = image::open(&card_path).unwrap().to_rgba8();

        let (s, photo) = decode_card(&back).unwrap();
        assert_eq!(s.name, "test pal");
        let photo = image::load_from_memory(&photo.expect("photo travels along"))
            .unwrap()
            .to_rgba8();
        assert_eq!(photo.dimensions(), (64, 64));
        assert_eq!(photo.get_pixel(1, 1)[3], 0, "transparency preserved");
        assert!(photo.get_pixel(32, 32)[3] > 0);
    }

    #[test]
    fn corrupted_card_is_rejected() {
        let mut card = encode_card(&friend(None), [80, 200, 255]).unwrap();
        // flip a payload bit inside the json region (well past the header)
        let p = card.get_pixel_mut(30, 0);
        p[0] ^= 0b01;
        assert!(decode_card(&card).is_err());
    }

    #[test]
    fn foreign_image_is_rejected() {
        let img = RgbaImage::from_pixel(64, 64, image::Rgba([200, 200, 200, 255]));
        let err = decode_card(&img).err().expect("must be rejected");
        assert!(err.contains("no friend data"), "{err}");
    }

    #[test]
    fn oversized_payload_errors_cleanly() {
        let mut img = RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 255]));
        assert!(embed(&mut img, &[0u8; 1024]).is_err());
    }

    #[test]
    fn jpeg_reencode_destroys_the_card() {
        // the documented caveat: lossy re-encoding must fail loudly, not
        // import a corrupted friend
        let dir = std::env::temp_dir().join("motivator-test-share-jpeg");
        std::fs::create_dir_all(&dir).unwrap();
        let card = encode_card(&friend(None), [80, 200, 255]).unwrap();
        let jpg = dir.join("card.jpg");
        image::DynamicImage::ImageRgba8(card)
            .to_rgb8()
            .save_with_format(&jpg, image::ImageFormat::Jpeg)
            .unwrap();
        let back = image::open(&jpg).unwrap().to_rgba8();
        assert!(decode_card(&back).is_err());
    }

    #[test]
    fn newer_card_version_is_reported() {
        let mut payload = Vec::new();
        payload.extend_from_slice(MAGIC);
        payload.push(VERSION + 1);
        payload.extend_from_slice(&[0u8; 12]);
        let mut img = RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 255]));
        embed(&mut img, &payload).unwrap();
        let err = decode_card(&img).err().expect("must be rejected");
        assert!(err.contains("update motivator"), "{err}");
    }

    #[test]
    fn share_photo_is_downscaled() {
        let dir = std::env::temp_dir().join("motivator-test-share-scale");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("big.png");
        RgbaImage::from_pixel(400, 400, image::Rgba([170, 120, 90, 255]))
            .save(&src)
            .unwrap();
        let card = encode_card(&friend(Some(src)), [80, 200, 255]).unwrap();
        let (_, photo) = decode_card(&card).unwrap();
        let photo = image::load_from_memory(&photo.unwrap()).unwrap();
        assert!(photo.width() <= SHARE_PHOTO && photo.height() <= SHARE_PHOTO);
    }

    #[test]
    fn import_into_sanitizes_and_activates() {
        std::env::set_var(
            "XDG_DATA_HOME",
            std::env::temp_dir().join("motivator-test-share-data"),
        );
        let mut cfg = Config::default();
        let n = cfg.friends.len();
        let s = SharedFriend {
            name: String::new(),
            accent: Accent::Pink,
            split: f32::NAN,
            persona: "cheerful".into(),
            chat_prompt: String::new(),
            quotes: vec![Quote {
                t: "hi".into(),
                src: QuoteSrc::Sample,
                w: 99,
            }],
            pool: Vec::new(),
            expansion: Expansion::Off,
            nudges: false,
            interval_secs: 0,
        };
        let id = import_into(&mut cfg, s, None).unwrap();
        assert_eq!(cfg.friends.len(), n + 1);
        assert_eq!(cfg.active, id);
        let f = cfg.friends.last().unwrap();
        assert_eq!(f.name, "friend");
        assert_eq!(f.persona, "cheerful");
        assert_eq!(f.split, 0.52);
        assert_eq!(f.quotes[0].w, 5);
        assert_eq!(f.interval_secs, 5);
    }
}
