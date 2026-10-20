//! Mod icon loading.
//!
//! Locates a mod's icon — the file named by modDesc.xml first, then the
//! usual Farming Simulator names (`icon.dds`, `icon.png`, `store.dds`,
//! ...) — in a folder mod or inside a zip mod, searching subfolders and
//! nested zip entries by file name when the exact path misses. DDS
//! textures are decoded to RGBA pixels (nothing displays DDS raw, so
//! conversion is the only way cards can show them); png/jpg icons are
//! passed through as encoded bytes for the UI toolkit to decode.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

/// A loaded icon, ready to hand to the UI.
#[derive(Debug, Clone)]
pub enum LoadedIcon {
    /// Decoded RGBA8 pixels (from a DDS file).
    Rgba {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
    /// Encoded image bytes (png/jpg).
    Encoded(Vec<u8>),
}

/// Icon names to try, in order, when the declared icon can't be found.
const FALLBACK_ICONS: [&str; 6] = [
    "icon.dds",
    "icon.png",
    "icon.jpg",
    "icon.jpeg",
    "store.dds",
    "store.png",
];

fn candidate_names(icon_filename: Option<&str>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if let Some(name) = icon_filename {
        names.push(name.to_string());
    }
    for name in FALLBACK_ICONS {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            names.push(name.to_string());
        }
    }
    names
}

pub fn load_icon(mod_path: &Path, icon_filename: Option<&str>) -> Option<LoadedIcon> {
    let candidates = candidate_names(icon_filename);
    let mut found = read_icon_candidates(mod_path, &candidates);
    if found.is_empty() {
        // The GIANTS naming convention: most mods ship their icon
        // as icon_<something>.dds (maps especially: icon_map.dds,
        // icon_<mapname>.dds). When the declared name and the fixed
        // fallbacks all miss, any icon_* image in the mod is a far
        // better answer than the letter placeholder (John found
        // whole map rows on placeholders, 2026-10-18 — the working
        // icons were the literal icon.dds ones).
        found = read_wild_candidates(mod_path);
    }
    for (name, bytes) in found {
        if bytes.starts_with(b"DDS ") || name.to_lowercase().ends_with(".dds") {
            // A DDS that won't decode shouldn't end the search — another
            // candidate (a png twin, a store image) may still work.
            if let Some(icon) = decode_dds(&bytes) {
                return Some(icon);
            }
        } else {
            return Some(LoadedIcon::Encoded(bytes));
        }
    }
    None
}

/// All icon files found for the mod, in candidate-priority order.
fn read_icon_candidates(
    mod_path: &Path,
    candidates: &[String],
) -> Vec<(String, Vec<u8>)> {
    if mod_path.is_dir() {
        read_folder_candidates(mod_path, candidates)
    } else if mod_path
        .extension()
        .map_or(false, |e| e.eq_ignore_ascii_case("zip"))
    {
        read_zip_candidates(mod_path, candidates)
    } else {
        Vec::new()
    }
}

fn basename(name: &str) -> String {
    name.replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or(name)
        .to_lowercase()
}

fn read_folder_candidates(mod_path: &Path, candidates: &[String]) -> Vec<(String, Vec<u8>)> {
    let mut found: Vec<(usize, String, Vec<u8>)> = Vec::new();

    // Exact paths first, in candidate order.
    for (i, name) in candidates.iter().enumerate() {
        if let Ok(bytes) = std::fs::read(mod_path.join(name)) {
            found.push((i, name.clone(), bytes));
        }
    }

    // Then one walk (icons can sit in a subfolder): keep the best —
    // lowest candidate index — basename match not already found exactly.
    let bases: Vec<String> = candidates.iter().map(|c| basename(c)).collect();
    let mut best: Option<(usize, std::path::PathBuf)> = None;
    for entry in walkdir::WalkDir::new(mod_path)
        .max_depth(4)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let file_base = entry.file_name().to_string_lossy().to_lowercase();
        if let Some(i) = bases.iter().position(|b| *b == file_base) {
            if best.as_ref().map_or(true, |(bi, _)| i < *bi) {
                best = Some((i, entry.path().to_path_buf()));
            }
        }
    }
    if let Some((i, path)) = best {
        if !found.iter().any(|(fi, _, _)| *fi == i) {
            if let Ok(bytes) = std::fs::read(&path) {
                found.push((i, candidates[i].clone(), bytes));
            }
        }
    }

    found.sort_by_key(|(i, _, _)| *i);
    found
        .into_iter()
        .map(|(_, name, bytes)| (name, bytes))
        .collect()
}

