//! Game Screens: the FS25 menu/loading background art.
//!
//! The game reads its splash art from the install folder's
//! `shared\` directory: `splash.dds` (the menu/splash
//! image), `splashBlur.dds` (its blurred twin, the same
//! composition) and `splash_highlandsFishing.dds` (the
//! Highlands Fishing variant — this module never writes
//! that file; it only reports whether it is there).
//!
//! The format law was proven by hand before this module
//! existed (three hand-built pairs, verified in game): a
//! written DDS is 4096x4096, DXT1/BC1, a full 13-level mip
//! chain (11,184,952 bytes); the source art is scaled to
//! the full canvas width and bottom-aligned on the square
//! canvas, because the game displays the bottom band; the
//! blur twin is the same composition under a strong
//! gaussian. The BC1 encoder here is a faithful port of
//! that proven reference algorithm (per-block min/max
//! endpoints inset by 1/16, RGB565 quantization, palette
//! interpolation, box-downsample mips via Lanczos halving).
//!
//! Safety follows the house pattern (see `keybindings`):
//! before the first write ever and before every Apply, the
//! current files are captured into
//! `<data dir>/game-screens-backups/<timestamp>/`; a failed
//! capture aborts the write. Restore Default puts back the
//! OLDEST capture — the stock originals.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// One of the two bundled Farmer Jay presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GsPreset {
    Daylight,
    Dark,
}

/// The canvas every written DDS is built on.
pub const CANVAS: u32 = 4096;
/// Byte length of one written DDS at the format law's size.
pub const DDS_BYTES: usize = 11_184_952;

pub const SPLASH_NAME: &str = "splash.dds";
pub const BLUR_NAME: &str = "splashBlur.dds";
pub const HIGHLANDS_NAME: &str = "splash_highlandsFishing.dds";

/// SHA-256 of the hand-built (reference encoder) Farmer Jay
/// Daylight pair's `splash.dds` — the file the owner
/// installed by hand before this page existed. Identity is
/// keyed on the splash file; see `identify`.
pub const PROVEN_DAYLIGHT_SPLASH_SHA: &str =
    "5d8c1f86e32a0f24177eaf9bf874a53f1f7dd6d8d1b867d5ee748fc6177fb7f0";
/// SHA-256 of the hand-built Farmer Jay Dark pair's
/// `splash.dds`, same reasoning as above.
pub const PROVEN_DARK_SPLASH_SHA: &str =
    "d24ab4c79a3d40dddf90f3cd8fc0d1ee06b68469d635f132637b6e7f387a589a";

// ---------------------------------------------------------------------
// RGB image buffer + resampling
// ---------------------------------------------------------------------

