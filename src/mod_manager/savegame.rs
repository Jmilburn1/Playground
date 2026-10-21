//! Configure Save: reading and surgically editing one save
//! slot's own files — the settings block and recorded mod
//! list in careerSavegame.xml, the money/loan attributes in
//! farms.xml, and the AutoDrive route file.
//!
//! Like game_files.rs, every write is a surgical text edit:
//! only the one element or attribute a field owns is
//! rewritten, and the rest of the file is left byte-for-byte
//! alone. A field the save does not carry stays `None` end to
//! end — Configure Save surfaces it as "not in this save
//! yet" and never invents it (owner ruling, 2026-10-21).

use super::layout::LinkKind;
use super::layout_exec::{create_projection, projection_proven, remove_projected_entry};
use super::types::Mod;
use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One save's settings, as read from careerSavegame.xml's
/// <settings> block. Every field is an Option: None means
/// the save does not carry that element, and the wizard
/// renders it read-only ("not in this save yet") instead of
/// offering a control that would fabricate it.
///
/// Enum-valued fields keep the game's own storage shape:
/// growth_mode 1=Seasonal / 2=Paused / 3=Off and fuel_usage
/// 1=Low / 2=Normal / 3=High are the mappings the field
/// verification pinned down. disaster_destruction_state,
/// dirt_interval, and the two helper sources are stored raw
/// — their label mappings are NOT verified, so the wizard
/// shows the raw value and does not offer to change them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CareerSettings {
    pub savegame_name: Option<String>,
    pub economic_difficulty: Option<String>,
    pub time_scale: Option<f64>,
    pub auto_save_interval: Option<f64>,
    pub traffic_enabled: Option<bool>,
    pub snow_enabled: Option<bool>,
    pub growth_mode: Option<i64>,
    pub planned_days_per_period: Option<i64>,
    pub fruit_destruction: Option<bool>,
    pub plowing_required_enabled: Option<bool>,
    pub stones_enabled: Option<bool>,
    pub lime_required: Option<bool>,
    pub weeds_enabled: Option<bool>,
    pub disaster_destruction_state: Option<String>,
    pub automatic_motor_start_enabled: Option<bool>,
    pub stop_and_go_braking: Option<bool>,
    pub trailer_fill_limit: Option<bool>,
    pub fuel_usage: Option<i64>,
    pub helper_buy_fuel: Option<bool>,
    pub helper_buy_seeds: Option<bool>,
    pub helper_buy_fertilizer: Option<bool>,
    pub helper_slurry_source: Option<i64>,
    pub helper_manure_source: Option<i64>,
    pub dirt_interval: Option<i64>,
    /// The fixedSeasonalVisuals flag, in whichever shape the
    /// save carries it (attribute on <settings>, or element).
    pub fixed_visual_month: Option<bool>,
    /// True when fixed_visual_month lives as an attribute on
    /// the <settings> tag rather than as an element.
    pub fixed_visual_month_is_attribute: bool,
}

/// One mod the save itself records, from a <mod> element in
/// careerSavegame.xml. `required` is the game's own flag —
/// the map's required mods carry it.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedMod {
    pub mod_name: String,
    pub title: Option<String>,
    pub version: Option<String>,
    pub required: bool,
}

/// The first farm's money and loan from farms.xml, plus how
/// many farms the file holds. Cash and loan live here, not
/// in careerSavegame.xml (owner ruling, 2026-10-21).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FarmFinances {
    pub farm_count: usize,
    pub money: Option<i64>,
    pub loan: Option<i64>,
}

/// Everything the wizard loads when a slot is picked.
#[derive(Debug, Clone, Default)]
pub struct LoadedSave {
    pub settings: CareerSettings,
    pub finances: FarmFinances,
    pub recorded: Vec<RecordedMod>,
    pub autodrive_present: bool,
}