fn read_zip_candidates(mod_path: &Path, candidates: &[String]) -> Vec<(String, Vec<u8>)> {
    let mut found: Vec<(String, Vec<u8>)> = Vec::new();
    let Ok(file) = std::fs::File::open(mod_path) else {
        return found;
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return found;
    };

    let entry_names: Vec<String> = (0..archive.len())
        .filter_map(|i| {
            archive
                .by_index(i)
                .ok()
                .map(|e| e.name().replace('\\', "/").to_lowercase())
        })
        .collect();

    let read_entry = |archive: &mut zip::ZipArchive<std::fs::File>, i: usize| -> Option<Vec<u8>> {
        let mut entry = archive.by_index(i).ok()?;
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).ok()?;
        Some(buf)
    };

    for name in candidates {
        let wanted = name.replace('\\', "/").to_lowercase();
        // Exact entry path first...
        let mut hit = entry_names.iter().position(|e| *e == wanted);
        // ...then any entry with the same file name, at any depth.
        if hit.is_none() {
            let base = basename(name);
            hit = entry_names
                .iter()
                .position(|e| e.rsplit('/').next().unwrap_or("") == base);
        }
        if let Some(i) = hit {
            if let Some(bytes) = read_entry(&mut archive, i) {
                found.push((name.clone(), bytes));
            }
        }
    }
    found
}

fn decode_dds(bytes: &[u8]) -> Option<LoadedIcon> {
    let mut cursor = Cursor::new(bytes);
    let dds = image_dds::ddsfile::Dds::read(&mut cursor).ok()?;
    let img = image_dds::image_from_dds(&dds, 0).ok()?;
    let (width, height) = (img.width(), img.height());
    Some(LoadedIcon::Rgba {
        width,
        height,
        pixels: img.into_raw(),
    })
}

/// Last-resort icon search: any file whose name starts with
/// `icon_` and ends in an image extension, best first (DDS before
/// PNG — the DDS is the mod icon proper when both exist — then by
/// name, so the pick is deterministic).
fn read_wild_candidates(mod_path: &Path) -> Vec<(String, Vec<u8>)> {
    fn wild_rank(name: &str) -> Option<(u8, String)> {
        let base = basename(name);
        if !base.starts_with("icon_") {
            return None;
        }
        let rank = if base.ends_with(".dds") {
            0
        } else if base.ends_with(".png") {
            1
        } else if base.ends_with(".jpg") || base.ends_with(".jpeg") {
            2
        } else {
            return None;
        };
        Some((rank, base))
    }

    let mut hits: Vec<(u8, String, Vec<u8>)> = Vec::new();
    if mod_path.is_dir() {
        for entry in walkdir::WalkDir::new(mod_path)
            .max_depth(4)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if let Some((rank, _)) = wild_rank(&name) {
                if let Ok(bytes) = std::fs::read(entry.path()) {
                    hits.push((rank, name, bytes));
                }
            }
        }
    } else if mod_path
        .extension()
        .map_or(false, |e| e.eq_ignore_ascii_case("zip"))
    {
        let Ok(file) = std::fs::File::open(mod_path) else {
            return Vec::new();
        };
        let Ok(mut archive) = zip::ZipArchive::new(file) else {
            return Vec::new();
        };
        for i in 0..archive.len() {
            let (entry_name, rank) = {
                let Ok(entry) = archive.by_index(i) else {
                    continue;
                };
                let raw = entry.name().replace('\\', "/").to_lowercase();
                match wild_rank(&raw) {
                    Some((rank, _)) => (raw, rank),
                    None => continue,
                }
            };
            let mut buf = Vec::new();
            if let Ok(mut entry) = archive.by_index(i) {
                use std::io::Read as _;
                if entry.read_to_end(&mut buf).is_ok() {
                    hits.push((rank, entry_name, buf));
                }
            }
        }
    }
    hits.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    hits.into_iter().map(|(_, name, bytes)| (name, bytes)).collect()
}

