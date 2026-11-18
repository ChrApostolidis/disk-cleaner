//! Installed apps: classic desktop programs (the registry's Uninstall entries) and
//! Microsoft Store packages, as Settings > Installed apps shows them. Read-only.
//!
//! Sizes come from the scan when it covers an app's install folder, since that is what the
//! folder really takes on disk; otherwise from the size the installer wrote to the registry.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AppKind {
    #[default]
    Desktop,
    Store,
}

/// Where an app's size came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SizeSource {
    /// Measured: the install folder's size on disk in the current scan.
    Scan,
    /// The installer's own estimate from the registry.
    Windows,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppEntry {
    /// Registry key path or package full name; stable while the app stays installed.
    pub id: String,
    pub name: String,
    pub publisher: Option<String>,
    pub version: Option<String>,
    pub kind: AppKind,
    /// Install folder, for "Show in Explorer". May be guessed from the app's icon path.
    pub location: Option<PathBuf>,
    /// Unix milliseconds (midnight UTC of the install day).
    pub installed_at: Option<i64>,
    /// Bytes.
    pub size: Option<u64>,
    pub size_source: Option<SizeSource>,
}

/// One entry as the system reports it, before filtering, merging and sizing.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RawApp {
    pub id: String,
    pub kind: AppKind,
    pub name: Option<String>,
    pub publisher: Option<String>,
    pub version: Option<String>,
    pub install_location: Option<PathBuf>,
    /// `DisplayIcon`, e.g. `"C:\Program Files\7-Zip\7zFM.exe",0`.
    pub icon: Option<String>,
    /// `InstallDate`, normally `YYYYMMDD`.
    pub install_date: Option<String>,
    /// `EstimatedSize`, in KiB.
    pub estimated_kb: Option<u64>,
    /// Hidden from Settings (`SystemComponent = 1`), e.g. MSI parts of a bigger product.
    pub system_component: bool,
    /// An update or hotfix entry (`ParentKeyName`, or `ReleaseType` Update/Hotfix).
    pub is_update: bool,
    /// Store packages: false for frameworks and resource packages, which have no app to run.
    pub launchable: bool,
}

pub trait AppSource {
    fn apps(&self) -> Result<Vec<RawApp>, String>;
}

pub struct SystemApps;

