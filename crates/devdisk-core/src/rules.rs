//! Loading and validating rules.json (ecosystems, regenerable folders, frameworks).

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const BUILTIN_JSON: &str = include_str!("../rules.json");

#[derive(Debug, thiserror::Error)]
pub enum RulesError {
    #[error("invalid rules json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid rules: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ManifestFormat {
    Npm,
    Composer,
    Text,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    /// None for `Text`: every marker file is searched instead.
    #[serde(default)]
    pub file: Option<String>,
    pub format: ManifestFormat,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Lockfile {
    pub file: String,
    pub manager: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Ecosystem {
    pub id: String,
    pub name: String,
    pub markers: Vec<String>,
    #[serde(default)]
    pub manifest: Option<Manifest>,
    #[serde(default)]
    pub lockfiles: Vec<Lockfile>,
    #[serde(default)]
    pub regenerable: Vec<String>,
    /// Regenerable folders whose reinstall depends on a lockfile (a subset of `regenerable`).
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub reinstall: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Framework {
    pub label: String,
    pub ecosystem: String,
    pub deps: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Rules {
    #[allow(dead_code)]
    version: u32,
    #[serde(default)]
    skip_dirs: Vec<String>,
    #[serde(default)]
    ignore_paths: Vec<String>,
    ecosystems: Vec<Ecosystem>,
    #[serde(default)]
    frameworks: Vec<Framework>,
}

fn has_separator(name: &str) -> bool {
    name.contains('/') || name.contains('\\')
}

/// Replaces each `%VAR%` using `lookup`. Returns None when a variable is unset,
/// so a path built on a missing variable is dropped instead of pointing at the wrong place.
fn expand_vars(template: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                out.push_str(&lookup(&after[..end])?);
                rest = &after[end + 1..];
            }
            None => {
                // Lone '%' is literal text.
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Some(out)
}

impl Rules {
    /// The embedded rules.json. A unit test guarantees it parses.
    pub fn builtin() -> Rules {
        Rules::from_json(BUILTIN_JSON).expect("embedded rules.json is valid")
    }

    /// Parses and validates the rules.
    pub fn from_json(json: &str) -> Result<Rules, RulesError> {
        let rules: Rules = serde_json::from_str(json)?;
        let invalid = |msg: String| Err(RulesError::Invalid(msg));

        let mut seen = HashSet::new();
        for eco in &rules.ecosystems {
            if !seen.insert(eco.id.as_str()) {
                return invalid(format!("duplicate ecosystem id `{}`", eco.id));
            }
            if eco.markers.is_empty() {
                return invalid(format!("ecosystem `{}` has no markers", eco.id));
            }
            if let Some(bad) = eco
                .dependencies
                .iter()
                .find(|d| !eco.regenerable.iter().any(|r| r.eq_ignore_ascii_case(d)))
            {
                return invalid(format!(
                    "dependency `{bad}` in `{}` is not in its regenerable list",
                    eco.id
                ));
            }
            if let Some(bad) = eco.regenerable.iter().find(|n| has_separator(n)) {
                return invalid(format!(
                    "regenerable name `{bad}` in `{}` contains a path separator",
                    eco.id
                ));
            }
        }
        if let Some(bad) = rules.skip_dirs.iter().find(|n| has_separator(n)) {
            return invalid(format!("skip_dirs name `{bad}` contains a path separator"));
        }
        for fw in &rules.frameworks {
            if !seen.contains(fw.ecosystem.as_str()) {
                return invalid(format!(
                    "framework `{}` references unknown ecosystem `{}`",
                    fw.label, fw.ecosystem
                ));
            }
        }
        Ok(rules)
    }

    pub fn ecosystems(&self) -> &[Ecosystem] {
        &self.ecosystems
    }

    pub fn frameworks(&self) -> &[Framework] {
        &self.frameworks
    }

    pub fn ecosystem(&self, id: &str) -> Option<&Ecosystem> {
        self.ecosystems.iter().find(|e| e.id == id)
    }

    /// True when `name` is a lockfile-dependent dependency folder of any of the ecosystems.
    pub fn is_dependency_dir(&self, ecosystem_ids: &[String], name: &str) -> bool {
        ecosystem_ids
            .iter()
            .filter_map(|id| self.ecosystem(id))
            .any(|e| e.dependencies.iter().any(|d| d.eq_ignore_ascii_case(name)))
    }

    /// Folder names where project detection never descends, anywhere. Case-insensitive.
    pub fn is_skip_dir(&self, name: &str) -> bool {
        self.skip_dirs.iter().any(|d| d.eq_ignore_ascii_case(name))
    }

    /// `ignore_paths` with %VAR% expanded from the environment. Entries whose variable is unset are dropped.
    pub fn ignore_paths(&self) -> Vec<PathBuf> {
        self.ignore_paths_with(&|var| std::env::var(var).ok().filter(|v| !v.is_empty()))
    }

    fn ignore_paths_with(&self, lookup: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
        self.ignore_paths
            .iter()
            .filter_map(|p| expand_vars(p, lookup))
            .map(PathBuf::from)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"{
        "version": 1,
        "ecosystems": [
            {"id": "rust", "name": "Rust", "markers": ["Cargo.toml"], "regenerable": ["target"]}
        ],
        "frameworks": []
    }"#;

    #[test]
    fn builtin_parses() {
        let rules = Rules::builtin();
        let ids: Vec<_> = rules.ecosystems().iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["node", "php", "rust", "python", "maven", "gradle"]);
        assert!(!rules.frameworks().is_empty());
        assert!(rules.ecosystem("node").is_some());
        assert!(rules.ecosystem("nope").is_none());
    }

    #[test]
    fn minimal_parses() {
        assert!(Rules::from_json(MINIMAL).is_ok());
    }

    #[test]
    fn rejects_bad_json() {
        assert!(matches!(Rules::from_json("{"), Err(RulesError::Json(_))));
    }

    #[test]
    fn rejects_duplicate_ecosystem_id() {
        let json = r#"{"version":1,"ecosystems":[
            {"id":"a","name":"A","markers":["a"]},
            {"id":"a","name":"B","markers":["b"]}]}"#;
        assert!(matches!(
            Rules::from_json(json),
            Err(RulesError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_unknown_framework_ecosystem() {
        let json = r#"{"version":1,"ecosystems":[{"id":"a","name":"A","markers":["a"]}],
            "frameworks":[{"label":"X","ecosystem":"zzz","deps":["x"]}]}"#;
        assert!(matches!(
            Rules::from_json(json),
            Err(RulesError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_separator_in_regenerable_and_skip_dirs() {
        let json = r#"{"version":1,"ecosystems":[
            {"id":"a","name":"A","markers":["a"],"regenerable":["x/y"]}]}"#;
        assert!(matches!(
            Rules::from_json(json),
            Err(RulesError::Invalid(_))
        ));
        let json = r#"{"version":1,"skip_dirs":["a\\b"],"ecosystems":[
            {"id":"a","name":"A","markers":["a"]}]}"#;
        assert!(matches!(
            Rules::from_json(json),
            Err(RulesError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_empty_markers() {
        let json = r#"{"version":1,"ecosystems":[{"id":"a","name":"A","markers":[]}]}"#;
        assert!(matches!(
            Rules::from_json(json),
            Err(RulesError::Invalid(_))
        ));
    }

    #[test]
    fn skip_dir_is_case_insensitive() {
        let rules = Rules::builtin();
        assert!(rules.is_skip_dir("node_modules"));
        assert!(rules.is_skip_dir(".GIT"));
        assert!(!rules.is_skip_dir("src"));
    }

    #[test]
    fn ignore_paths_expand_vars_and_drop_unset() {
        let json = r#"{"version":1,
            "ignore_paths":["%ROOT%\\sub","%MISSING%\\x","plain","%ProgramFiles(x86)%"],
            "ecosystems":[{"id":"a","name":"A","markers":["a"]}]}"#;
        let rules = Rules::from_json(json).unwrap();
        let lookup = |name: &str| match name {
            "ROOT" => Some("C:\\r".to_string()),
            "ProgramFiles(x86)" => Some("C:\\pf86".to_string()),
            _ => None,
        };
        let paths = rules.ignore_paths_with(&lookup);
        assert_eq!(
            paths,
            [
                PathBuf::from("C:\\r\\sub"),
                PathBuf::from("plain"),
                PathBuf::from("C:\\pf86")
            ]
        );
    }

    #[test]
    fn ignore_paths_reads_real_environment() {
        // PATH is set on every platform tests run on.
        let json = r#"{"version":1,"ignore_paths":["%PATH%","%DEVDISK_SURELY_UNSET_VAR%"],
            "ecosystems":[{"id":"a","name":"A","markers":["a"]}]}"#;
        let paths = Rules::from_json(json).unwrap().ignore_paths();
        assert_eq!(paths, [PathBuf::from(std::env::var("PATH").unwrap())]);
    }

    #[test]
    fn dependency_dirs_are_regenerable_and_looked_up_case_insensitively() {
        let r = Rules::builtin();
        let ids = vec!["node".to_string()];
        assert!(r.is_dependency_dir(&ids, "Node_Modules"));
        assert!(!r.is_dependency_dir(&ids, "dist"));
        assert!(!r.is_dependency_dir(&ids, "vendor"));
        assert!(r.is_dependency_dir(&["php".to_string()], "vendor"));
        assert!(r.is_dependency_dir(&["python".to_string()], ".venv"));
        assert!(!r.is_dependency_dir(&["rust".to_string()], "target"));
        assert!(!r.is_dependency_dir(&[], "node_modules"));
    }

    #[test]
    fn dependency_must_be_listed_as_regenerable() {
        let json = r#"{"version":1,"ecosystems":[{"id":"x","name":"X","markers":["m"],
            "regenerable":["a"],"dependencies":["b"]}]}"#;
        assert!(matches!(
            Rules::from_json(json),
            Err(RulesError::Invalid(_))
        ));
    }
}