/// Byte range of the content inside <settings>...</settings>.
fn settings_span(xml: &str) -> Option<(usize, usize)> {
    let open_start = xml.find("<settings")?;
    let open_end = xml[open_start..].find('>')? + open_start + 1;
    let close = xml[open_end..].find("</settings>")? + open_end;
    Some((open_end, close))
}

/// The <settings ...> open tag itself, attributes included.
fn settings_tag(xml: &str) -> Option<(usize, usize, String)> {
    let open_start = xml.find("<settings")?;
    let open_end = xml[open_start..].find('>')? + open_start + 1;
    Some((open_start, open_end, xml[open_start..open_end].to_string()))
}

fn element_text(xml: &str, name: &str) -> Option<String> {
    let (s, e) = settings_span(xml)?;
    let span = &xml[s..e];
    let open = format!("<{}>", name);
    let close = format!("</{}>", name);
    let a = span.find(&open)? + open.len();
    let b = span[a..].find(&close)? + a;
    let text = span[a..b].trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// Replace the text of one element inside <settings>.
/// Returns the edited document, or None when the element is
/// not there to rewrite.
fn set_element_text(xml: &str, name: &str, value: &str) -> Option<String> {
    let (s, e) = settings_span(xml)?;
    let span = &xml[s..e];
    let open = format!("<{}>", name);
    let close = format!("</{}>", name);
    let a = span.find(&open)? + open.len();
    let b = span[a..].find(&close)? + a;
    let mut out = String::with_capacity(xml.len() + value.len());
    out.push_str(&xml[..s + a]);
    out.push_str(value);
    out.push_str(&xml[s + b..]);
    Some(out)
}

fn attr_in(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{}=\"", name);
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(tag[start..end].to_string())
}

