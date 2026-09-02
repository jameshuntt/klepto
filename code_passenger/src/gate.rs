//! Applying a klepto `GatePlan` to a tree (rules A1–A4 in `GATE_RULES.md`).
//! Pure until `EditSet::write`; the oracle uses `write_to` on a scratch copy.
//!
//! Out of scope: edits inside macro bodies; carriers the planner could not
//! name (they are listed under `unknown`, a person decides).

use crate::error::{PassengerError, Result};
use klepto::{GateEdit, GatePlan, ManifestEdit};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEdit {
    /// Absolute path in the tree the plan was computed on.
    pub path: PathBuf,
    pub before: String,
    pub after: String,
}

/// Everything a plan changes (rule A3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditSet {
    pub files: Vec<FileEdit>,
    pub manifest: Option<FileEdit>,
}

impl EditSet {
    pub fn is_empty(&self) -> bool {
        self.files.iter().all(|f| f.before == f.after) && self.manifest.as_ref().is_none_or(|m| m.before == m.after)
    }

    fn all(&self) -> impl Iterator<Item = &FileEdit> {
        self.files.iter().chain(self.manifest.iter())
    }

    /// Unified diffs, one per changed file, in path order, with absolute names.
    pub fn diff(&self) -> String {
        self.diff_named(|p| p.to_string_lossy().into_owned())
    }

    /// Unified diffs with names relative to `root` (`a/app/src/lib.rs`).
    pub fn diff_from(&self, root: &Path) -> String {
        self.diff_named(|p| {
            relative(root, p)
                .map(|r| format!("a/{}", r.to_string_lossy()))
                .unwrap_or_else(|_| p.to_string_lossy().into_owned())
        })
    }

    fn diff_named(&self, name: impl Fn(&Path) -> String) -> String {
        let mut edits: Vec<&FileEdit> = self.all().filter(|f| f.before != f.after).collect();
        edits.sort_by(|a, b| a.path.cmp(&b.path));
        let mut out = String::new();
        for e in edits {
            let n = name(&e.path);
            let d = similar::TextDiff::from_lines(&e.before, &e.after);
            out.push_str(&d.unified_diff().header(&n, &n.replacen("a/", "b/", 1)).to_string());
        }
        out
    }

    /// Writes every `after` in place.
    pub fn write(&self) -> Result<()> {
        for e in self.all() {
            if e.before != e.after {
                std::fs::write(&e.path, &e.after)?;
            }
        }
        Ok(())
    }

    /// Writes the same contents into another tree with the same layout;
    /// returns the relative paths written.
    pub fn write_to(&self, from_root: &Path, to_root: &Path) -> Result<Vec<PathBuf>> {
        let mut written = Vec::new();
        for e in self.all() {
            let rel = relative(from_root, &e.path)?;
            let target = to_root.join(&rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&target, &e.after)?;
            written.push(rel);
        }
        Ok(written)
    }
}

/// `path` relative to `root`, trying the root as given and canonicalised.
pub(crate) fn relative(root: &Path, path: &Path) -> Result<PathBuf> {
    if let Ok(r) = path.strip_prefix(root) {
        return Ok(r.to_path_buf());
    }
    std::fs::canonicalize(root)
        .ok()
        .and_then(|c| path.strip_prefix(&c).ok().map(Path::to_path_buf))
        .ok_or_else(|| PassengerError::Path(format!("{} is not under {}", path.display(), root.display())))
}

fn skip_dir(name: &OsStr) -> bool {
    name == "target" || name == ".git" || name == ".passenger"
}

/// Rule A2: the manifest of `package` under `root` — the `Cargo.toml`
/// whose `[package].name` (normalised) is the package.
pub fn find_manifest(root: &Path, package: &str) -> Result<PathBuf> {
    let want = package.replace('-', "_");
    let mut found: Vec<PathBuf> = Vec::new();
    let walk = WalkDir::new(root).into_iter().filter_entry(|e| !(e.depth() > 0 && e.file_type().is_dir() && skip_dir(e.file_name())));
    for entry in walk.filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() || entry.file_name() != "Cargo.toml" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
        let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { continue };
        let name = doc.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()).map(|s| s.replace('-', "_"));
        if name.as_deref() == Some(want.as_str()) {
            found.push(entry.path().to_path_buf());
        }
    }
    found.sort();
    found
        .into_iter()
        .next()
        .ok_or_else(|| PassengerError::Unsupported(format!("no Cargo.toml with [package].name = \"{package}\" under {}", root.display())))
}

