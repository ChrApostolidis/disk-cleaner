//! Search bar queries (wildcards, AND terms, size and age filters) over files and projects.
//!
//! Syntax: whitespace-separated tokens, `"double quotes"` group words into one term.
//! `>500MB` / `<10KB` bound the size (both bounds are strict, so `>1KB` excludes exactly 1 KB),
//! `older:90d` / `newer:1h` bound the age, anything else is a term. Terms containing `*` or `?`
//! must match the whole text; plain terms match as substrings. Everything is case-insensitive.

use std::cmp::Ordering;
use std::time::{Duration, SystemTime};

use crate::index::{NodeId, NodeKind, Tree};
use crate::projects::Project;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// `*`: any run of characters, including none.
    Star,
    /// `?`: exactly one character.
    Any,
    /// A literal, already case-folded.
    Lit(char),
}

/// A search term compiled once at parse time so matching never re-parses the pattern.
#[derive(Debug, Clone, PartialEq)]
struct Term {
    toks: Vec<Tok>,
    wildcard: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    terms: Vec<Term>,
    /// Exclusive lower bound (`>`).
    size_over: Option<u64>,
    /// Exclusive upper bound (`<`).
    size_under: Option<u64>,
    /// `older:`: modified before `now - d`.
    older: Option<Duration>,
    /// `newer:`: modified at or after `now - d`.
    newer: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    #[error("invalid size filter `{0}` (try >500MB)")]
    BadSize(String),
    #[error("invalid age filter `{0}` (try older:90d or newer:1h)")]
    BadAge(String),
}

fn fold(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

/// Splits on whitespace outside quotes. The flag says the token contained a quote, which makes
/// it a plain term even if it looks like a filter.
fn tokenize(input: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut in_quote = false;
    let mut has_token = false;
    for c in input.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                quoted = true;
                has_token = true;
            }
            c if c.is_whitespace() && !in_quote => {
                if has_token {
                    out.push((std::mem::take(&mut cur), quoted));
                }
                quoted = false;
                has_token = false;
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        out.push((cur, quoted));
    }
    out
}

fn parse_size(token: &str) -> Option<u64> {
    let s = &token[1..];
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    if num.is_empty() {
        return None;
    }
    let mult: u64 = match unit.to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "kb" => 1 << 10,
        "mb" => 1 << 20,
        "gb" => 1 << 30,
        "tb" => 1 << 40,
        _ => return None,
    };
    let n: f64 = num.parse().ok()?;
    Some((n * mult as f64).round() as u64)
}

fn parse_age(value: &str) -> Option<Duration> {
    let unit = value.chars().last()?;
    let digits = &value[..value.len() - unit.len_utf8()];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u64 = digits.parse().ok()?;
    let secs = match unit.to_ascii_lowercase() {
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        'w' => 7 * 86_400,
        'y' => 365 * 86_400,
        _ => return None,
    };
    Some(Duration::from_secs(n.checked_mul(secs)?))
}

fn compile_term(text: &str) -> Term {
    let wildcard = text.contains(['*', '?']);
    let toks = text
        .chars()
        .map(|c| match c {
            '*' if wildcard => Tok::Star,
            '?' if wildcard => Tok::Any,
            c => Tok::Lit(fold(c)),
        })
        .collect();
    Term { toks, wildcard }
}

/// Whole-text wildcard match with single-star backtracking: linear in practice, no recursion.
fn glob<T: Copy>(toks: &[Tok], text: &[T], eq: impl Fn(char, T) -> bool) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        let step = match toks.get(p) {
            Some(Tok::Star) => {
                star = Some((p + 1, t));
                p += 1;
                continue;
            }
            Some(Tok::Any) => true,
            Some(&Tok::Lit(c)) => eq(c, text[t]),
            None => false,
        };
        if step {
            p += 1;
            t += 1;
        } else if let Some((sp, st)) = star {
            p = sp;
            t = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    toks[p..].iter().all(|t| *t == Tok::Star)
}

