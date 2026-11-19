//! Installed languages (every version) and developer tools, with how much space each takes.
//! Read-only: versions come from files and folder names, never from running the tools.
//!
//! Languages are found by code (each has its own layout). Tools are matched from the
//! installed-apps list and `devtools.json`, which also names the data folders they fill.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

use serde::{Deserialize, Serialize};

use crate::apps::{AppEntry, SizeSource};

const BUILTIN_JSON: &str = include_str!("../devtools.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DevKind {
    Language,
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DevSizeSource {
    /// The folder's size on disk in the current scan.
    Scan,
    /// Walked just now, because the scan does not cover it.
    Measured,
    /// The installer's own estimate (tools matched from the installed-apps list).
    Windows,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevInstall {
    /// e.g. "v24.22.1", "stable-x86_64-pc-windows-msvc", "Extensions".
    pub label: String,
    pub version: Option<String>,
    pub path: Option<PathBuf>,
    /// The version used when the command is run (first on PATH, rustup default, newest SDK).
    pub active: bool,
    /// Bytes.
    pub size: Option<u64>,
    pub size_source: Option<DevSizeSource>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevItem {
    pub id: String,
    pub name: String,
    pub kind: DevKind,
    pub installs: Vec<DevInstall>,
    /// Sum of the installs' known sizes; None when none is known.
    pub size: Option<u64>,
}

/// A WSL distribution, from the registry.
#[derive(Debug, Clone, PartialEq)]
pub struct WslDistro {
    pub name: String,
    pub base_path: PathBuf,
}

/// Everything detection looks at. `DevEnv::system` fills it from the real machine; tests
/// point it at a temporary folder.
#[derive(Debug, Clone, Default)]
pub struct DevEnv {
    pub home: PathBuf,
    pub local_app_data: PathBuf,
    pub app_data: PathBuf,
    /// `Program Files` and `Program Files (x86)`.
    pub program_files: Vec<PathBuf>,
    /// Root of the system drive (`C:\`), where tools like PHP are often unpacked.
    pub system_drive: PathBuf,
    /// Environment variables by upper-case name.
    pub vars: std::collections::HashMap<String, String>,
    /// PATH entries in order.
    pub path: Vec<PathBuf>,
    pub wsl: Vec<WslDistro>,
    pub apps: Vec<AppEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolRules {
    pub tools: Vec<ToolRule>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolRule {
    pub id: String,
    pub name: String,
    /// Installed-app name prefixes, matched case-insensitively at a word boundary.
    #[serde(default)]
    pub apps: Vec<String>,
    #[serde(default)]
    pub data: Vec<DataRule>,
    /// List each WSL distribution's disk under this tool.
    #[serde(default)]
    pub wsl_distros: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DataRule {
    pub label: String,
    /// May use `%VAR%`; entries with an unset variable are skipped.
    pub path: String,
}

impl DevEnv {
    /// This machine's folders, environment, PATH and WSL distributions. `apps` is the
    /// installed-apps list, used to match tools and versions.
    pub fn system(apps: Vec<AppEntry>) -> DevEnv {
        let get = |name: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .unwrap_or_default()
        };
        let mut program_files = vec![get("ProgramFiles")];
        let x86 = get("ProgramFiles(x86)");
        if !x86.as_os_str().is_empty() && key(&x86) != key(&program_files[0]) {
            program_files.push(x86);
        }
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        DevEnv {
            home: get("USERPROFILE"),
            local_app_data: get("LOCALAPPDATA"),
            app_data: get("APPDATA"),
            program_files,
            system_drive: PathBuf::from(format!("{drive}\\")),
            vars: std::env::vars()
                .map(|(k, v)| (k.to_uppercase(), v))
                .collect(),
            path: std::env::var_os("PATH")
                .map(|p| {
                    std::env::split_paths(&p)
                        .filter(|d| !d.as_os_str().is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            wsl: wsl_distros(),
            apps,
        }
    }
}

#[cfg(windows)]
fn wsl_distros() -> Vec<WslDistro> {
    use crate::reg::Key;
    use windows_sys::Win32::System::Registry::HKEY_CURRENT_USER;

    let Some(root) = Key::open(
        HKEY_CURRENT_USER,
        r"Software\Microsoft\Windows\CurrentVersion\Lxss",
        0,
    ) else {
        return Vec::new();
    };
    root.subkeys()
        .iter()
        .filter_map(|sub| {
            let k = root.sub(sub)?;
            Some(WslDistro {
                name: k.string("DistributionName")?,
                base_path: PathBuf::from(k.string("BasePath")?),
            })
        })
        .collect()
}

#[cfg(not(windows))]
fn wsl_distros() -> Vec<WslDistro> {
    Vec::new()
}

impl ToolRules {
    pub fn builtin() -> ToolRules {
        serde_json::from_str(BUILTIN_JSON).expect("devtools.json is valid")
    }
}

/// Languages and tools found in `env`, each sorted biggest first (unknown size last).
/// `measure` sizes a folder that has no size yet; it runs on several threads.
pub fn detect(
    env: &DevEnv,
    rules: &ToolRules,
    measure: &(dyn Fn(&Path) -> Option<(u64, DevSizeSource)> + Sync),
) -> (Vec<DevItem>, Vec<DevItem>) {
    let detectors: [(&str, &str, fn(&DevEnv) -> Vec<DevInstall>); 7] = [
        ("node", "Node.js", node),
        ("python", "Python", python),
        ("java", "Java", java),
        (".net", ".NET SDK", dotnet),
        ("rust", "Rust", rust),
        ("go", "Go", go),
        ("php", "PHP", php),
    ];
    let mut langs: Vec<DevItem> = detectors
        .iter()
        .map(|(id, name, find)| {
            let mut installs = find(env);
            installs.sort_by(|a, b| match (&a.version, &b.version) {
                (Some(x), Some(y)) => compare_versions(y, x),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => a.label.cmp(&b.label),
            });
            DevItem {
                id: (*id).into(),
                name: (*name).into(),
                kind: DevKind::Language,
                installs,
                size: None,
            }
        })
        .filter(|i| !i.installs.is_empty())
        .collect();
    let mut found = tools(env, rules);
    measure_all(&mut langs, &mut found, measure);
    (finish(langs), finish(found))
}

/// Sums each item's known sizes and sorts biggest first, unknown last, then by name.
fn finish(mut items: Vec<DevItem>) -> Vec<DevItem> {
    for item in &mut items {
        let known: Vec<u64> = item.installs.iter().filter_map(|i| i.size).collect();
        item.size = (!known.is_empty()).then(|| known.iter().sum());
    }
    items.sort_by(|a, b| {
        b.size
            .is_some()
            .cmp(&a.size.is_some())
            .then(b.size.cmp(&a.size))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    items
}

/// Sizes every install that has a folder but no size yet, a few folders at a time.
fn measure_all(
    langs: &mut [DevItem],
    tools: &mut [DevItem],
    measure: &(dyn Fn(&Path) -> Option<(u64, DevSizeSource)> + Sync),
) {
    const WORKERS: usize = 4;
    let mut todo: Vec<&mut DevInstall> = langs
        .iter_mut()
        .chain(tools.iter_mut())
        .flat_map(|i| i.installs.iter_mut())
        .filter(|i| i.size.is_none() && i.path.as_deref().is_some_and(Path::exists))
        .collect();
    let paths: Vec<PathBuf> = todo.iter().filter_map(|i| i.path.clone()).collect();
    let results: Vec<Mutex<Option<(u64, DevSizeSource)>>> =
        paths.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..WORKERS.min(paths.len()) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, AtomicOrdering::Relaxed);
                    let Some(path) = paths.get(i) else { break };
                    let got = measure(path);
                    *results[i].lock().unwrap_or_else(|e| e.into_inner()) = got;
                }
            });
        }
    });
    for (install, result) in todo.iter_mut().zip(results) {
        if let Some((size, source)) = result.into_inner().unwrap_or_else(|e| e.into_inner()) {
            install.size = Some(size);
            install.size_source = Some(source);
        }
    }
}

/// True when an installed app's name is `prefix` or starts with it followed by a
/// non-alphanumeric character ("Git 2.45" and "Git" match "Git"; "GitHub Desktop" does not).
pub fn app_matches(name: &str, prefix: &str) -> bool {
    let (name, prefix) = (name.to_lowercase(), prefix.to_lowercase());
    match name.strip_prefix(&prefix) {
        Some(rest) => rest.chars().next().is_none_or(|c| !c.is_alphanumeric()),
        None => false,
    }
}

/// Orders version strings numerically ("10.0.204" > "9.0.201"); a pre-release
/// ("10.0.100-preview.1") sorts before its release.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let split = |v: &str| {
        let (core, pre) = match v.split_once('-') {
            Some((c, p)) => (c.to_owned(), Some(p.to_owned())),
            None => (v.to_owned(), None),
        };
        let nums: Vec<u64> = core
            .split('.')
            .map(|p| {
                let digits: String = p.chars().take_while(char::is_ascii_digit).collect();
                digits.parse().unwrap_or(0)
            })
            .collect();
        (nums, pre)
    };
    let ((na, pa), (nb, pb)) = (split(a), split(b));
    na.cmp(&nb).then_with(|| match (pa, pb) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => x.cmp(&y),
    })
}