/// Rules A1–A2.
pub fn apply(root: &Path, plan: &GatePlan) -> Result<EditSet> {
    let mut by_file: BTreeMap<PathBuf, Vec<&GateEdit>> = BTreeMap::new();
    for g in &plan.gates {
        by_file.entry(g.location.path.clone()).or_default().push(g);
    }
    let mut files = Vec::new();
    for (path, mut gates) in by_file {
        let before = std::fs::read_to_string(&path)?;
        let after = gate_file(&before, &mut gates);
        files.push(FileEdit { path, before, after });
    }
    let manifest = if plan.manifest.is_empty() {
        None
    } else {
        let path = find_manifest(root, &plan.package)?;
        let before = std::fs::read_to_string(&path)?;
        let after = edit_manifest(&before, &plan.manifest)?;
        Some(FileEdit { path, before, after })
    };
    Ok(EditSet { files, manifest })
}

fn is_doc_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("///") || t.starts_with("//!")
}

/// Rule A1: the attribute that replaces a carrier's own `#[cfg]` — the
/// planner's predicate is what to *add*, so the existing condition is kept
/// by conjunction (`#[cfg(unix)]` + `feature = "b"` → `all(unix, feature = "b")`).
pub fn merged_attribute(existing: &klepto::CfgPredicate, predicate: &klepto::CfgPredicate) -> String {
    let merged = existing.clone().and(predicate.clone()).simplify();
    format!("#[cfg({})]", merged.render())
}

/// Rule A1. The carrier's leading lines are its doc comments and
/// attributes (multi-line attributes tracked by bracket balance); the
/// first `#[cfg(` among them is replaced, otherwise the attribute is
/// inserted right above the declaration line.
fn gate_file(before: &str, gates: &mut [&GateEdit]) -> String {
    let had_trailing_newline = before.ends_with('\n');
    let mut lines: Vec<String> = before.lines().map(str::to_string).collect();
    gates.sort_by(|a, b| b.location.line.cmp(&a.location.line).then_with(|| a.item.cmp(&b.item)));
    let mut done: BTreeSet<u32> = BTreeSet::new();
    for g in gates.iter() {
        let Some(line) = g.location.line else { continue };
        if !done.insert(line) {
            continue;
        }
        let start = (line as usize).saturating_sub(1);
        if start >= lines.len() {
            continue;
        }
        let mut j = start;
        let mut replaced = false;
        while j < lines.len() {
            let t = lines[j].trim_start().to_string();
            if is_doc_line(&t) {
                j += 1;
                continue;
            }
            if !t.starts_with("#[") {
                break;
            }
            if t.starts_with("#[cfg(") {
                let indent: String = lines[j].chars().take_while(|c| c.is_whitespace()).collect();
                lines[j] = format!("{indent}{}", merged_attribute(&g.existing_cfg, &g.predicate));
                replaced = true;
                break;
            }
            // skip a (possibly multi-line) attribute by bracket balance
            let mut depth: i32 = 0;
            loop {
                for ch in lines[j].chars() {
                    match ch {
                        '[' => depth += 1,
                        ']' => depth -= 1,
                        _ => {}
                    }
                }
                j += 1;
                if depth <= 0 || j >= lines.len() {
                    break;
                }
            }
        }
        if !replaced {
            let at = j.min(lines.len());
            let indent: String = lines.get(at).map(|l| l.chars().take_while(|c| c.is_whitespace()).collect()).unwrap_or_default();
            lines.insert(at, format!("{indent}{}", g.attribute_text()));
        }
    }
    let mut out = lines.join("\n");
    if had_trailing_newline {
        out.push('\n');
    }
    out
}

fn string_array(items: &[String]) -> toml_edit::Array {
    let mut a = toml_edit::Array::new();
    for s in items {
        a.push(s.as_str());
    }
    a
}

fn set_optional(item: &mut toml_edit::Item) {
    if let Some(s) = item.as_str().map(str::to_string) {
        let mut t = toml_edit::InlineTable::new();
        t.insert("version", toml_edit::Value::from(s.as_str()));
        t.insert("optional", toml_edit::Value::from(true));
        *item = toml_edit::value(t);
    } else if let Some(t) = item.as_table_like_mut() {
        t.insert("optional", toml_edit::value(true));
    }
}

