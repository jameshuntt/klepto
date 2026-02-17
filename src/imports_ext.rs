use crate::model::{ImportOrigin, StolenPath, UseKind};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Extension methods for `Vec<StolenPath>` / `&[StolenPath]`
///
/// Usage:
/// ```rust
/// use klepto::ImportVecExt;
/// let imports = k.imports().collect().unique_prefer_pub_use();
/// let by_origin = imports.group_by_origin_owned();
/// ```
pub trait ImportVecExt {
    /// Deduplicate while preserving order (keeps first occurrence).
    fn unique(self) -> Vec<StolenPath>;

    /// Deduplicate by (full_path, use-kind, is_absolute), but if duplicates exist,
    /// prefer the `pub use` version (more relevant for public surface audits).
    fn unique_prefer_pub_use(self) -> Vec<StolenPath>;

    /// Group imports by origin, returning borrowed references (no cloning).
    fn group_by_origin(&self) -> BTreeMap<ImportOrigin, Vec<&StolenPath>>;

    /// Same as `group_by_origin`, but returns owned/cloned groups.
    fn group_by_origin_owned(&self) -> BTreeMap<ImportOrigin, Vec<StolenPath>>;

    /// Group by root crate segment (`std`, `serde`, `crate`, etc.).
    fn group_by_root(&self) -> BTreeMap<String, Vec<&StolenPath>>;

    /// A quick “at a glance” count breakdown.
    fn summary(&self) -> ImportSummary;
}

/// Simple import summary counts (post-dedup usually).
#[derive(Debug, Clone, Default)]
pub struct ImportSummary {
    pub total: usize,
    pub by_origin: BTreeMap<ImportOrigin, usize>,
    pub by_root: BTreeMap<String, usize>,
    pub pub_use_count: usize,
    pub glob_count: usize,
    pub rename_count: usize,
    pub absolute_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ImportKey {
    full_path: String,
    kind: UseKind,
    is_absolute: bool,
    // NOTE: for `unique()` we also include is_public_use so you can keep both forms
    // if you want—BUT our `unique()` uses a slightly different key (see below).
    is_public_use: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PreferKey {
    full_path: String,
    kind: UseKind,
    is_absolute: bool,
}

fn origin_or_unknown(i: &StolenPath) -> ImportOrigin {
    i.origin.clone().unwrap_or(ImportOrigin::UnknownExternal)
}

fn is_absolute(i: &StolenPath) -> bool {
    i.is_absolute.unwrap_or(false)
}

impl ImportVecExt for Vec<StolenPath> {
    fn unique(self) -> Vec<StolenPath> {
        // Keeps first occurrence; key includes public-use flag so you can keep both
        // `use X::Y` and `pub use X::Y` if they both exist.
        let mut seen: HashSet<ImportKey> = HashSet::new();
        let mut out = Vec::with_capacity(self.len());

        for i in self {
            let key = ImportKey {
                full_path: i.full_path.clone(),
                kind: i.kind.clone(),
                is_absolute: is_absolute(&i),
                is_public_use: i.is_public_use,
            };
            if seen.insert(key) {
                out.push(i);
            }
        }
        out
    }

    fn unique_prefer_pub_use(self) -> Vec<StolenPath> {
        // Dedup by (path,kind,abs), but prefer pub use if duplicates exist.
        // Preserves overall order of FIRST time the key is seen, but may replace the stored item.
        let mut order: Vec<PreferKey> = Vec::new();
        let mut map: HashMap<PreferKey, StolenPath> = HashMap::new();

        for i in self {
            let key = PreferKey {
                full_path: i.full_path.clone(),
                kind: i.kind.clone(),
                is_absolute: is_absolute(&i),
            };

            match map.get_mut(&key) {
                None => {
                    order.push(key.clone());
                    map.insert(key, i);
                }
                Some(existing) => {
                    // replace if new one is pub use and existing is not
                    if i.is_public_use && !existing.is_public_use {
                        *existing = i;
                    }
                }
            }
        }

        order.into_iter().filter_map(|k| map.remove(&k)).collect()
    }

    fn group_by_origin(&self) -> BTreeMap<ImportOrigin, Vec<&StolenPath>> {
        let mut m: BTreeMap<ImportOrigin, Vec<&StolenPath>> = BTreeMap::new();
        for i in self {
            m.entry(origin_or_unknown(i)).or_default().push(i);
        }
        m
    }

    fn group_by_origin_owned(&self) -> BTreeMap<ImportOrigin, Vec<StolenPath>> {
        let mut m: BTreeMap<ImportOrigin, Vec<StolenPath>> = BTreeMap::new();
        for i in self {
            m.entry(origin_or_unknown(i)).or_default().push(i.clone());
        }
        m
    }

    fn group_by_root(&self) -> BTreeMap<String, Vec<&StolenPath>> {
        let mut m: BTreeMap<String, Vec<&StolenPath>> = BTreeMap::new();
        for i in self {
            m.entry(i.root.clone()).or_default().push(i);
        }
        m
    }

    fn summary(&self) -> ImportSummary {
        let mut s = ImportSummary::default();
        s.total = self.len();

        for i in self {
            *s.by_origin.entry(origin_or_unknown(i)).or_insert(0) += 1;
            *s.by_root.entry(i.root.clone()).or_insert(0) += 1;

            if i.is_public_use {
                s.pub_use_count += 1;
            }
            match &i.kind {
                UseKind::Glob => s.glob_count += 1,
                UseKind::Rename { .. } => s.rename_count += 1,
                _ => {}
            }
            if is_absolute(i) {
                s.absolute_count += 1;
            }
        }

        s
    }
}
