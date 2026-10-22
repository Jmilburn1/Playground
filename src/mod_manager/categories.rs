//! Mod category detection.
//!
//! There is no single "category" field in modDesc.xml — the category
//! lives in each store item's own XML as `<storeData><category>`,
//! where the game keeps ids like `tractorsS` or `slurryTanks`.
//! This module reads every store item the modDesc declares and
//! names its category THE GAME'S WAY (owner ruling, 2026-10-22):
//! the declared shop category, faithfully named, size classes
//! included — Playground invents no buckets of its own, so the
//! category here is the same shelf the player buys the item
//! from in the shop. An item with no declared category falls
//! back to its vehicle/placeable type and the custom-type
//! parent chain. A pack can land in several categories. A map
//! is detected from the file itself (a top-level `maps/`
//! folder, or a modDesc `<maps>` declaration) and its only
//! category is Maps; a mod with no store items and no map is
//! a script/gameplay mod.

use super::moddesc::ModDescInfo;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

pub const MAPS: &str = "Maps";
pub const SCRIPTS: &str = "Scripts";
pub const UNCATEGORIZED: &str = "Uncategorized";
/// The one category a `.dlc` pack carries (owner ruling,
/// 2026-10-21): GIANTS' DLC files are encrypted — no
/// modDesc to detect from — so the category is assigned
/// by file kind, exactly with this spelling, and the
/// layout homes the pack under it (Ruling A: the card's
/// category is the home).
pub const DLC_CATEGORY: &str = "DLC's";

/// The categories for the mod at `path`, sorted and deduplicated.
pub fn detect_categories(path: &Path, info: Option<&ModDescInfo>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let Some(info) = info else {
        return vec![UNCATEGORIZED.to_string()];
    };

    if info.has_map {
        // A map is a map (owner ruling, 2026-10-22): maps used to
        // also inherit buckets from the store items they bundle,
        // which put maps under "Animal buildings", "Decorations",
        // and the like. Maps is the map's only category.
        return vec![MAPS.to_string()];
    }
    for item_file in &info.store_items {
        if let Some(xml) = read_text_file(path, item_file) {
            if let Some(item) = parse_store_item(&xml) {
                found.push(bucket_for(&item, &info.type_parents));
            }
        }
    }

    found.sort();
    found.dedup();
    if found.is_empty() {
        if info.store_items.is_empty() && !info.has_map {
            vec![SCRIPTS.to_string()]
        } else {
            vec![UNCATEGORIZED.to_string()]
        }
    } else {
        found
    }
}

/// Signals pulled out of one store item XML.
struct StoreItem {
    /// Root element: "vehicle", "placeable", "handTool", ...
    root: String,
    /// The root's `type` attribute (vehicle/placeable type name).
    type_name: Option<String>,
    /// `<storeData><category>` text, when declared.
    category: Option<String>,
    /// `<placeableType>` text (placeables), a fallback signal.
    placeable_type: Option<String>,
}