/// Every installed app worth listing, biggest first (unknown size last, ties by name).
/// `size_in_scan` returns the scanned size on disk of a folder, or None when the scan
/// does not cover it. `windows_dir` is the Windows folder (`C:\Windows`): Store packages
/// living there are parts of Windows, not apps the user installed.
pub fn list_apps(
    source: &dyn AppSource,
    size_in_scan: &dyn Fn(&Path) -> Option<u64>,
    windows_dir: &Path,
) -> Result<Vec<AppEntry>, String> {
    // Insertion order is kept so that equal entries resolve the same way every time.
    let mut picked: Vec<Candidate> = Vec::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    for raw in source.apps()? {
        let Some(c) = Candidate::from_raw(raw, windows_dir) else {
            continue;
        };
        match by_key.get(&c.key) {
            Some(&i) if c.beats(&picked[i]) => picked[i] = c,
            Some(_) => {}
            None => {
                by_key.insert(c.key.clone(), picked.len());
                picked.push(c);
            }
        }
    }
    let mut out: Vec<AppEntry> = picked
        .into_iter()
        .map(|c| c.into_entry(size_in_scan, windows_dir))
        .collect();
    out.sort_by(|a, b| {
        b.size
            .is_some()
            .cmp(&a.size.is_some())
            .then(b.size.cmp(&a.size))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

/// A listed app before sizing, plus what is needed to pick one of several duplicates.
struct Candidate {
    /// Same key = same app: desktop by name and version, Store by package family.
    key: String,
    name: String,
    version: Option<String>,
    /// Store: numeric version parts, newest wins. Desktop: how much the entry tells us.
    rank: Vec<u64>,
    raw: RawApp,
}

impl Candidate {
    fn from_raw(raw: RawApp, windows_dir: &Path) -> Option<Self> {
        if raw.system_component || raw.is_update || !raw.launchable {
            return None;
        }
        match raw.kind {
            AppKind::Desktop => {
                let name = clean(raw.name.as_deref())?;
                let version = clean(raw.version.as_deref());
                let rank = vec![
                    u64::from(raw.install_location.is_some()),
                    u64::from(raw.estimated_kb.is_some()),
                    u64::from(raw.icon.is_some()),
                ];
                Some(Self {
                    key: format!(
                        "d|{}|{}",
                        name.to_lowercase(),
                        version.as_deref().unwrap_or("").to_lowercase()
                    ),
                    name,
                    version,
                    rank,
                    raw,
                })
            }
            AppKind::Store => {
                if raw
                    .install_location
                    .as_deref()
                    .is_some_and(|p| is_under(p, windows_dir))
                {
                    return None;
                }
                let pkg = PackageName::parse(&raw.id);
                let name = clean(raw.name.as_deref())
                    .filter(|n| indirect_source(n, &raw.id).is_none())
                    .unwrap_or_else(|| pkg.name.to_owned());
                Some(Self {
                    key: format!("s|{}", pkg.family.to_lowercase()),
                    name,
                    version: pkg.version.map(str::to_owned),
                    rank: pkg
                        .version
                        .map(|v| v.split('.').map(|n| n.parse().unwrap_or(0)).collect())
                        .unwrap_or_default(),
                    raw,
                })
            }
        }
    }

    fn beats(&self, other: &Self) -> bool {
        self.rank.cmp(&other.rank) == Ordering::Greater
    }

    fn into_entry(
        self,
        size_in_scan: &dyn Fn(&Path) -> Option<u64>,
        windows_dir: &Path,
    ) -> AppEntry {
        let raw = self.raw;
        // Only a folder the app itself names is measured: one guessed from the icon may be
        // a subfolder (or a shared one), and its size would be wrong.
        let named = raw.install_location.filter(|p| is_specific_folder(p));
        let (size, size_source) = match named.as_deref().and_then(size_in_scan) {
            Some(n) => (Some(n), Some(SizeSource::Scan)),
            None => match raw.estimated_kb.filter(|&kb| kb > 0) {
                Some(kb) => (Some(kb.saturating_mul(1024)), Some(SizeSource::Windows)),
                None => (None, None),
            },
        };
        let location = named.or_else(|| {
            raw.icon
                .as_deref()
                .and_then(|i| location_from_icon(i, windows_dir))
        });
        AppEntry {
            id: raw.id,
            name: self.name,
            publisher: clean(raw.publisher.as_deref()),
            version: self.version,
            kind: raw.kind,
            location,
            installed_at: raw.install_date.as_deref().and_then(parse_install_date),
            size,
            size_source,
        }
    }
}

/// The parts of a package full name, `Name_Version_Arch_ResourceId_PublisherId`.
struct PackageName<'a> {
    name: &'a str,
    version: Option<&'a str>,
    /// `Name_PublisherId`: the same app across versions and architectures.
    family: String,
}

impl<'a> PackageName<'a> {
    fn parse(full: &'a str) -> Self {
        let parts: Vec<&str> = full.split('_').collect();
        match parts.as_slice() {
            [name, version, _, _, publisher] => Self {
                name,
                version: Some(version),
                family: format!("{name}_{publisher}"),
            },
            _ => Self {
                name: full,
                version: None,
                family: full.to_owned(),
            },
        }
    }
}

fn clean(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Case-insensitive "is `path` the folder `dir` or inside it", the way Windows compares.
fn is_under(path: &Path, dir: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    let (p, d) = (norm(path), norm(dir));
    !d.is_empty() && (p == d || p.starts_with(&format!("{d}\\")))
}

/// `YYYYMMDD` to Unix milliseconds at midnight UTC. None for anything else.
pub fn parse_install_date(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() != 8 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (y, m, d): (i64, u32, u32) = (
        s[..4].parse().ok()?,
        s[4..6].parse().ok()?,
        s[6..].parse().ok()?,
    );
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days_in_month = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if d == 0 || d > days_in_month {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86_400_000)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The folder an app probably lives in, from its `DisplayIcon`. None for icons kept under
/// the Windows folder (MSI caches them in `C:\Windows\Installer`), which say nothing about it.
pub fn location_from_icon(icon: &str, windows_dir: &Path) -> Option<PathBuf> {
    let icon = icon.trim();
    // `"C:\path\app.exe",0` or `C:\path\app.exe,-101`: the part after the comma is an icon index.
    let file = match icon.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => match icon.rsplit_once(',') {
            Some((path, index)) if index.trim().parse::<i64>().is_ok() => path,
            _ => icon,
        },
    };
    let file = Path::new(file.trim());
    let in_installer_cache = file.components().any(|c| {
        matches!(c, Component::Normal(n) if n.to_string_lossy().eq_ignore_ascii_case("Package Cache"))
    });
    if !file.is_absolute() || is_under(file, windows_dir) || in_installer_cache {
        return None;
    }
    let mut dir = file.parent()?;
    // Executables often sit in `<app>\bin`; the app's folder is the one above.
    if dir
        .file_name()
        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("bin"))
    {
        dir = dir.parent()?;
    }
    is_specific_folder(dir).then(|| dir.to_path_buf())
}

/// True for a folder narrow enough to belong to one app. Rejects drive roots and
/// top-level folders like `C:\Program Files`, which some installers write by mistake.
pub fn is_specific_folder(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .filter(|c| matches!(c, Component::Normal(_)))
            .count()
            >= 2
}

/// What `SHLoadIndirectString` needs to resolve a package's display name, or None when the
/// name is already plain text. Short `ms-resource:` names must be qualified with the package.
pub fn indirect_source(display_name: &str, package_full_name: &str) -> Option<String> {
    let name = display_name.trim();
    if name.starts_with("@{") {
        Some(name.to_owned())
    } else if name.starts_with("ms-resource:") {
        Some(format!("@{{{package_full_name}?{name}}}"))
    } else {
        None
    }
}

#[cfg(windows)]
mod sys {
    use std::path::PathBuf;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
        REG_SAM_FLAGS, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW,
        RegOpenKeyExW,
    };
    use windows_sys::Win32::UI::Shell::SHLoadIndirectString;

    use super::{AppKind, AppSource, RawApp, SystemApps, indirect_source};

    const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    const PACKAGES: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// An open registry key, closed on drop.
    struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: the handle came from a successful RegOpenKeyExW and is closed once.
            unsafe { RegCloseKey(self.0) };
        }
    }

    impl Key {
        fn open(parent: HKEY, path: &str, view: REG_SAM_FLAGS) -> Option<Key> {
            let path = wide(path);
            let mut out: HKEY = null_mut();
            // SAFETY: `path` is NUL-terminated and `out` is a valid out pointer.
            let status =
                unsafe { RegOpenKeyExW(parent, path.as_ptr(), 0, KEY_READ | view, &mut out) };
            (status == ERROR_SUCCESS).then_some(Key(out))
        }

        fn sub(&self, name: &str) -> Option<Key> {
            Key::open(self.0, name, 0)
        }

        /// Names of the direct subkeys. Stops at the first error (normally ERROR_NO_MORE_ITEMS).
        fn subkeys(&self) -> Vec<String> {
            let mut out = Vec::new();
            // Key names are at most 255 characters.
            let mut buf = [0u16; 256];
            for index in 0.. {
                let mut len = buf.len() as u32;
                // SAFETY: `buf` is valid for `len` u16s; the optional out pointers are null.
                let status = unsafe {
                    RegEnumKeyExW(
                        self.0,
                        index,
                        buf.as_mut_ptr(),
                        &mut len,
                        null(),
                        null_mut(),
                        null_mut(),
                        null_mut(),
                    )
                };
                if status != ERROR_SUCCESS {
                    break;
                }
                out.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
            out
        }

        /// A string value; REG_EXPAND_SZ values come back expanded.
        fn string(&self, name: &str) -> Option<String> {
            let name = wide(name);
            let mut bytes = 0u32;
            // SAFETY: a null data pointer asks only for the size.
            let status = unsafe {
                RegGetValueW(
                    self.0,
                    null(),
                    name.as_ptr(),
                    RRF_RT_REG_SZ,
                    null_mut(),
                    null_mut(),
                    &mut bytes,
                )
            };
            if status != ERROR_SUCCESS {
                return None;
            }
            // Expansion can need more room than the size query reported, so retry a few times.
            for _ in 0..3 {
                let mut buf = vec![0u16; (bytes as usize).div_ceil(2) + 1];
                let mut size = (buf.len() * 2) as u32;
                // SAFETY: `buf` is valid for `size` bytes.
                let status = unsafe {
                    RegGetValueW(
                        self.0,
                        null(),
                        name.as_ptr(),
                        RRF_RT_REG_SZ,
                        null_mut(),
                        buf.as_mut_ptr().cast(),
                        &mut size,
                    )
                };
                match status {
                    ERROR_SUCCESS => {
                        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                        return Some(String::from_utf16_lossy(&buf[..len]));
                    }
                    ERROR_MORE_DATA => bytes = size.max(bytes.saturating_mul(2)),
                    _ => return None,
                }
            }
            None
        }

        fn dword(&self, name: &str) -> Option<u32> {
            let name = wide(name);
            let mut data = 0u32;
            let mut size = size_of::<u32>() as u32;
            // SAFETY: `data`/`size` describe a valid 4-byte buffer.
            let status = unsafe {
                RegGetValueW(
                    self.0,
                    null(),
                    name.as_ptr(),
                    RRF_RT_REG_DWORD,
                    null_mut(),
                    (&mut data as *mut u32).cast(),
                    &mut size,
                )
            };
            (status == ERROR_SUCCESS).then_some(data)
        }
    }

    fn desktop_apps(out: &mut Vec<RawApp>) {
        // 64-bit, 32-bit and per-user installs each have their own list.
        let views = [
            (HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY, "HKLM64"),
            (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, "HKLM32"),
            (HKEY_CURRENT_USER, 0, "HKCU"),
        ];
        for (hive, view, label) in views {
            let Some(root) = Key::open(hive, UNINSTALL, view) else {
                continue;
            };
            for sub in root.subkeys() {
                let Some(k) = root.sub(&sub) else { continue };
                let release = k
                    .string("ReleaseType")
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                out.push(RawApp {
                    id: format!(r"{label}\{sub}"),
                    kind: AppKind::Desktop,
                    name: k.string("DisplayName"),
                    publisher: k.string("Publisher"),
                    version: k.string("DisplayVersion"),
                    install_location: k.string("InstallLocation").map(PathBuf::from),
                    icon: k.string("DisplayIcon"),
                    install_date: k.string("InstallDate"),
                    estimated_kb: k.dword("EstimatedSize").map(u64::from),
                    system_component: k.dword("SystemComponent") == Some(1),
                    is_update: k.string("ParentKeyName").is_some()
                        || release.contains("update")
                        || release.contains("hotfix"),
                    launchable: true,
                });
            }
        }
    }

    /// Resolves `@{Package?ms-resource:...}` to the localized text, or None.
    fn load_indirect(source: &str) -> Option<String> {
        let source = wide(source);
        let mut buf = [0u16; 512];
        // SAFETY: `source` is NUL-terminated; `buf` is valid for its length; reserved is null.
        let hr = unsafe {
            SHLoadIndirectString(source.as_ptr(), buf.as_mut_ptr(), buf.len() as u32, null())
        };
        if hr < 0 {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    fn store_apps(out: &mut Vec<RawApp>) {
        let Some(root) = Key::open(HKEY_CURRENT_USER, PACKAGES, 0) else {
            return;
        };
        for full in root.subkeys() {
            let Some(k) = root.sub(&full) else { continue };
            // Each app a package exposes is a subkey; frameworks and resource packs have none.
            let launchable = !k.subkeys().is_empty();
            let raw_name = k.string("DisplayName");
            // Resolving loads the package's resources, so skip it for what will be dropped anyway.
            let name = match (
                launchable,
                raw_name.as_deref().and_then(|n| indirect_source(n, &full)),
            ) {
                (true, Some(src)) => load_indirect(&src).or(raw_name),
                _ => raw_name,
            };
            out.push(RawApp {
                id: full.clone(),
                kind: AppKind::Store,
                name,
                install_location: k.string("PackageRootFolder").map(PathBuf::from),
                launchable,
                ..RawApp::default()
            });
        }
    }

    impl AppSource for SystemApps {
        fn apps(&self) -> Result<Vec<RawApp>, String> {
            let mut out = Vec::new();
            desktop_apps(&mut out);
            store_apps(&mut out);
            Ok(out)
        }
    }
}

#[cfg(not(windows))]
impl AppSource for SystemApps {
    fn apps(&self) -> Result<Vec<RawApp>, String> {
        Err("unsupported".into())
    }
}

// Windows paths throughout, so the tests only run there.
#[cfg(all(test, windows))]
mod tests {
    use std::cell::RefCell;

    use super::*;

    struct Fake(Result<Vec<RawApp>, String>);

    impl AppSource for Fake {
        fn apps(&self) -> Result<Vec<RawApp>, String> {
            self.0.clone()
        }
    }

    fn win() -> PathBuf {
        PathBuf::from(r"C:\Windows")
    }

    fn desktop(id: &str, name: &str) -> RawApp {
        RawApp {
            id: id.to_owned(),
            kind: AppKind::Desktop,
            name: Some(name.to_owned()),
            launchable: true,
            ..RawApp::default()
        }
    }

    fn store(full_name: &str, name: Option<&str>, root: &str) -> RawApp {
        RawApp {
            id: full_name.to_owned(),
            kind: AppKind::Store,
            name: name.map(str::to_owned),
            install_location: Some(PathBuf::from(root)),
            launchable: true,
            ..RawApp::default()
        }
    }

    fn no_scan(_: &Path) -> Option<u64> {
        None
    }

    fn list(raw: Vec<RawApp>) -> Vec<AppEntry> {
        list_apps(&Fake(Ok(raw)), &no_scan, &win()).unwrap()
    }

    fn names(apps: &[AppEntry]) -> Vec<&str> {
        apps.iter().map(|a| a.name.as_str()).collect()
    }

    #[test]
    fn hidden_update_nameless_and_unlaunchable_entries_are_left_out() {
        let mut sys = desktop("k1", "VC++ Runtime part");
        sys.system_component = true;
        let mut upd = desktop("k2", "Security Update for Office");
        upd.is_update = true;
        let mut nameless = desktop("k3", "x");
        nameless.name = None;
        let mut blank = desktop("k4", "x");
        blank.name = Some("   ".into());
        let mut framework = store(
            "Microsoft.VCLibs.140.00_14.0.33519.0_x64__8wekyb3d8bbwe",
            Some("VCLibs"),
            r"C:\Program Files\WindowsApps\Microsoft.VCLibs",
        );
        framework.launchable = false;
        let apps = list(vec![
            sys,
            upd,
            nameless,
            blank,
            framework,
            desktop("k5", "7-Zip"),
        ]);
        assert_eq!(names(&apps), ["7-Zip"]);
    }

    #[test]
    fn store_packages_inside_the_windows_folder_are_left_out() {
        let apps = list(vec![
            store(
                "Microsoft.Windows.FilePicker_10.0.1_neutral_neutral_cw5n1h2txyewy",
                Some("File Picker"),
                r"c:\windows\SystemApps\Microsoft.Windows.FilePicker_cw5n1h2txyewy",
            ),
            store(
                "Nahimic_1.10.15.0_x64__w2gh52qy24etm",
                Some("Nahimic"),
                r"C:\Program Files\WindowsApps\Nahimic_1.10.15.0_x64__w2gh52qy24etm",
            ),
        ]);
        assert_eq!(names(&apps), ["Nahimic"]);
    }

    #[test]
    fn store_version_comes_from_the_package_name_and_the_newest_version_wins() {
        let apps = list(vec![
            store(
                "Acme.Notes_1.9.0.0_x64__abc",
                Some("Notes"),
                r"C:\Program Files\WindowsApps\Acme.Notes_1.9.0.0_x64__abc",
            ),
            store(
                "Acme.Notes_1.10.0.0_x64__abc",
                Some("Notes"),
                r"C:\Program Files\WindowsApps\Acme.Notes_1.10.0.0_x64__abc",
            ),
            // Same name, other publisher: a different app.
            store(
                "Acme.Notes_0.1.0.0_x64__zzz",
                Some("Notes"),
                r"C:\Program Files\WindowsApps\Acme.Notes_0.1.0.0_x64__zzz",
            ),
        ]);
        let mut versions: Vec<_> = apps.iter().map(|a| a.version.clone().unwrap()).collect();
        versions.sort();
        assert_eq!(versions, ["0.1.0.0", "1.10.0.0"]);
        assert!(apps.iter().all(|a| a.kind == AppKind::Store));
    }

    #[test]
    fn unresolved_store_names_fall_back_to_the_package_name() {
        let apps = list(vec![
            store(
                "C27EB4BA.DropboxOEM_23.4.38.0_x64__xbfy0k16fey96",
                Some("ms-resource:OEMAppName"),
                r"C:\Program Files\WindowsApps\C27EB4BA.DropboxOEM_23.4.38.0_x64__xbfy0k16fey96",
            ),
            store(
                "Acme.Paint_2.0.0.0_x64__abc",
                None,
                r"C:\Program Files\WindowsApps\Acme.Paint_2.0.0.0_x64__abc",
            ),
            store(
                "Acme.Draw_2.0.0.0_x64__abc",
                Some("@{Acme.Draw_2.0.0.0_x64__abc?ms-resource://Acme.Draw/Resources/Name}"),
                r"C:\Program Files\WindowsApps\Acme.Draw_2.0.0.0_x64__abc",
            ),
        ]);
        let mut got = names(&apps);
        got.sort();
        assert_eq!(got, ["Acme.Draw", "Acme.Paint", "C27EB4BA.DropboxOEM"]);
    }

    #[test]
    fn the_same_desktop_app_in_two_registry_views_is_listed_once_keeping_the_richer_entry() {
        let mut bare = desktop(r"HKLM32\Git_is1", "Git");
        bare.version = Some("2.45.2".into());
        let mut rich = desktop(r"HKLM64\Git_is1", "git");
        rich.version = Some("2.45.2".into());
        rich.install_location = Some(PathBuf::from(r"C:\Program Files\Git"));
        rich.estimated_kb = Some(100);
        let mut other_version = desktop(r"HKCU\Git", "Git");
        other_version.version = Some("2.50.0".into());
        let apps = list(vec![bare, rich, other_version]);
        assert_eq!(apps.len(), 2);
        let old = apps
            .iter()
            .find(|a| a.version.as_deref() == Some("2.45.2"))
            .unwrap();
        assert_eq!(old.id, r"HKLM64\Git_is1");
        assert_eq!(old.size, Some(100 * 1024));
    }

    #[test]
    fn scan_size_beats_the_registry_estimate_which_beats_nothing() {
        let mut scanned = desktop("a", "Scanned");
        scanned.install_location = Some(PathBuf::from(r"C:\Tools\Scanned"));
        scanned.estimated_kb = Some(1);
        let mut estimated = desktop("b", "Estimated");
        estimated.install_location = Some(PathBuf::from(r"D:\Elsewhere\Estimated"));
        estimated.estimated_kb = Some(2);
        let mut zero = desktop("c", "Zero estimate");
        zero.estimated_kb = Some(0);
        let unknown = desktop("d", "Unknown");

        let asked = RefCell::new(Vec::new());
        let in_scan = |p: &Path| {
            asked.borrow_mut().push(p.to_path_buf());
            (p == Path::new(r"C:\Tools\Scanned")).then_some(5_000_000)
        };
        let apps = list_apps(
            &Fake(Ok(vec![unknown, zero, estimated, scanned])),
            &in_scan,
            &win(),
        )
        .unwrap();
        let got: Vec<_> = apps
            .iter()
            .map(|a| (a.name.as_str(), a.size, a.size_source))
            .collect();
        assert_eq!(
            got,
            [
                ("Scanned", Some(5_000_000), Some(SizeSource::Scan)),
                ("Estimated", Some(2048), Some(SizeSource::Windows)),
                ("Unknown", None, None),
                ("Zero estimate", None, None),
            ]
        );
    }

    #[test]
    fn a_folder_guessed_from_the_icon_is_shown_but_never_measured() {
        let mut app = desktop("a", "Android Studio");
        app.icon = Some(r"C:\Program Files\Android\Android Studio\bin\studio.exe".into());
        app.estimated_kb = Some(3);
        let in_scan = |_: &Path| Some(999);
        let apps = list_apps(&Fake(Ok(vec![app])), &in_scan, &win()).unwrap();
        assert_eq!(
            apps[0].location.as_deref(),
            Some(Path::new(r"C:\Program Files\Android\Android Studio"))
        );
        assert_eq!(apps[0].size, Some(3 * 1024));
        assert_eq!(apps[0].size_source, Some(SizeSource::Windows));
    }

    #[test]
    fn a_too_broad_install_location_is_ignored() {
        let mut app = desktop("a", "Sloppy");
        app.install_location = Some(PathBuf::from(r"C:\Program Files\"));
        app.icon = Some(r"C:\Program Files\Sloppy\sloppy.exe,0".into());
        let in_scan = |_: &Path| Some(999);
        let apps = list_apps(&Fake(Ok(vec![app])), &in_scan, &win()).unwrap();
        assert_eq!(
            apps[0].location.as_deref(),
            Some(Path::new(r"C:\Program Files\Sloppy"))
        );
        assert_eq!(apps[0].size, None);
    }

    #[test]
    fn sorted_biggest_first_unknown_last_ties_by_name_ignoring_case() {
        let sized = |id: &str, name: &str, kb: Option<u64>| {
            let mut a = desktop(id, name);
            a.estimated_kb = kb;
            a
        };
        let apps = list(vec![
            sized("1", "none-b", None),
            sized("2", "small", Some(1)),
            sized("3", "Tie-b", Some(5)),
            sized("4", "None-a", None),
            sized("5", "big", Some(90)),
            sized("6", "tie-a", Some(5)),
        ]);
        assert_eq!(
            names(&apps),
            ["big", "tie-a", "Tie-b", "small", "None-a", "none-b"]
        );
    }

    #[test]
    fn names_publishers_and_versions_are_trimmed_and_blank_ones_dropped() {
        let mut app = desktop("a", "  Docker Desktop ");
        app.publisher = Some("  ".into());
        app.version = Some(" 4.48.0 ".into());
        app.install_date = Some("20240712".into());
        let apps = list(vec![app]);
        assert_eq!(apps[0].name, "Docker Desktop");
        assert_eq!(apps[0].publisher, None);
        assert_eq!(apps[0].version.as_deref(), Some("4.48.0"));
        assert_eq!(apps[0].installed_at, Some(1_720_742_400_000));
    }

    #[test]
    fn source_errors_are_passed_on() {
        let err = list_apps(&Fake(Err("registry said no".into())), &no_scan, &win()).unwrap_err();
        assert_eq!(err, "registry said no");
    }

    #[test]
    fn install_dates() {
        assert_eq!(parse_install_date("19700101"), Some(0));
        assert_eq!(parse_install_date("20240712"), Some(1_720_742_400_000));
        assert_eq!(parse_install_date("20000229"), Some(951_782_400_000));
        assert_eq!(parse_install_date(" 20240712 "), Some(1_720_742_400_000));
        for bad in [
            "",
            "2024071",
            "2024-07-12",
            "20241301",
            "20240732",
            "20230229",
            "abcdefgh",
        ] {
            assert_eq!(parse_install_date(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn icon_locations() {
        let w = win();
        let loc = |s: &str| location_from_icon(s, &w).map(|p| p.to_string_lossy().into_owned());
        assert_eq!(
            loc(r#""C:\Program Files\7-Zip\7zFM.exe",0"#).as_deref(),
            Some(r"C:\Program Files\7-Zip")
        );
        assert_eq!(
            loc(r"C:\Program Files\Git\mingw64\share\git\git.ico").as_deref(),
            Some(r"C:\Program Files\Git\mingw64\share\git")
        );
        assert_eq!(
            loc(r"C:\Tools\App\bin\app.exe,-101").as_deref(),
            Some(r"C:\Tools\App")
        );
        assert_eq!(
            loc(r"C:\Tools\App\BIN\app.exe").as_deref(),
            Some(r"C:\Tools\App")
        );
        assert_eq!(loc(r"C:\Windows\Installer\{123}\icon.ico"), None);
        assert_eq!(loc(r"c:\windows\system32\msiexec.exe"), None);
        // Bundle installers point the icon at their cached setup, not at the app.
        assert_eq!(
            loc(r"C:\ProgramData\Package Cache\{6E4A}\dotnet-sdk.exe,0"),
            None
        );
        assert_eq!(
            loc(r"C:\Users\me\AppData\Local\package cache\{3EBA}\PowerToysSetup.exe"),
            None
        );
        assert_eq!(loc(r"C:\app.exe"), None, "drive root is too broad");
        assert_eq!(
            loc(r"C:\Program Files\app.exe"),
            None,
            "top-level folder is too broad"
        );
        assert_eq!(loc("app.exe"), None, "relative");
        assert_eq!(loc(""), None);
    }

    #[test]
    fn specific_folders() {
        assert!(is_specific_folder(Path::new(r"C:\Program Files\7-Zip")));
        assert!(is_specific_folder(Path::new(r"C:\Program Files\7-Zip\")));
        assert!(is_specific_folder(Path::new(r"D:\Games\Thing")));
        assert!(!is_specific_folder(Path::new(r"C:\")));
        assert!(!is_specific_folder(Path::new(r"C:\Program Files")));
        assert!(!is_specific_folder(Path::new(r"C:\Program Files\")));
        assert!(!is_specific_folder(Path::new(r"relative\folder")));
        assert!(!is_specific_folder(Path::new("")));
    }

    #[test]
    fn indirect_name_sources() {
        let full = "C27EB4BA.DropboxOEM_23.4.38.0_x64__xbfy0k16fey96";
        assert_eq!(
            indirect_source("ms-resource:OEMAppName", full).as_deref(),
            Some("@{C27EB4BA.DropboxOEM_23.4.38.0_x64__xbfy0k16fey96?ms-resource:OEMAppName}")
        );
        let qualified = "@{X_1_x64__p?ms-resource://X/Resources/Name}";
        assert_eq!(indirect_source(qualified, full).as_deref(), Some(qualified));
        assert_eq!(indirect_source("Nahimic", full), None);
    }

    #[test]
    fn json_keys_match_the_ui_types() {
        let mut app = desktop("a", "A");
        app.estimated_kb = Some(1);
        let v = serde_json::to_value(&list(vec![app])[0]).unwrap();
        assert_eq!(v["kind"], "desktop");
        assert_eq!(v["sizeSource"], "windows");
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "id",
                "installedAt",
                "kind",
                "location",
                "name",
                "publisher",
                "size",
                "sizeSource",
                "version"
            ]
        );
    }

    // Read-only against the real registry.
    #[test]
    #[ignore = "reads the real installed-apps registry"]
    fn real_apps_can_be_listed() {
        let windows = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(win);
        let t = std::time::Instant::now();
        let apps = list_apps(&SystemApps, &no_scan, &windows).unwrap();
        let store = apps.iter().filter(|a| a.kind == AppKind::Store).count();
        let located = apps.iter().filter(|a| a.location.is_some()).count();
        println!(
            "{} apps ({store} Store, {located} with a folder) in {:?}",
            apps.len(),
            t.elapsed()
        );
        for a in apps.iter().take(15) {
            println!("{:>12?}  {:<40} {:?}", a.size, a.name, a.location);
        }
        assert!(!apps.is_empty());
    }
}
