use klepto::{FileLocation, UseSite, UseSiteKind};
use regex::Regex;

use crate::scanning::patterns::UsePatternRegistry;
use std::collections::BTreeSet;

fn normalize_tail(tail_raw: &str) -> String {
    tail_raw.split_whitespace().collect::<String>()
}

fn kind_hint_from_line(trimmed: &str) -> Option<UseSiteKind> {
    if trimmed.starts_with("use ") {
        Some(UseSiteKind::UseStmt)
    } else if trimmed.starts_with("extern crate ") {
        Some(UseSiteKind::ExternCrate)
    } else if trimmed.contains("#[") {
        Some(UseSiteKind::Attribute)
    } else {
        None
    }
}

/// Core engine (same idea as before) — now assumes `dep/tail/bang` captures.
pub fn scan_use_sites_with<F>(
    file_path: &std::path::Path,
    content: &str,
    re: &Regex,
    mut accept_dep: F,
) -> Vec<UseSite>
where
    F: FnMut(&str) -> bool,
{
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

        let opens = line.chars().filter(|&c| c == '{').count();
        if opens > 0 {
            let header = header_buf.join("\n");
            if let Some(label) = crate::engine::scope_label_from_header(&header) {
                stack.push((label, depth + 1));
            }
        }

        let scope = stack.last().map(|s| s.0.clone()).unwrap_or_else(|| "file".to_string());
        let hint = kind_hint_from_line(trimmed);

        for caps in re.captures_iter(line) {
            let dep = caps.name("dep").unwrap().as_str();
            if !accept_dep(dep) { continue; }

            let tail_raw = caps.name("tail").unwrap().as_str();
            let bang = caps.name("bang").is_some();

            let tail = normalize_tail(tail_raw);
            let head = tail.split("::").next().unwrap_or("").to_string();
            let path = format!("{dep}::{tail}");

            let kind = if let Some(k) = hint {
                k
            } else if bang {
                UseSiteKind::MacroCall
            } else {
                UseSiteKind::Path
            };

let col0 = caps.get(0).map(|m| m.start()).unwrap_or(0);

out.push(UseSite {
    dep: dep.to_string(),
    path,
    head,
    kind,
    scope: scope.clone(),
    location: FileLocation {
        path: file_path.to_path_buf(),
        line: Some(line_no as u32),
        column: Some(col0 as u32),
    },
});
        }

        let closes = line.chars().filter(|&c| c == '}').count();
        depth += opens;
        for _ in 0..closes {
            if depth == 0 { break; }
            if let Some(top) = stack.last()
                && top.1 == depth {
                    stack.pop();
                }
            depth -= 1;
        }
    }

    out
}

// Wrapper: external deps
pub fn scan_dep_use_sites(file_path: &std::path::Path, content: &str, used_deps: &BTreeSet<String>) -> Vec<UseSite> {
    let re = UsePatternRegistry::global().dep_path();
    scan_use_sites_with(file_path, content, &re, |dep| used_deps.contains(dep))
}

// Wrapper: internal roots
pub fn scan_internal_use_sites(file_path: &std::path::Path, content: &str, crate_name: &str) -> Vec<UseSite> {
    let re = UsePatternRegistry::global().internal_root_for_crate(crate_name);
    scan_use_sites_with(file_path, content, &re, |_| true)
}
