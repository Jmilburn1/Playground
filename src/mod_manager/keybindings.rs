//! The Keybindings page's model: the game's
//! `inputBinding.xml` (in the profile folder), read and
//! written as TEXT, never as a DOM.
//!
//! The owner's real file defeats strict parsers on purpose:
//! it opens with a DUPLICATED `<?xml ...?>` declaration and
//! carries root attributes, comments and shapes a DOM
//! round-trip would normalize away. So the model is a
//! text-splice: the document is the original string, parsing
//! finds the ordered `<actionBinding>` blocks and the light
//! element view inside them (action name; per binding:
//! device, input, axisComponent, neutralInput, index), and
//! an edit rewrites only the affected action block. A load
//! followed by a save with zero edits reproduces the file
//! byte-for-byte.
//!
//! Conflict rules, auto-resolve, device labels, input
//! humanizing and the key-capture mapping are pure functions
//! beside the model, unit-tested without touching disk. The
//! backup helpers are the only IO in the module.

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The file's name inside the profile folder.
pub const FILE_NAME: &str = "inputBinding.xml";

/// The keyboard device id — the only device v1 captures
/// new keys for.
pub const KEYBOARD_DEVICE: &str = "KB_MOUSE_DEFAULT";

/// One `<binding>` element's light view. All values are
/// the file's own strings, verbatim.
#[derive(Debug, Clone, PartialEq)]
pub struct KbBinding {
    pub device: String,
    pub input: String,
    pub axis_component: String,
    pub neutral_input: String,
    pub index: String,
}

/// One `<actionBinding>` block's light view: the action
/// name and its bindings, in file order.
#[derive(Debug, Clone, PartialEq)]
pub struct KbAction {
    pub name: String,
    pub bindings: Vec<KbBinding>,
}

/// The document: the file's text, whole and unparsed.
/// Parsing is a view computed on demand; edits splice the
/// text and the next view re-parses, so spans never go
/// stale in a caller's hands.
#[derive(Debug, Clone)]
pub struct BindingsFile {
    text: String,
}

impl BindingsFile {
    pub fn parse(text: String) -> Self {
        Self { text }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The ordered action view of the current text.
    pub fn actions(&self) -> Vec<KbAction> {
        parse_actions(&self.text)
            .into_iter()
            .map(|a| KbAction {
                name: a.name,
                bindings: a.bindings.into_iter().map(|b| b.view).collect(),
            })
            .collect()
    }

    /// Devices actually present in the file, in
    /// first-appearance order.
    pub fn devices(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for action in parse_actions(&self.text) {
            for binding in &action.bindings {
                if !out.contains(&binding.view.device) {
                    out.push(binding.view.device.clone());
                }
            }
        }
        out
    }

    /// Rewrite one binding's input value (the binding is
    /// the `pos`-th binding of the named action, in the
    /// current text's order). Only the value bytes inside
    /// that one element change. False when the action or
    /// binding is not there.
    pub fn set_input(&mut self, action: &str, pos: usize, new_input: &str) -> bool {
        let actions = parse_actions(&self.text);
        let Some(action) = actions.iter().find(|a| a.name == action) else {
            return false;
        };
        let Some(binding) = action.bindings.get(pos) else {
            return false;
        };
        let element = &self.text[binding.span.0..binding.span.1];
        let Some((start, end)) = attr_value_span(element, "input") else {
            return false;
        };
        let start = binding.span.0 + start;
        let end = binding.span.0 + end;
        self.text.replace_range(start..end, new_input);
        true
    }

    /// Remove one binding element (the `pos`-th binding of
    /// the named action) together with its line's indent
    /// and trailing newline, so the block closes up the way
    /// the game itself would write it. False when not there.
    pub fn remove_binding(&mut self, action: &str, pos: usize) -> bool {
        let actions = parse_actions(&self.text);
        let Some(action) = actions.iter().find(|a| a.name == action) else {
            return false;
        };
        let Some(binding) = action.bindings.get(pos) else {
            return false;
        };
        let (mut start, mut end) = binding.span;
        // Take the line's indent with the element...
        while start > 0 {
            let c = self.text.as_bytes()[start - 1];
            if c == b' ' || c == b'\t' {
                start -= 1;
            } else {
                break;
            }
        }
        // ...and its line ending.
        if self.text.as_bytes().get(end) == Some(&b'\r') {
            end += 1;
        }
        if self.text.as_bytes().get(end) == Some(&b'\n') {
            end += 1;
        }
        self.text.replace_range(start..end, "");
        true
    }

    /// Remove one action's whole `<actionBinding>` block,
    /// together with its line's indent and one trailing
    /// line ending, so the document closes up the way the
    /// game itself would write it. Every other byte is
    /// untouched. False when the action is not there.
    pub fn remove_action_block(&mut self, action: &str) -> bool {
        let actions = parse_actions(&self.text);
        let Some(action) = actions.iter().find(|a| a.name == action) else {
            return false;
        };
        let (mut start, mut end) = action.span;
        // Take the line's indent with the block...
        while start > 0 {
            let c = self.text.as_bytes()[start - 1];
            if c == b' ' || c == b'\t' {
                start -= 1;
            } else {
                break;
            }
        }
        // ...and its line ending.
        if self.text.as_bytes().get(end) == Some(&b'\r') {
            end += 1;
        }
        if self.text.as_bytes().get(end) == Some(&b'\n') {
            end += 1;
        }
        self.text.replace_range(start..end, "");
        true
    }

    /// Stage the purge of inactive bindings (owner
    /// ruling, 2026-10-22): remove the whole action block
    /// of every action `purge_candidates` names against
    /// `owners` — actions positively owned by an
    /// archived-state mod, and nothing else (see
    /// `is_purge_eligible`). This is a staged edit like
    /// any other: nothing reaches disk until the page's
    /// save flow runs, and Discard abandons it. Returns
    /// how many action blocks were removed and how many
    /// distinct mods they belonged to.
    pub fn stage_purge(
        &mut self,
        owners: &HashMap<String, ActionOwner>,
    ) -> PurgeSummary {
        let names = purge_candidates(&self.actions(), owners);
        let mut mods: HashSet<&str> = HashSet::new();
        let mut removed = 0;
        for name in &names {
            if let Some(owner) = owners.get(name) {
                mods.insert(owner.title.as_str());
            }
            if self.remove_action_block(name) {
                removed += 1;
            }
        }
        PurgeSummary {
            actions: removed,
            mods: mods.len(),
        }
    }

    /// Append a binding to the named action, in the file's
    /// own element shape: device / input / axisComponent
    /// "+" / neutralInput "0", index one past the action's
    /// highest index on that device (1 when it has none).
    /// False when the action is not there.
    pub fn add_binding(&mut self, action: &str, device: &str, input: &str) -> bool {
        let actions = parse_actions(&self.text);
        let Some(action) = actions.iter().find(|a| a.name == action) else {
            return false;
        };
        let next_index = action
            .bindings
            .iter()
            .filter(|b| b.view.device == device)
            .filter_map(|b| b.view.index.parse::<u32>().ok())
            .max()
            .map(|m| m + 1)
            .unwrap_or(1);
        // The indent the block's own binding lines use
        // (the action line's indent + 4 when it has none).
        let indent = match action.bindings.first() {
            Some(first) => line_indent(&self.text, first.span.0),
            None => format!("{}    ", line_indent(&self.text, action.span.0)),
        };
        let element = format!(
            "<binding device=\"{}\" input=\"{}\" axisComponent=\"+\" neutralInput=\"0\" index=\"{}\"></binding>",
            device, input, next_index
        );
        let close = action.inner_end;
        if self.text[..close].ends_with('\n') {
            let at = self.text[..close].rfind('\n').map(|i| i + 1).unwrap_or(close);
            self.text.insert_str(at, &format!("{}{}\n", indent, element));
        } else {
            self.text
                .insert_str(close, &format!("\n{}{}", indent, element));
        }
        true
    }

    /// Per-group Auto-Resolve, staged on this document:
    /// clears are staged ONLY for HARD findings (the
    /// v1.6.2 tiers — see `classify`). A Hard finding's
    /// same-mod pair must live in `group` (the pair's
    /// owner group); the pair's first action in file
    /// order keeps the input, and the binding is removed
    /// from the pair's later holders. Shared and Base
    /// findings, and Warnings, are never auto-resolved.
    /// Returns how many bindings were cleared.
    pub fn auto_resolve(
        &mut self,
        device: &str,
        group: &HashSet<String>,
        owners: &HashMap<String, String>,
    ) -> usize {
        let actions = self.actions();
        let findings = analyze(&actions, device);
        let mut cleared = 0usize;
        for finding in findings
            .iter()
            .filter(|f| classify(f, owners) == Some(FindingTier::Hard))
        {
            // The owners holding two or more of this
            // input's actions — each is a same-mod pair
            // (or a larger same-mod pile-up).
            let mut by_owner: HashMap<&str, Vec<&str>> = HashMap::new();
            for action in &finding.actions {
                if let Some(owner) = owners.get(action) {
                    by_owner
                        .entry(owner.as_str())
                        .or_default()
                        .push(action.as_str());
                }
            }
            for holders in by_owner.values().filter(|h| h.len() > 1) {
                // The pair must live in this group — a
                // different group's pass never touches it.
                if !holders.iter().all(|h| group.contains(*h)) {
                    continue;
                }
                // First in file order keeps the input; the
                // pair's later holders lose it. Each
                // action's bindings go high-to-low so a
                // removal leaves the others' positions
                // valid.
                for holder in holders.iter().skip(1) {
                    let Some(action) =
                        actions.iter().find(|a| &a.name == holder)
                    else {
                        continue;
                    };
                    let mut positions: Vec<usize> = action
                        .bindings
                        .iter()
                        .enumerate()
                        .filter(|(_, b)| {
                            b.device == device && b.input == finding.input
                        })
                        .map(|(i, _)| i)
                        .collect();
                    positions.sort_unstable_by(|a, b| b.cmp(a));
                    for pos in positions {
                        if self.remove_binding(holder, pos) {
                            cleared += 1;
                        }
                    }
                }
            }
        }
        cleared
    }

    /// How many action blocks differ from `other` — the
    /// page footer's dirty count. An action present in one
    /// document and not the other counts once.
    pub fn changed_blocks(&self, other: &BindingsFile) -> usize {
        let mine = block_texts(&self.text);
        let theirs = block_texts(&other.text);
        let mut count = 0;
        for (name, block) in &mine {
            match theirs.get(name) {
                Some(other_block) if other_block == block => {}
                _ => count += 1,
            }
        }
        for name in theirs.keys() {
            if !mine.contains_key(name) {
                count += 1;
            }
        }
        count
    }
}

/// Action name -> the block's exact text (first block of
/// a name wins; the game writes each action once).
fn block_texts(text: &str) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for action in parse_actions(text) {
        out.entry(action.name.clone())
            .or_insert_with(|| text[action.span.0..action.span.1].to_string());
    }
    out
}

