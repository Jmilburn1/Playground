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
        let mut seen: std::collections::HashMap<String, &Mod> = std::collections::HashMap::new();
        
        for mod1 in mods {
            if let Some(existing) = seen.get(&mod1.name) {
                conflicts.push(Conflict {
                    mod1: existing.name.clone(),
                    mod2: mod1.name.clone(),
                    conflict_type: ConflictType::DuplicateMod,
                    description: format!("Duplicate mod: {}", mod1.name),
                });
            } else {
                seen.insert(mod1.name.clone(), mod1);
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
        for mod1 in mods {
            for mod2 in mods {
                if mod1.name == mod2.name && mod1.version != mod2.version {
                    conflicts.push(Conflict {
                        mod1: mod1.name.clone(),
                        mod2: mod2.name.clone(),
                        conflict_type: ConflictType::VersionMismatch,
                        description: format!("Different versions: {} vs {}", mod1.version, mod2.version),
                    });
                }
            }
        }
    }
}