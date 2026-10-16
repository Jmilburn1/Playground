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
use std::path::Path;

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
    for (name, bytes) in read_icon_candidates(mod_path, &candidates) {
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
