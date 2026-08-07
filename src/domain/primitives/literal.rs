//! Guards the `disallowed-methods` list of `clippy.toml`, which is what keeps the bare `literal`
//! constructor out of the code (see the rule in `CLAUDE.md`).
//!
//! The list needs guarding because both ways of getting it wrong are silent: clippy ignores a path
//! that resolves to nothing without a word, and it has no globs, so a domain type nobody adds is
//! simply unprotected. Running clippy here would mean cargo inside cargo, so this reads the
//! declarations and the config instead and compares them.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use regex::Regex;

fn primitives_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/domain/primitives")
}

/// Every type with a `literal` constructor and the module it is declared in — the path clippy
/// resolves it to. Two ways to get one: `#[domain_type]` generates it for the numbers (the
/// string types get none), or it is written out by hand, as `Count` does.
fn declared_types() -> BTreeMap<String, String> {
    let newtype = Regex::new(r"struct (\w+)\((\w+)\)").expect("a valid pattern");
    let by_macro = Regex::new(r"number!\((\w+),").expect("a valid pattern");
    let ids = Regex::new(r"(?s)(?:signed_)?id!\s*[({](.*?)[)}]").expect("a valid pattern");
    let name = Regex::new(r"\b([A-Z]\w*)\b").expect("a valid pattern");
    let impl_header = Regex::new(r"^impl\s*(?:<[^>]*>)?\s*(\w+)").expect("a valid pattern");

    let mut found = BTreeMap::new();
    let mut files = vec![primitives_dir()];
    while let Some(path) = files.pop() {
        if path.is_dir() {
            files.extend(fs::read_dir(&path).expect("a readable directory")
                .map(|entry| entry.expect("a readable entry").path()));
            continue
        }
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue
        }
        let relative = path.strip_prefix(env!("CARGO_MANIFEST_DIR")).expect("a path inside the crate");
        let module = relative.with_extension("").to_string_lossy()
            .replace(['/', '\\'], "::")
            .replacen("src::", "", 1)
            .trim_end_matches("::mod")
            .to_owned();
        let source = fs::read_to_string(&path).expect("a readable source file");

        for captures in newtype.captures_iter(&source) {
            if &captures[2] != "String" {
                found.insert(captures[1].to_owned(), module.clone());
            }
        }
        for captures in by_macro.captures_iter(&source) {
            found.insert(captures[1].to_owned(), module.clone());
        }
        for captures in ids.captures_iter(&source) {
            for captures in name.captures_iter(&captures[1]) {
                found.insert(captures[1].to_owned(), module.clone());
            }
        }
        // A constructor written by hand rather than generated, attributed to the `impl` it sits
        // in. Without this the guard would miss exactly the types the macro never saw.
        let mut current_impl = None;
        for line in source.lines() {
            if let Some(captures) = impl_header.captures(line) {
                current_impl = Some(captures[1].to_owned());
            }
            if line.contains("fn literal(")
                && let Some(name) = current_impl.as_ref()
            {
                found.insert(name.clone(), module.clone());
            }
        }
    }
    found
}

/// The paths `clippy.toml` forbids, as `(type, module)`.
fn forbidden_types() -> BTreeMap<String, String> {
    let config = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("clippy.toml"))
        .expect("clippy.toml must be readable");
    Regex::new(r#"path = "dick_grower_bot::(.+?)::(\w+)::literal""#).expect("a valid pattern")
        .captures_iter(&config)
        .map(|captures| (captures[2].to_owned(), captures[1].to_owned()))
        .collect()
}

/// A type missing here is one whose bare constructor nobody stops; a stale entry is a path that
/// resolves to nothing, which clippy passes over in silence.
#[test]
fn every_domain_number_is_forbidden_by_its_real_path() {
    let declared = declared_types();
    let forbidden = forbidden_types();

    let missing: Vec<_> = declared.iter().filter(|(name, _)| !forbidden.contains_key(*name)).collect();
    assert!(missing.is_empty(), "not forbidden by clippy.toml: {missing:?}");

    let stale: Vec<_> = forbidden.iter().filter(|(name, _)| !declared.contains_key(*name)).collect();
    assert!(stale.is_empty(), "listed in clippy.toml but declared nowhere: {stale:?}");

    let misplaced: Vec<_> = forbidden.iter()
        .filter(|(name, module)| declared.get(*name) != Some(module))
        .map(|(name, module)| format!("{name}: listed as {module}, declared in {}", declared[name]))
        .collect();
    assert!(misplaced.is_empty(), "these paths resolve to nothing: {misplaced:#?}");
}
