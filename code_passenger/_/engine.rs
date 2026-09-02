use crate::{
    cargo::load_manifest,
    error::{PassengerError, Result},
    model::{DocumentDetails, FeatureNote, FileReport, ManifestInfo, UsedSymbols},
    packs,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

fn rel_to_src(src_root: &Path, p: &Path) -> Result<String> {
    let rel = p.strip_prefix(src_root)
        .map_err(|e| PassengerError::Path(e.to_string()))?;
    Ok(rel.to_string_lossy().to_string())
}

// very pragmatic “scope notes”
fn scope_label_from_header(header: &str) -> Option<String> {
    let h = header.trim();

    if let Some(idx) = h.find("fn ") {
        let after = &h[idx + 3..];
        let name: String = after
            .chars()
            .skip_while(|c| c.is_whitespace())
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() { return Some(format!("fn {name}")); }
    }

    if h.contains("impl ") {
        let preview = h.replace('\n', " ");
        let preview = preview.split('{').next().unwrap_or(&preview);
        let preview = preview.split(" where ").next().unwrap_or(preview);
        let preview = preview.trim();
        if let Some(rest) = preview.strip_prefix("impl ") {
            return Some(format!("impl {}", rest.trim()));
        }
        return Some(format!("impl {}", preview));
    }

    for kw in ["mod", "trait", "struct", "enum"] {
        if let Some(idx) = h.find(&format!("{kw} ")) {
            let after = &h[idx + kw.len() + 1..];
            let name: String = after
                .chars()
                .skip_while(|c| c.is_whitespace())
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() { return Some(format!("{kw} {name}")); }
        }
    }

    None
}

fn compute_dep_scopes(content: &str, used_deps: &BTreeSet<String>) -> BTreeMap<String, BTreeSet<String>> {
    let mut dep_scopes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for d in used_deps {
        dep_scopes.insert(d.clone(), BTreeSet::new());
    }

    let mut depth: usize = 0;
    let mut stack: Vec<(String, usize)> = vec![];
    let mut header_buf: Vec<String> = vec![];

    for line in content.lines() {
        let trimmed = line.trim();

        // record dep hits
        for dep in used_deps {
            if trimmed.contains(&format!("{dep}::")) || trimmed.contains(&format!("::{dep}::")) || trimmed.contains(&format!("#[{dep}::")) {
                let scope = stack.last().map(|s| s.0.clone()).unwrap_or_else(|| "file".to_string());
                dep_scopes.get_mut(dep).unwrap().insert(scope);
            }
        }

        if !trimmed.is_empty() {
            header_buf.push(line.to_string());
            if header_buf.len() > 10 { header_buf.remove(0); }
        }

        let opens = line.chars().filter(|c| *c == '{').count();
        let closes = line.chars().filter(|c| *c == '}').count();

        if opens > 0 {
            let header = header_buf.join("\n");
            if let Some(label) = scope_label_from_header(&header) {
                stack.push((label, depth + 1));
            }
        }

        depth += opens;
        for _ in 0..closes {
            if depth == 0 { break; }
            if let Some(top) = stack.last() {
                if top.1 == depth {
                    stack.pop();
                }
            }
            depth -= 1;
        }
    }

    dep_scopes
}

pub struct RunContext {
    pub root: PathBuf,
    pub src_rel: String,
    pub manifest_path: PathBuf,
    pub lang: String,
}

pub struct EngineOutput {
    pub manifest: ManifestInfo,
    pub reports: Vec<FileReport>,
}

pub fn run_scan(ctx: &RunContext) -> Result<EngineOutput> {
    let manifest = load_manifest(ctx.manifest_path.to_str().ok_or_else(|| PassengerError::Path("bad manifest path".into()))?)?;
    let src_root = ctx.root.join(&ctx.src_rel);

    let mut reports = Vec::new();

    // currently only Rust pack uses optional_deps-aware detection
    let pack = packs::get_pack(&ctx.lang)?;

    for ent in WalkDir::new(&src_root).into_iter().filter_map(|e| e.ok()) {
        if !ent.file_type().is_file() { continue; }
        let path = ent.path();
        if !pack.matches_path(path) { continue; }

        let rel = rel_to_src(&src_root, path)?;
        let filename = path.file_name().unwrap().to_string_lossy().to_string();

        let content = fs::read_to_string(path)?;

        let used: UsedSymbols = if ctx.lang == "rust" || ctx.lang == "rs" {
            // usage depends on optional_deps list
            #[cfg(feature="lang_rust")]
            {
                crate::packs::rust::detectors::detect_usage_with_deps(
                    &content,
                    &manifest.all_deps,
                    &manifest.crate_name
                )
                // crate::packs::rust::detect_usage_with_optiona l_deps(&content, &manifest.optional_deps)
            }
            #[cfg(not(feature="lang_rust"))]
            {
                UsedSymbols::default()
            }
        } else {
            pack.detect_usage(&content)
        };

        let external_use_sites = scan_dep_use_sites(&content, &used.packages);

        let mut external_dep_symbol_counts: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
        for u in &external_use_sites {
            external_dep_symbol_counts
                .entry(u.dep.clone()).or_default()
                .entry(u.head.clone()).and_modify(|c| *c += 1)
                .or_insert(1);
        }

        let internal_use_sites = scan_internal_use_sites(&content, &manifest.crate_name);

        let mut internal_dep_symbol_counts: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
        for u in &internal_use_sites {
            internal_dep_symbol_counts
                .entry(u.dep.clone()).or_default()
                .entry(u.head.clone()).and_modify(|c| *c += 1)
                .or_insert(1);
        }

        let dep_scopes = compute_dep_scopes(&content, &used.packages);

        // feature notes + corpus selection
        let mut notes: Vec<FeatureNote> = vec![];
        let mut corpus_features: BTreeSet<String> = BTreeSet::new();

        // corpus heuristic: features that actually gate optional deps OR known “platform features”
        let corpus_name_whitelist: BTreeSet<&'static str> =
            ["std", "alloc", "no_std"].into_iter().collect();

        for (feat, deps) in &manifest.feature_deps {
            let deps_vec: Vec<String> = deps.iter().cloned().collect();
            let used_in_file = deps.iter().any(|d| used.packages.contains(d));

            // union scopes of deps
            let mut scopes = BTreeSet::<String>::new();
            for d in deps {
                if let Some(s) = dep_scopes.get(d) {
                    scopes.extend(s.iter().cloned());
                }
            }

            let corpus = !deps.is_empty() || corpus_name_whitelist.contains(feat.as_str());

            if corpus && used_in_file {
                corpus_features.insert(feat.clone());
            }

            notes.push(FeatureNote {
                feature_name: feat.clone(),
                deps: deps_vec,
                scope: scopes.into_iter().collect(),
                corpus,
                used_in_file,
            });
        }

        reports.push(FileReport {
            document: DocumentDetails { filename, relative_path: rel },
            used,
            notes,
            corpus_features: corpus_features.into_iter().collect(),
            internal_use_sites,
            internal_dep_symbol_counts,
            external_use_sites,
            external_dep_symbol_counts
        });
    }

    Ok(EngineOutput { manifest, reports })
}










#[allow(unused)]use regex::RegexSet;
#[allow(unused)]fn build_dep_regexset(optional_deps: &BTreeSet<String>) -> (Vec<String>, RegexSet) {
    // look for dep:: (very common for crates) with a non-ident boundary before it
    let deps: Vec<String> = optional_deps.iter().cloned().collect();
    let pats: Vec<String> = deps.iter()
        .map(|d| format!(r"(?m)(?:^|[^A-Za-z0-9_]){}\s*::", regex::escape(d)))
        .collect();
    let set = RegexSet::new(&pats).expect("valid regexset");
    (deps, set)
}