// ---------------------------------------------------------------------
// Parsing (text-splice)
// ---------------------------------------------------------------------

struct ParsedBinding {
    view: KbBinding,
    /// Byte range of the whole `<binding ...></binding>`
    /// element in the document.
    span: (usize, usize),
}

struct ParsedAction {
    name: String,
    bindings: Vec<ParsedBinding>,
    /// Byte range of the whole block in the document.
    span: (usize, usize),
    /// Byte offset of the block's `</actionBinding>`.
    inner_end: usize,
}

/// The end of the tag that opens at `start` (the offset
/// of its `>`), skipping `>` inside quoted values.
fn find_tag_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = start;
    while i < bytes.len() {
        match quote {
            Some(q) if bytes[i] == q => quote = None,
            Some(_) => {}
            None if bytes[i] == b'"' || bytes[i] == b'\'' => quote = Some(bytes[i]),
            None if bytes[i] == b'>' => return Some(i),
            None => {}
        }
        i += 1;
    }
    None
}

/// The value of attribute `name` within one tag's text.
fn attr(tag: &str, name: &str) -> Option<String> {
    let (start, end) = attr_value_span(tag, name)?;
    Some(tag[start..end].to_string())
}

/// The value's byte span within a tag's text. The needle
/// carries a leading space so a lookup can never land on
/// the tail of a longer attribute name (`input` inside
/// `neutralInput`).
fn attr_value_span(tag: &str, name: &str) -> Option<(usize, usize)> {
    let needle = format!(" {}=\"", name);
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some((start, end))
}

/// The whitespace that opens the line holding offset `at`.
fn line_indent(text: &str, at: usize) -> String {
    let line_start = text[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    text[line_start..at]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// Find `<element` occurrences at or after `from` whose
/// next character can legally follow a tag name. Yields
/// each occurrence's start.
fn find_elements<'a>(text: &'a str, element: &str, from: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut search = from;
    while let Some(rel) = text[search..].find(element) {
        let start = search + rel;
        match text.as_bytes().get(start + element.len()) {
            Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') | Some(b'>')
            | Some(b'/') => out.push(start),
            _ => {}
        }
        search = start + element.len();
    }
    out
}

fn parse_actions(text: &str) -> Vec<ParsedAction> {
    let mut out = Vec::new();
    for start in find_elements(text, "<actionBinding", 0) {
        let Some(tag_end) = find_tag_end(text, start) else {
            continue;
        };
        let tag = &text[start..=tag_end];
        let self_closing = tag[..tag.len() - 1].trim_end().ends_with('/');
        let (block_end, inner_end) = if self_closing {
            (tag_end + 1, tag_end + 1)
        } else {
            match text[tag_end + 1..].find("</actionBinding>") {
                Some(rel) => (tag_end + 1 + rel + "</actionBinding>".len(), tag_end + 1 + rel),
                None => continue,
            }
        };
        let name = attr(tag, "action").unwrap_or_default();
        let mut bindings = Vec::new();
        for bstart in find_elements(&text[..inner_end], "<binding", tag_end + 1) {
            let Some(btag_end) = find_tag_end(text, bstart) else {
                continue;
            };
            let btag = &text[bstart..=btag_end];
            let bself_closing = btag[..btag.len() - 1].trim_end().ends_with('/');
            let bend = if bself_closing {
                btag_end + 1
            } else {
                match text[btag_end + 1..].find("</binding>") {
                    Some(rel) => btag_end + 1 + rel + "</binding>".len(),
                    None => btag_end + 1,
                }
            };
            bindings.push(ParsedBinding {
                view: KbBinding {
                    device: attr(btag, "device").unwrap_or_default(),
                    input: attr(btag, "input").unwrap_or_default(),
                    axis_component: attr(btag, "axisComponent").unwrap_or_default(),
                    neutral_input: attr(btag, "neutralInput").unwrap_or_default(),
                    index: attr(btag, "index").unwrap_or_default(),
                },
                span: (bstart, bend),
            });
        }
        out.push(ParsedAction {
            name,
            bindings,
            span: (start, block_end),
            inner_end,
        });
    }
    out
}

// ---------------------------------------------------------------------
// Conflict analysis (pure)
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingKind {
    Conflict,
    Warning,
}

/// One input's finding on one device: the input, the
/// severity, and the actions involved, in file order.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub input: String,
    pub kind: FindingKind,
    pub actions: Vec<String>,
}

