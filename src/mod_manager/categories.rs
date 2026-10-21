//! Mod category detection.
//!
//! There is no single "category" field in modDesc.xml — the category
//! lives in each store item's own XML as `<storeData><category>`,
//! where the game keeps ids like `tractorsSmall` or `slurryTanks`.
//! This module reads every store item the modDesc declares, maps its
//! category id (or, failing that, its vehicle/placeable type and the
//! custom-type parent chain) to a player-facing bucket, and returns
//! the mod's buckets: a pack can land in several. Map mods declare
//! `<maps>` in the modDesc; a mod with no store items and no map is
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
        found.push(MAPS.to_string());
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

/// Map one store item to its display bucket.
fn bucket_for(item: &StoreItem, type_parents: &HashMap<String, String>) -> String {
    if item.root.eq_ignore_ascii_case("placeable") {
        return placeable_bucket(item);
    }
    if item.root.eq_ignore_ascii_case("handTool") {
        return "Hand tools".to_string();
    }
    if !item.root.eq_ignore_ascii_case("vehicle") {
        return prettify(&item.root);
    }
    // Vehicle: declared shop category first.
    if let Some(category) = &item.category {
        if let Some(bucket) = category_bucket(&category.to_lowercase()) {
            return bucket.to_string();
        }
        return prettify(category);
    }
    // Then the vehicle type, walking custom-type parents to a vanilla
    // family when the type itself is unknown.
    let mut current = item.type_name.clone();
    for _ in 0..8 {
        let Some(name) = current else { break };
        if let Some(bucket) = vehicle_type_bucket(&name.to_lowercase()) {
            return bucket.to_string();
        }
        current = type_parents.get(&name).cloned();
    }
    "Vehicles".to_string()
}

fn placeable_bucket(item: &StoreItem) -> String {
    if let Some(category) = &item.category {
        let c = category.to_lowercase();
        if c.contains("silo") {
            return "Silos & storage".to_string();
        }
        if c.contains("husbandry") || c.contains("animal") {
            return "Animal buildings".to_string();
        }
        if c.contains("production") || c.contains("factor") {
            return "Productions".to_string();
        }
        if c.contains("greenhouse") {
            return "Greenhouses".to_string();
        }
        if c.contains("shed") || c.contains("building") || c.contains("house") {
            return "Sheds & buildings".to_string();
        }
        if c.contains("solar") || c.contains("wind") || c.contains("generator") {
            return "Generators".to_string();
        }
        if c.contains("decor") {
            return "Decorations".to_string();
        }
    }
    if let Some(kind) = &item.placeable_type {
        let k = kind.to_lowercase();
        if k.contains("husbandry") {
            return "Animal buildings".to_string();
        }
        if k.contains("silo") {
            return "Silos & storage".to_string();
        }
        if k.contains("production") || k.contains("factory") {
            return "Productions".to_string();
        }
    }
    "Placeables".to_string()
}

/// Known game shop-category ids -> display buckets (vehicles/tools).
/// Order matters: manure is checked before generic "spreader".
fn category_bucket(c: &str) -> Option<&'static str> {
    let has = |needle: &str| c.contains(needle);
    if has("tractor") {
        Some("Tractors")
    } else if has("truck") {
        Some("Trucks")
    } else if has("car") && !has("scarifier") {
        Some("Cars")
    } else if has("harvest") {
        Some("Harvesters")
    } else if has("slurry") || has("manure") {
        Some("Tanks & spreaders")
    } else if has("sprayer") || has("fertilizer") {
        Some("Sprayers & fertilizer")
    } else if has("header") || has("cutter") {
        Some("Headers & cutters")
    } else if has("mower") || has("tedder") || has("windrower") || has("bale") || has("forage") {
        Some("Hay & forage")
    } else if has("seeder") || has("planter") || has("sowing") {
        Some("Seeding")
    } else if has("plow")
        || has("cultivator")
        || has("harrow")
        || has("subsoiler")
        || has("mulcher")
        || has("roller")
        || has("weeder")
        || has("spader")
    {
        Some("Tillage")
    } else if has("trailer") || has("dolly") || has("lowloader") || has("augerwagon") {
        Some("Trailers")
    } else if has("loader") || has("telehandler") || has("weight") || has("forklift") {
        Some("Loaders & handling")
    } else if has("forest") || has("wood") {
        Some("Forestry")
    } else {
        None
    }
}

/// Vanilla vehicle type names -> display buckets (fallback path).
fn vehicle_type_bucket(t: &str) -> Option<&'static str> {
    let has = |needle: &str| t.contains(needle);
    if has("tractor") {
        Some("Tractors")
    } else if has("harvester") || has("combine") {
        Some("Harvesters")
    } else if has("sprayer") {
        Some("Sprayers & fertilizer")
    } else if has("manure") || has("slurry") {
        Some("Tanks & spreaders")
    } else if has("cutter") || has("header") {
        Some("Headers & cutters")
    } else if has("baler") || has("mower") {
        Some("Hay & forage")
    } else if has("trailer") || has("augerwagon") || has("dolly") {
        Some("Trailers")
    } else if has("truck") || has("locomotive") {
        Some("Trucks")
    } else if has("car") && !has("scarifier") {
        Some("Cars")
    } else {
        None
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