use regex::Regex;

fn scan_dep_use_sites(
    content: &str,
    used_deps: &std::collections::BTreeSet<String>,
) -> Vec<crate::model::UseSite> {
    use crate::model::{UseKind, UseSite};

    // Matches: ::dep::a::b   or dep::a::b  (with optional whitespace around ::)
    // Captures dep + tail; tail is 1+ ident segments separated by ::
    let re = Regex::new(
        r#"(?x)
        (?P<global>::\s*)?
        (?P<dep>[A-Za-z_][A-Za-z0-9_]*)
        \s*::\s*
        (?P<tail>[A-Za-z_][A-Za-z0-9_]*(?:\s*::\s*[A-Za-z_][A-Za-z0-9_]*)*)
        (?P<bang>\s*!)?
        "#
    ).expect("valid regex");

    // Reuse your scope stack logic (same idea as compute_dep_scopes)
    let mut depth: usize = 0;
    let mut stack: Vec<(String, usize)> = vec![];
    let mut header_buf: Vec<String> = vec![];

    let mut out: Vec<UseSite> = vec![];

    for (idx0, line) in content.lines().enumerate() {
        let line_no = idx0 + 1;
        let trimmed = line.trim();

        // keep rolling header buffer
        if !trimmed.is_empty() {
            header_buf.push(line.to_string());
            if header_buf.len() > 10 { header_buf.remove(0); }
        }

        // detect scope opens before we record matches (so uses on same line get correct scope after `{`)
        let opens = line.chars().filter(|c| *c == '{').count();
        if opens > 0 {
            let header = header_buf.join("\n");
            if let Some(label) = crate::engine::scope_label_from_header(&header) {
                stack.push((label, depth + 1));
            }
        }

        let scope = stack.last().map(|s| s.0.clone()).unwrap_or_else(|| "file".to_string());

        // classify “kind” by line context
        let kind_hint = if trimmed.starts_with("use ") {
            Some(UseKind::UseStmt)
        } else if trimmed.starts_with("extern crate ") {
            Some(UseKind::ExternCrate)
        } else if trimmed.contains("#[") {
            Some(UseKind::Attribute)
        } else {
            None
        };

        for caps in re.captures_iter(line) {
            let dep = caps.name("dep").unwrap().as_str().to_string();
            if !used_deps.contains(&dep) { continue; }

            let tail_raw = caps.name("tail").unwrap().as_str();
            let bang = caps.name("bang").is_some();

            // normalize tail spacing: "a :: b" -> "a::b"
            let tail = tail_raw.split_whitespace().collect::<String>().replace("::", "::");

            // head = first segment after dep::
            let head = tail.split("::").next().unwrap_or("").to_string();
            let path = format!("{dep}::{tail}");

            let kind = if let Some(k) = kind_hint.clone() {
                // if line is `use ...` etc, keep that (most useful)
                k
            } else if bang {
                UseKind::MacroCall
            } else {
                UseKind::Path
            };

            out.push(UseSite {
                dep,
                path,
                head,
                kind,
                line: line_no,
                scope: scope.clone(),
            });
        }

        // now update depth and pop scopes on closes
        let closes = line.chars().filter(|c| *c == '}').count();
        depth += opens;
        for _ in 0..closes {
            if depth == 0 { break; }
            if let Some(top) = stack.last() {
                if top.1 == depth {
                    stack.pop();
                }
            }
            depth -= 1;
        }
    }

    out
}


