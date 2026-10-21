//! Savegame slots (Configure Save).
//!
//! FS25 keeps one folder per save slot — `savegame1`, `savegame2`,
//! ... — inside the game profile folder (the folder that also holds
//! the `mods` directory). This module finds the occupied slots and
//! pulls the display facts out of each slot's careerSavegame.xml /
//! farms.xml. Parsing is deliberately defensive: element names vary
//! a little across game versions, so several candidate names are
//! tried and anything missing just stays None — a slot with no
//! readable career file is skipped entirely.

use quick_xml::events::Event;
use quick_xml::Reader;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SaveSlot {
    /// 1-based slot number (savegame1 -> 1).
    pub slot: usize,
    /// Folder name, e.g. "savegame1" — also the loadout key.
    pub dir_name: String,
    pub path: PathBuf,
    pub map: Option<String>,
    /// The save's own name (careerSavegame's savegameName).
    pub name: Option<String>,
    pub saved_at: Option<String>,
    pub play_minutes: Option<f64>,
    pub money: Option<i64>,
    /// How many mods the save itself records in its
    /// careerSavegame.xml (Configure Save, Step 1).
    pub mod_count: usize,
}

impl SaveSlot {
    /// "19h 27m" from a minute count, when known.
    pub fn play_time_display(&self) -> Option<String> {
        self.play_minutes.map(|m| {
            let total = m.round() as i64;
            format!("{}h {}m", total / 60, total % 60)
        })
    }
}

/// Occupied save slots under a game profile folder, in slot order.
pub fn list_save_slots(profile_dir: &Path) -> Vec<SaveSlot> {
    let mut slots = Vec::new();
    for n in 1..=20usize {
        let dir_name = format!("savegame{}", n);
        let dir = profile_dir.join(&dir_name);
        let career = dir.join("careerSavegame.xml");
        if !career.exists() {
            continue;
        }
        let mut slot = SaveSlot {
            slot: n,
            dir_name,
            path: dir.clone(),
            map: None,
            name: None,
            saved_at: None,
            play_minutes: None,
            money: None,
            mod_count: 0,
        };
        if let Ok(xml) = std::fs::read_to_string(&career) {
            slot.map = tag_text(&xml, &["mapTitle", "mapName", "mapId"]);
            slot.name = tag_text(&xml, &["savegameName"]);
            slot.saved_at = tag_text(&xml, &["saveDateTime", "saveDate", "lastSaveDate"]);
            slot.play_minutes =
                tag_text(&xml, &["playTime", "playtime", "totalPlayTime"])
                    .and_then(|t| t.parse::<f64>().ok());
            slot.mod_count = super::savegame::recorded_mods(&xml).len();
        }
        let farms = dir.join("farms.xml");
        if let Ok(xml) = std::fs::read_to_string(&farms) {
            slot.money = farm_money(&xml);
        }
        slots.push(slot);
    }
    slots
}

/// Text of the first element (document order) whose name matches one
/// of `candidates`, whichever candidate that is.
fn tag_text(xml: &str, candidates: &[&str]) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut current: Option<String> = None;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                current = if candidates.contains(&name.as_str()) {
                    Some(name)
                } else {
                    None
                };
            }
            Ok(Event::End(_)) => current = None,
            Ok(Event::Text(e)) => {
                if current.is_some() {
                    if let Ok(text) = e.unescape() {
                        let text = text.trim().to_string();
                        if !text.is_empty() {
                            return Some(text);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    None
}

/// Total money across all <farm> elements in farms.xml.
fn farm_money(xml: &str) -> Option<i64> {
    let mut reader = Reader::from_str(xml);
    let mut total: i64 = 0;
    let mut any = false;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if e.name().as_ref() == b"farm" {
                    if let Some(money) = super::moddesc::attr_value(&e, "money") {
                        if let Ok(v) = money.parse::<i64>() {
                            total += v;
                            any = true;
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    if any {
        Some(total)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_slot() {
        let base = std::env::temp_dir().join(format!("mm-saves-test-{}", std::process::id()));
        let slot_dir = base.join("savegame1");
        std::fs::create_dir_all(&slot_dir).unwrap();
        std::fs::write(
            slot_dir.join("careerSavegame.xml"),
            "<careerSavegame><mapTitle>NewHope</mapTitle>\
             <saveDateTime>2026-10-02 20:15</saveDateTime>\
             <playTime>1167.0</playTime></careerSavegame>",
        )
        .unwrap();
        std::fs::write(
            slot_dir.join("farms.xml"),
            "<farms><farm farmId=\"1\" money=\"181992\"/></farms>",
        )
        .unwrap();

        let slots = list_save_slots(&base);
        assert_eq!(slots.len(), 1);
        let s = &slots[0];
        assert_eq!(s.slot, 1);
        assert_eq!(s.map.as_deref(), Some("NewHope"));
        assert_eq!(s.play_minutes, Some(1167.0));
        assert_eq!(s.play_time_display().as_deref(), Some("19h 27m"));
        assert_eq!(s.money, Some(181992));

        std::fs::remove_dir_all(&base).ok();
    }
}