/// Classify one device's bindings:
/// - Conflict: the same input AND the same axisComponent
///   are held by two or more DIFFERENT actions.
/// - Warning: an input is shared across different actions
///   only via differing axisComponent (the normal
///   axis-sharing case, + against -), or one action holds
///   the same input in multiple slots.
/// Normalization is the raw input string, case-sensitive;
/// axisComponent participates only as stated above.
pub fn analyze(actions: &[KbAction], device: &str) -> Vec<Finding> {
    // Inputs in first-appearance order, each with its
    // (action, axisComponent) entries in file order.
    let mut order: Vec<String> = Vec::new();
    let mut entries: std::collections::HashMap<String, Vec<(String, String)>> =
        std::collections::HashMap::new();
    for action in actions {
        for binding in &action.bindings {
            if binding.device != device {
                continue;
            }
            if !entries.contains_key(&binding.input) {
                order.push(binding.input.clone());
            }
            entries
                .entry(binding.input.clone())
                .or_default()
                .push((action.name.clone(), binding.axis_component.clone()));
        }
    }
    let mut findings = Vec::new();
    for input in order {
        let entries = &entries[&input];
        let mut distinct_actions: Vec<String> = Vec::new();
        for (action, _) in entries {
            if !distinct_actions.contains(action) {
                distinct_actions.push(action.clone());
            }
        }
        // Conflict test: one axisComponent value held by
        // two or more different actions.
        let mut by_axis: std::collections::HashMap<&str, Vec<&str>> =
            std::collections::HashMap::new();
        for (action, axis) in entries {
            by_axis.entry(axis.as_str()).or_default().push(action);
        }
        let conflict = by_axis.values().any(|holders| {
            let mut seen: Vec<&str> = Vec::new();
            for h in holders {
                if !seen.contains(h) {
                    seen.push(h);
                }
            }
            seen.len() > 1
        });
        let duplicate_slot = distinct_actions.len() < entries.len();
        let kind = if conflict {
            FindingKind::Conflict
        } else if distinct_actions.len() > 1 || duplicate_slot {
            FindingKind::Warning
        } else {
            continue;
        };
        findings.push(Finding {
            input,
            kind,
            actions: distinct_actions,
        });
    }
    findings
}

// ---------------------------------------------------------------------
// Finding tiers (v1.6.2)
// ---------------------------------------------------------------------

/// A Conflict finding's tier, by who owns the colliding
/// actions (the manager's action -> owner map; an action
/// with no owner is Base Game):
/// - Hard: two or more colliding actions belong to the
///   SAME mod — a mod fighting itself for a key.
/// - Shared: not Hard, but at least one colliding action
///   is mod-owned.
/// - Base: no colliding action is mod-owned.
/// The plain same-key count alarmed without informing —
/// the game reuses keys across in-game situations the
/// file does not record — so the page's banner tiers the
/// truth instead (owner ruling, 2026-10-22).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingTier {
    Hard,
    Shared,
    Base,
}

/// Classify one finding. `owners` maps an action name to
/// its owning mod's title. Warnings are not tiered and
/// yield None.
pub fn classify(
    finding: &Finding,
    owners: &HashMap<String, String>,
) -> Option<FindingTier> {
    if finding.kind != FindingKind::Conflict {
        return None;
    }
    let mut per_owner: HashMap<&str, usize> = HashMap::new();
    let mut any_owned = false;
    for action in &finding.actions {
        if let Some(owner) = owners.get(action) {
            any_owned = true;
            *per_owner.entry(owner.as_str()).or_insert(0) += 1;
        }
    }
    if per_owner.values().any(|&n| n > 1) {
        Some(FindingTier::Hard)
    } else if any_owned {
        Some(FindingTier::Shared)
    } else {
        Some(FindingTier::Base)
    }
}

/// One device's findings, counted by tier, plus the
/// Warning count and how many distinct mods own an
/// action in a Hard finding (the banner's K). Partition
/// property: hard + shared + base == the device's
/// Conflict findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TierCounts {
    pub hard: usize,
    pub shared: usize,
    pub base: usize,
    pub warnings: usize,
    pub hard_mods: usize,
}

pub fn tier_counts(
    findings: &[Finding],
    owners: &HashMap<String, String>,
) -> TierCounts {
    let mut counts = TierCounts::default();
    let mut hard_mods: HashSet<&str> = HashSet::new();
    for finding in findings {
        match classify(finding, owners) {
            Some(FindingTier::Hard) => {
                counts.hard += 1;
                for action in &finding.actions {
                    if let Some(owner) = owners.get(action) {
                        hard_mods.insert(owner.as_str());
                    }
                }
            }
            Some(FindingTier::Shared) => counts.shared += 1,
            Some(FindingTier::Base) => counts.base += 1,
            None => counts.warnings += 1,
        }
    }
    counts.hard_mods = hard_mods.len();
    counts
}

/// What the Keybindings page's action -> owner join
/// says about ONE action (built by the manager from the
/// owning mod's record): the mod's title, the category
/// its modDesc declared for the action (if any), and the
/// mod's state. Actions no loaded mod claims have no
/// ActionOwner at all — they group under Base Game.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionOwner {
    pub title: String,
    pub category: Option<String>,
    pub state: KbOwnerState,
}

/// The owning mod's state, in the vocabulary the page's
/// active/archive distinction needs. `Other` covers the
/// Mod record's remaining states (Conflict, Error): the
/// mod is neither cleanly loaded nor cleanly shelved, so
/// its actions stay visible and are never purgeable —
/// only a positively Archived mod is ever set aside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KbOwnerState {
    Active,
    Archived,
    Other,
}

/// The title-only projection of the ownership map — the
/// shape `classify`, `tier_counts` and `auto_resolve`
/// consume.
pub fn owner_titles(
    owners: &HashMap<String, ActionOwner>,
) -> HashMap<String, String> {
    owners
        .iter()
        .map(|(action, owner)| (action.clone(), owner.title.clone()))
        .collect()
}

/// The actions the page groups and analyzes over. With
/// `show_archived` on this is every action (the v1.6.2
/// behavior). With it off, actions owned by an
/// archived-state mod are excluded ENTIRELY — not
/// reclassified as Base Game, not merely hidden from the
/// group list: an unloaded mod's binding cannot fire in
/// game, so it has no business in the findings, the tier
/// counts or the banner (owner ruling, 2026-10-22).
/// Base-game, unknown-owner, active-owned and
/// other-state actions always pass. This is a view over
/// the document: the document itself is untouched, and a
/// no-edit save still round-trips byte-for-byte,
/// archived blocks included.
pub fn visible_actions(
    actions: &[KbAction],
    owners: &HashMap<String, ActionOwner>,
    show_archived: bool,
) -> Vec<KbAction> {
    if show_archived {
        return actions.to_vec();
    }
    actions
        .iter()
        .filter(|action| {
            !matches!(
                owners.get(&action.name),
                Some(owner) if owner.state == KbOwnerState::Archived
            )
        })
        .cloned()
        .collect()
}

/// The purge's eligibility rule, the ONLY predicate its
/// code path consults: an action is purgeable exactly
/// when a loaded mod positively owns it AND that mod's
/// state is Archived. Active-owned, other-state,
/// base-game and unknown-owner actions are structurally
/// unreachable — there is no second path in.
pub fn is_purge_eligible(
    action: &str,
    owners: &HashMap<String, ActionOwner>,
) -> bool {
    matches!(
        owners.get(action),
        Some(owner) if owner.state == KbOwnerState::Archived
    )
}

/// The actions a purge would remove, in file order.
pub fn purge_candidates(
    actions: &[KbAction],
    owners: &HashMap<String, ActionOwner>,
) -> Vec<String> {
    actions
        .iter()
        .filter(|action| is_purge_eligible(&action.name, owners))
        .map(|action| action.name.clone())
        .collect()
}

/// A purge's size, as the page reports it before staging:
/// how many action blocks, from how many distinct mods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PurgeSummary {
    pub actions: usize,
    pub mods: usize,
}

pub fn purge_summary(
    actions: &[KbAction],
    owners: &HashMap<String, ActionOwner>,
) -> PurgeSummary {
    let names = purge_candidates(actions, owners);
    let mods: HashSet<&str> = names
        .iter()
        .filter_map(|name| owners.get(name))
        .map(|owner| owner.title.as_str())
        .collect();
    PurgeSummary {
        actions: names.len(),
        mods: mods.len(),
    }
}

