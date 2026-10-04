use std::{env, fs, path::PathBuf};

fn main() {
    // Cargo build-script directives must be written to stdout.
    println!("cargo:rerun-if-changed=i18n");
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("i18n");
    let mut files: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "ftl"))
        .collect();
    files.sort();
    let mut source = String::from("const CATALOGS: &[(&str, &str)] = &[\n");
    for path in files {
        let locale = path.file_stem().unwrap().to_str().unwrap();
        let filename = path.file_name().unwrap().to_str().unwrap();
        // Shared build output must not embed another worktree's absolute path.
        source.push_str(&format!(
            "({locale:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/i18n/\", {filename:?}))),\n"
        ));
    }
    source.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("catalogs.rs"),
        source,
    )
    .unwrap();
}
