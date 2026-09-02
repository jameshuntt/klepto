//! Rules U1–U4 (Phase 1) and Q8 (Phase 2): use sites derived from resolutions,
//! each carrying the effective cfg of the occurrence it came from.

use crate::cfg::CfgPredicate;
use crate::model::{FileLocation, Resolution, UseKind, UseSite, UseSiteKind};
use crate::resolve::{file_packages, join_key, norm};
use std::collections::BTreeSet;
use std::path::Path;

fn crate_root(canonical: &str) -> &str {
    canonical.split("::").next().unwrap_or("")
}

impl crate::Klepto {
    /// Walk every occurrence; `pick(resolution, package_of_occurrence)` returns
    /// the canonical paths to report for it (usually zero or one). Each site
    /// comes with its effective cfg and the package it belongs to.
    pub(crate) fn sites_with_cfg_and_package(
        &self,
        pick: &dyn Fn(&Resolution, &str) -> Vec<String>,
        include_extern_crates: bool,
    ) -> Vec<(UseSite, CfgPredicate, String)> {
        let file_pkg = file_packages(self);
        let crate_pkg = norm(&self.crate_name);
        let pkg_of = |p: &Path| -> String { file_pkg.get(p).cloned().unwrap_or_else(|| crate_pkg.clone()) };

        let mut out: Vec<(UseSite, CfgPredicate, String)> = Vec::new();
        let push = |out: &mut Vec<(UseSite, CfgPredicate, String)>,
                    res: Option<&Resolution>,
                    pkg: &str,
                    kind: UseSiteKind,
                    location: &FileLocation,
                    scope: &str,
                    cfg: &CfgPredicate,
                    glob: bool| {
            let Some(res) = res else { return };
            for canonical in pick(res, pkg) {
                let path = if glob { format!("{canonical}::*") } else { canonical.clone() };
                let mut it = canonical.split("::");
                let dep = it.next().unwrap_or("").to_string();
                let head = it.next().unwrap_or("").to_string();
                let site = UseSite { dep, path, head, kind, location: location.clone(), scope: scope.to_string() };
                out.push((site, cfg.clone(), pkg.to_string()));
            }
        };

        // `use` items (module-level and inside fn bodies)
        for imp in &self.imports {
            let pkg = pkg_of(&imp.location.path);
            let scope = imp.enclosing_fn.clone().unwrap_or_else(|| join_key(&pkg, &imp.module_path));
            push(&mut out, imp.resolution.as_ref(), &pkg, UseSiteKind::UseStmt, &imp.location, &scope, &imp.cfg, matches!(imp.kind, UseKind::Glob));
        }
        // paths (multi-segment and resolved single identifiers)
        for p in &self.paths {
            let pkg = pkg_of(&p.location.path);
            let scope = p.enclosing_fn.clone().unwrap_or_else(|| join_key(&pkg, &p.module_path));
            push(&mut out, p.resolution.as_ref(), &pkg, UseSiteKind::Path, &p.location, &scope, &p.cfg, false);
        }
        // macro invocations
        for m in &self.macros_inv {
            let pkg = pkg_of(&m.location.path);
            let scope = m.enclosing_fn.clone().unwrap_or_else(|| join_key(&pkg, &m.module_path));
            push(&mut out, m.resolution.as_ref(), &pkg, UseSiteKind::MacroCall, &m.location, &scope, &m.cfg, false);
        }
        // attributes: own path and argument paths
        for a in &self.attrs {
            let pkg = pkg_of(&a.location.path);
            let scope = a.enclosing_fn.clone().unwrap_or_else(|| join_key(&pkg, &a.module_path));
            push(&mut out, a.resolution.as_ref(), &pkg, UseSiteKind::Attribute, &a.location, &scope, &a.cfg, false);
            for res in &a.arg_resolutions {
                push(&mut out, Some(res), &pkg, UseSiteKind::Attribute, &a.location, &scope, &a.cfg, false);
            }
        }
        if include_extern_crates {
            for e in &self.extern_crates {
                let pkg = pkg_of(&e.location.path);
                let scope = join_key(&pkg, &e.module_path);
                let res = Resolution::External(norm(&e.name));
                push(&mut out, Some(&res), &pkg, UseSiteKind::ExternCrate, &e.location, &scope, &e.cfg, false);
            }
        }

        out
    }

    fn sites_where(&self, pick: &dyn Fn(&Resolution, &str) -> Vec<String>, include_extern_crates: bool) -> Vec<UseSite> {
        self.sites_with_cfg_and_package(pick, include_extern_crates).into_iter().map(|(s, _, _)| s).collect()
    }

    /// U1: every place one of `used_deps` is touched — `use` items, paths
    /// (including aliased ones such as `Regex::new()` after
    /// `use regex::Regex;`), macro calls, attributes and `extern crate`
    /// items — with the enclosing function (or module) as `scope`.
    pub fn dep_use_sites(&self, used_deps: &BTreeSet<String>) -> Vec<UseSite> {
        let used: BTreeSet<String> = used_deps.iter().map(|d| norm(d)).collect();
        self.sites_where(
            &|res, _| match res {
                Resolution::External(e) if used.contains(crate_root(e)) => vec![e.clone()],
                _ => Vec::new(),
            },
            true,
        )
    }

    /// Q8: `dep_use_sites` paired with each site's effective cfg.
    pub fn dep_use_sites_with_cfg(&self, used_deps: &BTreeSet<String>) -> Vec<(UseSite, CfgPredicate)> {
        let used: BTreeSet<String> = used_deps.iter().map(|d| norm(d)).collect();
        self.sites_with_cfg_and_package(
            &|res, _| match res {
                Resolution::External(e) if used.contains(crate_root(e)) => vec![e.clone()],
                _ => Vec::new(),
            },
            true,
        )
        .into_iter()
        .map(|(s, c, _)| (s, c))
        .collect()
    }

    /// U3: sites that *may* belong to one of `used_deps`: their resolution is
    /// `Unresolved` with a candidate in that dependency (glob imports). Never
    /// included in [`Self::dep_use_sites`].
    pub fn possible_dep_use_sites(&self, used_deps: &BTreeSet<String>) -> Vec<UseSite> {
        let used: BTreeSet<String> = used_deps.iter().map(|d| norm(d)).collect();
        self.sites_where(
            &|res, _| match res {
                Resolution::Unresolved { candidates, .. } => {
                    candidates.iter().filter(|c| used.contains(crate_root(c))).cloned().collect()
                }
                _ => Vec::new(),
            },
            false,
        )
    }

    /// U2: every occurrence that names an item of the occurrence's own
    /// package; `dep` is the package name and `path` the canonical item path.
    pub fn internal_use_sites(&self) -> Vec<UseSite> {
        self.sites_where(
            &|res, pkg| match res {
                Resolution::Local(l) if l == pkg || l.starts_with(&format!("{pkg}::")) => vec![l.clone()],
                _ => Vec::new(),
            },
            false,
        )
    }
}