fn parse_store_item(xml: &str) -> Option<StoreItem> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut item = StoreItem {
        root: String::new(),
        type_name: None,
        category: None,
        placeable_type: None,
    };
    let mut stack: Vec<String> = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if item.root.is_empty() {
                    item.root = name.clone();
                    item.type_name = super::moddesc::attr_value(&e, "type");
                }
                stack.push(name);
            }
            Ok(Event::End(_)) => {
                stack.pop();
            }
            Ok(Event::Text(e)) => {
                if let Ok(text) = e.unescape() {
                    let text = text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    match stack.join("/").as_str() {
                        "vehicle/storeData/category" | "placeable/storeData/category" => {
                            item.category = Some(text.to_string())
                        }
                        "placeable/placeableType" => {
                            item.placeable_type = Some(text.to_string())
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    if item.root.is_empty() {
        None
    } else {
        Some(item)
    }
}

/// Read a text file belonging to a mod: a path inside its folder, or
/// an entry of its zip (exact path first, then basename at any depth).
fn read_text_file(mod_path: &Path, rel: &str) -> Option<String> {
    let rel = rel.replace('\\', "/");
    if mod_path.is_dir() {
        std::fs::read_to_string(mod_path.join(&rel)).ok()
    } else if mod_path
        .extension()
        .map_or(false, |e| e.eq_ignore_ascii_case("zip"))
    {
        let file = std::fs::File::open(mod_path).ok()?;
        let mut archive = zip::ZipArchive::new(file).ok()?;
        let wanted = rel.trim_start_matches('/').to_lowercase();
        let base = wanted.rsplit('/').next().unwrap_or(&wanted).to_string();
        let mut fallback: Option<usize> = None;
        for i in 0..archive.len() {
            let name = {
                let entry = archive.by_index(i).ok()?;
                entry.name().replace('\\', "/").to_lowercase()
            };
            if name == wanted {
                let mut entry = archive.by_index(i).ok()?;
                let mut s = String::new();
                entry.read_to_string(&mut s).ok()?;
                return Some(s);
            }
            if fallback.is_none()
                && name.rsplit('/').next().unwrap_or("") == base.as_str()
            {
                fallback = Some(i);
            }
        }
        let i = fallback?;
        let mut entry = archive.by_index(i).ok()?;
        let mut s = String::new();
        entry.read_to_string(&mut s).ok()?;
        Some(s)
    } else {
        None
    }
}

/// Map one store item to its category — the game's category,
/// named the game's way (owner ruling, 2026-10-22).
fn bucket_for(item: &StoreItem, type_parents: &HashMap<String, String>) -> String {
    // A declared shop category wins for every kind of item:
    // the mod itself says which shelf it sits on.
    if let Some(category) = &item.category {
        return game_category_name(category);
    }
    if item.root.eq_ignore_ascii_case("handTool") {
        return "Hand tools".to_string();
    }
    if item.root.eq_ignore_ascii_case("placeable") {
        // No declared category: the placeable's own type,
        // cleaned, then the game's catch-all section.
        if let Some(kind) = &item.placeable_type {
            return prettify(kind);
        }
        return "Placeables".to_string();
    }
    if !item.root.eq_ignore_ascii_case("vehicle") {
        return prettify(&item.root);
    }
    // A vehicle with no declared category: walk its type and
    // the custom-type parent chain to a vanilla family.
    let mut current = item.type_name.clone();
    for _ in 0..8 {
        let Some(name) = current else { break };
        if let Some(bucket) = vehicle_type_name(&name.to_lowercase()) {
            return bucket.to_string();
        }
        current = type_parents.get(&name).cloned();
    }
    "Vehicles".to_string()
}

/// The game's shop name for a declared category id. The ids
/// whose cleaned form would lose meaning get their exact shop
/// names — the size classes (owner, 2026-10-22: the S/M/L
/// distinction is a map-fit distinction and is preserved).
/// Every other id is camelCase whose cleaned form IS the
/// shop's name (slurryTanks -> "Slurry Tanks"), so an id we
/// don't know displays the game's own word, cleaned — never
/// a bucket Playground invented.
fn game_category_name(id: &str) -> String {
    match id.to_lowercase().as_str() {
        "tractorss" => "Small Tractors".to_string(),
        "tractorsm" => "Medium Tractors".to_string(),
        "tractorsl" => "Large Tractors".to_string(),
        _ => prettify(id),
    }
}

/// Vanilla vehicle type names -> category names (fallback for
/// a vehicle that declares no shop category of its own).
fn vehicle_type_name(t: &str) -> Option<&'static str> {
    match t {
        "tractor" => Some("Tractors"),
        "harvester" | "combine" => Some("Harvesters"),
        "forageharvester" => Some("Forage Harvesters"),
        "sprayer" => Some("Sprayers"),
        "trailer" => Some("Trailers"),
        "truck" => Some("Trucks"),
        "car" => Some("Cars"),
        "telehandler" => Some("Telehandlers"),
        "wheelloader" => Some("Wheel Loaders"),
        "skidsteer" => Some("Skid Steers"),
        "frontloader" => Some("Front Loaders"),
        "baler" => Some("Balers"),
        "mower" => Some("Mowers"),
        _ => None,
    }
}

/// "slurryTanks" -> "Slurry Tanks"; a last-resort display name for
/// category ids and root elements we don't have a bucket for.
fn prettify(id: &str) -> String {
    let mut out = String::new();
    for ch in id.chars() {
        if ch.is_uppercase() && !out.is_empty() {
            out.push(' ');
        }
        out.push(ch);
    }
    let mut chars = out.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => id.to_string(),
    }
}