fn substring<T: Copy>(toks: &[Tok], text: &[T], eq: impl Fn(char, T) -> bool) -> bool {
    let n = toks.len();
    if n == 0 {
        return true;
    }
    if n > text.len() {
        return false;
    }
    text.windows(n).any(|w| {
        toks.iter().zip(w).all(|(tok, &t)| match tok {
            Tok::Lit(c) => eq(*c, t),
            _ => false,
        })
    })
}

impl Term {
    fn run<T: Copy>(&self, text: &[T], eq: impl Fn(char, T) -> bool) -> bool {
        if self.wildcard {
            glob(&self.toks, text, eq)
        } else {
            substring(&self.toks, text, eq)
        }
    }

    fn matches(&self, text: &str) -> bool {
        if text.is_ascii() {
            // Names are overwhelmingly ASCII; comparing bytes avoids allocating a folded copy.
            self.run(text.as_bytes(), |c, b| c == b.to_ascii_lowercase() as char)
        } else {
            let folded: Vec<char> = text.chars().map(fold).collect();
            self.run(&folded, |c, t| c == t)
        }
    }
}

impl Query {
    pub fn parse(input: &str) -> Result<Query, QueryError> {
        let mut q = Query::default();
        for (token, quoted) in tokenize(input) {
            if quoted {
                if !token.is_empty() {
                    q.terms.push(compile_term(&token));
                }
                continue;
            }
            let lower = token.to_ascii_lowercase();
            if token.starts_with(['>', '<']) {
                let v = parse_size(&token).ok_or_else(|| QueryError::BadSize(token.clone()))?;
                if token.starts_with('>') {
                    q.size_over = Some(q.size_over.map_or(v, |o| o.max(v)));
                } else {
                    q.size_under = Some(q.size_under.map_or(v, |o| o.min(v)));
                }
            } else if let Some(v) = lower.strip_prefix("older:") {
                let d = parse_age(v).ok_or_else(|| QueryError::BadAge(token.clone()))?;
                q.older = Some(q.older.map_or(d, |o| o.max(d)));
            } else if let Some(v) = lower.strip_prefix("newer:") {
                let d = parse_age(v).ok_or_else(|| QueryError::BadAge(token.clone()))?;
                q.newer = Some(q.newer.map_or(d, |o| o.min(d)));
            } else {
                q.terms.push(compile_term(&token));
            }
        }
        Ok(q)
    }

    pub fn is_empty(&self) -> bool {
        *self == Query::default()
    }