/// A plain RGB8 image, row-major, `pixels.len() == w*h*3`.
#[derive(Debug, Clone, PartialEq)]
pub struct RgbImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl RgbImage {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self> {
        anyhow::ensure!(
            pixels.len() == (width as usize) * (height as usize) * 3,
            "pixel buffer does not match the stated dimensions"
        );
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn solid(width: u32, height: u32, rgb: [u8; 3]) -> Self {
        let mut pixels = vec![0u8; (width as usize) * (height as usize) * 3];
        for px in pixels.chunks_exact_mut(3) {
            px.copy_from_slice(&rgb);
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    #[inline]
    pub fn px(&self, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * self.width + x) as usize) * 3;
        [self.pixels[i], self.pixels[i + 1], self.pixels[i + 2]]
    }
}

/// Resize with the image crate's Lanczos3 — the same
/// resampling family the reference pipeline used (PIL
/// LANCZOS) for both the composition scale and the mip
/// halving.
pub fn resize(img: &RgbImage, width: u32, height: u32) -> RgbImage {
    if img.width == width && img.height == height {
        return img.clone();
    }
    let buf = image::RgbImage::from_raw(img.width, img.height, img.pixels.clone())
        .expect("RgbImage invariant: buffer matches dimensions");
    let out = image::imageops::resize(
        &buf,
        width,
        height,
        image::imageops::FilterType::Lanczos3,
    );
    RgbImage {
        width,
        height,
        pixels: out.into_raw(),
    }
}

/// A separable gaussian blur, computed as the classic
/// three-box-blur approximation of a gaussian of the
/// given sigma (fast at the canvas's size, where a direct
/// kernel convolution at sigma 30 would crawl). Runs in
/// f32; edges clamp.
pub fn gaussian_blur(img: &RgbImage, sigma: f32) -> RgbImage {
    if sigma <= 0.0 {
        return img.clone();
    }
    let mut cur: Vec<f32> = img.pixels.iter().map(|&v| v as f32).collect();
    let (w, h) = (img.width as usize, img.height as usize);
    for size in box_sizes(sigma, 3) {
        let radius = (size - 1) / 2;
        cur = box_blur_pass(&cur, w, h, radius, true);
        cur = box_blur_pass(&cur, w, h, radius, false);
    }
    let pixels = cur
        .iter()
        .map(|&v| v.round().clamp(0.0, 255.0) as u8)
        .collect();
    RgbImage {
        width: img.width,
        height: img.height,
        pixels,
    }
}

/// The box sizes whose repeated blur approximates a
/// gaussian of `sigma` (the standard W3C derivation).
fn box_sizes(sigma: f32, n: usize) -> Vec<usize> {
    let w_ideal = ((12.0 * sigma * sigma / n as f32) + 1.0).sqrt();
    let mut wl = w_ideal.floor() as usize;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let (wlf, nf) = (wl as f32, n as f32);
    let m_ideal = (12.0 * sigma * sigma - nf * wlf * wlf - 4.0 * nf * wlf - 3.0 * nf)
        / (-4.0 * wlf - 4.0);
    let m = m_ideal.round().clamp(0.0, n as f32) as usize;
    (0..n).map(|i| if i < m { wl } else { wu }).collect()
}

/// One sliding-window box pass (horizontal or vertical)
/// over an f32 RGB buffer; indices clamp at the edges.
fn box_blur_pass(src: &[f32], w: usize, h: usize, radius: usize, horizontal: bool) -> Vec<f32> {
    let mut dst = vec![0f32; src.len()];
    let (lines, len) = if horizontal { (h, w) } else { (w, h) };
    let denom = (2 * radius + 1) as f32;
    let at = |buf: &[f32], line: usize, i: isize, c: usize| -> f32 {
        let i = i.clamp(0, len as isize - 1) as usize;
        let (x, y) = if horizontal { (i, line) } else { (line, i) };
        buf[(y * w + x) * 3 + c]
    };
    for line in 0..lines {
        for c in 0..3 {
            // Seed the window at position 0 with clamped taps.
            let mut acc = 0f32;
            for k in -(radius as isize)..=(radius as isize) {
                acc += at(src, line, k, c);
            }
            for i in 0..len {
                let (x, y) = if horizontal { (i, line) } else { (line, i) };
                dst[(y * w + x) * 3 + c] = acc / denom;
                acc -= at(src, line, i as isize - radius as isize, c);
                acc += at(src, line, i as isize + radius as isize + 1, c);
            }
        }
    }
    dst
}

/// The reference pipeline's sharpen step: PIL
/// UnsharpMask(radius=2, percent=60, threshold=2) — each
/// channel moves away from its blurred self by 60% of the
/// difference, but only where the difference exceeds the
/// threshold.
pub fn unsharp_mask(img: &RgbImage) -> RgbImage {
    let blurred = gaussian_blur(img, 2.0);
    let mut pixels = Vec::with_capacity(img.pixels.len());
    for (px, b) in img.pixels.iter().zip(blurred.pixels.iter()) {
        let (px, b) = (*px as f32, *b as f32);
        let diff = px - b;
        let out = if diff.abs() > 2.0 {
            px + diff * 0.60
        } else {
            px
        };
        pixels.push(out.round().clamp(0.0, 255.0) as u8);
    }
    RgbImage {
        width: img.width,
        height: img.height,
        pixels,
    }
}

/// The height a source scaled to the full canvas width
/// lands at (rounded, at least 1).
pub fn scaled_height(art_w: u32, art_h: u32, canvas: u32) -> u32 {
    let h = (art_h as f64 * canvas as f64 / art_w.max(1) as f64).round();
    if h < 1.0 {
        1
    } else if h > u32::MAX as f64 {
        u32::MAX
    } else {
        h as u32
    }
}

/// Compose art onto a square canvas: scaled to the full
/// canvas width, sharpened, bottom-aligned on black. The
/// canvas-offset math lives here so the tests can pin it
/// at small sizes; production always uses `CANVAS`.
pub fn compose_sized(art: &RgbImage, canvas: u32) -> RgbImage {
    let sh = scaled_height(art.width, art.height, canvas);
    let scaled = unsharp_mask(&resize(art, canvas, sh));
    let mut out = RgbImage::solid(canvas, canvas, [0, 0, 0]);
    // Bottom-aligned: the art's top row sits at canvas - sh,
    // which is negative for taller-than-square art (the top
    // is cropped, exactly like the reference paste).
    let y0 = canvas as i64 - sh as i64;
    let row_bytes = canvas as usize * 3;
    for cy in 0..canvas as i64 {
        let sy = cy - y0;
        if sy < 0 || sy >= sh as i64 {
            continue;
        }
        let src = (sy as usize) * row_bytes;
        let dst = (cy as usize) * row_bytes;
        out.pixels[dst..dst + row_bytes].copy_from_slice(&scaled.pixels[src..src + row_bytes]);
    }
    out
}

/// Compose at the format law's canvas size.
pub fn compose(art: &RgbImage) -> RgbImage {
    compose_sized(art, CANVAS)
}

/// The band the game shows of a square composition: the
/// bottom 16:9 slice.
pub fn visible_band(img: &RgbImage) -> RgbImage {
    let band_h = (img.height * 9 / 16).max(1).min(img.height);
    let y0 = img.height - band_h;
    let row = img.width as usize * 3;
    RgbImage {
        width: img.width,
        height: band_h,
        pixels: img.pixels[(y0 as usize * row)..].to_vec(),
    }
}

/// Fit an image inside `max_w`x`max_h` (never upscale) and
/// hand it back as RGBA for an iced image handle.
pub fn preview_rgba(img: &RgbImage, max_w: u32, max_h: u32) -> (u32, u32, Vec<u8>) {
    let scale = (max_w as f32 / img.width as f32)
        .min(max_h as f32 / img.height as f32)
        .min(1.0);
    let (w, h) = (
        ((img.width as f32 * scale).round() as u32).max(1),
        ((img.height as f32 * scale).round() as u32).max(1),
    );
    let small = resize(img, w, h);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for px in small.pixels.chunks_exact(3) {
        rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
    }
    (w, h, rgba)
}

// ---------------------------------------------------------------------
// BC1 (DXT1) codec — the reference algorithm, ported
// ---------------------------------------------------------------------

/// Quantize one 0..=255 channel float to an RGB565 field,
/// with the reference's rounding ((x*n + 127) / 255).
fn to_565_channel(x: f32, levels: f32) -> u32 {
    ((x * levels + 127.0) / 255.0).floor().clamp(0.0, levels) as u32
}

fn to_565(r: f32, g: f32, b: f32) -> u16 {
    ((to_565_channel(r, 31.0) << 11)
        | (to_565_channel(g, 63.0) << 5)
        | to_565_channel(b, 31.0)) as u16
}

/// Expand an RGB565 value back to 0..=255 floats, with
/// the reference's bit replication.
fn from_565(v: u16) -> [f32; 3] {
    let (r, g, b) = ((v >> 11) & 31, (v >> 5) & 63, v & 31);
    [
        ((r << 3) | (r >> 2)) as f32,
        ((g << 2) | (g >> 4)) as f32,
        ((b << 3) | (b >> 2)) as f32,
    ]
}

/// Encode one mip level: pad to a multiple of 4 (edge
/// replication, minimum 4x4), then one 8-byte block per
/// 4x4 tile: endpoints c0 >= c1, palette interpolation,
/// nearest-palette index per pixel.
pub fn encode_bc1_level(img: &RgbImage) -> Vec<u8> {
    let (w, h) = (img.width as usize, img.height as usize);
    let pw = ((w + 3) / 4 * 4).max(4);
    let ph = ((h + 3) / 4 * 4).max(4);
    let mut out = Vec::with_capacity((pw / 4) * (ph / 4) * 8);
    let sample = |x: usize, y: usize| -> [f32; 3] {
        let px = img.px(
            (x.min(w - 1)) as u32,
            (y.min(h - 1)) as u32,
        );
        [px[0] as f32, px[1] as f32, px[2] as f32]
    };
    for by in (0..ph).step_by(4) {
        for bx in (0..pw).step_by(4) {
            let mut block = [[0f32; 3]; 16];
            for i in 0..16 {
                block[i] = sample(bx + (i % 4), by + (i / 4));
            }
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            for px in &block {
                for c in 0..3 {
                    lo[c] = lo[c].min(px[c]);
                    hi[c] = hi[c].max(px[c]);
                }
            }
            for c in 0..3 {
                let inset = (hi[c] - lo[c]) / 16.0;
                lo[c] += inset;
                hi[c] -= inset;
            }
            let lo565 = to_565(lo[0], lo[1], lo[2]);
            let hi565 = to_565(hi[0], hi[1], hi[2]);
            let (c0, c1) = if hi565 < lo565 {
                (lo565, hi565)
            } else {
                (hi565, lo565)
            };
            let p0 = from_565(c0);
            let p1 = from_565(c1);
            let palette = [
                p0,
                p1,
                [
                    (2.0 * p0[0] + p1[0]) / 3.0,
                    (2.0 * p0[1] + p1[1]) / 3.0,
                    (2.0 * p0[2] + p1[2]) / 3.0,
                ],
                [
                    (p0[0] + 2.0 * p1[0]) / 3.0,
                    (p0[1] + 2.0 * p1[1]) / 3.0,
                    (p0[2] + 2.0 * p1[2]) / 3.0,
                ],
            ];
            let flat = c0 == c1;
            let mut packed: u32 = 0;
            for i in 0..16 {
                let idx = if flat {
                    0
                } else {
                    let mut best = 0usize;
                    let mut best_d = f32::MAX;
                    for (pi, pal) in palette.iter().enumerate() {
                        let d = (block[i][0] - pal[0]).powi(2)
                            + (block[i][1] - pal[1]).powi(2)
                            + (block[i][2] - pal[2]).powi(2);
                        if d < best_d {
                            best_d = d;
                            best = pi;
                        }
                    }
                    best as u32
                };
                packed |= idx << (2 * i);
            }
            out.extend_from_slice(&c0.to_le_bytes());
            out.extend_from_slice(&c1.to_le_bytes());
            out.extend_from_slice(&packed.to_le_bytes());
        }
    }
    out
}

/// Decode one 8-byte BC1 block into 16 RGB pixels
/// (row-major within the block).
fn decode_bc1_block(block: &[u8]) -> [[u8; 3]; 16] {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let packed = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    let p0 = from_565(c0);
    let p1 = from_565(c1);
    let mix = |a: f32, b: f32, wa: f32, wb: f32| ((wa * a + wb * b) / (wa + wb)).round() as u8;
    let palette: [[u8; 3]; 4] = if c0 > c1 {
        [
            [p0[0] as u8, p0[1] as u8, p0[2] as u8],
            [p1[0] as u8, p1[1] as u8, p1[2] as u8],
            [
                mix(p0[0], p1[0], 2.0, 1.0),
                mix(p0[1], p1[1], 2.0, 1.0),
                mix(p0[2], p1[2], 2.0, 1.0),
            ],
            [
                mix(p0[0], p1[0], 1.0, 2.0),
                mix(p0[1], p1[1], 1.0, 2.0),
                mix(p0[2], p1[2], 1.0, 2.0),
            ],
        ]
    } else {
        [
            [p0[0] as u8, p0[1] as u8, p0[2] as u8],
            [p1[0] as u8, p1[1] as u8, p1[2] as u8],
            [
                mix(p0[0], p1[0], 1.0, 1.0),
                mix(p0[1], p1[1], 1.0, 1.0),
                mix(p0[2], p1[2], 1.0, 1.0),
            ],
            [0, 0, 0],
        ]
    };
    let mut out = [[0u8; 3]; 16];
    for i in 0..16 {
        out[i] = palette[((packed >> (2 * i)) & 3) as usize];
    }
    out
}

/// Decode a BC1 level's bytes into an image of `w`x`h`
/// (the byte length must cover the padded block grid).
pub fn decode_bc1_level(data: &[u8], w: u32, h: u32) -> Result<RgbImage> {
    let (bw, bh) = ((w + 3) / 4, (h + 3) / 4);
    anyhow::ensure!(
        data.len() >= (bw * bh * 8) as usize,
        "BC1 data is shorter than the block grid needs"
    );
    let mut img = RgbImage::solid(w, h, [0, 0, 0]);
    for by in 0..bh {
        for bx in 0..bw {
            let off = ((by * bw + bx) * 8) as usize;
            let block = decode_bc1_block(&data[off..off + 8]);
            for i in 0..16 {
                let (x, y) = (bx * 4 + (i as u32 % 4), by * 4 + (i as u32 / 4));
                if x < w && y < h {
                    let d = ((y * w + x) as usize) * 3;
                    img.pixels[d..d + 3].copy_from_slice(&block[i as usize]);
                }
            }
        }
    }
    Ok(img)
}

/// The mip chain dimensions of a `w`x`h` image: halve
/// (floor, min 1) until 1x1, inclusive.
pub fn mip_sizes(w: u32, h: u32) -> Vec<(u32, u32)> {
    let mut sizes = Vec::new();
    let (mut cw, mut ch) = (w, h);
    loop {
        sizes.push((cw, ch));
        if cw == 1 && ch == 1 {
            break;
        }
        cw = (cw / 2).max(1);
        ch = (ch / 2).max(1);
    }
    sizes
}

/// Total byte length of the DDS `write_dds` produces for
/// a `w`x`h` image: 128-byte header + every level's
/// padded block bytes.
pub fn dds_len(w: u32, h: u32) -> usize {
    128 + mip_sizes(w, h)
        .iter()
        .map(|&(lw, lh)| {
            let pw = ((lw + 3) / 4 * 4).max(4) as usize;
            let ph = ((lh + 3) / 4 * 4).max(4) as usize;
            (pw / 4) * (ph / 4) * 8
        })
        .sum::<usize>()
}

/// Serialize an image as a DDS in the format law's shape:
/// the reference's exact 128-byte header, then every mip
/// level BC1-encoded, halving by Lanczos until 1x1.
pub fn write_dds(img: &RgbImage) -> Vec<u8> {
    let sizes = mip_sizes(img.width, img.height);
    let mut data = Vec::with_capacity(dds_len(img.width, img.height));
    let mut cur = img.clone();
    for (i, &(lw, lh)) in sizes.iter().enumerate() {
        if i > 0 {
            cur = resize(&cur, lw, lh);
        }
        data.extend_from_slice(&encode_bc1_level(&cur));
    }
    let mut header = Vec::with_capacity(128);
    header.extend_from_slice(b"DDS ");
    header.extend_from_slice(&124u32.to_le_bytes());
    header.extend_from_slice(&(0x1u32 | 0x2 | 0x4 | 0x1000 | 0x20000 | 0x80000).to_le_bytes());
    header.extend_from_slice(&img.height.to_le_bytes());
    header.extend_from_slice(&img.width.to_le_bytes());
    let linear = (img.width / 4) * (img.height / 4) * 8;
    header.extend_from_slice(&linear.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    header.extend_from_slice(&(sizes.len() as u32).to_le_bytes());
    header.extend_from_slice(&[0u8; 44]);
    header.extend_from_slice(&32u32.to_le_bytes());
    header.extend_from_slice(&0x4u32.to_le_bytes());
    header.extend_from_slice(b"DXT1");
    header.extend_from_slice(&[0u8; 20]);
    header.extend_from_slice(&(0x8u32 | 0x400000 | 0x1000).to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    header.extend_from_slice(&[0u8; 12]);
    debug_assert_eq!(header.len(), 128);
    header.extend_from_slice(&data);
    header
}

/// Decode mip 0 of a DXT1 DDS byte string (the only DDS
/// shape this app writes or needs to preview).
pub fn decode_dds(bytes: &[u8]) -> Result<RgbImage> {
    anyhow::ensure!(bytes.len() >= 128, "not a DDS file (too short)");
    anyhow::ensure!(&bytes[0..4] == b"DDS ", "not a DDS file (no magic)");
    let u32_at = |off: usize| u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
    let (h, w) = (u32_at(12), u32_at(16));
    anyhow::ensure!(w > 0 && h > 0, "DDS header states no dimensions");
    anyhow::ensure!(
        &bytes[84..88] == b"DXT1",
        "only DXT1 DDS files are supported here"
    );
    decode_bc1_level(&bytes[128..], w, h)
}

/// Decode user-supplied image bytes: DXT1 DDS by the
/// codec above; PNG/JPEG/WebP/BMP through the image crate.
pub fn decode_source(bytes: &[u8]) -> Result<RgbImage> {
    if bytes.starts_with(b"DDS ") {
        return decode_dds(bytes);
    }
    let img = image::load_from_memory(bytes)
        .context("the file could not be read as an image (PNG, JPG, WebP, BMP or DXT1 DDS)")?
        .to_rgb8();
    let (w, h) = (img.width(), img.height());
    RgbImage::new(w, h, img.into_raw())
}

/// Build the (splash, splashBlur) DDS byte pair for one
/// piece of source art, by the format law: compose,
/// gaussian-blur the twin at sigma 30, encode both.
pub fn build_pair(art: &RgbImage) -> (Vec<u8>, Vec<u8>) {
    let canvas = compose(art);
    let blurred = gaussian_blur(&canvas, 30.0);
    (write_dds(&canvas), write_dds(&blurred))
}

/// SHA-256 of a byte string, lowercase hex — the identity
/// currency of the current-screen panel.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

// ---------------------------------------------------------------------
// Files: backups, apply, identity
// ---------------------------------------------------------------------

/// The folder the game reads its splash art from.
pub fn shared_dir(install: &Path) -> PathBuf {
    install.join("shared")
}

/// Where the Game Screens install folder stands, decided
/// purely from the stored setting and the disk. Game
/// Screens needs the game's true install folder (the one
/// holding `shared`); the app historically only knew the
/// profile folder (game.xml), which is a different place —
/// this is the resolution the page renders from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallResolution {
    /// The setting names a folder whose `shared` exists.
    Usable(PathBuf),
    /// No install folder has been picked yet.
    Unset,
    /// A folder is stored, but it has no `shared` folder
    /// under it (the profile folder lands here, correctly).
    NoSharedFolder(PathBuf),
}

/// Resolve the stored Game Screens install-folder setting
/// to its standing. Pure apart from the one `is_dir` probe.
pub fn resolve_install_dir(setting: Option<&Path>) -> InstallResolution {
    let Some(dir) = setting else {
        return InstallResolution::Unset;
    };
    if shared_dir(dir).is_dir() {
        InstallResolution::Usable(dir.to_path_buf())
    } else {
        InstallResolution::NoSharedFolder(dir.to_path_buf())
    }
}

/// Why a picked folder was refused as the install folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallPickError {
    /// The pick (kept here as picked, before normalization)
    /// has no `shared` folder under it — after accepting a
    /// pick of the `shared` folder itself, see
    /// `validate_install_pick`.
    NoSharedFolder(PathBuf),
}

impl InstallPickError {
    /// The refusal in words, naming what was picked.
    pub fn reason(&self) -> String {
        match self {
            InstallPickError::NoSharedFolder(picked) => format!(
                "{} has no shared folder under it — the game install folder is the one holding the shared folder.",
                picked.display()
            ),
        }
    }
}

/// Validate a folder picked as the game install folder.
/// A pick of the install folder itself is accepted when
/// its `shared` folder exists; a pick of the `shared`
/// folder itself is normalized to its parent first. Any
/// other folder is refused, with the pick named.
pub fn validate_install_pick(picked: &Path) -> Result<PathBuf, InstallPickError> {
    let candidate = normalize_install_pick(picked);
    if shared_dir(&candidate).is_dir() {
        Ok(candidate)
    } else {
        Err(InstallPickError::NoSharedFolder(picked.to_path_buf()))
    }
}

/// The pick normalization: a folder named `shared` stands
/// for its parent, the install folder.
fn normalize_install_pick(picked: &Path) -> PathBuf {
    let named_shared = picked
        .file_name()
        .map_or(false, |n| n.eq_ignore_ascii_case("shared"));
    if named_shared {
        if let Some(parent) = picked.parent() {
            return parent.to_path_buf();
        }
    }
    picked.to_path_buf()
}

/// Where Game Screens backups live, under the data dir.
pub fn backups_root(data_dir: &Path) -> PathBuf {
    data_dir.join("game-screens-backups")
}

/// One timestamped capture in the backups root.
#[derive(Debug, Clone)]
pub struct BackupInfo {
    pub dir: PathBuf,
    /// The capture's timestamp name (its folder name).
    pub stamp: String,
    /// Which of the trio the capture holds.
    pub files: Vec<&'static str>,
}

const TRIO: [&str; 3] = [SPLASH_NAME, BLUR_NAME, HIGHLANDS_NAME];

/// A capture folder's timestamp name for `now` — the same
/// sortable shape the Keybindings backups use.
fn stamp_name(now: &chrono::DateTime<chrono::Local>) -> String {
    now.format("%Y%m%d-%H%M%S").to_string()
}

/// Capture the current splash trio (whichever of them
/// exist; the splash itself must) into a fresh timestamped
/// folder under `root`. Any failure is an error — callers
/// treat a failed capture as an aborted write, never a
/// skipped step.
pub fn capture_backup(shared: &Path, root: &Path) -> Result<PathBuf> {
    anyhow::ensure!(
        shared.join(SPLASH_NAME).is_file(),
        "no {} in {} to capture",
        SPLASH_NAME,
        shared.display()
    );
    std::fs::create_dir_all(root)
        .with_context(|| format!("failed to create the backups folder {}", root.display()))?;
    let base = stamp_name(&chrono::Local::now());
    let mut dir = root.join(&base);
    let mut n = 2;
    while dir.exists() {
        dir = root.join(format!("{}-{}", base, n));
        n += 1;
    }
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create the backup folder {}", dir.display()))?;
    for name in TRIO {
        let from = shared.join(name);
        if from.is_file() {
            std::fs::copy(&from, dir.join(name)).with_context(|| {
                format!("failed to capture {} into {}", name, dir.display())
            })?;
        }
    }
    Ok(dir)
}

/// All captures under `root`, newest first (timestamp
/// names sort lexicographically).
pub fn list_backups(root: &Path) -> Vec<BackupInfo> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let files: Vec<&'static str> = TRIO
                .iter()
                .copied()
                .filter(|name| dir.join(name).is_file())
                .collect();
            if files.is_empty() {
                continue;
            }
            out.push(BackupInfo {
                stamp: entry.file_name().to_string_lossy().to_string(),
                dir,
                files,
            });
        }
    }
    out.sort_by(|a, b| b.stamp.cmp(&a.stamp));
    out
}