fn crate_ident(crate_name: &str) -> String {
    crate_name.replace('-', "_")
}

/// Internal roots we want to treat as “internal deps”:
/// - crate::...
/// - self::...
/// - super::...
/// - my_crate_name::...
fn scan_internal_use_sites_usesites_struct(content: &str, crate_name: &str) -> crate::model::UseSites {
    use crate::model::UseSites;
    use regex::Regex;
    use std::collections::BTreeMap;

    let crate_id = crate_ident(crate_name);

    // capture:
    //   root  = crate|self|super|<crate_id>
    //   sym   = next identifier after ::
    //
    // examples:
    //   crate::engine::RunContext   => root=crate, sym=engine
    //   code_passenger::model::X    => root=code_passenger, sym=model
    //   super::foo::Bar             => root=super, sym=foo
    let re = Regex::new(&format!(
        r"(?x)
        (?P<root>\bcrate\b|\bself\b|\bsuper\b|\b{}\b)
        \s*::\s*
        (?P<sym>[A-Za-z_][A-Za-z0-9_]*)
        ",
        regex::escape(&crate_id)
    )).expect("valid regex");

    let mut out: UseSites = BTreeMap::new();

    for caps in re.captures_iter(content) {
        let root = caps.name("root").unwrap().as_str().to_string();
        let sym = caps.name("sym").unwrap().as_str().to_string();
        *out.entry(root).or_default().entry(sym).or_insert(0) += 1;
    }

    out
}