// ---- paths ----

/// Comparison key the way Windows compares paths: case-insensitive, either slash,
/// no `\\?\` prefix, no trailing separator.
fn key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_start_matches(r"\\?\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// The path with links and junctions resolved (nvm's `nodejs` link), or as given.
fn resolved_key(path: &Path) -> String {
    key(&std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
}

fn strip_verbatim(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s))
}

fn is_inside(path: &Path, dir_key: &str) -> bool {
    let p = resolved_key(path);
    p == dir_key || p.starts_with(&format!("{dir_key}\\"))
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn var(env: &DevEnv, name: &str) -> Option<PathBuf> {
    env.vars
        .get(&name.to_uppercase())
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The first PATH folder holding `exe`, skipping folders `skip` rejects.
fn on_path(env: &DevEnv, exe: &str, skip: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    env.path
        .iter()
        .filter(|d| !skip(d))
        .find(|d| d.join(exe).is_file())
        .cloned()
}

/// Turns found folders into installs: drops duplicates (the same folder reached twice,
/// e.g. through JAVA_HOME) and marks the one whose resolved path is `active`.
fn installs(
    found: Vec<(String, Option<String>, PathBuf)>,
    active: Option<&Path>,
) -> Vec<DevInstall> {
    let active = active.map(resolved_key);
    let mut seen = HashSet::new();
    found
        .into_iter()
        .filter(|(_, _, p)| seen.insert(resolved_key(p)))
        .map(|(label, version, path)| DevInstall {
            active: active.as_deref() == Some(resolved_key(&path).as_str()),
            label,
            version,
            path: Some(path),
            size: None,
            size_source: None,
        })
        .collect()
}

// ---- languages ----

fn node(env: &DevEnv) -> Vec<DevInstall> {
    let mut found = Vec::new();
    let roots = [
        var(env, "NVM_HOME"),
        Some(env.local_app_data.join("nvm")),
        Some(env.app_data.join("nvm")),
    ];
    for root in roots.into_iter().flatten() {
        for dir in subdirs(&root) {
            let name = name_of(&dir);
            let is_version = name
                .strip_prefix('v')
                .is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit()));
            if is_version && dir.join("node.exe").is_file() {
                found.push((name.clone(), Some(name[1..].to_owned()), dir));
            }
        }
    }
    for pf in &env.program_files {
        let dir = pf.join("nodejs");
        // nvm often puts its link here; that is a version already listed, not another install.
        // Compared against the resolved parent, so short names (PROGRA~1) don't look like links.
        let is_link = resolved_key(&dir) != format!("{}\\nodejs", resolved_key(pf));
        if dir.join("node.exe").is_file() && !is_link {
            let version = env
                .apps
                .iter()
                .find(|a| a.name.eq_ignore_ascii_case("Node.js"))
                .and_then(|a| a.version.clone());
            found.push(("nodejs".into(), version, dir));
        }
    }
    let active = on_path(env, "node.exe", |_| false);
    installs(found, active.as_deref())
}

