use crate::model::{UseSite, UseSiteKind};

fn norm_crate_root(s: &str) -> String {
    s.replace('-', "_")
}

// Parses "dep::a::b" or "::dep::a::b" into (dep, head, full_path)
// Returns None for single-segment paths like "println"
fn split_dep_path(raw: &str) -> Option<(String, String, String)> {
    let s = raw.trim();
    let s = s.strip_prefix("::").unwrap_or(s);
    let mut it = s.split("::");

    let dep = it.next()?.to_string();
    let rest: Vec<String> = it.map(|x| x.to_string()).collect();
    if rest.is_empty() {
        return None;
    }
    let head = rest[0].clone();
    let full = format!("{}::{}", dep, rest.join("::"));
    Some((dep, head, full))
}

fn scope_from(enclosing_fn: &Option<String>, module_path: &[String]) -> String {
    if let Some(f) = enclosing_fn.clone() {
        f
    } else if module_path.is_empty() {
        "file".to_string()
    } else {
        format!("crate::{}", module_path.join("::"))
    }
}

impl crate::Klepto {
    /// Equivalent to your `scan_dep_use_sites(content, used_deps)` but AST-based.
    pub fn dep_use_sites(
        &self,
        used_deps: &std::collections::BTreeSet<String>,
    ) -> Vec<UseSite> {
        use std::collections::HashSet;

        let used: HashSet<String> = used_deps.iter().map(|d| norm_crate_root(d)).collect();
        let mut out: Vec<UseSite> = Vec::new();

        // 1) use statements (imports)
        for imp in &self.imports {
            let dep = norm_crate_root(&imp.root);
            if !used.contains(&dep) {
                continue;
            }
            if imp.segments.is_empty() || imp.segments[0] == "*" {
                // still a use site; head can be "*" or empty
            }
            let head = imp.segments.get(0).cloned().unwrap_or_else(|| "*".to_string());

            out.push(UseSite {
                dep: imp.root.clone(),
                path: imp.full_path.clone(),
                head,
                kind: UseSiteKind::UseStmt,
                location: imp.location.clone(),
                scope: scope_from(&None, &imp.module_path), // imports are usually not inside fns
            });
        }

        // 2) dep::... paths anywhere (your regex’s main job)
        for p in &self.paths {
            let Some((dep, head, full)) = split_dep_path(&p.path) else { continue; };
            if !used.contains(&norm_crate_root(&dep)) {
                continue;
            }

            out.push(UseSite {
                dep,
                path: full,
                head,
                kind: UseSiteKind::Path,
                location: p.location.clone(),
                scope: scope_from(&p.enclosing_fn, &p.module_path),
            });
        }

        // 3) dep::mac! calls
        // for m in &self.macros_inv {
        //     let Some((dep, head, full)) = split_dep_path(&m.path) else { continue; };
        //     if !used.contains(&norm_crate_root(&dep)) {
        //         continue;
        //     }
// 
        //     out.push(UseSite {
        //         dep,
        //         path: full,
        //         head,
        //         kind: UseSiteKind::MacroCall,
        //         location: m.location.clone(),
        //         scope: scope_from(&m.enclosing_fn, &m.module_path),
        //     });
        // }
for m in &self.macros_inv {
    let Some(p) = m.path.as_deref() else { continue; };
    let Some((dep, head, full)) = split_dep_path(p) else { continue; };

    if !used.contains(&norm_crate_root(&dep)) {
        continue;
    }

    out.push(UseSite {
        dep,
        path: full,
        head,
        kind: UseSiteKind::MacroCall,
        location: m.location.clone(),
        scope: scope_from(&m.enclosing_fn, &m.module_path),
    });
}

        out
    }

    /// Equivalent to your `scan_internal_use_sites(content, crate_name)` but AST-based.
    pub fn internal_use_sites(&self) -> Vec<UseSite> {
        let crate_id = norm_crate_root(&self.crate_name);
        let mut out = Vec::new();

        let is_internal_root = |r: &str| matches!(r, "crate" | "self" | "super") || norm_crate_root(r) == crate_id;

        // Internal `use ...` imports
        for imp in &self.imports {
            if !is_internal_root(&imp.root) {
                continue;
            }
            let head = imp.segments.get(0).cloned().unwrap_or_else(|| "*".to_string());
            out.push(UseSite {
                dep: imp.root.clone(),
                path: imp.full_path.clone(),
                head,
                kind: UseSiteKind::UseStmt,
                location: imp.location.clone(),
                scope: scope_from(&None, &imp.module_path),
            });
        }

        // Internal `crate::...` / `self::...` / `super::...` / `<crate_id>::...` paths
        for p in &self.paths {
            let Some((dep, head, full)) = split_dep_path(&p.path) else { continue; };
            if !is_internal_root(&dep) {
                continue;
            }
            out.push(UseSite {
                dep,
                path: full,
                head,
                kind: UseSiteKind::Path,
                location: p.location.clone(),
                scope: scope_from(&p.enclosing_fn, &p.module_path),
            });
        }

        // Internal macro calls too (crate::mac!)
//        for m in &self.macros_inv {
//            let Some((dep, head, full)) = split_dep_path(&m.path) else { continue; };
//            if !is_internal_root(&dep) {
//                continue;
//            }
//            out.push(UseSite {
//                dep,
//                path: full,
//                head,
//                kind: UseSiteKind::MacroCall,
//                location: m.location.clone(),
//                scope: scope_from(&m.enclosing_fn, &m.module_path),
//            });
//        }
for m in &self.macros_inv {
    let Some(p) = m.path.as_deref() else { continue; };
    let Some((dep, head, full)) = split_dep_path(p) else { continue; };

    if !is_internal_root(&dep) {
        continue;
    }

    out.push(UseSite {
        dep,
        path: full,
        head,
        kind: UseSiteKind::MacroCall,
        location: m.location.clone(),
        scope: scope_from(&m.enclosing_fn, &m.module_path),
    });
}



        out
    }
}