/// The banner's two lines: the Hard headline (or its
/// calm variant when a device has none) and, when any of
/// its parts is nonzero, the quiet second line with the
/// Shared / Base / Warning counts.
pub fn banner_lines(counts: &TierCounts) -> (String, Option<String>) {
    let line1 = if counts.hard > 0 {
        format!(
            "{} Hard Keybinding Conflicts Detected (Affecting {} Mods)",
            counts.hard, counts.hard_mods
        )
    } else {
        "No hard keybinding conflicts on this device.".to_string()
    };
    let line2 = if counts.shared + counts.base + counts.warnings > 0 {
        Some(format!(
            "{} Shared Keys Involving Mods · {} Base Game Overlaps · {} Warnings",
            counts.shared, counts.base, counts.warnings
        ))
    } else {
        None
    };
    (line1, line2)
}

// ---------------------------------------------------------------------
// Display transforms (pure)
// ---------------------------------------------------------------------

/// A device id's tab label. The keyboard and any XInput
/// gamepad get their plain names; anything else (a wheel's
/// long encoded id, say) gets a short generic label — the
/// raw id is never printed.
pub fn device_label(device: &str) -> String {
    if device == KEYBOARD_DEVICE {
        "Keyboard & Mouse".to_string()
    } else if device.contains("XINPUT_GAMEPAD") {
        "Gamepad".to_string()
    } else {
        "Wheel / Other device".to_string()
    }
}

/// Action names for display: underscores to spaces, and
/// nothing else — a pure display transform.
pub fn humanize_action(name: &str) -> String {
    name.replace('_', " ")
}