fn python(env: &DevEnv) -> Vec<DevInstall> {
    let starts_python = |p: &PathBuf| name_of(p).to_lowercase().starts_with("python");
    let mut dirs: Vec<PathBuf> = subdirs(&env.local_app_data.join("Programs").join("Python"));
    for root in env.program_files.iter().chain([&env.system_drive]) {
        dirs.extend(subdirs(root).into_iter().filter(starts_python));
    }
    let pyenv_root = var(env, "PYENV_ROOT")
        .or_else(|| var(env, "PYENV"))
        .unwrap_or_else(|| env.home.join(".pyenv").join("pyenv-win"));
    let pyenv_versions = subdirs(&pyenv_root.join("versions"));

    let mut found = Vec::new();
    for dir in dirs {
        if dir.join("python.exe").is_file() {
            let version =
                patchlevel_version(&dir).or_else(|| python_folder_version(&name_of(&dir)));
            found.push((name_of(&dir), version, dir));
        }
    }
    for dir in pyenv_versions {
        if dir.join("python.exe").is_file() {
            let version = patchlevel_version(&dir).or_else(|| Some(name_of(&dir)));
            found.push((name_of(&dir), version, dir));
        }
    }
    // `WindowsApps\python.exe` only opens the Microsoft Store.
    let stub = key(&env.local_app_data.join("Microsoft").join("WindowsApps"));
    let active = on_path(env, "python.exe", |d| key(d) == stub);
    installs(found, active.as_deref())
}

/// `PY_VERSION` from `include\patchlevel.h`, e.g. "3.11.9".
fn patchlevel_version(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("include").join("patchlevel.h")).ok()?;
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("#define PY_VERSION "))?;
    Some(line.split('"').nth(1)?.to_owned())
}

/// "Python312" → "3.12", "Python3.12" → "3.12".
fn python_folder_version(name: &str) -> Option<String> {
    let rest = name.get(6..)?.trim_start_matches(['-', ' ']);
    if rest.contains('.') {
        return Some(rest.to_owned());
    }
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (digits.len() >= 2).then(|| format!("{}.{}", &digits[..1], &digits[1..]))
}

fn java(env: &DevEnv) -> Vec<DevInstall> {
    const VENDORS: [&str; 8] = [
        "Java",
        "Eclipse Adoptium",
        "Microsoft",
        "Zulu",
        "Amazon Corretto",
        "BellSoft",
        "Semeru",
        "OpenJDK",
    ];
    let java_home = var(env, "JAVA_HOME").map(|p| {
        if name_of(&p).eq_ignore_ascii_case("bin") {
            p.parent().map(Path::to_path_buf).unwrap_or(p)
        } else {
            p
        }
    });
    let mut dirs = Vec::new();
    for pf in &env.program_files {
        for vendor in VENDORS {
            dirs.extend(subdirs(&pf.join(vendor)));
        }
    }
    dirs.extend(subdirs(&env.home.join(".jdks")));
    dirs.extend(java_home.clone());

    let mut found = Vec::new();
    for dir in dirs {
        if !dir.join("bin").join("java.exe").is_file() {
            continue;
        }
        let Ok(release) = std::fs::read_to_string(dir.join("release")) else {
            continue;
        };
        let version = release
            .lines()
            .find_map(|l| l.strip_prefix("JAVA_VERSION="))
            .map(|v| v.trim().trim_matches('"').to_owned());
        found.push((name_of(&dir), version, dir));
    }
    // The java.exe that runs: a JDK's own bin on PATH, else whatever JAVA_HOME names
    // (Oracle's `javapath` launcher on PATH is not a JDK itself).
    let from_path = on_path(env, "java.exe", |_| false)
        .and_then(|bin| bin.parent().map(Path::to_path_buf))
        .filter(|jdk| {
            found
                .iter()
                .any(|(_, _, p)| resolved_key(p) == resolved_key(jdk))
        });
    let active = from_path.or(java_home);
    installs(found, active.as_deref())
}

fn dotnet(env: &DevEnv) -> Vec<DevInstall> {
    let roots = var(env, "DOTNET_ROOT")
        .into_iter()
        .chain(env.program_files.iter().map(|pf| pf.join("dotnet")));
    let mut found = Vec::new();
    for root in roots {
        for dir in subdirs(&root.join("sdk")) {
            let name = name_of(&dir);
            if name.starts_with(|c: char| c.is_ascii_digit()) {
                found.push((name.clone(), Some(name), dir));
            }
        }
    }
    // Without a global.json, `dotnet` uses the newest SDK installed.
    let newest = found
        .iter()
        .max_by(|a, b| compare_versions(a.1.as_deref().unwrap_or(""), b.1.as_deref().unwrap_or("")))
        .map(|f| f.2.clone());
    installs(found, newest.as_deref())
}

