//! Recognizing a single folder as a project root and labeling its framework.

use std::path::{Path, PathBuf};

use crate::rules::{Ecosystem, ManifestFormat, Rules};

pub struct ChildEntry<'a> {
    pub name: &'a str,
    pub is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DetectedProject {
    pub root: PathBuf,
    /// Matched ecosystem ids, in rules order.
    pub ecosystems: Vec<String>,
    /// Matched framework label, if any.
    pub framework: Option<String>,
    /// Framework label, or else the first ecosystem's `name` ("Rust", "Node.js").
    pub label: String,
    /// Version of the framework's first dep, prefix chars like ^ ~ >= = v stripped. None for text manifests.
    pub version: Option<String>,
    /// Manager of the first lockfile found, in rules order across matched ecosystems.
    pub package_manager: Option<String>,
    pub has_lockfile: bool,
    /// Regenerable folder names that exist as child *dirs*, union over matched ecosystems, no duplicates.
    pub regenerable: Vec<String>,
    /// reinstall[package_manager] from the ecosystem that owns that lockfile.
    pub reinstall_command: Option<String>,
}

impl DetectedProject {
    /// "React project: my-shop" → "{label} project: {folder name}"
    pub fn display_name(&self) -> String {
        let folder = self
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.display().to_string());
        format!("{} project: {folder}", self.label)
    }
}

/// What a manifest tells us about dependencies.
enum Deps {
    /// (package name, version spec) pairs from a structured manifest.
    Named(Vec<(String, String)>),
    /// Lowercased text of every marker file, for ecosystems without a parseable manifest.
    Text(String),
}

impl Deps {
    /// Some(version) if `dep` is present; the inner Option is None when no version is known.
    fn find(&self, dep: &str) -> Option<Option<String>> {
        match self {
            Deps::Named(list) => {
                let hit = match dep.strip_suffix('*') {
                    Some(prefix) if prefix.ends_with('/') => list
                        .iter()
                        .find(|(name, _)| starts_with_ignore_case(name, prefix)),
                    _ => list.iter().find(|(name, _)| name.eq_ignore_ascii_case(dep)),
                }?;
                Some(clean_version(&hit.1))
            }
            Deps::Text(text) => text.contains(&dep.to_ascii_lowercase()).then_some(None),
        }
    }
}

fn starts_with_ignore_case(name: &str, prefix: &str) -> bool {
    name.len() >= prefix.len()
        && name.is_char_boundary(prefix.len())
        && name[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn clean_version(raw: &str) -> Option<String> {
    let v = raw.trim_start_matches(['^', '~', '>', '<', '=', 'v', 'V', ' ']);
    (!v.is_empty()).then(|| v.to_string())
}

fn find_child<'a>(
    children: &'a [ChildEntry<'a>],
    name: &str,
    want_dir: bool,
) -> Option<&'a ChildEntry<'a>> {
    children
        .iter()
        .find(|c| c.is_dir == want_dir && c.name.eq_ignore_ascii_case(name))
}

fn read_deps(dir: &Path, eco: &Ecosystem, children: &[ChildEntry]) -> Option<Deps> {
    let manifest = eco.manifest.as_ref()?;
    match manifest.format {
        ManifestFormat::Text => {
            let mut text = String::new();
            for marker in &eco.markers {
                if let Some(child) = find_child(children, marker, false)
                    && let Ok(bytes) = std::fs::read(dir.join(child.name))
                {
                    text.push_str(&String::from_utf8_lossy(&bytes).to_ascii_lowercase());
                    text.push('\n');
                }
            }
            Some(Deps::Text(text))
        }
        ManifestFormat::Npm | ManifestFormat::Composer => {
            let sections: &[&str] = if manifest.format == ManifestFormat::Npm {
                &["dependencies", "devDependencies"]
            } else {
                &["require", "require-dev"]
            };
            let file = manifest.file.as_deref()?;
            let child = find_child(children, file, false)?;
            let bytes = std::fs::read(dir.join(child.name)).ok()?;
            let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            let mut list = Vec::new();
            for section in sections {
                if let Some(map) = json.get(section).and_then(|v| v.as_object()) {
                    for (name, ver) in map {
                        list.push((name.clone(), ver.as_str().unwrap_or("").to_string()));
                    }
                }
            }
            Some(Deps::Named(list))
        }
    }
}