/// Internal roots we want to treat as “internal deps”:
/// - crate::...
/// - self::...
/// - super::...
/// - <crate_name>::...
fn scan_internal_use_sites(content: &str, crate_name: &str) -> Vec<crate::model::UseSite> {
    use crate::model::{UseKind, UseSite};
    use regex::Regex;

    let crate_id = crate_ident(crate_name);

    // Matches: ::crate::a::b  or crate::a::b (and same for self/super/<crate_id>)
    // Captures root + tail (1+ segments), optional bang for macros.
    let re = Regex::new(&format!(
        r#"(?x)
        (?P<global>::\s*)?
        (?P<root>\bcrate\b|\bself\b|\bsuper\b|\b{}\b)
        \s*::\s*
        (?P<tail>[A-Za-z_][A-Za-z0-9_]*(?:\s*::\s*[A-Za-z_][A-Za-z0-9_]*)*)
        (?P<bang>\s*!)?
        "#,
        regex::escape(&crate_id)
    ))
    .expect("valid regex");

    // same scope tracking approach as scan_dep_use_sites
    let mut depth: usize = 0;
    let mut stack: Vec<(String, usize)> = vec![];
    let mut header_buf: Vec<String> = vec![];

    let mut out: Vec<UseSite> = vec![];

    for (idx0, line) in content.lines().enumerate() {
        let line_no = idx0 + 1;
        let trimmed = line.trim();

        if !trimmed.is_empty() {
            header_buf.push(line.to_string());
            if header_buf.len() > 10 { header_buf.remove(0); }
        }

        let opens = line.chars().filter(|c| *c == '{').count();
        if opens > 0 {
            let header = header_buf.join("\n");
            if let Some(label) = scope_label_from_header(&header) {
                stack.push((label, depth + 1));
            }
        }

        let scope = stack.last().map(|s| s.0.clone()).unwrap_or_else(|| "file".to_string());

        let kind_hint = if trimmed.starts_with("use ") {
            Some(UseKind::UseStmt)
        } else if trimmed.starts_with("extern crate ") {
            Some(UseKind::ExternCrate)
        } else if trimmed.contains("#[") {
            Some(UseKind::Attribute)
        } else {
            None
        };

        for caps in re.captures_iter(line) {
            let dep = caps.name("root").unwrap().as_str().to_string();

            let tail_raw = caps.name("tail").unwrap().as_str();
            let bang = caps.name("bang").is_some();

            // normalize: remove whitespace inside tail (so "foo :: bar" => "foo::bar")
            let tail = tail_raw.split_whitespace().collect::<String>();

            let head = tail.split("::").next().unwrap_or("").to_string();
            let path = format!("{dep}::{tail}");

            let kind = if let Some(k) = kind_hint.clone() {
                k
            } else if bang {
                UseKind::MacroCall
            } else {
                UseKind::Path
            };

            out.push(UseSite {
                dep,
                path,
                head,
                kind,
                line: line_no,
                scope: scope.clone(),
            });
        }

        let closes = line.chars().filter(|c| *c == '}').count();
        depth += opens;
        for _ in 0..closes {
            if depth == 0 { break; }
            if let Some(top) = stack.last() {
                if top.1 == depth {
                    stack.pop();
                }
            }
            depth -= 1;
        }
    }

    out
}

fn build_symbol_counts(use_sites: &[crate::model::UseSite])
    -> std::collections::BTreeMap<String, std::collections::BTreeMap<String, usize>>
{
    use std::collections::BTreeMap;
    let mut m: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for u in use_sites {
        *m.entry(u.dep.clone())
            .or_default()
            .entry(u.head.clone())
            .or_insert(0) += 1;
    }
    m
}

fn scan_internal_use_sites_alt(content: &str, crate_name: &str) -> Vec<crate::model::UseSite> {
    use crate::model::{UseKind, UseSite};
    use regex::Regex;

    let crate_id = crate_name.replace('-', "_");
    let re = Regex::new(&format!(
        r#"(?x)
        (?P<root>\bcrate\b|\bself\b|\bsuper\b|\b{}\b)
        \s*::\s*
        (?P<tail>[A-Za-z_][A-Za-z0-9_]*(?:\s*::\s*[A-Za-z_][A-Za-z0-9_]*)*)
        (?P<bang>\s*!)?
        "#,
        regex::escape(&crate_id)
    )).unwrap();

    let mut out = Vec::new();

    for (i, line) in content.lines().enumerate() {
        let line_no = i + 1;
        let trimmed = line.trim();

        let kind_hint = if trimmed.starts_with("use ") {
            Some(UseKind::UseStmt)
        } else {
            None
        };

        for caps in re.captures_iter(line) {
            let root = caps.name("root").unwrap().as_str().to_string();
            let tail_raw = caps.name("tail").unwrap().as_str();
            let bang = caps.name("bang").is_some();

            let tail = tail_raw.split_whitespace().collect::<String>().replace("::", "::");
            let head = tail.split("::").next().unwrap_or("").to_string();
            let path = format!("{root}::{tail}");

            let kind = kind_hint.clone().unwrap_or_else(|| {
                if bang { UseKind::MacroCall } else { UseKind::Path }
            });

            out.push(UseSite {
                dep: root,
                path,
                head,
                kind,
                line: line_no,
                scope: "file".into(), // if you want, reuse your scope stack logic like external
            });
        }
    }

    out
}