fn rust(env: &DevEnv) -> Vec<DevInstall> {
    let home = var(env, "RUSTUP_HOME").unwrap_or_else(|| env.home.join(".rustup"));
    let default = var(env, "RUSTUP_TOOLCHAIN")
        .map(|p| p.to_string_lossy().into_owned())
        .or_else(|| {
            let text = std::fs::read_to_string(home.join("settings.toml")).ok()?;
            let line = text
                .lines()
                .find(|l| l.trim_start().starts_with("default_toolchain"))?;
            Some(line.split('"').nth(1)?.to_owned())
        });
    let mut found = Vec::new();
    let mut active = None;
    for dir in subdirs(&home.join("toolchains")) {
        let name = name_of(&dir);
        let manifest = dir
            .join("lib")
            .join("rustlib")
            .join("multirust-channel-manifest.toml");
        let version = std::fs::read_to_string(manifest).ok().and_then(|text| {
            let mut lines = text.lines().skip_while(|l| l.trim() != "[pkg.rustc]");
            lines.next()?;
            let line = lines.find(|l| l.trim_start().starts_with("version"))?;
            Some(
                line.split('"')
                    .nth(1)?
                    .split_whitespace()
                    .next()?
                    .to_owned(),
            )
        });
        if default.as_deref() == Some(name.as_str()) {
            active = Some(dir.clone());
        }
        found.push((name, version, dir));
    }
    installs(found, active.as_deref())
}

fn go(env: &DevEnv) -> Vec<DevInstall> {
    let mut dirs: Vec<PathBuf> = var(env, "GOROOT").into_iter().collect();
    dirs.extend(env.program_files.iter().map(|pf| pf.join("Go")));
    dirs.extend(
        subdirs(&env.home.join("sdk"))
            .into_iter()
            .filter(|d| name_of(d).starts_with("go")),
    );
    let mut found = Vec::new();
    for dir in dirs {
        let Ok(text) = std::fs::read_to_string(dir.join("VERSION")) else {
            continue;
        };
        let version = text
            .lines()
            .next()
            .map(|l| l.trim().trim_start_matches("go").to_owned())
            .filter(|v| !v.is_empty());
        found.push((name_of(&dir), version, dir));
    }
    let active =
        on_path(env, "go.exe", |_| false).and_then(|bin| bin.parent().map(Path::to_path_buf));
    installs(found, active.as_deref())
}

fn php(env: &DevEnv) -> Vec<DevInstall> {
    let starts_php = |p: &PathBuf| name_of(p).to_lowercase().starts_with("php");
    let drive = &env.system_drive;
    let mut dirs: Vec<PathBuf> = subdirs(drive).into_iter().filter(starts_php).collect();
    dirs.push(drive.join("xampp").join("php"));
    dirs.extend(subdirs(&drive.join("laragon").join("bin").join("php")));
    for pf in &env.program_files {
        dirs.extend(subdirs(&pf.join("PHP")));
        dirs.extend(subdirs(pf).into_iter().filter(starts_php));
    }
    dirs.extend(
        env.path
            .iter()
            .filter(|d| d.join("php.exe").is_file())
            .cloned(),
    );
    let found = dirs
        .into_iter()
        .filter(|d| d.join("php.exe").is_file())
        .map(|d| {
            let version =
                php_folder_version(&name_of(&d)).or_else(|| exe_version(&d.join("php.exe")));
            (name_of(&d), version, d)
        })
        .collect();
    let active = on_path(env, "php.exe", |_| false);
    installs(found, active.as_deref())
}