fn parse_bool(text: Option<String>) -> Option<bool> {
    match text.as_deref() {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

fn parse_i64(text: Option<String>) -> Option<i64> {
    text.and_then(|t| t.parse::<i64>().ok())
}

fn parse_f64(text: Option<String>) -> Option<f64> {
    text.and_then(|t| t.parse::<f64>().ok())
}

/// Read every known settings field out of a careerSavegame
/// document. Missing elements stay None.
pub fn read_settings(xml: &str) -> CareerSettings {
    let mut s = CareerSettings {
        savegame_name: element_text(xml, "savegameName"),
        economic_difficulty: element_text(xml, "economicDifficulty"),
        time_scale: parse_f64(element_text(xml, "timeScale")),
        auto_save_interval: parse_f64(element_text(xml, "autoSaveInterval")),
        traffic_enabled: parse_bool(element_text(xml, "trafficEnabled")),
        snow_enabled: parse_bool(element_text(xml, "isSnowEnabled")),
        growth_mode: parse_i64(element_text(xml, "growthMode")),
        planned_days_per_period: parse_i64(element_text(xml, "plannedDaysPerPeriod")),
        fruit_destruction: parse_bool(element_text(xml, "fruitDestruction")),
        plowing_required_enabled: parse_bool(element_text(xml, "plowingRequiredEnabled")),
        stones_enabled: parse_bool(element_text(xml, "stonesEnabled")),
        lime_required: parse_bool(element_text(xml, "limeRequired")),
        weeds_enabled: parse_bool(element_text(xml, "weedsEnabled")),
        disaster_destruction_state: element_text(xml, "disasterDestructionState"),
        automatic_motor_start_enabled: parse_bool(element_text(xml, "automaticMotorStartEnabled")),
        stop_and_go_braking: parse_bool(element_text(xml, "stopAndGoBraking")),
        trailer_fill_limit: parse_bool(element_text(xml, "trailerFillLimit")),
        fuel_usage: parse_i64(element_text(xml, "fuelUsage")),
        helper_buy_fuel: parse_bool(element_text(xml, "helperBuyFuel")),
        helper_buy_seeds: parse_bool(element_text(xml, "helperBuySeeds")),
        helper_buy_fertilizer: parse_bool(element_text(xml, "helperBuyFertilizer")),
        helper_slurry_source: parse_i64(element_text(xml, "helperSlurrySource")),
        helper_manure_source: parse_i64(element_text(xml, "helperManureSource")),
        dirt_interval: parse_i64(element_text(xml, "dirtInterval")),
        ..Default::default()
    };
    // Fixed Visual Month: MT's manager reads a
    // fixedSeasonalVisuals ATTRIBUTE on <settings>; the
    // element form is checked too, so whichever shape a real
    // save carries is the shape the wizard edits.
    if let Some((_, _, tag)) = settings_tag(xml) {
        if let Some(v) = attr_in(&tag, "fixedSeasonalVisuals") {
            s.fixed_visual_month = parse_bool(Some(v));
            s.fixed_visual_month_is_attribute = true;
        }
    }
    if s.fixed_visual_month.is_none() {
        if let Some(v) = parse_bool(element_text(xml, "fixedSeasonalVisuals")) {
            s.fixed_visual_month = Some(v);
            s.fixed_visual_month_is_attribute = false;
        }
    }
    s
}

/// The mods a save records for itself, in document order.
pub fn recorded_mods(xml: &str) -> Vec<RecordedMod> {
    let mut out = Vec::new();
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if e.name().as_ref() == b"mod" {
                    if let Some(mod_name) = super::moddesc::attr_value(&e, "modName") {
                        out.push(RecordedMod {
                            mod_name,
                            title: super::moddesc::attr_value(&e, "title"),
                            version: super::moddesc::attr_value(&e, "version"),
                            required: super::moddesc::attr_value(&e, "required")
                                .map(|v| v == "true")
                                .unwrap_or(false),
                        });
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// Byte span of the first <farm ...> tag in a farms.xml
/// document (start of "<farm" through its closing '>').
fn first_farm_tag(xml: &str) -> Option<(usize, usize)> {
    let start = xml.find("<farm")?;
    // "<farm" also prefixes nothing else in farms.xml, but
    // guard against "<farms" (the root) matching.
    if xml[start..].starts_with("<farms") {
        let inner = xml[start + 1..].find("<farm")? + start + 1;
        let end = xml[inner..].find('>')? + inner + 1;
        return Some((inner, end));
    }
    let end = xml[start..].find('>')? + start + 1;
    Some((start, end))
}

/// The first farm's money/loan, and the farm count.
pub fn farm_finances(xml: &str) -> FarmFinances {
    let mut count = 0usize;
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if e.name().as_ref() == b"farm" {
                    count += 1;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    let mut finances = FarmFinances {
        farm_count: count,
        money: None,
        loan: None,
    };
    if let Some((s, e)) = first_farm_tag(xml) {
        let tag = &xml[s..e];
        finances.money = attr_in(tag, "money").and_then(|v| v.parse::<i64>().ok());
        finances.loan = attr_in(tag, "loan").and_then(|v| v.parse::<i64>().ok());
    }
    finances
}

/// Set one attribute inside a single tag's text, replacing
/// its value or inserting the attribute before the close.
fn set_attr_in_tag(tag: &str, name: &str, value: &str) -> String {
    let needle = format!("{}=\"", name);
    if let Some(start) = tag.find(&needle) {
        let vstart = start + needle.len();
        if let Some(vend) = tag[vstart..].find('"') {
            let mut out = String::with_capacity(tag.len() + value.len());
            out.push_str(&tag[..vstart]);
            out.push_str(value);
            out.push_str(&tag[vstart + vend..]);
            return out;
        }
    }
    // Insert before the closing bracket (keeping a
    // self-closing "/>" intact).
    let insert_at = if tag.ends_with("/>") {
        tag.len() - 2
    } else {
        tag.len() - 1
    };
    let mut out = String::with_capacity(tag.len() + name.len() + value.len() + 4);
    out.push_str(&tag[..insert_at]);
    out.push_str(&format!(" {}=\"{}\"", name, value));
    out.push_str(&tag[insert_at..]);
    out
}

/// Rewrite the first farm's money and loan in a farms.xml
/// document. None when there is no <farm> tag to edit.
pub fn apply_finances_to_xml(xml: &str, money: i64, loan: i64) -> Option<String> {
    let (s, e) = first_farm_tag(xml)?;
    let tag = &xml[s..e];
    let tag = set_attr_in_tag(tag, "money", &money.to_string());
    let tag = set_attr_in_tag(&tag, "loan", &loan.to_string());
    let mut out = String::with_capacity(xml.len());
    out.push_str(&xml[..s]);
    out.push_str(&tag);
    out.push_str(&xml[e..]);
    Some(out)
}

/// Load everything the wizard needs from a slot folder.
/// Unreadable files degrade to empty pieces, the same
/// defensive posture as the slot list itself.
pub fn load_slot(slot_dir: &Path) -> LoadedSave {
    let career = std::fs::read_to_string(slot_dir.join("careerSavegame.xml"))
        .unwrap_or_default();
    let farms = std::fs::read_to_string(slot_dir.join("farms.xml")).unwrap_or_default();
    LoadedSave {
        settings: read_settings(&career),
        finances: farm_finances(&farms),
        recorded: recorded_mods(&career),
        autodrive_present: slot_dir.join("AutoDrive_config.xml").exists(),
    }
}

/// A copy of `path` next to the original, named
/// "<file>.playground-backup" — the rollback copy every
/// Configure Save apply writes before it touches a file.
pub fn backup_file(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .map(|n| format!("{}.playground-backup", n.to_string_lossy()))
        .unwrap_or_else(|| "backup.playground-backup".to_string());
    let dest = path.with_file_name(name);
    std::fs::copy(path, &dest)
        .with_context(|| format!("Could not back up {:?}", path))?;
    Ok(dest)
}

fn fmt_num(v: f64) -> String {
    if v == v.trunc() {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

fn fmt_bool(v: bool) -> String {
    if v {
        "On".to_string()
    } else {
        "Off".to_string()
    }
}

/// One pending element write: (element name, display label,
/// old display, new display, exact text to write).
struct PendingWrite {
    element: &'static str,
    label: &'static str,
    old: String,
    new: String,
    text: String,
}

fn pending_writes(original: &CareerSettings, edited: &CareerSettings) -> Vec<PendingWrite> {
    let mut out: Vec<PendingWrite> = Vec::new();
    macro_rules! fld {
        ($field:ident, $element:expr, $label:expr, $text:expr, $disp:expr) => {
            if edited.$field != original.$field {
                if let Some(v) = &edited.$field {
                    out.push(PendingWrite {
                        element: $element,
                        label: $label,
                        old: original
                            .$field
                            .as_ref()
                            .map(|o| ($disp)(o))
                            .unwrap_or_else(|| "—".to_string()),
                        new: ($disp)(v),
                        text: ($text)(v),
                    });
                }
            }
        };
    }
    let raw_s = |v: &String| v.clone();
    let raw_i = |v: &i64| v.to_string();
    let bool_t = |v: &bool| v.to_string();
    let bool_d = |v: &bool| fmt_bool(*v);
    let f_t = |v: &f64| format!("{:.6}", v);
    let f_d = |v: &f64| fmt_num(*v);

    fld!(savegame_name, "savegameName", "Save name", raw_s, raw_s);
    fld!(economic_difficulty, "economicDifficulty", "Economic difficulty", raw_s, raw_s);
    fld!(time_scale, "timeScale", "Timescale", f_t, f_d);
    fld!(auto_save_interval, "autoSaveInterval", "Autosave interval", f_t, f_d);
    fld!(traffic_enabled, "trafficEnabled", "Traffic", bool_t, bool_d);
    fld!(snow_enabled, "isSnowEnabled", "Snow", bool_t, bool_d);
    fld!(growth_mode, "growthMode", "Seasonal growth", raw_i, raw_i);
    fld!(planned_days_per_period, "plannedDaysPerPeriod", "Days per month", raw_i, raw_i);
    fld!(fruit_destruction, "fruitDestruction", "Crop destruction", bool_t, bool_d);
    fld!(plowing_required_enabled, "plowingRequiredEnabled", "Periodic plowing", bool_t, bool_d);
    fld!(stones_enabled, "stonesEnabled", "Fieldstone", bool_t, bool_d);
    fld!(lime_required, "limeRequired", "Lime required", bool_t, bool_d);
    fld!(weeds_enabled, "weedsEnabled", "Weeds", bool_t, bool_d);
    fld!(automatic_motor_start_enabled, "automaticMotorStartEnabled", "Automatic engine start", bool_t, bool_d);
    fld!(stop_and_go_braking, "stopAndGoBraking", "Stop & go braking", bool_t, bool_d);
    fld!(trailer_fill_limit, "trailerFillLimit", "Trailer fill limit", bool_t, bool_d);
    fld!(fuel_usage, "fuelUsage", "Fuel usage", raw_i, raw_i);
    fld!(helper_buy_fuel, "helperBuyFuel", "AI refill — fuel", bool_t, bool_d);
    fld!(helper_buy_seeds, "helperBuySeeds", "AI refill — seeds", bool_t, bool_d);
    fld!(helper_buy_fertilizer, "helperBuyFertilizer", "AI refill — fertilizer", bool_t, bool_d);
    out
}

/// Display labels for the two growth modes the wizard can
/// set — shared by the change list so a change reads
/// "Seasonal growth: Seasonal → Paused", not "1 → 2".
fn growth_label(v: i64) -> &'static str {
    match v {
        1 => "Seasonal",
        2 => "Paused",
        3 => "Off",
        _ => "Unknown",
    }
}

fn fuel_label(v: i64) -> &'static str {
    match v {
        1 => "Low",
        2 => "Normal",
        3 => "High",
        _ => "Unknown",
    }
}

/// Human-readable change lines for the Review page:
/// "Label: old → new" for every field the draft changed.
/// Fields whose storage is a raw code (growth, fuel) are
/// translated to their labels here.
pub fn describe_changes(original: &CareerSettings, edited: &CareerSettings) -> Vec<String> {
    let mut lines: Vec<String> = pending_writes(original, edited)
        .into_iter()
        .map(|w| {
            let (old, new) = match w.element {
                "growthMode" => (
                    original
                        .growth_mode
                        .map(growth_label)
                        .unwrap_or("—")
                        .to_string(),
                    edited
                        .growth_mode
                        .map(growth_label)
                        .unwrap_or("—")
                        .to_string(),
                ),
                "fuelUsage" => (
                    original.fuel_usage.map(fuel_label).unwrap_or("—").to_string(),
                    edited.fuel_usage.map(fuel_label).unwrap_or("—").to_string(),
                ),
                _ => (w.old, w.new),
            };
            format!("{}: {} → {}", w.label, old, new)
        })
        .collect();
    if edited.fixed_visual_month != original.fixed_visual_month {
        if let Some(v) = edited.fixed_visual_month {
            lines.push(format!(
                "Fixed visual month: {} → {}",
                original.fixed_visual_month.map(fmt_bool).unwrap_or_else(|| "—".to_string()),
                fmt_bool(v)
            ));
        }
    }
    lines
}

/// Apply the draft's element changes to a careerSavegame
/// document. Returns (new document, applied labels, skipped
/// labels) — a skip is a field whose element could not be
/// found to rewrite, reported rather than hidden.
pub fn apply_settings_to_xml(
    xml: &str,
    original: &CareerSettings,
    edited: &CareerSettings,
) -> (String, Vec<String>, Vec<String>) {
    let mut out = xml.to_string();
    let mut applied = Vec::new();
    let mut skipped = Vec::new();
    for w in pending_writes(original, edited) {
        match set_element_text(&out, w.element, &w.text) {
            Some(next) => {
                out = next;
                applied.push(w.label.to_string());
            }
            None => skipped.push(w.label.to_string()),
        }
    }
    // Fixed Visual Month rides in its own shape: an
    // attribute on <settings> or a fixedSeasonalVisuals
    // element, whichever the save itself uses.
    if edited.fixed_visual_month != original.fixed_visual_month {
        if let Some(v) = edited.fixed_visual_month {
            let text = v.to_string();
            let mut done = false;
            if original.fixed_visual_month_is_attribute {
                if let Some((s, e, tag)) = settings_tag(&out) {
                    let new_tag = set_attr_in_tag(&tag, "fixedSeasonalVisuals", &text);
                    let mut next = String::with_capacity(out.len());
                    next.push_str(&out[..s]);
                    next.push_str(&new_tag);
                    next.push_str(&out[e..]);
                    out = next;
                    done = true;
                }
            } else if let Some(next) = set_element_text(&out, "fixedSeasonalVisuals", &text) {
                out = next;
                done = true;
            }
            if done {
                applied.push("Fixed visual month".to_string());
            } else {
                skipped.push("Fixed visual month".to_string());
            }
        }
    }
    (out, applied, skipped)
}

/// What reconciling the game mods folder did, in counts and
/// plain notes. The notes carry everything that did NOT
/// happen cleanly — a link that failed, an entry left in
/// place — for the results page.
#[derive(Debug, Default)]
pub struct ReconcileReport {
    pub linked: usize,
    pub already_present: usize,
    pub removed: usize,
    pub notes: Vec<String>,
}

/// Make the game mods folder hold exactly the selected mods,
/// by LINK operations only (the owner's law): a selected mod
/// missing from the folder is projected in from its library
/// copy; an unselected entry comes out only when it proves
/// to be a projection of a library copy (the Layout
/// executor's own test), never on suspicion alone.
pub fn reconcile_mods_folder(
    game_dir: &Path,
    selected: &[Mod],
    library: &[Mod],
) -> ReconcileReport {
    let mut report = ReconcileReport::default();
    let wanted: HashSet<String> = selected.iter().map(|m| m.file_name()).collect();

    for m in selected {
        let dest = game_dir.join(m.file_name());
        if dest.exists() {
            report.already_present += 1;
            continue;
        }
        let kind = if m.path.is_dir() {
            LinkKind::Junction
        } else {
            LinkKind::Hardlink
        };
        match create_projection(&m.path, &dest, kind) {
            Ok(()) => report.linked += 1,
            Err(e) => report
                .notes
                .push(format!("Could not link {}: {}", m.file_name(), e)),
        }
    }

    let entries = match std::fs::read_dir(game_dir) {
        Ok(entries) => entries,
        Err(e) => {
            report
                .notes
                .push(format!("Could not read the mods folder: {}", e));
            return report;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if wanted.contains(&name) {
            continue;
        }
        let path = entry.path();
        let is_mod_entry = path.is_dir()
            || name.to_lowercase().ends_with(".zip")
            || name.to_lowercase().ends_with(".dlc");
        if !is_mod_entry {
            continue;
        }
        // The library twin: the same file name living
        // outside the game folder.
        let twin = library
            .iter()
            .find(|m| m.file_name() == name && !m.path.starts_with(game_dir));
        match twin {
            Some(twin) => {
                let kind = if twin.path.is_dir() {
                    LinkKind::Junction
                } else {
                    LinkKind::Hardlink
                };
                if projection_proven(&twin.path, &path, kind) {
                    match remove_projected_entry(&path) {
                        Ok(()) => report.removed += 1,
                        Err(e) => report
                            .notes
                            .push(format!("Could not remove {}: {}", name, e)),
                    }
                } else {
                    report.notes.push(format!(
                        "Left in place: {} — not a verified Playground link.",
                        name
                    ));
                }
            }
            None => {
                report.notes.push(format!(
                    "Left in place: {} — no library copy to verify it against.",
                    name
                ));
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAREER: &str = r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
<careerSavegame revision="2" valid="true">
    <settings>
        <savegameName>My game save</savegameName>
        <mapId>MapUS</mapId>
        <economicDifficulty>EASY</economicDifficulty>
        <trafficEnabled>true</trafficEnabled>
        <growthMode>1</growthMode>
        <plannedDaysPerPeriod>1</plannedDaysPerPeriod>
        <fuelUsage>2</fuelUsage>
        <disasterDestructionState>ENABLED</disasterDestructionState>
        <dirtInterval>3</dirtInterval>
        <timeScale>5.000000</timeScale>
        <autoSaveInterval>15.000000</autoSaveInterval>
    </settings>
    <statistics>
        <money>100000</money>
    </statistics>
</careerSavegame>"#;

    #[test]
    fn reads_settings_and_leaves_gaps_none() {
        let s = read_settings(CAREER);
        assert_eq!(s.savegame_name.as_deref(), Some("My game save"));
        assert_eq!(s.economic_difficulty.as_deref(), Some("EASY"));
        assert_eq!(s.time_scale, Some(5.0));
        assert_eq!(s.growth_mode, Some(1));
        assert_eq!(s.fuel_usage, Some(2));
        assert_eq!(s.disaster_destruction_state.as_deref(), Some("ENABLED"));
        assert_eq!(s.dirt_interval, Some(3));
        // Absent elements stay absent — never invented.
        assert_eq!(s.weeds_enabled, None);
        assert_eq!(s.fixed_visual_month, None);
    }

    #[test]
    fn writes_only_the_changed_elements() {
        let original = read_settings(CAREER);
        let mut edited = original.clone();
        edited.time_scale = Some(6.0);
        edited.traffic_enabled = Some(false);
        edited.growth_mode = Some(2);
        let (out, applied, skipped) = apply_settings_to_xml(CAREER, &original, &edited);
        assert!(skipped.is_empty());
        assert_eq!(applied.len(), 3);
        assert!(out.contains("<timeScale>6.000000</timeScale>"));
        assert!(out.contains("<trafficEnabled>false</trafficEnabled>"));
        assert!(out.contains("<growthMode>2</growthMode>"));
        // Untouched pieces stay untouched, including the
        // <money> echo in <statistics> (outside <settings>).
        assert!(out.contains("<money>100000</money>"));
        assert!(out.contains("<economicDifficulty>EASY</economicDifficulty>"));
        let changes = describe_changes(&original, &edited);
        assert!(changes.iter().any(|c| c == "Seasonal growth: Seasonal → Paused"));
        assert!(changes.iter().any(|c| c == "Timescale: 5 → 6"));
    }

    #[test]
    fn finances_read_and_write_the_first_farm() {
        let farms = "<farms><farm farmId=\"1\" name=\"Farm\" money=\"181992\" loan=\"5000\"/></farms>";
        let f = farm_finances(farms);
        assert_eq!(f.farm_count, 1);
        assert_eq!(f.money, Some(181992));
        assert_eq!(f.loan, Some(5000));
        let out = apply_finances_to_xml(farms, 200000, 0).unwrap();
        assert!(out.contains("money=\"200000\""));
        assert!(out.contains("loan=\"0\""));
        // A missing attribute is inserted, not faked elsewhere.
        let bare = "<farms><farm farmId=\"1\"/></farms>";
        let out = apply_finances_to_xml(bare, 10, 20).unwrap();
        assert!(out.contains("money=\"10\""));
        assert!(out.contains("loan=\"20\""));
    }

    #[test]
    fn recorded_mods_parse_with_required_flags() {
        let xml = "<careerSavegame><mods>\
            <mod modName=\"FS25_Map\" title=\"The Map\" version=\"1.0\" required=\"true\"/>\
            <mod modName=\"FS25_Tractor\" title=\"Tractor\" version=\"2.0\" required=\"false\"/>\
            </mods></careerSavegame>";
        let mods = recorded_mods(xml);
        assert_eq!(mods.len(), 2);
        assert!(mods[0].required);
        assert!(!mods[1].required);
        assert_eq!(mods[0].mod_name, "FS25_Map");
    }
}
