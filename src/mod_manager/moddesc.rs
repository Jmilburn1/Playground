//! modDesc.xml reading for Farming Simulator mods.
//!
//! Works for both folder mods (`<mod folder>/modDesc.xml`) and zip mods
//! (`modDesc.xml` at the root of the archive). All failures are non-fatal:
//! a mod without a readable modDesc simply keeps its filename-based defaults.

use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::Read;
use std::path::Path;

/// Metadata extracted from a modDesc.xml.
#[derive(Debug, Clone, Default)]
pub struct ModDescInfo {
    pub title: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub icon_filename: Option<String>,
    /// xmlFilename of every `<storeItem>` — the purchasable items the
    /// mod adds (vehicles, placeables, tools). Category detection
    /// reads each item's own XML for its shop category.
    pub store_items: Vec<String>,
    /// True when the mod is a map: the modDesc declares a
    /// `<maps>` section, or the mod physically carries a
    /// top-level `maps/` folder — the owner's ground-truth
    /// signal (2026-10-22): maps are the only mods that have
    /// one, so the file itself settles what the paperwork
    /// could get wrong.
    pub has_map: bool,
    /// Custom vehicle/placeable type name -> parent type name, from
    /// `<vehicleTypes>` / `<placeableTypes>`. Lets category detection
    /// resolve a custom item type to the vanilla family it descends
    /// from when the item declares no shop category itself.
    pub type_parents: std::collections::HashMap<String, String>,
}

/// Read and parse the modDesc.xml for the mod at `path`, if one exists.
pub fn read_metadata(path: &Path) -> Option<ModDescInfo> {
    let (xml, maps_folder) = read_moddesc_xml(path)?;
    let mut info = parse_moddesc(&xml)?;
    // The physical proof (owner ruling, 2026-10-22): a top-level
    // maps/ folder inside the mod marks it as a map whether or
    // not the modDesc remembered to declare a <maps> section.
    if maps_folder {
        info.has_map = true;
    }
    Some(info)
}

/// The modDesc.xml text, plus whether the mod physically
/// carries a top-level `maps/` folder (a `maps` directory for
/// a folder mod; any `maps/...` entry for a zip).
fn read_moddesc_xml(path: &Path) -> Option<(String, bool)> {
    if path.is_dir() {
        let xml = std::fs::read_to_string(path.join("modDesc.xml")).ok()?;
        let maps_folder = path.join("maps").is_dir();
        Some((xml, maps_folder))
    } else if path
        .extension()
        .map_or(false, |e| e.eq_ignore_ascii_case("zip"))
    {
        let file = std::fs::File::open(path).ok()?;
        let mut archive = zip::ZipArchive::new(file).ok()?;
        // One pass over the entry list: prefer a root-level
        // modDesc.xml, remember the first nested one as a
        // fallback for sloppily packed mods, and note any
        // top-level maps/ entry on the way.
        let mut root: Option<usize> = None;
        let mut fallback: Option<usize> = None;
        let mut maps_folder = false;
        for i in 0..archive.len() {
            let name = {
                let entry = archive.by_index(i).ok()?;
                entry.name().replace('\\', "/")
            };
            let lower = name.to_lowercase();
            if lower.starts_with("maps/") {
                maps_folder = true;
            }
            if name.eq_ignore_ascii_case("modDesc.xml") {
                root = Some(i);
            } else if fallback.is_none() && lower.ends_with("/moddesc.xml") {
                fallback = Some(i);
            }
        }
        let i = root.or(fallback)?;
        let mut entry = archive.by_index(i).ok()?;
        let mut s = String::new();
        entry.read_to_string(&mut s).ok()?;
        Some((s, maps_folder))
    } else {
        None
    }
}

fn apply_text(
    info: &mut ModDescInfo,
    path: &[String],
    text: &str,
    title_first: &mut Option<String>,
    desc_first: &mut Option<String>,
) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    match path.join("/").as_str() {
        "modDesc/version" => info.version = Some(trimmed.to_string()),
        "modDesc/author" => info.author = Some(trimmed.to_string()),
        "modDesc/iconFilename" => info.icon_filename = Some(trimmed.to_string()),
        "modDesc/title" => {
            if title_first.is_none() {
                *title_first = Some(trimmed.to_string());
            }
        }
        p if p.starts_with("modDesc/title/") => {
            let lang = p.rsplit('/').next().unwrap_or("");
            if lang.eq_ignore_ascii_case("en") {
                info.title = Some(trimmed.to_string());
            }
            if title_first.is_none() {
                *title_first = Some(trimmed.to_string());
            }
        }
        p if p.starts_with("modDesc/description/") => {
            let lang = p.rsplit('/').next().unwrap_or("");
            if lang.eq_ignore_ascii_case("en") {
                info.description = Some(trimmed.to_string());
            }
            if desc_first.is_none() {
                *desc_first = Some(trimmed.to_string());
            }
        }
        _ => {}
    }
}

pub fn attr_value(e: &quick_xml::events::BytesStart, key: &str) -> Option<String> {
    for attr in e.attributes().flatten() {
        if attr.key.local_name().as_ref() == key.as_bytes() {
            return Some(String::from_utf8_lossy(&attr.value).into_owned());
        }
    }
    None
}

/// Record the structural facts category detection needs from one
/// element: store items, maps, and custom type declarations. Called
/// for both Start and Empty events — storeItem/type elements are
/// usually self-closing, which quick-xml reports as Empty.
fn apply_structure(
    info: &mut ModDescInfo,
    stack: &[String],
    e: &quick_xml::events::BytesStart,
) {
    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    let path = format!("{}/{}", stack.join("/"), name);
    match path.as_str() {
        "modDesc/storeItems/storeItem" => {
            if let Some(file) = attr_value(e, "xmlFilename") {
                info.store_items.push(file);
            }
        }
        "modDesc/maps/map" => info.has_map = true,
        "modDesc/vehicleTypes/type" | "modDesc/placeableTypes/type" => {
            if let (Some(child), Some(parent)) =
                (attr_value(e, "name"), attr_value(e, "parent"))
            {
                info.type_parents.insert(child, parent);
            }
        }
        _ => {}
    }
}

fn parse_moddesc(xml: &str) -> Option<ModDescInfo> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut info = ModDescInfo::default();
    let mut title_first: Option<String> = None;
    let mut desc_first: Option<String> = None;
    let mut stack: Vec<String> = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                apply_structure(&mut info, &stack, &e);
                stack.push(String::from_utf8_lossy(e.name().as_ref()).into_owned());
            }
            Ok(Event::Empty(e)) => {
                apply_structure(&mut info, &stack, &e);
            }
            Ok(Event::End(_)) => {
                stack.pop();
            }
            Ok(Event::Text(e)) => {
                if let Ok(text) = e.unescape() {
                    apply_text(&mut info, &stack, &text, &mut title_first, &mut desc_first);
                }
            }
            Ok(Event::CData(e)) => {
                let text = String::from_utf8_lossy(&e);
                apply_text(&mut info, &stack, &text, &mut title_first, &mut desc_first);
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    if info.title.is_none() {
        info.title = title_first;
    }
    if info.description.is_none() {
        info.description = desc_first;
    }
    if info.title.is_none()
        && info.version.is_none()
        && info.author.is_none()
        && info.store_items.is_empty()
        && !info.has_map
    {
        None
    } else {
        Some(info)
    }
}
