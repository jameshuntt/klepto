mod common;

use common::*;

#[test]
fn workspace_scan_sees_all_members() {
    let k = ws();
    let files: Vec<String> = k.files.iter().map(|f| file_name(&f.path)).collect();
    assert!(files.contains(&"lib.rs".to_string()), "files: {files:?}");
    assert!(files.contains(&"util.rs".to_string()), "files: {files:?}");
    assert!(files.contains(&"pbkdf.rs".to_string()), "files: {files:?}");
    // app/lib.rs, app/util.rs, app/pbkdf.rs, mydep/lib.rs, extra/lib.rs
    assert_eq!(k.files.len(), 5, "files: {files:?}");
}

#[test]
fn functions_and_imports_are_extracted() {
    let k = app_folder();
    let names: Vec<&str> = k.functions.iter().map(|f| f.name.as_str()).collect();
    for expected in ["top", "shouty", "uses_alias", "util_pub", "derive_key"] {
        assert!(names.contains(&expected), "missing {expected} in {names:?}");
    }
    let imports: Vec<&str> = k.imports.iter().map(|i| i.full_path.as_str()).collect();
    assert!(imports.contains(&"mydep::Widget"), "imports: {imports:?}");
    assert!(imports.contains(&"std::collections::HashMap"), "imports: {imports:?}");
}
