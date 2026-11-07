#![allow(missing_docs)]

use std::{ffi::OsStr, fs, process::Command};

#[test]
fn schema_diagnostics_point_to_the_invalid_declaration() {
    let tests = trybuild::TestCases::new();
    let mut fixtures: Vec<_> = fs::read_dir("tests/ui")
        .expect("UI fixture directory exists")
        .map(|entry| entry.expect("UI fixture can be read").path())
        .filter(|path| {
            path.extension() == Some(OsStr::new("rs"))
                && path.file_name() != Some(OsStr::new("index_trait_bound.rs"))
        })
        .collect();
    fixtures.sort();
    for fixture in fixtures {
        tests.compile_fail(fixture);
    }

    // Rust 1.88 formats this compiler-owned E0277 differently. Keep complete
    // snapshots for both renderings and require their invalid inputs to agree.
    assert_eq!(
        include_str!("ui/index_trait_bound.rs"),
        include_str!("ui/rust188/index_trait_bound.rs"),
        "compiler-specific UI fixtures must use identical source"
    );
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(compiler)
        .arg("--version")
        .output()
        .expect("active Rust compiler can report its version");
    assert!(output.status.success(), "Rust version command failed");
    let version = String::from_utf8(output.stdout).expect("Rust version is UTF-8");
    let trait_fixture = if version.starts_with("rustc 1.88.") {
        "tests/ui/rust188/index_trait_bound.rs"
    } else {
        "tests/ui/index_trait_bound.rs"
    };
    tests.compile_fail(trait_fixture);
}