const DEP_SECTIONS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];

fn make_optional(doc: &mut toml_edit::DocumentMut, dep: &str) -> Result<()> {
    let mut hit = false;
    let mut visit = |tbl: &mut dyn toml_edit::TableLike| {
        let keys: Vec<String> = tbl.iter().map(|(k, _)| k.to_string()).collect();
        for key in keys {
            if key.replace('-', "_") != dep {
                continue;
            }
            if let Some(item) = tbl.get_mut(&key) {
                set_optional(item);
                hit = true;
            }
        }
    };
    for section in DEP_SECTIONS {
        if let Some(t) = doc.get_mut(section).and_then(|i| i.as_table_like_mut()) {
            visit(t);
        }
    }
    if let Some(targets) = doc.get_mut("target").and_then(|i| i.as_table_like_mut()) {
        let names: Vec<String> = targets.iter().map(|(k, _)| k.to_string()).collect();
        for name in names {
            if let Some(tt) = targets.get_mut(&name).and_then(|i| i.as_table_like_mut()) {
                for section in DEP_SECTIONS {
                    if let Some(t) = tt.get_mut(section).and_then(|i| i.as_table_like_mut()) {
                        visit(t);
                    }
                }
            }
        }
    }
    if hit {
        Ok(())
    } else {
        Err(PassengerError::Unsupported(format!("dependency `{dep}` is not declared in the manifest")))
    }
}

/// Rule A2.
fn edit_manifest(before: &str, edits: &[ManifestEdit]) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = before.parse().map_err(|e: toml_edit::TomlError| PassengerError::Toml(e.to_string()))?;
    for e in edits {
        match e {
            ManifestEdit::MakeOptional { dep } => make_optional(&mut doc, dep)?,
            ManifestEdit::SetFeature { name, enables } => {
                if !doc.contains_key("features") {
                    doc["features"] = toml_edit::table();
                }
                let feats = doc["features"]
                    .as_table_like_mut()
                    .ok_or_else(|| PassengerError::Toml("[features] is not a table".into()))?;
                feats.insert(name, toml_edit::value(string_array(enables)));
            }
            ManifestEdit::SetDefault { features } => {
                if !doc.contains_key("features") {
                    doc["features"] = toml_edit::table();
                }
                let feats = doc["features"]
                    .as_table_like_mut()
                    .ok_or_else(|| PassengerError::Toml("[features] is not a table".into()))?;
                feats.insert("default", toml_edit::value(string_array(features)));
            }
        }
    }
    Ok(doc.to_string())
}

/// Rule O1: the cargo workspace `root` belongs to — `root` itself, or the
/// parent workspace when `root` is a member package (`cargo locate-project
/// --workspace`). The scratch copy must hold the whole workspace or cargo
/// refuses to build the member.
pub fn workspace_root(root: &Path) -> Result<PathBuf> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = std::process::Command::new(cargo)
        .args(["locate-project", "--workspace", "--message-format", "plain"])
        .current_dir(root)
        .output()?;
    if !out.status.success() {
        return Ok(root.to_path_buf());
    }
    let manifest = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let ws = Path::new(&manifest).parent().map(Path::to_path_buf).unwrap_or_else(|| root.to_path_buf());
    // prefer the spelling the caller used when it is the same directory
    let same = std::fs::canonicalize(&ws).ok() == std::fs::canonicalize(root).ok();
    Ok(if same { root.to_path_buf() } else { ws })
}

/// Rule A4: the tree minus `target`, `.git` and `.passenger` at any depth;
/// symlinks are not followed.
pub fn scratch_copy(root: &Path) -> Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    let walk = WalkDir::new(root).into_iter().filter_entry(|e| !(e.depth() > 0 && e.file_type().is_dir() && skip_dir(e.file_name())));
    for entry in walk.filter_map(|e| e.ok()) {
        if entry.depth() == 0 {
            continue;
        }
        let ft = entry.file_type();
        if ft.is_symlink() {
            continue;
        }
        let rel = entry.path().strip_prefix(root).map_err(|e| PassengerError::Path(e.to_string()))?;
        let target = dir.path().join(rel);
        if ft.is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if ft.is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(dir)
}