/// The oldest capture under `root` — the stock originals,
/// captured before Playground's first write.
pub fn oldest_backup(root: &Path) -> Option<PathBuf> {
    list_backups(root).last().map(|b| b.dir.clone())
}

/// Put a capture's files back over the shared folder.
/// Restoring never touches a file the capture does not
/// hold, and a capture holding neither written file is
/// refused.
pub fn restore_backup(shared: &Path, backup: &Path) -> Result<()> {
    anyhow::ensure!(
        backup.join(SPLASH_NAME).is_file() || backup.join(BLUR_NAME).is_file(),
        "that backup holds no splash files to restore"
    );
    for name in TRIO {
        let from = backup.join(name);
        if from.is_file() {
            std::fs::copy(&from, shared.join(name)).with_context(|| {
                format!("failed to restore {} into {}", name, shared.display())
            })?;
        }
    }
    Ok(())
}

/// Write a built pair into the shared folder, capturing
/// the current files first. The capture failing aborts
/// the write with nothing changed. Returns the capture's
/// folder.
pub fn apply_pair(
    shared: &Path,
    root: &Path,
    splash: &[u8],
    blur: &[u8],
) -> Result<PathBuf> {
    anyhow::ensure!(
        shared.is_dir(),
        "the shared folder {} does not exist",
        shared.display()
    );
    anyhow::ensure!(
        shared.join(SPLASH_NAME).is_file(),
        "no {} in {} — the Game Screens install folder (Settings > Game Folders) may be wrong",
        SPLASH_NAME,
        shared.display()
    );
    let backup = capture_backup(shared, root)?;
    std::fs::write(shared.join(SPLASH_NAME), splash)
        .with_context(|| format!("failed to write {}", SPLASH_NAME))?;
    std::fs::write(shared.join(BLUR_NAME), blur)
        .with_context(|| format!("failed to write {}", BLUR_NAME))?;
    Ok(backup)
}

