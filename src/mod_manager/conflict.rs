use super::types::Mod;

#[derive(Debug, Clone)]
pub struct Conflict {
    pub mod1: String,
    pub mod2: String,
    pub conflict_type: ConflictType,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConflictType {
    OverlappingFiles,
    DuplicateMod,
    MissingDependency,
    VersionMismatch,
    Incompatible,
}

pub struct ConflictDetector;

impl ConflictDetector {
    /// The scan/load pipeline's entry point: conflict
    /// detection runs only when the `check_conflicts`
    /// setting is on. Off means the computation is skipped
    /// entirely and the honest answer is no conflicts.
    pub fn detect_conflicts_if_enabled(mods: &[Mod], enabled: bool) -> Vec<Conflict> {
        if enabled {
            Self::detect_conflicts(mods)
        } else {
            Vec::new()
        }
    }

    pub fn detect_conflicts(mods: &[Mod]) -> Vec<Conflict> {
        let mut conflicts = Vec::new();

        // Check for duplicate mods (same name)
        Self::check_duplicates(mods, &mut conflicts);
        
        // Check for file overlaps
        Self::check_overlaps(mods, &mut conflicts);
        
        // Check for missing dependencies
        Self::check_dependencies(mods, &mut conflicts);
        
        // Check for version mismatches
        Self::check_versions(mods, &mut conflicts);

        conflicts
    }

    fn check_duplicates(mods: &[Mod], conflicts: &mut Vec<Conflict>) {
        // Identity is the exact file name (see Mod::file_name):
        // two entries sharing it are copies of one file; mods
        // that merely share a TITLE are different mods.
        let mut seen: std::collections::HashMap<String, &Mod> = std::collections::HashMap::new();

        for mod1 in mods {
            let key = mod1.file_name();
            if let Some(existing) = seen.get(&key) {
                conflicts.push(Conflict {
                    mod1: existing.file_name(),
                    mod2: key.clone(),
                    conflict_type: ConflictType::DuplicateMod,
                    description: format!("Duplicate mod: {}", key),
                });
            } else {
                seen.insert(key, mod1);
            }
        }
    }

    fn check_overlaps(mods: &[Mod], conflicts: &mut Vec<Conflict>) {
        for (i, mod1) in mods.iter().enumerate() {
            for mod2 in mods.iter().skip(i + 1) {
                if mod1.path == mod2.path {
                    conflicts.push(Conflict {
                        mod1: mod1.name.clone(),
                        mod2: mod2.name.clone(),
                        conflict_type: ConflictType::OverlappingFiles,
                        description: "Mods share the same path".to_string(),
                    });
                }
            }
        }
    }

    fn check_dependencies(mods: &[Mod], conflicts: &mut Vec<Conflict>) {
        for mod1 in mods {
            for dep in &mod1.dependencies {
                if !mods.iter().any(|m| &m.name == dep) {
                    conflicts.push(Conflict {
                        mod1: mod1.name.clone(),
                        mod2: dep.clone(),
                        conflict_type: ConflictType::MissingDependency,
                        description: format!("Missing dependency: {}", dep),
                    });
                }
            }
        }
    }

    fn check_versions(mods: &[Mod], conflicts: &mut Vec<Conflict>) {
        // Same exact file name, different versions: copies of
        // one file that have diverged — a genuine conflict.
        // (Unordered pairs only; the old loop also emitted the
        // mirror image of every pair.)
        for (i, mod1) in mods.iter().enumerate() {
            for mod2 in mods.iter().skip(i + 1) {
                if mod1.file_name() == mod2.file_name() && mod1.version != mod2.version {
                    conflicts.push(Conflict {
                        mod1: mod1.file_name(),
                        mod2: mod2.file_name(),
                        conflict_type: ConflictType::VersionMismatch,
                        description: format!("Different versions: {} vs {}", mod1.version, mod2.version),
                    });
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// One folder mod named SameMod under `parent`: two of
    /// these, one per parent, are exact-name duplicates.
    fn fixture_mod(parent: &std::path::Path) -> Mod {
        let dir = parent.join("SameMod");
        std::fs::create_dir_all(&dir).unwrap();
        Mod::new(dir).unwrap()
    }

    #[test]
    fn duplicate_conflict_computes_only_when_enabled() {
        let base = std::env::temp_dir().join(format!(
            "conflict-gate-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let mods = vec![
            fixture_mod(&base.join("a")),
            fixture_mod(&base.join("b")),
        ];

        let on = ConflictDetector::detect_conflicts_if_enabled(&mods, true);
        assert!(
            on.iter()
                .any(|c| c.conflict_type == ConflictType::DuplicateMod),
            "the duplicate fixture must conflict when checking is on: {:?}",
            on.iter().map(|c| &c.description).collect::<Vec<_>>()
        );

        let off = ConflictDetector::detect_conflicts_if_enabled(&mods, false);
        assert!(
            off.is_empty(),
            "checking off must compute no conflicts at all"
        );

        std::fs::remove_dir_all(&base).ok();
    }
}