// ---------------------------------------------------------------------------
// Decoded-icon cache (John, 2026-10-19: "we need a cache for
// the modIcons"). The icon job otherwise opens every archive
// and decodes every icon from scratch on every launch and
// every Rescan (~2,900 mods). The cache stores each mod's
// finished picture once, in the app data dir beside
// config.toml and the update database, keyed by the mod
// file's name + size + mtime: a replaced mod changes size or
// mtime and re-decodes alone; everything else reads a
// ready-made thumbnail. Orphaned entries are swept by the
// caller after each icon job.

pub fn icon_cache_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(
        home.join(".config")
            .join("fs-mod-manager")
            .join("icon-cache"),
    )
}

/// The cache key for a mod file, or None if it can't be
/// stat'ed (the caller then just decodes without caching).
pub fn icon_cache_key(mod_path: &Path) -> Option<String> {
    let meta = std::fs::metadata(mod_path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stem = mod_path.file_stem()?.to_str()?;
    let clean: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Some(format!("{}-{}-{}", clean, meta.len(), mtime))
}

/// load_icon with the on-disk cache in front of it.
pub fn load_icon_cached(
    mod_path: &Path,
    icon_filename: Option<&str>,
    cache_dir: Option<&Path>,
) -> Option<LoadedIcon> {
    let key = cache_dir.and_then(|dir| {
        icon_cache_key(mod_path).map(|k| (dir.to_path_buf(), k))
    });
    if let Some((dir, k)) = &key {
        for ext in ["png", "jpg"] {
            if let Ok(bytes) = std::fs::read(dir.join(format!("{}.{}", k, ext))) {
                if !bytes.is_empty() {
                    return Some(LoadedIcon::Encoded(bytes));
                }
            }
        }
    }
    let loaded = load_icon(mod_path, icon_filename)?;
    if let Some((dir, k)) = &key {
        let _ = std::fs::create_dir_all(dir);
        match &loaded {
            LoadedIcon::Rgba {
                width,
                height,
                pixels,
            } => {
                let img = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(
                    *width,
                    *height,
                    pixels.clone(),
                );
                if let Some(img) = img {
                    let mut out = Cursor::new(Vec::new());
                    if img.write_to(&mut out, image::ImageFormat::Png).is_ok() {
                        write_cache_file(&dir.join(format!("{}.png", k)), &out.into_inner());
                    }
                }
            }
            LoadedIcon::Encoded(bytes) => {
                let ext = if bytes.starts_with(b"\x89PNG") {
                    "png"
                } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
                    "jpg"
                } else {
                    ""
                };
                if !ext.is_empty() {
                    write_cache_file(&dir.join(format!("{}.{}", k, ext)), bytes);
                }
            }
        }
    }
    Some(loaded)
}

fn write_cache_file(path: &Path, bytes: &[u8]) {
    // Write-then-rename, like the update database: a crash
    // mid-write never leaves a torn cache entry behind.
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// Delete cache entries whose key no current mod produced.
pub fn sweep_icon_cache(cache_dir: &Path, keep: &std::collections::HashSet<String>) {
    let Ok(rd) = std::fs::read_dir(cache_dir) else {
        return;
    };
    for entry in rd.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        let stem = name
            .rsplit_once('.')
            .map(|(s, _)| s.to_string())
            .unwrap_or(name);
        if !keep.contains(&stem) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
