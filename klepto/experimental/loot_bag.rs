use syn::{visit::{self, Visit}, ItemFn, Visibility};

struct LootBag {
    pub_functions: Vec<String>,
}

impl<'ast> Visit<'ast> for LootBag {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        // Only "steal" it if it's public
        if let Visibility::Public(_) = node.vis {
            self.pub_functions.push(node.sig.ident.to_string());
        }
        // Delegate to inner nodes (in case of fns inside fns)
        visit::visit_item_fn(self, node);
    }
}

fn walk_tree(tree: &syn::UseTree, prefix: String) {
    match tree {
        syn::UseTree::Path(path) => {
            let new_prefix = format!("{}{}::", prefix, path.ident);
            walk_tree(&path.tree, new_prefix);
        }
        syn::UseTree::Group(group) => {
            for item in &group.items {
                walk_tree(item, prefix.clone());
            }
        }
        syn::UseTree::Name(name) => {
            println!("Caught import: {}{}", prefix, name.ident);
        }
        _ => {} // Handle Glob (*) or Rename (as)
    }
}

use std::fs;
use syn::File;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Read the file as a plain string
    let content = fs::read_to_string("src/main.rs")?;

    // 2. Use syn to turn that string into a structured File object
    // This is where the "magic" happens outside of a proc-macro
    let ast = syn::parse_file(&content)?;

    // 3. Now Klepto can walk the tree!
    // No regex, no bracket-matching nightmares.
    Ok(())
}

// The "Klepto" Vision
let loot = Klepto::new("src/lib.rs")
    .steal_imports()
    .filter(|imp| imp.starts_with("std"))
    .collect();


// Inside Klepto's logic
pub fn functions_in_impl_of(&mut self, struct_name: &str) -> Vec<CapturedFn> {
    let mut results = Vec::new();
    
    // Klepto walks the ASTs it collected from the folder
    for file_ast in &self.asts {
        for item in &file_ast.items {
            if let syn::Item::Impl(item_impl) = item {
                // Check if this is the impl for 'struct_name'
                if let syn::Type::Path(tp) = &*item_impl.self_ty {
                    if tp.path.is_ident(struct_name) {
                        // We found the right impl! Now "steal" the functions.
                        for impl_item in &item_impl.items {
                            if let syn::ImplItem::Fn(m) = impl_item {
                                results.push(CapturedFn::from(m));
                            }
                        }
                    }
                }
            }
        }
    }
    results
}


use syn::{UseTree, UsePath, UseGroup, Ident};

pub struct StolenPath {
    pub root: String,     // 'std', 'crate', 'regex'
    pub full_path: String, // 'std::collections::HashMap'
    pub is_macro: bool,
}

impl Klepto {
    fn harvest_use_tree(&self, tree: &UseTree, prefix: String) -> Vec<StolenPath> {
        match tree {
            UseTree::Path(UsePath { ident, tree, .. }) => {
                let new_prefix = if prefix.is_empty() { 
                    ident.to_string() 
                } else { 
                    format!("{}::{}", prefix, ident) 
                };
                self.harvest_use_tree(tree, new_prefix)
            }
            UseTree::Group(UseGroup { items, .. }) => {
                items.iter()
                    .flat_map(|i| self.harvest_use_tree(i, prefix.clone()))
                    .collect()
            }
            UseTree::Name(n) => vec![StolenPath {
                root: prefix.split("::").next().unwrap_or("").to_string(),
                full_path: format!("{}::{}", prefix, n.ident),
                is_macro: false, // In syn, ItemUse rarely contains the '!'
            }],
            _ => vec![] // Handle Globs/Renames
        }
    }
}



#[derive(Debug, Clone)]
pub struct StolenPath {
    pub root: String,      // e.g., "std" or "crate"
    pub segments: Vec<String>, // e.g., ["collections", "HashMap"]
    pub is_internal: bool, // replaces your InternalRoot logic
}