/// First framework rule for `eco` whose deps all match, with the version of its first dep.
fn match_framework(
    rules: &Rules,
    eco: &Ecosystem,
    deps: &Deps,
) -> Option<(String, Option<String>)> {
    rules
        .frameworks()
        .iter()
        .filter(|fw| fw.ecosystem == eco.id)
        .find_map(|fw| {
            let mut first_version = None;
            for (i, dep) in fw.deps.iter().enumerate() {
                let version = deps.find(dep)?;
                if i == 0 {
                    first_version = version;
                }
            }
            Some((fw.label.clone(), first_version))
        })
}

/// Some(..) if any marker *file* is among `children`. Reads manifests from disk under `dir`.
/// An unreadable or invalid manifest is not an error: the project is still reported, with no framework.
pub fn detect_dir(dir: &Path, children: &[ChildEntry], rules: &Rules) -> Option<DetectedProject> {
    let matched: Vec<&Ecosystem> = rules
        .ecosystems()
        .iter()
        .filter(|eco| {
            eco.markers
                .iter()
                .any(|m| find_child(children, m, false).is_some())
        })
        .collect();
    let first = matched.first()?;

    let mut framework = None;
    let mut version = None;
    let mut package_manager = None;
    let mut reinstall_command = None;
    let mut regenerable: Vec<String> = Vec::new();

    for eco in &matched {
        if framework.is_none()
            && let Some(deps) = read_deps(dir, eco, children)
            && let Some((label, ver)) = match_framework(rules, eco, &deps)
        {
            framework = Some(label);
            version = ver;
        }
        if package_manager.is_none()
            && let Some(lock) = eco
                .lockfiles
                .iter()
                .find(|l| find_child(children, &l.file, false).is_some())
        {
            package_manager = Some(lock.manager.clone());
            reinstall_command = eco.reinstall.get(&lock.manager).cloned();
        }
        for name in &eco.regenerable {
            if find_child(children, name, true).is_some() && !regenerable.contains(name) {
                regenerable.push(name.clone());
            }
        }
    }

    Some(DetectedProject {
        root: dir.to_path_buf(),
        ecosystems: matched.iter().map(|e| e.id.clone()).collect(),
        label: framework.clone().unwrap_or_else(|| first.name.clone()),
        framework,
        version,
        has_lockfile: package_manager.is_some(),
        package_manager,
        regenerable,
        reinstall_command,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Creates `files` (name, content) and `dirs` under a temp dir and detects it.
    fn detect(files: &[(&str, &str)], dirs: &[&str]) -> Option<DetectedProject> {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("my-shop");
        fs::create_dir(&root).unwrap();
        for (name, content) in files {
            fs::write(root.join(name), content).unwrap();
        }
        for d in dirs {
            fs::create_dir(root.join(d)).unwrap();
        }
        let names: Vec<(String, bool)> = fs::read_dir(&root)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().to_string_lossy().into_owned(),
                    e.file_type().unwrap().is_dir(),
                )
            })
            .collect();
        let children: Vec<ChildEntry> = names
            .iter()
            .map(|(n, d)| ChildEntry {
                name: n,
                is_dir: *d,
            })
            .collect();
        detect_dir(&root, &children, &Rules::builtin())
    }

    fn pkg(deps: &str, dev: &str) -> String {
        format!(r#"{{"dependencies":{{{deps}}},"devDependencies":{{{dev}}}}}"#)
    }

    fn node_framework(deps: &str, dev: &str) -> Option<String> {
        detect(&[("package.json", &pkg(deps, dev))], &[])
            .unwrap()
            .framework
    }

    #[test]
    fn node_frameworks_one_per_row() {
        let cases: &[(&str, &str)] = &[
            (r#""next":"14.0.0""#, "Next.js"),
            (r#""@angular/core":"17.0.0""#, "Angular"),
            (r#""nuxt":"3.0.0""#, "Nuxt"),
            (r#""@sveltejs/kit":"2.0.0""#, "SvelteKit"),
            (r#""astro":"4.0.0""#, "Astro"),
            (r#""@remix-run/node":"2.0.0""#, "Remix"),
            (r#""expo":"50.0.0""#, "React Native"),
            (r#""react-native":"0.73.0""#, "React Native"),
            (r#""@nestjs/core":"10.0.0""#, "NestJS"),
            (r#""vue":"3.4.0""#, "Vue"),
            (r#""react":"18.0.0""#, "React"),
            (r#""express":"4.18.0""#, "Node.js (Express)"),
        ];
        for (deps, label) in cases {
            assert_eq!(node_framework(deps, "").as_deref(), Some(*label), "{deps}");
        }
    }

    #[test]
    fn desktop_shells_win_over_their_frontend_framework() {
        assert_eq!(
            node_framework(r#""react":"19.0.0""#, r#""@tauri-apps/cli":"^2.0.0""#).as_deref(),
            Some("Tauri")
        );
        assert_eq!(
            node_framework(r#""react":"19.0.0""#, r#""electron":"^33.0.0""#).as_deref(),
            Some("Electron")
        );
    }

    #[test]
    fn node_precedence() {
        assert_eq!(
            node_framework(r#""next":"14.0.0","react":"18.0.0""#, "").as_deref(),
            Some("Next.js")
        );
        assert_eq!(
            node_framework(r#""react":"18.0.0","vite":"5.0.0""#, "").as_deref(),
            Some("React (Vite)")
        );
        assert_eq!(
            node_framework(r#""react":"18.0.0""#, "").as_deref(),
            Some("React")
        );
    }

    #[test]
    fn dev_dependencies_count() {
        assert_eq!(
            node_framework(r#""react":"18.0.0""#, r#""vite":"5.0.0""#).as_deref(),
            Some("React (Vite)")
        );
    }

    #[test]
    fn unknown_node_deps_give_plain_node() {
        let p = detect(&[("package.json", &pkg(r#""left-pad":"1.0.0""#, ""))], &[]).unwrap();
        assert_eq!(p.label, "Node.js");
        assert_eq!(p.framework, None);
        assert_eq!(p.ecosystems, ["node"]);
    }

    #[test]
    fn invalid_package_json_is_still_a_node_project() {
        let p = detect(&[("package.json", "{ not json")], &[]).unwrap();
        assert_eq!(p.label, "Node.js");
        assert_eq!(p.framework, None);
    }

    #[test]
    fn cargo_toml_is_rust() {
        let p = detect(&[("Cargo.toml", "[package]\nname = \"x\"")], &["target"]).unwrap();
        assert_eq!(p.label, "Rust");
        assert_eq!(p.framework, None);
        assert_eq!(p.regenerable, ["target"]);
        assert_eq!(p.reinstall_command, None);
    }

    #[test]
    fn laravel_via_composer_require() {
        let p = detect(
            &[(
                "composer.json",
                r#"{"require":{"php":"^8.2","laravel/framework":"^11.0"}}"#,
            )],
            &["vendor"],
        )
        .unwrap();
        assert_eq!(p.label, "Laravel");
        assert_eq!(p.version.as_deref(), Some("11.0"));
        assert_eq!(p.regenerable, ["vendor"]);
    }

    #[test]
    fn symfony_via_composer_require_dev() {
        let p = detect(
            &[(
                "composer.json",
                r#"{"require-dev":{"symfony/framework-bundle":"^7.0"}}"#,
            )],
            &[],
        )
        .unwrap();
        assert_eq!(p.label, "Symfony");
    }

    #[test]
    fn spring_boot_via_pom_text() {
        let p = detect(
            &[(
                "pom.xml",
                "<parent><artifactId>spring-boot-starter-parent</artifactId></parent>",
            )],
            &["target"],
        )
        .unwrap();
        assert_eq!(p.label, "Spring Boot");
        assert_eq!(p.ecosystems, ["maven"]);
        assert_eq!(p.version, None);
    }

    #[test]
    fn spring_boot_via_gradle_text() {
        let p = detect(
            &[(
                "build.gradle.kts",
                "plugins { id(\"org.springframework.boot\") version \"3.2.0\" }",
            )],
            &["build"],
        )
        .unwrap();
        assert_eq!(p.label, "Spring Boot");
        assert_eq!(p.regenerable, ["build"]);
    }

    #[test]
    fn python_text_frameworks() {
        let p = detect(&[("requirements.txt", "Django==5.0\nrequests")], &[]).unwrap();
        assert_eq!(p.label, "Django");
        let p = detect(&[("pyproject.toml", "dependencies = [\"fastapi\"]")], &[]).unwrap();
        assert_eq!(p.label, "FastAPI");
        let p = detect(&[("setup.py", "install_requires=['Flask']")], &[]).unwrap();
        assert_eq!(p.label, "Flask");
        let p = detect(&[("requirements.txt", "requests")], &[]).unwrap();
        assert_eq!(p.label, "Python");
    }

    #[test]
    fn text_search_covers_every_marker_file() {
        let p = detect(
            &[("pyproject.toml", "[tool]"), ("requirements.txt", "flask")],
            &[],
        )
        .unwrap();
        assert_eq!(p.label, "Flask");
    }

    #[test]
    fn version_prefix_is_stripped() {
        for (raw, want) in [
            ("^14.1.0", "14.1.0"),
            ("~14.1.0", "14.1.0"),
            (">=14.1.0", "14.1.0"),
            ("=14.1.0", "14.1.0"),
            ("v14.1.0", "14.1.0"),
            ("14.1.0", "14.1.0"),
        ] {
            let p = detect(
                &[("package.json", &pkg(&format!(r#""next":"{raw}""#), ""))],
                &[],
            )
            .unwrap();
            assert_eq!(p.version.as_deref(), Some(want), "{raw}");
        }
    }

    #[test]
    fn wildcard_dep_version_comes_from_matching_package() {
        let p = detect(
            &[("package.json", &pkg(r#""@remix-run/node":"^2.5.0""#, ""))],
            &[],
        )
        .unwrap();
        assert_eq!(p.version.as_deref(), Some("2.5.0"));
    }

    #[test]
    fn lockfile_gives_manager_and_reinstall() {
        let p = detect(
            &[("package.json", &pkg("", "")), ("pnpm-lock.yaml", "")],
            &[],
        )
        .unwrap();
        assert!(p.has_lockfile);
        assert_eq!(p.package_manager.as_deref(), Some("pnpm"));
        assert_eq!(
            p.reinstall_command.as_deref(),
            Some("pnpm install --frozen-lockfile")
        );
    }

    #[test]
    fn no_lockfile() {
        let p = detect(&[("package.json", &pkg("", ""))], &[]).unwrap();
        assert!(!p.has_lockfile);
        assert_eq!(p.package_manager, None);
        assert_eq!(p.reinstall_command, None);
    }

    #[test]
    fn lockfile_must_be_a_file() {
        let p = detect(&[("package.json", &pkg("", ""))], &["yarn.lock"]).unwrap();
        assert!(!p.has_lockfile);
    }

    #[test]
    fn regenerable_only_existing_dirs() {
        let p = detect(
            &[("package.json", &pkg("", "")), ("dist", "i am a file")],
            &["node_modules", ".next", "src"],
        )
        .unwrap();
        let mut got = p.regenerable.clone();
        got.sort();
        assert_eq!(got, [".next", "node_modules"]);
    }

    #[test]
    fn multiple_ecosystems_merge() {
        let p = detect(
            &[
                ("package.json", &pkg(r#""vue":"^3.4.0""#, "")),
                (
                    "composer.json",
                    r#"{"require":{"laravel/framework":"^11"}}"#,
                ),
                ("composer.lock", ""),
            ],
            &["node_modules", "vendor"],
        )
        .unwrap();
        assert_eq!(p.ecosystems, ["node", "php"]);
        // First ecosystem with a framework match wins.
        assert_eq!(p.label, "Vue");
        assert_eq!(p.package_manager.as_deref(), Some("composer"));
        assert_eq!(p.reinstall_command.as_deref(), Some("composer install"));
        assert_eq!(p.regenerable, ["node_modules", "vendor"]);
    }

    #[test]
    fn regenerable_has_no_duplicates() {
        // maven and rust both list `target`.
        let p = detect(&[("Cargo.toml", ""), ("pom.xml", "")], &["target"]).unwrap();
        assert_eq!(p.regenerable, ["target"]);
    }

    #[test]
    fn name_matching_ignores_case() {
        let p = detect(
            &[
                ("Package.JSON", &pkg(r#""next":"14.0.0""#, "")),
                ("YARN.LOCK", ""),
            ],
            &["Node_Modules"],
        )
        .unwrap();
        assert_eq!(p.label, "Next.js");
        assert_eq!(p.package_manager.as_deref(), Some("yarn"));
        assert_eq!(p.regenerable, ["node_modules"]);
    }

    #[test]
    fn no_marker_gives_none() {
        assert!(detect(&[("readme.md", "hi")], &["src"]).is_none());
    }

    #[test]
    fn marker_that_is_a_dir_gives_none() {
        assert!(detect(&[], &["package.json"]).is_none());
    }

    #[test]
    fn display_name_uses_label_and_folder() {
        let p = detect(&[("package.json", &pkg(r#""react":"18.0.0""#, ""))], &[]).unwrap();
        assert_eq!(p.display_name(), "React project: my-shop");
    }
}
