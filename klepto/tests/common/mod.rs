#![allow(dead_code)]

use klepto::Klepto;
use std::path::{Path, PathBuf};

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(name)
}

/// The `ws` fixture parsed through cargo metadata (path-only deps, offline).
pub fn ws() -> Klepto {
    Klepto::new("app")
        .scan_workspace_root(fixture("ws"))
        .expect("cargo metadata on fixture ws")
        .parse()
        .expect("parse fixture ws")
}

/// Only the `app` member of the `ws` fixture, without cargo metadata.
pub fn app_folder() -> Klepto {
    Klepto::new("app")
        .scan_in_folder(fixture("ws").join("app").join("src"))
        .parse()
        .expect("parse fixture ws/app")
}

pub fn file_name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}