impl StolenPath {
    pub fn full_path(&self) -> String {
        format!("{}::{}", self.root, self.segments.join("::"))
    }
}



use syn::{UseTree, UsePath, UseGroup};

pub fn extract_paths(tree: &UseTree, mut current_prefix: Vec<String>) -> Vec<StolenPath> {
    match tree {
        // Path segment: e.g., the 'std' in 'std::collections'
        UseTree::Path(UsePath { ident, tree, .. }) => {
            current_prefix.push(ident.to_string());
            extract_paths(tree, current_prefix)
        }
        
        // Group: e.g., the '{HashMap, BTreeMap}'
        UseTree::Group(UseGroup { items, .. }) => {
            items.iter()
                .flat_map(|item| extract_paths(item, current_prefix.clone()))
                .collect()
        }
        
        // Name: The "leaf" of the tree (e.g., 'HashMap')
        UseTree::Name(n) => {
            let mut segments = current_prefix;
            segments.push(n.ident.to_string());
            
            let root = segments.remove(0);
            let is_internal = matches!(root.as_str(), "crate" | "self" | "super");
            
            vec![StolenPath {
                root,
                segments,
                is_internal,
            }]
        }
        
        // Glob: e.g., 'use std::io::*;'
        UseTree::Glob(_) => {
            let root = current_prefix.remove(0);
            vec![StolenPath {
                root,
                segments: vec!["*".to_string()],
                is_internal: false,
            }]
        }
        
        _ => vec![],
    }
}



pub fn from_folder(path: &str) -> Vec<StolenPath> {
    let mut all_loot = Vec::new();
    for entry in walkdir::WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
        if entry.path().extension().map_or(false, |ext| ext == "rs") {
            let content = std::fs::read_to_string(entry.path()).unwrap();
            if let Ok(file) = syn::parse_file(&content) {
                for item in file.items {
                    if let syn::Item::Use(item_use) = item {
                        all_loot.extend(extract_paths(&item_use.tree, vec![]));
                    }
                }
            }
        }
    }
    all_loot
}



// Example of the shitti.es CLI using Klepto
let bad_stuff = Klepto::new()
    .from_folder("src/")
    .functions()
    .filter(|func| func.is_public() && !func.has_docs())
    .collect();

println!("Found {} shitties in your public API!", bad_stuff.len());



// A sketch of how Klepto handles your "Temporal Audit"
let mut file_entries: Vec<_> = walkdir::WalkDir::new("workspace_a")
    .into_iter()
    .filter_map(|e| e.ok())
    .filter(|e| e.path().extension().map_or(false, |ext| ext == "rs"))
    .map(|e| {
        let meta = e.metadata().unwrap();
        (e.path().to_owned(), meta.modified().unwrap())
    })
    .collect();

// Sort by newest first
file_entries.sort_by(|a, b| b.1.cmp(&a.1));

// The "Gold Standard" patterns from your most recent work
let gold_standard = Klepto::analyze_patterns(&file_entries[..10]);



use std::fs;
use std::time::SystemTime;

pub fn get_shitty_candidates(root: &str) -> Vec<PathBuf> {
    let mut files: Vec<_> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension() == Some("rs".as_ref()))
        .collect();

    // Sort by modification time: newest files first
    files.sort_by_key(|entry| {
        fs::metadata(entry.path())
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH)
    });
    files.reverse(); // Newest at the top

    files.into_iter().map(|e| e.path().to_owned()).collect()
}



// A "Shitty" scanner for Klepto: Detecting non-no_std errors
impl Klepto {
    pub fn find_std_errors(&self) -> Vec<FileLocation> {
        self.scan_attributes(|attr| {
            // If we find #[derive(thiserror::Error)], it's a "shitty"
            attr.path().is_ident("error") || attr.path().segments.first().map_or(false, |s| s.ident == "thiserror")
        })
    }
}