/// One input string for a chip: each space-separated
/// token loses its `KEY_` prefix, its underscores become
/// spaces and its words are title-cased
/// ("MOUSE_BUTTON_LEFT" -> "Mouse Button Left",
/// "KEY_lshift KEY_n" -> "Lshift + N").
pub fn humanize_input(input: &str) -> String {
    input
        .split(' ')
        .filter(|t| !t.is_empty())
        .map(|token| {
            let token = token.strip_prefix("KEY_").unwrap_or(token);
            token
                .split('_')
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        Some(first) => {
                            first.to_uppercase().collect::<String>()
                                + &chars.as_str().to_lowercase()
                        }
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

// ---------------------------------------------------------------------
// Key capture (pure mapping)
// ---------------------------------------------------------------------

/// Map one pressed key to the file's input vocabulary.
/// Returns the full input string — modifier combos are
/// space-joined token lists in the file ("KEY_lshift
/// KEY_n", "KEY_lctrl KEY_lalt KEY_f"), so held modifiers
/// prefix the key's token in ctrl, alt, shift order, each
/// on its left side unless the key event's own location
/// says otherwise. A modifier pressed alone binds its own
/// token (the file carries plain "KEY_lshift" bindings).
/// Escape never maps — the caller treats it as cancel.
/// None when the key has no name in the file's vocabulary.
pub fn fs_input_for_key(
    key: &iced::keyboard::Key,
    location: iced::keyboard::Location,
    modifiers: iced::keyboard::Modifiers,
) -> Option<String> {
    use iced::keyboard::key::Named;
    use iced::keyboard::{Key, Location};
    // A modifier on its own binds its own token, sided by
    // the event's location.
    let side = |left: &str, right: &str| -> String {
        if location == Location::Right {
            right.to_string()
        } else {
            left.to_string()
        }
    };
    let base: String = match key {
        Key::Named(Named::Shift) => return Some(side("KEY_lshift", "KEY_rshift")),
        Key::Named(Named::Control) => return Some(side("KEY_lctrl", "KEY_rctrl")),
        Key::Named(Named::Alt) => return Some(side("KEY_lalt", "KEY_ralt")),
        Key::Named(Named::Space) => "KEY_space".to_string(),
        Key::Named(Named::Enter) => {
            if location == Location::Numpad {
                "KEY_KP_enter".to_string()
            } else {
                "KEY_return".to_string()
            }
        }
        Key::Named(Named::Tab) => "KEY_tab".to_string(),
        Key::Named(Named::Backspace) => "KEY_backspace".to_string(),
        Key::Named(Named::ArrowUp) => "KEY_up".to_string(),
        Key::Named(Named::ArrowDown) => "KEY_down".to_string(),
        Key::Named(Named::ArrowLeft) => "KEY_left".to_string(),
        Key::Named(Named::ArrowRight) => "KEY_right".to_string(),
        Key::Named(Named::Delete) => "KEY_delete".to_string(),
        Key::Named(Named::Insert) => "KEY_insert".to_string(),
        Key::Named(Named::Home) => "KEY_home".to_string(),
        Key::Named(Named::End) => "KEY_end".to_string(),
        Key::Named(Named::PageUp) => "KEY_pageup".to_string(),
        Key::Named(Named::PageDown) => "KEY_pagedown".to_string(),
        Key::Named(Named::Pause) => "KEY_pause".to_string(),
        Key::Named(Named::PrintScreen) => "KEY_print".to_string(),
        Key::Named(Named::ScrollLock) => "KEY_scrolllock".to_string(),
        Key::Named(Named::Clear) => "KEY_clear".to_string(),
        Key::Named(Named::F1) => "KEY_f1".to_string(),
        Key::Named(Named::F2) => "KEY_f2".to_string(),
        Key::Named(Named::F3) => "KEY_f3".to_string(),
        Key::Named(Named::F4) => "KEY_f4".to_string(),
        Key::Named(Named::F5) => "KEY_f5".to_string(),
        Key::Named(Named::F6) => "KEY_f6".to_string(),
        Key::Named(Named::F7) => "KEY_f7".to_string(),
        Key::Named(Named::F8) => "KEY_f8".to_string(),
        Key::Named(Named::F9) => "KEY_f9".to_string(),
        Key::Named(Named::F10) => "KEY_f10".to_string(),
        Key::Named(Named::F11) => "KEY_f11".to_string(),
        Key::Named(Named::F12) => "KEY_f12".to_string(),
        Key::Character(text) => {
            let text = text.as_str();
            let mut chars = text.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            if location == Location::Numpad {
                return match c {
                    '0'..='9' => Some(format!("KEY_KP_{}", c)),
                    '.' => Some("KEY_KP_period".to_string()),
                    '+' => Some("KEY_KP_plus".to_string()),
                    '-' => Some("KEY_KP_minus".to_string()),
                    '*' => Some("KEY_KP_multiply".to_string()),
                    '/' => Some("KEY_KP_divide".to_string()),
                    _ => None,
                };
            }
            // Shifted symbols speak for their base key.
            let c = match c {
                '!' => '1',
                '@' => '2',
                '#' => '3',
                '$' => '4',
                '%' => '5',
                '^' => '6',
                '&' => '7',
                '*' => '8',
                '(' => '9',
                ')' => '0',
                '_' => '-',
                '+' => '=',
                '|' => '\\',
                ':' => ';',
                '"' => '\'',
                '<' => ',',
                '>' => '.',
                '?' => '/',
                other => other,
            };
            match c {
                'a'..='z' => format!("KEY_{}", c),
                'A'..='Z' => format!("KEY_{}", c.to_ascii_lowercase()),
                '0'..='9' => format!("KEY_{}", c),
                ',' => "KEY_comma".to_string(),
                '.' => "KEY_period".to_string(),
                '-' => "KEY_minus".to_string(),
                '=' => "KEY_equals".to_string(),
                ';' => "KEY_semicolon".to_string(),
                '\'' => "KEY_quote".to_string(),
                '[' => "KEY_leftbracket".to_string(),
                ']' => "KEY_rightbracket".to_string(),
                '\\' => "KEY_backslash".to_string(),
                '/' => "KEY_slash".to_string(),
                _ => return None,
            }
        }
        _ => return None,
    };
    let mut tokens: Vec<String> = Vec::new();
    if modifiers.control() {
        tokens.push("KEY_lctrl".to_string());
    }
    if modifiers.alt() {
        tokens.push("KEY_lalt".to_string());
    }
    if modifiers.shift() {
        tokens.push("KEY_lshift".to_string());
    }
    tokens.push(base);
    Some(tokens.join(" "))
}

// ---------------------------------------------------------------------
// Backups and the live file (the module's only IO)
// ---------------------------------------------------------------------

pub fn bindings_path(profile: &Path) -> PathBuf {
    profile.join(FILE_NAME)
}

pub fn backups_dir(profile: &Path) -> PathBuf {
    profile.join("inputBinding-backups")
}

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// The next free timestamped backup path in the backups
/// folder (a same-second collision earns a suffix).
fn free_backup_path(profile: &Path) -> PathBuf {
    let dir = backups_dir(profile);
    let base = format!("inputBinding-{}", stamp());
    let mut candidate = dir.join(format!("{}.xml", base));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{}-{}.xml", base, n));
        n += 1;
    }
    candidate
}

/// Copy the live file into the backups folder under a
/// timestamped name. Fails when there is no live file —
/// a save must never write unbacked-up.
pub fn backup_current(profile: &Path) -> Result<PathBuf> {
    let live = bindings_path(profile);
    if !live.exists() {
        anyhow::bail!("There is no {} to back up", FILE_NAME);
    }
    std::fs::create_dir_all(backups_dir(profile))
        .context("Could not create the backups folder")?;
    let dest = free_backup_path(profile);
    std::fs::copy(&live, &dest)
        .with_context(|| format!("Could not back up {:?} to {:?}", live, dest))?;
    Ok(dest)
}

/// Existing backups, newest first (timestamped names
/// sort chronologically), each with its size in bytes.
pub fn list_backups(profile: &Path) -> Vec<(PathBuf, u64)> {
    let dir = backups_dir(profile);
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<(PathBuf, u64)> = read
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| {
                    let n = n.to_string_lossy();
                    n.starts_with("inputBinding-") && n.ends_with(".xml")
                })
                .unwrap_or(false)
        })
        .map(|p| {
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            (p, size)
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out
}

/// Restore one backup over the live file: the current
/// live file is backed up first, the same way a save
/// backs it up, then the chosen backup is copied in.
pub fn restore_backup(profile: &Path, backup: &Path) -> Result<()> {
    if bindings_path(profile).exists() {
        backup_current(profile)?;
    }
    std::fs::copy(backup, bindings_path(profile))
        .with_context(|| format!("Could not restore {:?}", backup))?;
    Ok(())
}

/// Reset to game defaults: MOVE the live file into the
/// backups folder under a timestamped name, leaving no
/// inputBinding.xml behind — the game regenerates its
/// factory defaults on its next launch. Returns where
/// the file went.
pub fn reset_to_defaults(profile: &Path) -> Result<PathBuf> {
    let live = bindings_path(profile);
    if !live.exists() {
        anyhow::bail!("There is no {} to reset", FILE_NAME);
    }
    std::fs::create_dir_all(backups_dir(profile))
        .context("Could not create the backups folder")?;
    let dest = free_backup_path(profile);
    std::fs::rename(&live, &dest)
        .with_context(|| format!("Could not move {:?} to {:?}", live, dest))?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed SYNTHETIC fixture — written for these
    /// tests, never the owner's file. It carries the real
    /// file's quirks at small scale: the duplicated XML
    /// declaration, root attributes, a comment, an unknown
    /// element, an encoded-style device id, and a binding
    /// vocabulary with a conflict, an axis-sharing warning
    /// and a duplicate-slot warning.
    const FIXTURE: &str = r#"<?xml version="1.0" encoding="utf-8" standalone="no" ?>
<?xml version="1.0" encoding="utf-8" standalone="no"?>
<inputBinding version="24" bindingVersion="1" mouseSensitivityScaleX="1.000000" mouseSensitivityScaleY="1.000000">
    <!-- a comment that must survive -->
    <unknownElement foo="bar"/>
    <actionBinding action="MOVE_FORWARD">
        <binding device="KB_MOUSE_DEFAULT" input="KEY_w" axisComponent="+" neutralInput="0" index="1"></binding>
        <binding device="0_XINPUT_GAMEPAD" input="BUTTON_5" axisComponent="+" neutralInput="0" index="1"></binding>
    </actionBinding>
    <actionBinding action="HORN">
        <binding device="KB_MOUSE_DEFAULT" input="KEY_x" axisComponent="+" neutralInput="0" index="1"></binding>
    </actionBinding>
    <actionBinding action="LIGHTS">
        <binding device="KB_MOUSE_DEFAULT" input="KEY_x" axisComponent="+" neutralInput="0" index="1"></binding>
        <binding device="0_046D-FAKE-0000-XFw/c2ltdWxhdGVk" input="BUTTON_2" axisComponent="+" neutralInput="0" index="2"></binding>
    </actionBinding>
    <actionBinding action="STEER_LEFT">
        <binding device="KB_MOUSE_DEFAULT" input="AXIS_1" axisComponent="-" neutralInput="0" index="1"></binding>
    </actionBinding>
    <actionBinding action="STEER_RIGHT">
        <binding device="KB_MOUSE_DEFAULT" input="AXIS_1" axisComponent="+" neutralInput="0" index="1"></binding>
    </actionBinding>
    <actionBinding action="DOUBLE_BOUND">
        <binding device="KB_MOUSE_DEFAULT" input="KEY_q" axisComponent="+" neutralInput="0" index="1"></binding>
        <binding device="KB_MOUSE_DEFAULT" input="KEY_q" axisComponent="+" neutralInput="0" index="2"></binding>
    </actionBinding>
    <actionBinding action="PAD_ONLY">
        <binding device="0_XINPUT_GAMEPAD" input="BUTTON_1" axisComponent="+" neutralInput="0" index="1"></binding>
    </actionBinding>
</inputBinding>
"#;

    #[test]
    fn splice_round_trip_is_byte_identical() {
        let doc = BindingsFile::parse(FIXTURE.to_string());
        assert_eq!(doc.text(), FIXTURE);
        let actions = doc.actions();
        assert_eq!(actions.len(), 7);
        assert_eq!(actions[0].name, "MOVE_FORWARD");
        assert_eq!(actions[0].bindings.len(), 2);
        assert_eq!(actions[0].bindings[0].device, "KB_MOUSE_DEFAULT");
        assert_eq!(actions[0].bindings[0].input, "KEY_w");
        assert_eq!(
            doc.devices(),
            vec![
                "KB_MOUSE_DEFAULT".to_string(),
                "0_XINPUT_GAMEPAD".to_string(),
                "0_046D-FAKE-0000-XFw/c2ltdWxhdGVk".to_string(),
            ]
        );
    }

    #[test]
    fn edit_one_block_leaves_all_other_bytes_identical() {
        let before = parse_actions(FIXTURE);
        let horn = before.iter().find(|a| a.name == "HORN").unwrap();
        let (block_start, block_end) = horn.span;

        let mut doc = BindingsFile::parse(FIXTURE.to_string());
        assert!(doc.set_input("HORN", 0, "KEY_y"));
        let after = doc.text();

        // Everything before and after the HORN block is
        // byte-for-byte what it was.
        assert_eq!(&after[..block_start], &FIXTURE[..block_start]);
        let new_block_end = block_end + "KEY_y".len() - "KEY_x".len();
        assert_eq!(&after[new_block_end..], &FIXTURE[block_end..]);
        // The edit itself landed, the duplicated
        // declaration survived, and no other binding moved.
        assert!(after[block_start..new_block_end].contains("input=\"KEY_y\""));
        assert_eq!(after.matches("<?xml").count(), 2);
        assert!(after.contains("mouseSensitivityScaleX=\"1.000000\""));
        let actions = doc.actions();
        let lights = actions.iter().find(|a| a.name == "LIGHTS").unwrap();
        assert_eq!(lights.bindings[0].input, "KEY_x");
    }

    #[test]
    fn conflict_and_warning_classification_matrix() {
        let doc = BindingsFile::parse(FIXTURE.to_string());
        let actions = doc.actions();
        let kb = analyze(&actions, "KB_MOUSE_DEFAULT");
        let by_input: std::collections::HashMap<&str, &Finding> =
            kb.iter().map(|f| (f.input.as_str(), f)).collect();
        // Same input, same axis, two different actions:
        // a conflict.
        let conflict = by_input["KEY_x"];
        assert_eq!(conflict.kind, FindingKind::Conflict);
        assert_eq!(
            conflict.actions,
            vec!["HORN".to_string(), "LIGHTS".to_string()]
        );
        // Same input across actions only via + against -:
        // the axis-sharing warning.
        let axis = by_input["AXIS_1"];
        assert_eq!(axis.kind, FindingKind::Warning);
        assert_eq!(
            axis.actions,
            vec!["STEER_LEFT".to_string(), "STEER_RIGHT".to_string()]
        );
        // One action holding one input in two slots: a
        // warning, never a conflict.
        let dup = by_input["KEY_q"];
        assert_eq!(dup.kind, FindingKind::Warning);
        assert_eq!(dup.actions, vec!["DOUBLE_BOUND".to_string()]);
        assert_eq!(kb.len(), 3);
        // The gamepad has a clean sheet in this fixture.
        assert!(analyze(&actions, "0_XINPUT_GAMEPAD").is_empty());
    }

    #[test]
    fn clear_and_add_binding_round_out_the_block() {
        let mut doc = BindingsFile::parse(FIXTURE.to_string());
        // Clear PAD_ONLY's only binding position is on
        // the gamepad; clear MOVE_FORWARD's keyboard one.
        assert!(doc.remove_binding("MOVE_FORWARD", 0));
        let actions = doc.actions();
        let forward = actions.iter().find(|a| a.name == "MOVE_FORWARD").unwrap();
        assert_eq!(forward.bindings.len(), 1);
        assert_eq!(forward.bindings[0].device, "0_XINPUT_GAMEPAD");
        // Add: the index follows the action's highest on
        // the device (none left -> 1).
        assert!(doc.add_binding("MOVE_FORWARD", "KB_MOUSE_DEFAULT", "KEY_lshift KEY_w"));
        let actions = doc.actions();
        let forward = actions.iter().find(|a| a.name == "MOVE_FORWARD").unwrap();
        let added = forward
            .bindings
            .iter()
            .find(|b| b.device == "KB_MOUSE_DEFAULT")
            .unwrap();
        assert_eq!(added.input, "KEY_lshift KEY_w");
        assert_eq!(added.index, "1");
        assert_eq!(added.axis_component, "+");
        // The rest of the document still parses whole.
        assert_eq!(actions.len(), 7);
    }

    /// An owners map from (action, mod title) pairs.
    fn owners_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(a, o)| (a.to_string(), o.to_string()))
            .collect()
    }

    fn finding(input: &str, kind: FindingKind, actions: &[&str]) -> Finding {
        Finding {
            input: input.to_string(),
            kind,
            actions: actions.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn tier_classification_matrix() {
        let owners = owners_of(&[
            ("A_ONE", "Mod Alpha"),
            ("A_TWO", "Mod Alpha"),
            ("B_ONE", "Mod Beta"),
            ("C_ONE", "Mod Gamma"),
        ]);
        // Two actions of the SAME mod on one key: Hard.
        assert_eq!(
            classify(
                &finding("KEY_a", FindingKind::Conflict, &["A_ONE", "A_TWO"]),
                &owners
            ),
            Some(FindingTier::Hard)
        );
        // The pair rule does not care what the input is —
        // an axis-compound input with a same-mod pair is
        // Hard all the same.
        assert_eq!(
            classify(
                &finding(
                    "MOUSE_BUTTON_MIDDLE AXIS_X+",
                    FindingKind::Conflict,
                    &["A_ONE", "A_TWO"]
                ),
                &owners
            ),
            Some(FindingTier::Hard)
        );
        // A mod action against a base action: Shared.
        assert_eq!(
            classify(
                &finding("KEY_b", FindingKind::Conflict, &["B_ONE", "BASE_ONE"]),
                &owners
            ),
            Some(FindingTier::Shared)
        );
        // Two mods, one action each: Shared, never Hard.
        assert_eq!(
            classify(
                &finding("KEY_c", FindingKind::Conflict, &["B_ONE", "C_ONE"]),
                &owners
            ),
            Some(FindingTier::Shared)
        );
        // No mod involved at all: Base.
        assert_eq!(
            classify(
                &finding(
                    "KEY_d",
                    FindingKind::Conflict,
                    &["BASE_ONE", "BASE_TWO"]
                ),
                &owners
            ),
            Some(FindingTier::Base)
        );
        // Warnings are not tiered.
        assert_eq!(
            classify(
                &finding("KEY_e", FindingKind::Warning, &["A_ONE", "A_TWO"]),
                &owners
            ),
            None
        );
    }

    /// A second SYNTHETIC fixture, shaped like the
    /// owner's file (duplicated declaration, root attrs,
    /// several devices), carrying exactly one finding of
    /// each tier plus two warnings on the keyboard.
    const TIER_FIXTURE: &str = r#"<?xml version="1.0" encoding="utf-8" standalone="no" ?>
<?xml version="1.0" encoding="utf-8" standalone="no"?>
<inputBinding version="24" bindingVersion="1" mouseSensitivityScaleX="1.000000" mouseSensitivityScaleY="1.000000">
    <actionBinding action="A_ONE">
        <binding device="KB_MOUSE_DEFAULT" input="KEY_a" axisComponent="+" neutralInput="0" index="1"></binding>
    </actionBinding>
    <actionBinding action="A_TWO">
        <binding device="KB_MOUSE_DEFAULT" input="KEY_a" axisComponent="+" neutralInput="0" index="1"></binding>
        <binding device="0_XINPUT_GAMEPAD" input="BUTTON_1" axisComponent="+" neutralInput="0" index="1"></binding>
    </actionBinding>
    <actionBinding action="B_ONE">
        <binding device="KB_MOUSE_DEFAULT" input="KEY_b" axisComponent="+" neutralInput="0" index="1"></binding>
    </actionBinding>
    <actionBinding action="BASE_ONE">
        <binding device="KB_MOUSE_DEFAULT" input="KEY_b" axisComponent="+" neutralInput="0" index="1"></binding>
    </actionBinding>
    <actionBinding action="C_ONE">
        <binding device="KB_MOUSE_DEFAULT" input="KEY_c" axisComponent="+" neutralInput="0" index="1"></binding>
    </actionBinding>
    <actionBinding action="D_ONE">
        <binding device="KB_MOUSE_DEFAULT" input="KEY_c" axisComponent="+" neutralInput="0" index="1"></binding>
    </actionBinding>
    <actionBinding action="BASE_TWO">
        <binding device="KB_MOUSE_DEFAULT" input="KEY_d" axisComponent="+" neutralInput="0" index="1"></binding>
    </actionBinding>
    <actionBinding action="BASE_THREE">
        <binding device="KB_MOUSE_DEFAULT" input="KEY_d" axisComponent="+" neutralInput="0" index="1"></binding>
    </actionBinding>
    <actionBinding action="A_THREE">
        <binding device="KB_MOUSE_DEFAULT" input="AXIS_9" axisComponent="-" neutralInput="0" index="1"></binding>
    </actionBinding>
    <actionBinding action="A_FOUR">
        <binding device="KB_MOUSE_DEFAULT" input="AXIS_9" axisComponent="+" neutralInput="0" index="1"></binding>
    </actionBinding>
    <actionBinding action="A_FIVE">
        <binding device="KB_MOUSE_DEFAULT" input="KEY_e" axisComponent="+" neutralInput="0" index="1"></binding>
        <binding device="KB_MOUSE_DEFAULT" input="KEY_e" axisComponent="+" neutralInput="0" index="2"></binding>
    </actionBinding>
</inputBinding>
"#;

    #[test]
    fn tier_counts_partition_a_mixed_fixture() {
        let owners = owners_of(&[
            ("A_ONE", "Mod Alpha"),
            ("A_TWO", "Mod Alpha"),
            ("A_THREE", "Mod Alpha"),
            ("A_FOUR", "Mod Alpha"),
            ("A_FIVE", "Mod Alpha"),
            ("B_ONE", "Mod Beta"),
            ("C_ONE", "Mod Gamma"),
            ("D_ONE", "Mod Delta"),
        ]);
        let doc = BindingsFile::parse(TIER_FIXTURE.to_string());
        let actions = doc.actions();
        let findings = analyze(&actions, "KB_MOUSE_DEFAULT");
        let counts = tier_counts(&findings, &owners);
        assert_eq!(counts.hard, 1);
        assert_eq!(counts.shared, 2);
        assert_eq!(counts.base, 1);
        assert_eq!(counts.warnings, 2);
        assert_eq!(counts.hard_mods, 1);
        // The partition property: the tiers account for
        // every Conflict finding exactly once.
        let conflicts = findings
            .iter()
            .filter(|f| f.kind == FindingKind::Conflict)
            .count();
        assert_eq!(counts.hard + counts.shared + counts.base, conflicts);
        // The gamepad has no findings in this fixture,
        // so its tiers are an empty partition.
        let pad = analyze(&actions, "0_XINPUT_GAMEPAD");
        assert_eq!(tier_counts(&pad, &owners), TierCounts::default());
        // And the banner over the keyboard counts.
        let (line1, line2) = banner_lines(&counts);
        assert_eq!(
            line1,
            "1 Hard Keybinding Conflicts Detected (Affecting 1 Mods)"
        );
        assert_eq!(
            line2.as_deref(),
            Some("2 Shared Keys Involving Mods · 1 Base Game Overlaps · 2 Warnings")
        );
    }

    #[test]
    fn banner_lines_calm_variant_and_empty_second_line() {
        // No Hard findings: the calm headline, and the
        // second line still reports what is there.
        let (line1, line2) = banner_lines(&TierCounts {
            hard: 0,
            shared: 1,
            base: 0,
            warnings: 0,
            hard_mods: 0,
        });
        assert_eq!(line1, "No hard keybinding conflicts on this device.");
        assert_eq!(
            line2.as_deref(),
            Some("1 Shared Keys Involving Mods · 0 Base Game Overlaps · 0 Warnings")
        );
        // A completely clean device: no second line.
        let (line1, line2) = banner_lines(&TierCounts::default());
        assert_eq!(line1, "No hard keybinding conflicts on this device.");
        assert_eq!(line2, None);
    }

    #[test]
    fn auto_resolve_clears_only_hard_same_mod_pairs() {
        let group: HashSet<String> = ["HORN".to_string(), "LIGHTS".to_string()]
            .into_iter()
            .collect();
        // HORN and LIGHTS belong to the SAME mod: KEY_x
        // is a Hard finding and the group is its owner
        // group. The first in file order (HORN) keeps it,
        // the pair's later holder (LIGHTS) is cleared.
        let owners = owners_of(&[("HORN", "Horn Mod"), ("LIGHTS", "Horn Mod")]);
        let mut doc = BindingsFile::parse(FIXTURE.to_string());
        let cleared = doc.auto_resolve("KB_MOUSE_DEFAULT", &group, &owners);
        assert_eq!(cleared, 1);
        let actions = doc.actions();
        let lights = actions.iter().find(|a| a.name == "LIGHTS").unwrap();
        assert!(lights
            .bindings
            .iter()
            .all(|b| !(b.device == "KB_MOUSE_DEFAULT" && b.input == "KEY_x")));
        // The wheel binding on LIGHTS was not touched.
        assert_eq!(lights.bindings.len(), 1);
        let horn = actions.iter().find(|a| a.name == "HORN").unwrap();
        assert_eq!(horn.bindings.len(), 1);
        // Warnings are never auto-resolved.
        let steer = actions.iter().find(|a| a.name == "STEER_LEFT").unwrap();
        assert_eq!(steer.bindings.len(), 1);

        // The same two actions under DIFFERENT mods make
        // KEY_x a Shared finding — nothing is cleared.
        let owners = owners_of(&[("HORN", "Horn Mod"), ("LIGHTS", "Lights Mod")]);
        let mut doc = BindingsFile::parse(FIXTURE.to_string());
        assert_eq!(doc.auto_resolve("KB_MOUSE_DEFAULT", &group, &owners), 0);

        // No owners at all: KEY_x is a Base finding —
        // nothing is cleared either.
        let mut doc = BindingsFile::parse(FIXTURE.to_string());
        assert_eq!(
            doc.auto_resolve("KB_MOUSE_DEFAULT", &group, &HashMap::new()),
            0
        );

        // The pair's owner group is some OTHER group: a
        // pass over a group that does not hold the whole
        // pair (here: the Base Game group's actions)
        // leaves the Hard finding alone.
        let owners = owners_of(&[("HORN", "Horn Mod"), ("LIGHTS", "Horn Mod")]);
        let other_group: HashSet<String> =
            ["MOVE_FORWARD".to_string()].into_iter().collect();
        let mut doc = BindingsFile::parse(FIXTURE.to_string());
        assert_eq!(
            doc.auto_resolve("KB_MOUSE_DEFAULT", &other_group, &owners),
            0
        );
    }

    #[test]
    fn labels_and_humanizing() {
        assert_eq!(device_label("KB_MOUSE_DEFAULT"), "Keyboard & Mouse");
        assert_eq!(device_label("0_XINPUT_GAMEPAD"), "Gamepad");
        assert_eq!(
            device_label("0_046D-FAKE-0000-XFw/c2ltdWxhdGVk"),
            "Wheel / Other device"
        );
        assert_eq!(humanize_action("ACTIVATE_HANDTOOL"), "ACTIVATE HANDTOOL");
        assert_eq!(humanize_input("MOUSE_BUTTON_LEFT"), "Mouse Button Left");
        assert_eq!(humanize_input("KEY_lshift KEY_n"), "Lshift + N");
        assert_eq!(humanize_input("KEY_space"), "Space");
    }

    #[test]
    fn capture_mapping_covers_the_files_vocabulary() {
        use iced::keyboard::key::Named;
        use iced::keyboard::{Key, Location, Modifiers};
        let none = Modifiers::empty();
        assert_eq!(
            fs_input_for_key(&Key::Character("r".into()), Location::Standard, none),
            Some("KEY_r".to_string())
        );
        assert_eq!(
            fs_input_for_key(
                &Key::Character("r".into()),
                Location::Standard,
                Modifiers::CTRL
            ),
            Some("KEY_lctrl KEY_r".to_string())
        );
        assert_eq!(
            fs_input_for_key(
                &Key::Character("n".into()),
                Location::Standard,
                Modifiers::SHIFT
            ),
            Some("KEY_lshift KEY_n".to_string())
        );
        assert_eq!(
            fs_input_for_key(&Key::Named(Named::F5), Location::Standard, none),
            Some("KEY_f5".to_string())
        );
        assert_eq!(
            fs_input_for_key(&Key::Named(Named::Shift), Location::Right, none),
            Some("KEY_rshift".to_string())
        );
        assert_eq!(
            fs_input_for_key(&Key::Character(",".into()), Location::Standard, none),
            Some("KEY_comma".to_string())
        );
        assert_eq!(
            fs_input_for_key(&Key::Character("3".into()), Location::Numpad, none),
            Some("KEY_KP_3".to_string())
        );
        // Escape and unmapped keys never bind.
        assert_eq!(
            fs_input_for_key(&Key::Named(Named::Escape), Location::Standard, none),
            None
        );
        assert_eq!(
            fs_input_for_key(&Key::Named(Named::CapsLock), Location::Standard, none),
            None
        );
    }

    /// Ownership WITH states over the TIER_FIXTURE:
    /// Mod Alpha active, Mod Beta archived, Mod Gamma
    /// active, Mod Delta archived; the BASE_* actions are
    /// unclaimed.
    fn state_owners() -> HashMap<String, ActionOwner> {
        let mk = |title: &str, state: KbOwnerState| ActionOwner {
            title: title.to_string(),
            category: None,
            state,
        };
        [
            ("A_ONE", mk("Mod Alpha", KbOwnerState::Active)),
            ("A_TWO", mk("Mod Alpha", KbOwnerState::Active)),
            ("A_THREE", mk("Mod Alpha", KbOwnerState::Active)),
            ("A_FOUR", mk("Mod Alpha", KbOwnerState::Active)),
            ("A_FIVE", mk("Mod Alpha", KbOwnerState::Active)),
            ("B_ONE", mk("Mod Beta", KbOwnerState::Archived)),
            ("C_ONE", mk("Mod Gamma", KbOwnerState::Active)),
            ("D_ONE", mk("Mod Delta", KbOwnerState::Archived)),
        ]
        .into_iter()
        .map(|(name, owner)| (name.to_string(), owner))
        .collect()
    }

    #[test]
    fn active_only_filter_recomputes_findings_and_banner() {
        let owners = state_owners();
        let doc = BindingsFile::parse(TIER_FIXTURE.to_string());
        let all = doc.actions();

        // Toggle off: the archived mods' actions are out
        // of the analysis set entirely — not reclassified,
        // not just hidden.
        let visible = visible_actions(&all, &owners, false);
        let names: Vec<&str> =
            visible.iter().map(|a| a.name.as_str()).collect();
        assert!(!names.contains(&"B_ONE"));
        assert!(!names.contains(&"D_ONE"));
        assert!(names.contains(&"A_ONE"));
        assert!(names.contains(&"BASE_ONE"));
        assert_eq!(visible.len(), all.len() - 2);
        // The document itself is untouched by the view.
        assert_eq!(doc.text(), TIER_FIXTURE);

        // Findings over the visible set: KEY_b keeps only
        // its base holder (no finding at all), KEY_c only
        // its active holder; the Hard pair, the Base pair
        // and both warnings survive exactly as they were.
        let findings = analyze(&visible, "KB_MOUSE_DEFAULT");
        let counts = tier_counts(&findings, &owner_titles(&owners));
        assert_eq!(counts.hard, 1);
        assert_eq!(counts.shared, 0);
        assert_eq!(counts.base, 1);
        assert_eq!(counts.warnings, 2);
        assert_eq!(counts.hard_mods, 1);
        // The partition still closes over what remains.
        let conflicts = findings
            .iter()
            .filter(|f| f.kind == FindingKind::Conflict)
            .count();
        assert_eq!(counts.hard + counts.shared + counts.base, conflicts);
        let (_, line2) = banner_lines(&counts);
        assert_eq!(
            line2.as_deref(),
            Some("0 Shared Keys Involving Mods · 1 Base Game Overlaps · 2 Warnings")
        );
    }

    #[test]
    fn show_archived_on_matches_the_unfiltered_page() {
        let owners = state_owners();
        let doc = BindingsFile::parse(TIER_FIXTURE.to_string());
        let all = doc.actions();
        // Parity with v1.6.2: with the toggle on the
        // visible set IS the document's action set, and
        // the counts are the ones the tier partition test
        // pins for this fixture.
        let visible = visible_actions(&all, &owners, true);
        assert_eq!(visible, all);
        let findings = analyze(&visible, "KB_MOUSE_DEFAULT");
        let counts = tier_counts(&findings, &owner_titles(&owners));
        assert_eq!(counts.hard, 1);
        assert_eq!(counts.shared, 2);
        assert_eq!(counts.base, 1);
        assert_eq!(counts.warnings, 2);
        assert_eq!(counts.hard_mods, 1);
    }

    #[test]
    fn purge_eligibility_is_archived_state_only() {
        let owners = state_owners();
        // Archived-owned: eligible.
        assert!(is_purge_eligible("B_ONE", &owners));
        assert!(is_purge_eligible("D_ONE", &owners));
        // Active-owned: never.
        assert!(!is_purge_eligible("A_ONE", &owners));
        assert!(!is_purge_eligible("C_ONE", &owners));
        // Unknown-owner (incl. base game): never.
        assert!(!is_purge_eligible("BASE_ONE", &owners));
        assert!(!is_purge_eligible("NOT_IN_THE_FILE", &owners));
        // Other-state owned: never.
        let mut owners = owners;
        owners.insert(
            "X_ONE".to_string(),
            ActionOwner {
                title: "Mod Xray".to_string(),
                category: None,
                state: KbOwnerState::Other,
            },
        );
        assert!(!is_purge_eligible("X_ONE", &owners));
        // With no archived owners at all, a purge stages
        // nothing and changes no byte.
        let no_archived: HashMap<String, ActionOwner> = owners
            .into_iter()
            .filter(|(_, o)| o.state != KbOwnerState::Archived)
            .collect();
        let mut doc = BindingsFile::parse(TIER_FIXTURE.to_string());
        assert_eq!(doc.stage_purge(&no_archived), PurgeSummary::default());
        assert_eq!(doc.text(), TIER_FIXTURE);
    }

    #[test]
    fn purge_takes_only_archived_owned_blocks_byte_clean() {
        let owners = state_owners();
        let mut doc = BindingsFile::parse(TIER_FIXTURE.to_string());
        // The count the first press reports in place.
        assert_eq!(
            purge_summary(&doc.actions(), &owners),
            PurgeSummary {
                actions: 2,
                mods: 2
            }
        );
        assert_eq!(
            purge_candidates(&doc.actions(), &owners),
            vec!["B_ONE".to_string(), "D_ONE".to_string()]
        );

        let staged = doc.stage_purge(&owners);
        assert_eq!(
            staged,
            PurgeSummary {
                actions: 2,
                mods: 2
            }
        );

        // Byte-exact expectation: the fixture with the
        // B_ONE and D_ONE blocks (each with its line) cut,
        // and every other byte where it was.
        let b_block = "    <actionBinding action=\"B_ONE\">\n        <binding device=\"KB_MOUSE_DEFAULT\" input=\"KEY_b\" axisComponent=\"+\" neutralInput=\"0\" index=\"1\"></binding>\n    </actionBinding>\n";
        let d_block = "    <actionBinding action=\"D_ONE\">\n        <binding device=\"KB_MOUSE_DEFAULT\" input=\"KEY_c\" axisComponent=\"+\" neutralInput=\"0\" index=\"1\"></binding>\n    </actionBinding>\n";
        let expected = TIER_FIXTURE.replace(b_block, "").replace(d_block, "");
        assert_eq!(doc.text(), expected);

        // Active-owned, base and unknown actions survive;
        // the purged ones are out of the document.
        let names: Vec<String> =
            doc.actions().into_iter().map(|a| a.name).collect();
        for kept in [
            "A_ONE",
            "A_TWO",
            "A_THREE",
            "A_FOUR",
            "A_FIVE",
            "C_ONE",
            "BASE_ONE",
            "BASE_TWO",
            "BASE_THREE",
        ] {
            assert!(
                names.iter().any(|n| n == kept),
                "{kept} must survive the purge"
            );
        }
        assert!(!names.iter().any(|n| n == "B_ONE"));
        assert!(!names.iter().any(|n| n == "D_ONE"));

        // A second purge on the staged document finds
        // nothing left to take and changes nothing.
        assert_eq!(doc.stage_purge(&owners), PurgeSummary::default());
        assert_eq!(doc.text(), expected);
    }

    #[test]
    fn remove_action_block_cuts_one_block_and_reports_misses() {
        let mut doc = BindingsFile::parse(FIXTURE.to_string());
        assert!(doc.remove_action_block("HORN"));
        let horn_block = "    <actionBinding action=\"HORN\">\n        <binding device=\"KB_MOUSE_DEFAULT\" input=\"KEY_x\" axisComponent=\"+\" neutralInput=\"0\" index=\"1\"></binding>\n    </actionBinding>\n";
        assert_eq!(doc.text(), FIXTURE.replace(horn_block, ""));
        assert_eq!(doc.actions().len(), 6);
        // Already gone, or never there: a clean miss.
        assert!(!doc.remove_action_block("HORN"));
        assert!(!doc.remove_action_block("NEVER_THERE"));
    }
}