/// What the current screen is, by hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveIdentity {
    /// No splash file to read.
    Missing,
    /// The live file is the oldest capture — the stock art.
    Stock,
    /// The live file is the Farmer Jay Daylight pair.
    Daylight,
    /// The live file is the Farmer Jay Dark pair.
    Dark,
    /// None of the known hashes — somebody else's art
    /// (including a hand-applied pair this build cannot
    /// attribute).
    Custom,
}

impl ActiveIdentity {
    pub fn label(&self) -> &'static str {
        match self {
            ActiveIdentity::Missing => "No splash file",
            ActiveIdentity::Stock => "FS25 Standard (stock)",
            ActiveIdentity::Daylight => "Farmer Jay Daylight",
            ActiveIdentity::Dark => "Farmer Jay Dark",
            ActiveIdentity::Custom => "Custom",
        }
    }
}

/// The splash hashes of pairs this app itself generated
/// for the two presets (recorded at Apply time, so an
/// app-applied pair keeps its identity across sessions
/// even though its bytes differ from the hand-built
/// reference pairs').
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct GeneratedHashes {
    pub daylight_splash: Option<String>,
    pub dark_splash: Option<String>,
}

impl GeneratedHashes {
    pub fn load(data_dir: &Path) -> Self {
        std::fs::read_to_string(data_dir.join("game-screens-state.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(data_dir.join("game-screens-state.json"), text)
            .context("failed to record the generated pair hashes")?;
        Ok(())
    }
}

/// Identify the live splash: the app-generated and proven
/// preset hashes first, then the stock capture, else
/// Custom. `live`/`stock` are hashes of the live file and
/// of the oldest capture's splash respectively.
pub fn identify(
    live: Option<&str>,
    stock: Option<&str>,
    generated: &GeneratedHashes,
) -> ActiveIdentity {
    let Some(live) = live else {
        return ActiveIdentity::Missing;
    };
    if live == PROVEN_DAYLIGHT_SPLASH_SHA || generated.daylight_splash.as_deref() == Some(live) {
        return ActiveIdentity::Daylight;
    }
    if live == PROVEN_DARK_SPLASH_SHA || generated.dark_splash.as_deref() == Some(live) {
        return ActiveIdentity::Dark;
    }
    if stock == Some(live) {
        return ActiveIdentity::Stock;
    }
    ActiveIdentity::Custom
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn max_delta(a: &RgbImage, b: &RgbImage) -> u32 {
        a.pixels
            .iter()
            .zip(b.pixels.iter())
            .map(|(x, y)| x.abs_diff(*y) as u32)
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn format_law_sizes() {
        // The law's numbers: 13 mip levels and the exact
        // byte length the hand-built pairs carry.
        assert_eq!(mip_sizes(4096, 4096).len(), 13);
        assert_eq!(dds_len(4096, 4096), DDS_BYTES);
        // Halving floors at 1 and stops at 1x1.
        assert_eq!(mip_sizes(8, 8), vec![(8, 8), (4, 4), (2, 2), (1, 1)]);
        assert_eq!(mip_sizes(6, 3), vec![(6, 3), (3, 1), (1, 1)]);
    }

    #[test]
    fn bc1_round_trip_on_a_small_canvas() {
        // Left half flat red, right half a black-to-white
        // horizontal ramp: the flat blocks exercise the
        // c0 == c1 path, the ramp the palette path.
        let mut img = RgbImage::solid(16, 16, [0, 0, 0]);
        for y in 0..16 {
            for x in 0..16 {
                let d = ((y * 16 + x) as usize) * 3;
                if x < 8 {
                    img.pixels[d..d + 3].copy_from_slice(&[200, 40, 30]);
                } else {
                    let v = ((x - 8) * 36) as u8;
                    img.pixels[d..d + 3].copy_from_slice(&[v, v, v]);
                }
            }
        }
        let back = decode_bc1_level(&encode_bc1_level(&img), 16, 16).unwrap();
        assert_eq!((back.width, back.height), (16, 16));
        // The flat field must survive nearly exactly (one
        // 565 quantization step at most).
        let flat_src = RgbImage::solid(8, 16, [200, 40, 30]);
        let flat_back = decode_bc1_level(&encode_bc1_level(&flat_src), 8, 16).unwrap();
        assert!(max_delta(&flat_src, &flat_back) <= 8);
        // Overall the round trip stays within BC1's known
        // loss (565 steps + palette interpolation).
        assert!(max_delta(&img, &back) <= 36, "delta {}", max_delta(&img, &back));
    }

    #[test]
    fn dds_write_read_round_trip() {
        let img = RgbImage::solid(64, 32, [24, 96, 160]);
        let bytes = write_dds(&img);
        assert_eq!(bytes.len(), dds_len(64, 32));
        assert_eq!(&bytes[0..4], b"DDS ");
        assert_eq!(&bytes[84..88], b"DXT1");
        let back = decode_dds(&bytes).unwrap();
        assert_eq!((back.width, back.height), (64, 32));
        assert!(max_delta(&img, &back) <= 8);
        // A non-DXT1 header is refused, not misread.
        let mut bad = bytes.clone();
        bad[84..88].copy_from_slice(b"DXT5");
        assert!(decode_dds(&bad).is_err());
    }

    #[test]
    fn composition_is_bottom_aligned() {
        // Wide art on a small canvas: scaled to full width,
        // its bottom row is the canvas's bottom row, and
        // the canvas top stays black.
        let art = RgbImage::solid(32, 16, [180, 60, 40]);
        assert_eq!(scaled_height(32, 16, 64), 32);
        let canvas = compose_sized(&art, 64);
        assert_eq!(canvas.px(0, 63), canvas.px(63, 63));
        let bottom = canvas.px(0, 63);
        assert!(bottom[0].abs_diff(180) <= 4 && bottom[1].abs_diff(60) <= 4);
        assert_eq!(canvas.px(0, 0), [0, 0, 0]);
        // Square art fills the whole canvas.
        let square = compose_sized(&RgbImage::solid(16, 16, [10, 120, 200]), 64);
        assert_ne!(square.px(0, 0), [0, 0, 0]);
        // The visible band is the bottom 16:9 slice.
        let band = visible_band(&canvas);
        assert_eq!((band.width, band.height), (64, 36));
        assert_eq!(band.px(0, 35), canvas.px(0, 63));
    }

    #[test]
    fn scaled_height_rounds_like_the_reference() {
        // The dark master lands exactly on 2304 (its true
        // 16:9 scale); the daylight master rounds to 2305 —
        // the hand script's hardcoded 2304 differs by one
        // row in 4096, which no display can show.
        assert_eq!(scaled_height(1672, 941, 4096), 2305);
        assert_eq!(scaled_height(2048, 1152, 4096), 2304);
    }

    fn temp_root(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "playground-gamescreens-test-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn seed_shared(root: &Path, splash: &[u8]) -> PathBuf {
        let shared = root.join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join(SPLASH_NAME), splash).unwrap();
        std::fs::write(shared.join(BLUR_NAME), b"blur-bytes").unwrap();
        std::fs::write(shared.join(HIGHLANDS_NAME), b"highlands-bytes").unwrap();
        shared
    }

    #[test]
    fn capture_apply_restore_cycle() {
        let root = temp_root("cycle");
        let shared = seed_shared(&root, b"stock-splash");
        let backups = root.join("backups");

        // Apply captures before it writes, and never
        // touches the highlands file.
        let taken = apply_pair(&shared, &backups, b"new-splash", b"new-blur").unwrap();
        assert_eq!(
            std::fs::read(taken.join(SPLASH_NAME)).unwrap(),
            b"stock-splash"
        );
        assert_eq!(std::fs::read(shared.join(SPLASH_NAME)).unwrap(), b"new-splash");
        assert_eq!(std::fs::read(shared.join(BLUR_NAME)).unwrap(), b"new-blur");
        assert_eq!(
            std::fs::read(shared.join(HIGHLANDS_NAME)).unwrap(),
            b"highlands-bytes"
        );

        // A second apply captures the first pair; the list
        // is newest first and the oldest is the stock one.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        apply_pair(&shared, &backups, b"third-splash", b"third-blur").unwrap();
        let listed = list_backups(&backups);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[1].dir, taken);
        let oldest = oldest_backup(&backups).unwrap();
        assert_eq!(oldest, taken);

        // Restore Default puts the stock bytes back.
        restore_backup(&shared, &oldest).unwrap();
        assert_eq!(
            std::fs::read(shared.join(SPLASH_NAME)).unwrap(),
            b"stock-splash"
        );

        // A capture with no splash to capture fails, and
        // so does an apply into a missing shared folder.
        let empty = root.join("empty-shared");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(capture_backup(&empty, &backups).is_err());
        assert!(apply_pair(&root.join("nope"), &backups, b"x", b"y").is_err());
    }

    #[test]
    fn install_resolution_from_the_setting() {
        // Unset: the page must ask for a pick.
        assert_eq!(resolve_install_dir(None), InstallResolution::Unset);

        let root = temp_root("resolve");
        // Set, but no shared folder under it (the profile
        // folder lands exactly here): invalid, named.
        let profile = root.join("profile");
        std::fs::create_dir_all(&profile).unwrap();
        assert_eq!(
            resolve_install_dir(Some(&profile)),
            InstallResolution::NoSharedFolder(profile.clone())
        );

        // Set and valid: usable, the folder itself.
        let install = root.join("install");
        seed_shared(&install, b"splash");
        assert_eq!(
            resolve_install_dir(Some(&install)),
            InstallResolution::Usable(install.clone())
        );
    }

    #[test]
    fn install_pick_validation_and_normalization() {
        let root = temp_root("pick");
        let install = root.join("install");
        let shared = seed_shared(&install, b"splash");

        // The install folder itself is accepted as-is.
        assert_eq!(validate_install_pick(&install), Ok(install.clone()));
        // Picking the shared folder itself normalizes to
        // its parent — case-insensitively, as Windows
        // folder names go.
        assert_eq!(validate_install_pick(&shared), Ok(install.clone()));
        let upper = install.join("SHARED");
        std::fs::create_dir_all(&upper).unwrap();
        assert_eq!(validate_install_pick(&upper), Ok(install.clone()));

        // A folder with no shared under it is refused, and
        // the refusal names what was picked.
        let bare = root.join("bare");
        std::fs::create_dir_all(&bare).unwrap();
        let err = validate_install_pick(&bare).unwrap_err();
        assert_eq!(
            err,
            InstallPickError::NoSharedFolder(bare.clone())
        );
        assert!(err.reason().contains(&bare.display().to_string()));
        assert!(err.reason().contains("shared"));
    }

    #[test]
    fn identity_states() {
        let generated = GeneratedHashes {
            daylight_splash: Some("gen-day".to_string()),
            dark_splash: Some("gen-dark".to_string()),
        };
        assert_eq!(identify(None, None, &generated), ActiveIdentity::Missing);
        assert_eq!(
            identify(Some(PROVEN_DAYLIGHT_SPLASH_SHA), None, &generated),
            ActiveIdentity::Daylight
        );
        assert_eq!(
            identify(Some("gen-day"), None, &generated),
            ActiveIdentity::Daylight
        );
        assert_eq!(
            identify(Some(PROVEN_DARK_SPLASH_SHA), None, &generated),
            ActiveIdentity::Dark
        );
        assert_eq!(
            identify(Some("gen-dark"), None, &generated),
            ActiveIdentity::Dark
        );
        assert_eq!(
            identify(Some("stock-hash"), Some("stock-hash"), &generated),
            ActiveIdentity::Stock
        );
        assert_eq!(
            identify(Some("mystery"), Some("stock-hash"), &generated),
            ActiveIdentity::Custom
        );
        // A generated-preset hash outranks the stock match:
        // applying a preset to a stock machine must not
        // rebrand the result as stock.
        assert_eq!(
            identify(Some("gen-dark"), Some("gen-dark"), &generated),
            ActiveIdentity::Dark
        );
    }

    #[test]
    fn pair_builds_at_small_scale() {
        // build_pair is the 4096 pipeline; exercise its
        // pieces at a size the test suite can afford, and
        // the pair contract (two DDS byte strings of the
        // law's length) through write_dds directly.
        let art = RgbImage::solid(64, 36, [90, 140, 60]);
        let canvas = compose_sized(&art, 128);
        let blurred = gaussian_blur(&canvas, 30.0);
        let a = write_dds(&canvas);
        let b = write_dds(&blurred);
        assert_eq!(a.len(), dds_len(128, 128));
        assert_eq!(b.len(), dds_len(128, 128));
        let back = decode_dds(&a).unwrap();
        assert_eq!((back.width, back.height), (128, 128));
    }
}