    /// Every term matches at least one of `texts`; size/age filters apply to `size` and
    /// `modified`. Ages are relative to `now`. Size bounds are strict (`>` and `<`), and an
    /// unknown `modified` never satisfies an age filter.
    pub fn matches(
        &self,
        texts: &[&str],
        size: u64,
        modified: Option<SystemTime>,
        now: SystemTime,
    ) -> bool {
        if self.size_over.is_some_and(|min| size <= min)
            || self.size_under.is_some_and(|max| size >= max)
        {
            return false;
        }
        if let Some(d) = self.older {
            // A window reaching before the clock's origin has nothing older than it.
            match (modified, now.checked_sub(d)) {
                (Some(m), Some(cutoff)) if m < cutoff => {}
                _ => return false,
            }
        }
        if let Some(d) = self.newer {
            match (modified, now.checked_sub(d)) {
                (Some(m), Some(cutoff)) if m >= cutoff => {}
                (Some(_), None) => {}
                _ => return false,
            }
        }
        self.terms
            .iter()
            .all(|term| texts.iter().any(|t| term.matches(t)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSort {
    Allocated,
    Size,
    Modified,
    Name,
}

fn cmp_name_ci(a: &str, b: &str) -> Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
}

/// Files only (no dirs). texts = [file name]; size filter uses `allocated`.
/// Returns at most `limit` ids ordered by `sort` (Name: case-insensitive; Modified: None sorts
/// last in both directions). Ties are broken by NodeId for stable output.
pub fn find_files(
    tree: &Tree,
    query: &Query,
    sort: FileSort,
    descending: bool,
    limit: usize,
    now: SystemTime,
) -> Vec<NodeId> {
    if limit == 0 {
        return Vec::new();
    }
    let mut hits: Vec<NodeId> = (0..tree.len())
        .filter(|&id| {
            let n = tree.node(id);
            n.kind == NodeKind::File
                && query.matches(&[n.name.as_str()], n.allocated, n.modified, now)
        })
        .collect();

    let dir = |o: Ordering| if descending { o.reverse() } else { o };
    let cmp = |&a: &NodeId, &b: &NodeId| {
        let (na, nb) = (tree.node(a), tree.node(b));
        let ord = match sort {
            FileSort::Allocated => dir(na.allocated.cmp(&nb.allocated)),
            FileSort::Size => dir(na.size.cmp(&nb.size)),
            FileSort::Name => dir(cmp_name_ci(&na.name, &nb.name)),
            FileSort::Modified => match (na.modified, nb.modified) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => return Ordering::Greater,
                (Some(_), None) => return Ordering::Less,
                (Some(x), Some(y)) => dir(x.cmp(&y)),
            },
        };
        ord.then(a.cmp(&b))
    };

    // The order is total (NodeId breaks ties), so partial selection yields the same top
    // `limit` as a full sort without sorting millions of hits on every keystroke.
    if limit < hits.len() {
        hits.select_nth_unstable_by(limit - 1, cmp);
        hits.truncate(limit);
    }
    hits.sort_unstable_by(cmp);
    hits
}

/// Indexes into `projects` that match, biggest `reclaimable_allocated` first.
/// texts = [project folder name, framework/ecosystem label]; size uses `total_allocated`;
/// age uses `last_activity`.
pub fn filter_projects(
    tree: &Tree,
    projects: &[Project],
    query: &Query,
    now: SystemTime,
) -> Vec<usize> {
    let mut hits: Vec<usize> = projects
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            let path = tree.path(p.node);
            let folder = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            query.matches(
                &[folder.as_str(), p.info.label.as_str()],
                p.total_allocated,
                p.last_activity,
                now,
            )
        })
        .map(|(i, _)| i)
        .collect();
    hits.sort_by(|&a, &b| {
        projects[b]
            .reclaimable_allocated
            .cmp(&projects[a].reclaimable_allocated)
            .then(a.cmp(&b))
    });
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::DetectedProject;
    use crate::index::TreeBuilder;
    use std::path::Path;