/// The product version an exe carries (Explorer: Properties > Details), as
/// "major.minor.patch". None when the file has no version info.
#[cfg(windows)]
fn exe_version(exe: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };

    let wide: Vec<u16> = exe.as_os_str().encode_wide().chain([0]).collect();
    let mut handle = 0u32;
    // SAFETY: `wide` is NUL-terminated; `handle` is a valid out pointer.
    let len = unsafe { GetFileVersionInfoSizeW(wide.as_ptr(), &mut handle) };
    if len == 0 {
        return None;
    }
    let mut data = vec![0u8; len as usize];
    // SAFETY: `data` is valid for `len` bytes.
    if unsafe { GetFileVersionInfoW(wide.as_ptr(), 0, len, data.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    let root: Vec<u16> = "\\".encode_utf16().chain([0]).collect();
    let mut info: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut info_len = 0u32;
    // SAFETY: `data` holds the version block just read; the out pointers are valid.
    let ok = unsafe {
        VerQueryValueW(
            data.as_ptr().cast(),
            root.as_ptr(),
            &mut info,
            &mut info_len,
        )
    };
    if ok == 0 || info.is_null() || (info_len as usize) < size_of::<VS_FIXEDFILEINFO>() {
        return None;
    }
    // SAFETY: VerQueryValueW returned a pointer into `data` to a VS_FIXEDFILEINFO of `info_len` bytes.
    let fixed = unsafe { std::ptr::read_unaligned(info.cast::<VS_FIXEDFILEINFO>()) };
    let (ms, ls) = (fixed.dwProductVersionMS, fixed.dwProductVersionLS);
    Some(format!("{}.{}.{}", ms >> 16, ms & 0xFFFF, ls >> 16))
}

#[cfg(not(windows))]
fn exe_version(_exe: &Path) -> Option<String> {
    None
}

/// "php-8.5.4" → "8.5.4", "php-8.1.10-Win32-vs16-x64" → "8.1.10", "php" → None.
fn php_folder_version(name: &str) -> Option<String> {
    let rest = name.get(3..)?.trim_start_matches(['-', '_', ' ']);
    let version: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let version = version.trim_end_matches('.');
    (!version.is_empty()).then(|| version.to_owned())
}

// ---- tools ----

/// `%VAR%` replaced from the environment; None when a variable is not set.
fn expand(env: &DevEnv, template: &str) -> Option<PathBuf> {
    let mut out = String::new();
    let mut parts = template.split('%');
    out.push_str(parts.next()?);
    while let Some(name) = parts.next() {
        let value = match name.to_uppercase().as_str() {
            "LOCALAPPDATA" => env.local_app_data.clone(),
            "APPDATA" => env.app_data.clone(),
            "USERPROFILE" => env.home.clone(),
            _ => var(env, name)?,
        };
        out.push_str(&value.to_string_lossy());
        out.push_str(parts.next()?);
    }
    Some(PathBuf::from(out))
}

fn tools(env: &DevEnv, rules: &ToolRules) -> Vec<DevItem> {
    // Data folders already listed, so a folder inside one (Docker's WSL distro) is not counted twice.
    let mut claimed: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for rule in &rules.tools {
        let mut list: Vec<DevInstall> = env
            .apps
            .iter()
            .filter(|a| rule.apps.iter().any(|p| app_matches(&a.name, p)))
            .map(|a| DevInstall {
                label: a.name.clone(),
                version: a.version.clone(),
                path: a.location.clone(),
                active: false,
                size: a.size,
                size_source: a.size_source.map(|s| match s {
                    SizeSource::Scan => DevSizeSource::Scan,
                    SizeSource::Windows => DevSizeSource::Windows,
                }),
            })
            .collect();
        let mut add = |label: &str, path: PathBuf, list: &mut Vec<DevInstall>| {
            if !path.exists() || claimed.iter().any(|c| is_inside(&path, c)) {
                return;
            }
            claimed.push(resolved_key(&path));
            list.push(DevInstall {
                label: label.to_owned(),
                version: None,
                path: Some(path),
                active: false,
                size: None,
                size_source: None,
            });
        };
        for data in &rule.data {
            if let Some(path) = expand(env, &data.path) {
                add(&data.label, path, &mut list);
            }
        }
        if rule.wsl_distros {
            for distro in &env.wsl {
                add(&distro.name, strip_verbatim(&distro.base_path), &mut list);
            }
        }
        if !list.is_empty() {
            out.push(DevItem {
                id: rule.id.clone(),
                name: rule.name.clone(),
                kind: DevKind::Tool,
                installs: list,
                size: None,
            });
        }
    }
    out
}

#[cfg(all(test, windows))]
mod tests {
    use std::fs;
    use std::sync::Mutex;

    use super::*;
    use crate::apps::AppKind;

    struct Fixture {
        _dir: tempfile::TempDir,
        env: DevEnv,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().to_path_buf();
            let env = DevEnv {
                home: root.join("home"),
                local_app_data: root.join("home").join("AppData").join("Local"),
                app_data: root.join("home").join("AppData").join("Roaming"),
                program_files: vec![root.join("Program Files"), root.join("Program Files (x86)")],
                system_drive: root.join("C"),
                ..DevEnv::default()
            };
            Self { _dir: dir, env }
        }

        fn file(&self, path: &Path, text: &str) {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn var(&mut self, name: &str, value: &Path) {
            self.env
                .vars
                .insert(name.into(), value.to_string_lossy().into_owned());
        }

        fn pf(&self) -> PathBuf {
            self.env.program_files[0].clone()
        }
    }

    fn app(
        name: &str,
        version: Option<&str>,
        location: Option<&Path>,
        size: Option<u64>,
    ) -> AppEntry {
        AppEntry {
            id: name.into(),
            name: name.into(),
            publisher: None,
            version: version.map(str::to_owned),
            kind: AppKind::Desktop,
            location: location.map(Path::to_path_buf),
            installed_at: None,
            size,
            size_source: size.map(|_| SizeSource::Windows),
        }
    }

    fn no_measure(_: &Path) -> Option<(u64, DevSizeSource)> {
        None
    }

    fn rules() -> ToolRules {
        ToolRules::builtin()
    }

    fn languages(env: &DevEnv) -> Vec<DevItem> {
        detect(env, &rules(), &no_measure).0
    }

    fn tools(env: &DevEnv) -> Vec<DevItem> {
        detect(env, &rules(), &no_measure).1
    }

    fn item<'a>(items: &'a [DevItem], id: &str) -> &'a DevItem {
        items.iter().find(|i| i.id == id).unwrap_or_else(|| {
            panic!(
                "{id} not found in {:?}",
                items.iter().map(|i| &i.id).collect::<Vec<_>>()
            )
        })
    }

    /// (version, active) per install, in the order given.
    fn versions(item: &DevItem) -> Vec<(Option<&str>, bool)> {
        item.installs
            .iter()
            .map(|i| (i.version.as_deref(), i.active))
            .collect()
    }

    #[test]
    fn node_versions_come_from_nvm_and_the_symlinked_one_on_path_is_active() {
        let mut f = Fixture::new();
        let nvm = f.env.local_app_data.join("nvm");
        f.file(&nvm.join("v20.11.0").join("node.exe"), "");
        f.file(&nvm.join("v24.22.1").join("node.exe"), "");
        f.file(&nvm.join("v18.0.0").join("readme.txt"), ""); // no node.exe: half-removed
        f.file(&nvm.join("settings.txt"), "root: x");
        let link = f.env.system_drive.join("nvm4w").join("nodejs");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        junction::create(nvm.join("v24.22.1"), &link).unwrap();
        f.var("NVM_HOME", &nvm);
        f.env.path = vec![f.env.home.join("bin"), link];
        let langs = languages(&f.env);
        let node = item(&langs, "node");
        assert_eq!(node.name, "Node.js");
        assert_eq!(node.kind, DevKind::Language);
        assert_eq!(
            versions(node),
            [(Some("24.22.1"), true), (Some("20.11.0"), false)]
        );
        assert_eq!(node.installs[0].label, "v24.22.1");
        assert_eq!(
            node.installs[1].path.as_deref(),
            Some(nvm.join("v20.11.0").as_path())
        );
    }

    #[test]
    fn a_standalone_node_gets_its_version_from_the_installed_apps() {
        let mut f = Fixture::new();
        let dir = f.pf().join("nodejs");
        f.file(&dir.join("node.exe"), "");
        f.env.apps = vec![app("Node.js", Some("22.1.0"), Some(&dir), None)];
        f.env.path = vec![dir.clone()];
        let langs = languages(&f.env);
        assert_eq!(versions(item(&langs, "node")), [(Some("22.1.0"), true)]);
    }

    #[test]
    fn python_versions_come_from_patchlevel_or_the_folder_name_and_the_store_stub_is_skipped() {
        let mut f = Fixture::new();
        let base = f.env.local_app_data.join("Programs").join("Python");
        let py311 = base.join("Python311");
        f.file(&py311.join("python.exe"), "");
        f.file(
            &py311.join("include").join("patchlevel.h"),
            "#define PY_MINOR_VERSION 11\n#define PY_VERSION              \"3.11.9\"\n",
        );
        let py312 = f.pf().join("Python312");
        f.file(&py312.join("python.exe"), "");
        // Left behind by an uninstall: no python.exe.
        f.file(&base.join("Python314").join("Lib").join("x.py"), "");
        let stub = f.env.local_app_data.join("Microsoft").join("WindowsApps");
        f.file(&stub.join("python.exe"), "");
        f.env.path = vec![stub, py312.clone(), py311];
        let langs = languages(&f.env);
        assert_eq!(
            versions(item(&langs, "python")),
            [(Some("3.12"), true), (Some("3.11.9"), false)]
        );
    }

    #[test]
    fn pyenv_versions_are_listed_by_folder_name() {
        let f = Fixture::new();
        let versions_dir = f.env.home.join(".pyenv").join("pyenv-win").join("versions");
        f.file(&versions_dir.join("3.10.4").join("python.exe"), "");
        let langs = languages(&f.env);
        assert_eq!(versions(item(&langs, "python")), [(Some("3.10.4"), false)]);
    }

    #[test]
    fn java_versions_come_from_release_files_and_java_home_may_point_at_bin() {
        let mut f = Fixture::new();
        let jdk25 = f.pf().join("Java").join("jdk-25");
        f.file(
            &jdk25.join("release"),
            "IMPLEMENTOR=\"Oracle\"\nJAVA_VERSION=\"25.0.1\"\n",
        );
        f.file(&jdk25.join("bin").join("java.exe"), "");
        let jdk17 = f.env.home.join(".jdks").join("temurin-17.0.9");
        f.file(&jdk17.join("release"), "JAVA_VERSION=\"17.0.9\"\n");
        f.file(&jdk17.join("bin").join("java.exe"), "");
        // A JRE folder without release file is not listed.
        f.file(
            &f.pf().join("Java").join("jre").join("bin").join("java.exe"),
            "",
        );
        // Oracle's launcher copy on PATH says nothing; JAVA_HOME (wrongly ending in bin) decides.
        let launcher = f
            .pf()
            .join("Common Files")
            .join("Oracle")
            .join("Java")
            .join("javapath");
        f.file(&launcher.join("java.exe"), "");
        f.env.path = vec![launcher];
        f.var("JAVA_HOME", &jdk25.join("bin"));
        let langs = languages(&f.env);
        assert_eq!(
            versions(item(&langs, "java")),
            [(Some("25.0.1"), true), (Some("17.0.9"), false)]
        );
    }

    #[test]
    fn a_jdk_on_path_wins_over_java_home() {
        let mut f = Fixture::new();
        let a = f.pf().join("Eclipse Adoptium").join("jdk-21");
        f.file(&a.join("release"), "JAVA_VERSION=\"21.0.2\"\n");
        f.file(&a.join("bin").join("java.exe"), "");
        let b = f.pf().join("Microsoft").join("jdk-17");
        f.file(&b.join("release"), "JAVA_VERSION=\"17.0.1\"\n");
        f.file(&b.join("bin").join("java.exe"), "");
        f.env.path = vec![b.join("bin")];
        f.var("JAVA_HOME", &a);
        let langs = languages(&f.env);
        assert_eq!(
            versions(item(&langs, "java")),
            [(Some("21.0.2"), false), (Some("17.0.1"), true)]
        );
    }

    #[test]
    // Without a global.json, `dotnet` picks the newest SDK, previews included.
    fn dotnet_sdks_are_listed_and_the_newest_is_active() {
        let f = Fixture::new();
        let sdk = f.pf().join("dotnet").join("sdk");
        for v in ["9.0.201", "10.0.204", "10.0.300-preview.1"] {
            f.file(&sdk.join(v).join("dotnet.dll"), "");
        }
        f.file(&sdk.join("NuGetFallbackFolder").join("x"), "");
        let langs = languages(&f.env);
        let dotnet = item(&langs, ".net");
        assert_eq!(dotnet.name, ".NET SDK");
        assert_eq!(
            versions(dotnet),
            [
                (Some("10.0.300-preview.1"), true),
                (Some("10.0.204"), false),
                (Some("9.0.201"), false)
            ]
        );
    }

    #[test]
    fn rust_toolchains_take_their_version_from_the_manifest_and_the_default_is_active() {
        let f = Fixture::new();
        let rustup = f.env.home.join(".rustup");
        f.file(
            &rustup.join("settings.toml"),
            "version = \"12\"\ndefault_toolchain = \"stable-x86_64-pc-windows-msvc\"\n",
        );
        let stable = rustup
            .join("toolchains")
            .join("stable-x86_64-pc-windows-msvc");
        f.file(
            &stable.join("lib").join("rustlib").join("multirust-channel-manifest.toml"),
            "[pkg.cargo]\nversion = \"1.100.0 (abc 2026-11-01)\"\n[pkg.rustc]\nversion = \"1.100.0 (0ef1bc386 2026-11-09)\"\n",
        );
        f.file(
            &rustup
                .join("toolchains")
                .join("nightly-x86_64-pc-windows-msvc")
                .join("bin")
                .join("rustc.exe"),
            "",
        );
        let langs = languages(&f.env);
        let rust = item(&langs, "rust");
        assert_eq!(versions(rust), [(Some("1.100.0"), true), (None, false)]);
        assert_eq!(rust.installs[0].label, "stable-x86_64-pc-windows-msvc");
        assert_eq!(rust.installs[1].label, "nightly-x86_64-pc-windows-msvc");
    }

    #[test]
    fn go_versions_come_from_the_version_file() {
        let mut f = Fixture::new();
        let go = f.pf().join("Go");
        f.file(&go.join("VERSION"), "go1.22.0\ntime 2024-02-06\n");
        f.file(&go.join("bin").join("go.exe"), "");
        let extra = f.env.home.join("sdk").join("go1.21.5");
        f.file(&extra.join("VERSION"), "go1.21.5\n");
        f.env.path = vec![go.join("bin")];
        let langs = languages(&f.env);
        assert_eq!(
            versions(item(&langs, "go")),
            [(Some("1.22.0"), true), (Some("1.21.5"), false)]
        );
    }

    #[test]
    fn php_is_found_on_path_and_in_common_folders_with_the_version_from_the_folder_name() {
        let mut f = Fixture::new();
        let a = f.env.system_drive.join("php-8.5.4");
        f.file(&a.join("php.exe"), "");
        let b = f
            .env
            .system_drive
            .join("laragon")
            .join("bin")
            .join("php")
            .join("php-8.1.10-Win32-vs16-x64");
        f.file(&b.join("php.exe"), "");
        let c = f.env.system_drive.join("tools").join("php");
        f.file(&c.join("php.exe"), "");
        f.env.path = vec![a];
        let langs = languages(&f.env);
        assert_eq!(
            versions(item(&langs, "php")),
            [(Some("8.5.4"), true), (Some("8.1.10"), false)],
            "a folder off PATH and outside the usual places is not found"
        );
        f.env.path.push(c);
        let langs = languages(&f.env);
        assert_eq!(
            item(&langs, "php").installs.len(),
            3,
            "a php.exe on PATH counts too"
        );
    }

    #[test]
    fn php_without_a_version_in_the_folder_name_reads_it_from_the_exe() {
        let f = Fixture::new();
        let dir = f.env.system_drive.join("xampp").join("php");
        fs::create_dir_all(&dir).unwrap();
        // Any exe with version info will do: notepad stands in for php.exe.
        let windows = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap();
        fs::copy(
            windows.join("System32").join("notepad.exe"),
            dir.join("php.exe"),
        )
        .unwrap();
        let langs = languages(&f.env);
        let version = item(&langs, "php").installs[0].version.clone().unwrap();
        assert!(version.starts_with("10."), "{version}");
        assert_eq!(
            version.split('.').count(),
            3,
            "major.minor.patch: {version}"
        );
    }

    #[test]
    fn languages_with_nothing_installed_are_left_out() {
        let f = Fixture::new();
        assert!(languages(&f.env).is_empty());
        assert!(tools(&f.env).is_empty());
    }

    #[test]
    fn tools_are_matched_from_installed_apps_by_name_prefix() {
        let mut f = Fixture::new();
        let git = f.pf().join("Git");
        f.env.apps = vec![
            app("Git", Some("2.45.2"), Some(&git), Some(300)),
            app("GitHub Desktop", Some("3.4"), None, Some(1)),
            app("Visual Studio Community 2022", Some("17.14"), None, None),
            app("Visual Studio Build Tools 2022", Some("17.12"), None, None),
            app(
                "IntelliJ IDEA Community Edition 2025.2",
                Some("252.1"),
                None,
                Some(2),
            ),
        ];
        let found = tools(&f.env);
        let g = item(&found, "git");
        assert_eq!(g.kind, DevKind::Tool);
        assert_eq!(g.installs.len(), 1);
        assert_eq!(g.installs[0].label, "Git");
        assert_eq!(g.installs[0].version.as_deref(), Some("2.45.2"));
        assert_eq!(g.installs[0].size, Some(300));
        assert_eq!(g.installs[0].size_source, Some(DevSizeSource::Windows));
        assert_eq!(item(&found, "visual-studio").installs.len(), 2);
        assert_eq!(item(&found, "jetbrains").installs.len(), 1);
    }

    #[test]
    fn app_name_matching_stops_at_word_boundaries() {
        assert!(app_matches("Git", "Git"));
        assert!(app_matches("git 2.45.2", "Git"));
        assert!(app_matches("Docker Desktop", "docker desktop"));
        assert!(app_matches(
            "Microsoft Visual Studio Code (User)",
            "Microsoft Visual Studio Code"
        ));
        assert!(!app_matches("GitHub Desktop", "Git"));
        assert!(!app_matches("Gi", "Git"));
        assert!(!app_matches("Riders Republic", "Rider"));
    }

    #[test]
    fn tool_data_folders_expand_variables_and_skip_missing_or_unset_ones() {
        let mut f = Fixture::new();
        let sdk = f.env.local_app_data.join("Android").join("Sdk");
        f.file(&sdk.join("platforms").join("x"), "");
        f.file(
            &f.env
                .home
                .join(".android")
                .join("avd")
                .join("Pixel_8.avd")
                .join("x"),
            "",
        );
        // ANDROID_HOME points at the same SDK: listed once. ANDROID_SDK_ROOT is unset.
        f.var("ANDROID_HOME", &sdk);
        let found = tools(&f.env);
        let android = item(&found, "android");
        let labels: Vec<_> = android.installs.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["Android SDK", "Emulators (AVDs)"]);
        assert!(
            android
                .installs
                .iter()
                .all(|i| i.version.is_none() && !i.active)
        );
        // VS Code has neither an app entry nor an extensions folder here.
        assert!(found.iter().all(|i| i.id != "vscode"));
    }

    #[test]
    fn wsl_distros_are_listed_but_not_ones_inside_docker_data_already_counted() {
        let mut f = Fixture::new();
        let docker = f.env.local_app_data.join("Docker").join("wsl");
        f.file(&docker.join("main").join("ext4.vhdx"), "x");
        let ubuntu = f
            .env
            .local_app_data
            .join("Packages")
            .join("Ubuntu")
            .join("LocalState");
        f.file(&ubuntu.join("ext4.vhdx"), "x");
        f.env.wsl = vec![
            WslDistro {
                name: "docker-desktop".into(),
                base_path: PathBuf::from(format!(r"\\?\{}", docker.join("main").display())),
            },
            WslDistro {
                name: "Ubuntu".into(),
                base_path: ubuntu.clone(),
            },
            WslDistro {
                name: "Gone".into(),
                base_path: f.env.home.join("missing"),
            },
        ];
        f.env.apps = vec![app("Docker Desktop", Some("4.48.0"), None, None)];
        let found = tools(&f.env);
        let docker_item = item(&found, "docker");
        assert_eq!(docker_item.installs.len(), 2, "the app and its disks");
        let wsl = item(&found, "wsl");
        let labels: Vec<_> = wsl.installs.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["Ubuntu"]);
        assert_eq!(wsl.installs[0].path.as_deref(), Some(ubuntu.as_path()));
    }

    #[test]
    fn folders_without_a_size_are_measured_in_parallel_and_items_sum_and_sort_by_size() {
        let mut f = Fixture::new();
        let nvm = f.env.local_app_data.join("nvm");
        f.file(&nvm.join("v20.0.0").join("node.exe"), "");
        f.file(&nvm.join("v22.0.0").join("node.exe"), "");
        let go = f.pf().join("Go");
        f.file(&go.join("VERSION"), "go1.22.0\n");
        let rust = f.env.home.join(".rustup").join("toolchains").join("stable");
        f.file(&rust.join("bin").join("rustc.exe"), "");
        let git = f.pf().join("Git");
        f.env.apps = vec![app("Git", Some("2.45.2"), Some(&git), Some(1000))];
        let asked = Mutex::new(Vec::new());
        let measure = |p: &Path| -> Option<(u64, DevSizeSource)> {
            asked.lock().unwrap().push(p.to_path_buf());
            let name = p.file_name()?.to_string_lossy().into_owned();
            match name.as_str() {
                "v20.0.0" => Some((100, DevSizeSource::Scan)),
                "v22.0.0" => Some((200, DevSizeSource::Measured)),
                "Go" => Some((50, DevSizeSource::Measured)),
                _ => None,
            }
        };
        let (langs, found) = detect(&f.env, &rules(), &measure);
        let ids: Vec<_> = langs.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["node", "go", "rust"], "biggest first, unknown last");
        assert_eq!(item(&langs, "node").size, Some(300));
        assert_eq!(item(&langs, "rust").size, None);
        let node = item(&langs, "node");
        assert_eq!(node.installs[0].size_source, Some(DevSizeSource::Measured));
        assert_eq!(node.installs[1].size_source, Some(DevSizeSource::Scan));
        // Git came with a size from the apps list, so it was not walked again.
        assert!(!asked.lock().unwrap().iter().any(|p| p == &git));
        assert_eq!(item(&found, "git").size, Some(1000));
    }

    #[test]
    fn versions_compare_numerically_with_prereleases_first() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("10.0.204", "9.0.201"), Greater);
        assert_eq!(compare_versions("1.2", "1.2.0"), Less);
        assert_eq!(compare_versions("10.0.100-preview.1", "10.0.100"), Less);
        assert_eq!(
            compare_versions("10.0.100-rc.2", "10.0.100-preview.7"),
            Greater
        );
        assert_eq!(compare_versions("3.11.9", "3.11.9"), Equal);
    }

    #[test]
    fn builtin_rules_parse_with_unique_ids() {
        let r = rules();
        let mut ids: Vec<_> = r.tools.iter().map(|t| t.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), r.tools.len());
        assert!(
            r.tools
                .iter()
                .all(|t| !t.apps.is_empty() || !t.data.is_empty() || t.wsl_distros)
        );
    }

    #[test]
    fn json_keys_match_the_ui_types() {
        let item = DevItem {
            id: "node".into(),
            name: "Node.js".into(),
            kind: DevKind::Language,
            installs: vec![DevInstall {
                label: "v1".into(),
                version: None,
                path: None,
                active: true,
                size: Some(1),
                size_source: Some(DevSizeSource::Measured),
            }],
            size: Some(1),
        };
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v["kind"], "language");
        assert_eq!(v["installs"][0]["sizeSource"], "measured");
        let keys = |v: &serde_json::Value| {
            let mut k: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        assert_eq!(keys(&v), ["id", "installs", "kind", "name", "size"]);
        assert_eq!(
            keys(&v["installs"][0]),
            ["active", "label", "path", "size", "sizeSource", "version"]
        );
    }

    // Read-only against the real machine.
    #[test]
    #[ignore = "reads this PC's installed languages and tools"]
    fn real_machine() {
        let env = DevEnv::system(Vec::new());
        let t = std::time::Instant::now();
        let (langs, found) = detect(&env, &rules(), &|_| None);
        println!("detected in {:?}", t.elapsed());
        for i in langs.iter().chain(&found) {
            println!("{:?} {}", i.kind, i.name);
            for x in &i.installs {
                println!(
                    "    {:<40} {:<14} active={} {:?}",
                    x.label,
                    x.version.as_deref().unwrap_or("-"),
                    x.active,
                    x.path
                );
            }
        }
    }
}
