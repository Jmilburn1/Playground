//! Surgical edits to the game's own settings files:
//! gameSettings.xml in the profile folder (the mods directory
//! override) and game.xml in the install folder (the developer
//! console switch). Each edit rewrites only the one value it
//! owns and leaves the rest of the file untouched.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn game_settings_path(profile_dir: &Path) -> PathBuf {
    profile_dir.join("gameSettings.xml")
}

pub fn game_xml_path(game_dir: &Path) -> PathBuf {
    game_dir.join("game.xml")
}

fn attr(line: &str, name: &str) -> Option<String> {
    let needle = format!("{}=\"", name);
    let start = line.find(&needle)? + needle.len();
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

/// The current mods directory override in gameSettings.xml:
/// (is it active, the directory text). None when the file or
/// the element is missing.
pub fn read_mods_override(profile_dir: &Path) -> Option<(bool, String)> {
    let text = std::fs::read_to_string(game_settings_path(profile_dir)).ok()?;
    let line = text.lines().find(|l| l.contains("modsDirectoryOverride"))?;
    let active = attr(line, "active").map(|a| a == "true").unwrap_or(false);
    let dir = attr(line, "directory").unwrap_or_default();
    Some((active, dir))
}

/// Point the game at `folder` for its mods: sets the
/// modsDirectoryOverride element (the third line of
/// gameSettings.xml) to active with that directory, inserting
/// the element right under the root tag when it isn't there yet.
pub fn set_mods_override(profile_dir: &Path, folder: &Path) -> Result<()> {
    let path = game_settings_path(profile_dir);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read {:?}", path))?;
    // The game writes these paths with forward slashes.
    let dir = folder.to_string_lossy().replace('\\', "/");
    let element =
        format!("<modsDirectoryOverride active=\"true\" directory=\"{}\"/>", dir);
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    if let Some(pos) = lines
        .iter()
        .position(|l| l.contains("modsDirectoryOverride"))
    {
        let indent: String = lines[pos]
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        lines[pos] = format!("{}{}", indent, element);
    } else if let Some(pos) = lines.iter().position(|l| l.contains("<gameSettings")) {
        lines.insert(pos + 1, format!("    {}", element));
    } else {
        anyhow::bail!("gameSettings.xml has no <gameSettings> root element");
    }
    let mut out = lines.join("\n");
    out.push('\n');
    std::fs::write(&path, out)
        .with_context(|| format!("Could not write {:?}", path))?;
    Ok(())
}

/// Byte span of the value inside <development><controls> in
/// game.xml, so reads and writes share one definition of where
/// the console switch lives.
fn controls_value_span(text: &str) -> Option<(usize, usize)> {
    let dev = text.find("<development>")?;
    let rest = &text[dev..];
    let open = rest.find("<controls>")? + "<controls>".len();
    let close = rest[open..].find("</controls>")? + open;
    Some((dev + open, dev + close))
}

/// The developer-console switch in game.xml. None when the
/// file or the element is missing.
pub fn console_enabled(game_dir: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(game_xml_path(game_dir)).ok()?;
    let (start, end) = controls_value_span(&text)?;
    Some(text[start..end].trim() == "true")
}

/// Flip the developer-console switch in game.xml, changing
/// nothing else in the file.
pub fn set_console(game_dir: &Path, on: bool) -> Result<()> {
    let path = game_xml_path(game_dir);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read {:?}", path))?;
    let (start, end) = controls_value_span(&text).ok_or_else(|| {
        anyhow::anyhow!("game.xml has no <development><controls> entry")
    })?;
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..start]);
    out.push_str(if on { "true" } else { "false" });
    out.push_str(&text[end..]);
    std::fs::write(&path, out)
        .with_context(|| format!("Could not write {:?}", path))?;
    Ok(())
}