    const DAY: u64 = 86_400;

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000)
    }

    fn ago(secs: u64) -> Option<SystemTime> {
        Some(now() - Duration::from_secs(secs))
    }

    fn q(s: &str) -> Query {
        Query::parse(s).unwrap()
    }

    fn m(query: &str, name: &str) -> bool {
        q(query).matches(&[name], 0, None, now())
    }

    // ---- parsing ----

    #[test]
    fn empty_queries_match_everything() {
        for s in ["", "   ", "\t \n"] {
            let query = q(s);
            assert!(query.is_empty());
            assert!(query.matches(&["anything"], 5, None, now()));
        }
        assert!(!q("a").is_empty());
        assert!(!q(">1").is_empty());
    }

    #[test]
    fn size_units_and_decimals() {
        let size_ok = |query: &str, size: u64| q(query).matches(&["x"], size, None, now());
        assert!(size_ok(">100", 101) && !size_ok(">100", 100));
        assert!(size_ok(">10kb", 10_241) && !size_ok(">10kb", 10_240));
        assert!(size_ok(">500MB", 500 * 1024 * 1024 + 1));
        assert!(!size_ok(">500MB", 500 * 1024 * 1024));
        assert!(size_ok(">1.5GB", 1_610_612_737) && !size_ok(">1.5GB", 1_610_612_736));
        assert!(size_ok(">1tb", (1 << 40) + 1) && !size_ok(">1TB", 1 << 40));
        assert!(size_ok(">5b", 6) && !size_ok(">5B", 5));
    }

    #[test]
    fn size_range_is_strict_on_both_ends() {
        let query = q(">1KB <10KB");
        let ok = |s| query.matches(&["x"], s, None, now());
        assert!(!ok(1024) && ok(1025) && ok(10_239) && !ok(10_240));
        // Two bounds of the same kind: the tighter one wins.
        let tight = q(">1KB >2KB <10KB <5KB");
        assert!(!tight.matches(&["x"], 2048, None, now()));
        assert!(tight.matches(&["x"], 2049, None, now()));
        assert!(!tight.matches(&["x"], 5120, None, now()));
    }

    #[test]
    fn bad_filters_are_errors_naming_the_token() {
        for bad in [">abc", ">5XB", ">", "<", "<1.2.3", ">-5", ">MB"] {
            assert_eq!(
                Query::parse(&format!("foo {bad}")),
                Err(QueryError::BadSize(bad.to_string())),
                "{bad}"
            );
        }
        for bad in [
            "older:5q",
            "older:",
            "newer:",
            "older:d",
            "newer:1.5h",
            "older:-3d",
        ] {
            assert_eq!(
                Query::parse(bad),
                Err(QueryError::BadAge(bad.to_string())),
                "{bad}"
            );
        }
        let msg = Query::parse(">abc").unwrap_err().to_string();
        assert!(msg.contains(">abc"));
    }

    #[test]
    fn quoted_segments_are_single_terms() {
        assert!(m("\"two words\"", "my two words.txt"));
        assert!(!m("\"two words\"", "two-words.txt"));
        assert!(m("\"two words\" my", "my two words.txt"));
        // Quoting protects filter-looking text.
        assert!(m("\">5\"", "a>5b"));
        assert!(q("\"\"").is_empty());
    }

    // ---- matching ----

    #[test]
    fn substring_is_case_insensitive() {
        assert!(m("read", "README.md"));
        assert!(m("ÄB", "xäbx"));
        assert!(!m("zzz", "README.md"));
    }

    #[test]
    fn wildcards_match_the_whole_text() {
        assert!(m("*.mp3", "song.MP3"));
        assert!(!m("*.mp3", "song.mp3.bak"));
        assert!(m("a?c", "abc") && m("a?c", "AXC"));
        assert!(!m("a?c", "ac") && !m("a?c", "abbc"));
        assert!(m("*", "") && m("a*", "a") && m("a*b*c", "aXXbYYc"));
        assert!(!m("a*b*c", "aXXbYY"));
        assert!(m("*a*a*", "banana") && !m("*a*a*a*a", "banana"));
        assert!(m("ä?x", "ÄBX"));
    }

    #[test]
    fn terms_are_anded_across_texts() {
        let query = q("*.mp3 dance");
        assert!(query.matches(&["dance-mix.mp3"], 0, None, now()));
        assert!(!query.matches(&["dance-mix.wav"], 0, None, now()));
        assert!(!query.matches(&["mix.mp3"], 0, None, now()));
        // Each term may be satisfied by a different text.
        let query = q("next.js shop");
        assert!(query.matches(&["my-shop", "Next.js"], 0, None, now()));
        assert!(!query.matches(&["my-blog", "Next.js"], 0, None, now()));
    }

    #[test]
    fn age_filters_use_now_and_skip_unknown_times() {
        let older = q("older:90d");
        assert!(older.matches(&["x"], 0, ago(91 * DAY), now()));
        assert!(!older.matches(&["x"], 0, ago(89 * DAY), now()));
        assert!(!older.matches(&["x"], 0, ago(90 * DAY), now()));
        assert!(!older.matches(&["x"], 0, None, now()));

        let newer = q("newer:1h");
        assert!(newer.matches(&["x"], 0, ago(59 * 60), now()));
        assert!(newer.matches(&["x"], 0, ago(3600), now()));
        assert!(!newer.matches(&["x"], 0, ago(3601), now()));
        assert!(!newer.matches(&["x"], 0, None, now()));

        assert!(q("newer:30m").matches(&["x"], 0, ago(29 * 60), now()));
        assert!(q("older:2w").matches(&["x"], 0, ago(15 * DAY), now()));
        assert!(!q("older:2w").matches(&["x"], 0, ago(13 * DAY), now()));
        assert!(q("OLDER:1Y").matches(&["x"], 0, ago(366 * DAY), now()));
        assert!(!q("older:1y").matches(&["x"], 0, ago(364 * DAY), now()));
    }

    #[test]
    fn age_window_combines_both_filters() {
        let query = q("older:1d newer:7d");
        assert!(query.matches(&["x"], 0, ago(3 * DAY), now()));
        assert!(!query.matches(&["x"], 0, ago(8 * DAY), now()));
        assert!(!query.matches(&["x"], 0, ago(DAY / 2), now()));
    }

    #[test]
    fn age_longer_than_the_epoch_does_not_panic() {
        let tiny_now = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let some = Some(SystemTime::UNIX_EPOCH);
        assert!(!q("older:1y").matches(&["x"], 0, some, tiny_now));
        assert!(q("newer:1y").matches(&["x"], 0, some, tiny_now));
    }

    // ---- find_files ----

    struct Fixture {
        tree: Tree,
        dir: NodeId,
        a: NodeId,
        b: NodeId,
        c: NodeId,
        d: NodeId,
    }

    fn fixture() -> Fixture {
        let mut tb = TreeBuilder::new(Path::new("/root"), None);
        let root = tb.root();
        let dir = tb.add_dir(root, "big.dir", None);
        // name, size, allocated, modified
        let a = tb.add_file(root, "banana.txt", 10, 4096, ago(DAY));
        let b = tb.add_file(dir, "Apple.TXT", 3000, 8192, ago(10 * DAY));
        let c = tb.add_file(root, "cherry.bin", 500, 4096, None);
        let d = tb.add_file(root, "date.txt", 20, 12288, ago(2 * DAY));
        Fixture {
            tree: tb.finish(),
            dir,
            a,
            b,
            c,
            d,
        }
    }

    fn find(f: &Fixture, query: &str, sort: FileSort, desc: bool, limit: usize) -> Vec<NodeId> {
        find_files(&f.tree, &q(query), sort, desc, limit, now())
    }

    #[test]
    fn find_files_excludes_dirs() {
        let f = fixture();
        let all = find(&f, "", FileSort::Name, false, 100);
        assert_eq!(all.len(), 4);
        assert!(!all.contains(&f.dir) && !all.contains(&f.tree.root()));
        assert!(find(&f, "big", FileSort::Name, false, 100).is_empty());
    }

    #[test]
    fn find_files_sorts_both_directions() {
        let f = fixture();
        let (a, b, c, d) = (f.a, f.b, f.c, f.d);
        // Allocated ties (a, c at 4096) fall back to NodeId ascending in both directions.
        let ids = |s, desc| find(&f, "", s, desc, 9);
        assert_eq!(ids(FileSort::Allocated, true), vec![d, b, a, c]);
        assert_eq!(ids(FileSort::Allocated, false), vec![a, c, b, d]);
        assert_eq!(ids(FileSort::Size, true), vec![b, c, d, a]);
        assert_eq!(ids(FileSort::Size, false), vec![a, d, c, b]);
        // Case-insensitive names: Apple, banana, cherry, date.
        assert_eq!(ids(FileSort::Name, false), vec![b, a, c, d]);
        assert_eq!(ids(FileSort::Name, true), vec![d, c, a, b]);
    }

    #[test]
    fn find_files_modified_puts_unknown_last_both_ways() {
        let f = fixture();
        let (a, b, c, d) = (f.a, f.b, f.c, f.d);
        assert_eq!(find(&f, "", FileSort::Modified, true, 9), vec![a, d, b, c]);
        assert_eq!(find(&f, "", FileSort::Modified, false, 9), vec![b, d, a, c]);
    }

    #[test]
    fn find_files_applies_filters_and_limit() {
        let f = fixture();
        // Size filter uses allocated: only b (8192) and d (12288) exceed 4KB.
        let by_alloc = |s| find(&f, s, FileSort::Allocated, true, 9);
        assert_eq!(by_alloc(">4KB"), vec![f.d, f.b]);
        assert_eq!(
            find(&f, "*.txt", FileSort::Name, false, 9),
            vec![f.b, f.a, f.d]
        );
        assert_eq!(by_alloc("older:5d"), vec![f.b]);
        assert_eq!(find(&f, "", FileSort::Allocated, true, 2), vec![f.d, f.b]);
        assert_eq!(find(&f, "", FileSort::Name, false, 1), vec![f.b]);
        assert!(find(&f, "", FileSort::Name, false, 0).is_empty());
    }

    #[test]
    fn limit_selection_matches_full_sort() {
        let mut tb = TreeBuilder::new(Path::new("/r"), None);
        let root = tb.root();
        for i in 0..200u64 {
            tb.add_file(root, &format!("f{i}"), 0, (i * 7919) % 101, None);
        }
        let tree = tb.finish();
        let query = Query::default();
        let full = find_files(&tree, &query, FileSort::Allocated, true, usize::MAX, now());
        assert_eq!(full.len(), 200);
        for limit in [1, 5, 50, 199, 200, 500] {
            let part = find_files(&tree, &query, FileSort::Allocated, true, limit, now());
            assert_eq!(part, full[..limit.min(full.len())]);
        }
    }

    // ---- filter_projects ----

    fn project(tree: &Tree, node: NodeId, label: &str, total: u64, reclaim: u64) -> Project {
        Project {
            node,
            info: DetectedProject {
                root: tree.path(node),
                ecosystems: vec![],
                framework: None,
                label: label.to_string(),
                version: None,
                package_manager: None,
                has_lockfile: false,
                regenerable: vec![],
                reinstall_command: None,
            },
            parent: None,
            total_allocated: total,
            reclaimable_allocated: reclaim,
            last_activity: ago(100 * DAY),
        }
    }

    fn project_fixture() -> (Tree, Vec<Project>) {
        let mut tb = TreeBuilder::new(Path::new("/root"), None);
        let root = tb.root();
        let shop = tb.add_dir(root, "my-shop", None);
        let blog = tb.add_dir(root, "Blog", None);
        let tool = tb.add_dir(root, "tool", None);
        let tree = tb.finish();
        let projects = vec![
            project(&tree, shop, "Next.js", 1000, 300),
            project(&tree, blog, "Rust", 5000, 900),
            project(&tree, tool, "Next.js", 100, 50),
        ];
        (tree, projects)
    }

    #[test]
    fn projects_match_label_and_folder_name() {
        let (tree, ps) = project_fixture();
        let f = |s: &str| filter_projects(&tree, &ps, &q(s), now());
        assert_eq!(f("next.js"), vec![0, 2]);
        assert_eq!(f("BLOG"), vec![1]);
        assert_eq!(f("next.js shop"), vec![0]);
        assert_eq!(f("my-*"), vec![0]);
        assert!(f("zzz").is_empty());
    }

    #[test]
    fn projects_are_ordered_by_reclaimable_and_use_their_own_fields() {
        let (tree, ps) = project_fixture();
        let f = |s: &str| filter_projects(&tree, &ps, &q(s), now());
        assert_eq!(f(""), vec![1, 0, 2]);
        // Size uses total_allocated, not reclaimable.
        assert_eq!(f(">500"), vec![1, 0]);
        // Age uses last_activity.
        assert_eq!(f("older:90d"), vec![1, 0, 2]);
        assert!(f("newer:90d").is_empty());
    }

    // ---- performance ----

    #[test]
    #[ignore = "run in release: cargo test -p devdisk-core --release -- --ignored"]
    fn wildcard_search_over_a_million_files_is_fast() {
        let mut tb = TreeBuilder::new(Path::new("/big"), None);
        let root = tb.root();
        let mut parent = root;
        for i in 0..1_000_000u64 {
            if i % 100 == 0 {
                parent = tb.add_dir(root, &format!("dir{i}"), None);
            }
            tb.add_file(
                parent,
                &format!("File_{i:07}.{}", if i % 10 == 0 { "MP3" } else { "dat" }),
                i,
                i * 2,
                ago(i % 1000 * DAY / 10),
            );
        }
        let tree = tb.finish();
        let query = q("*.mp3 file_0*");
        let start = std::time::Instant::now();
        let hits = find_files(&tree, &query, FileSort::Allocated, true, 200, now());
        let elapsed = start.elapsed();
        eprintln!("find_files over 1M files: {elapsed:?}");
        assert_eq!(hits.len(), 200);
        assert!(elapsed < Duration::from_millis(500), "{elapsed:?}");
    }
}
