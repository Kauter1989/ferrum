//! The crate stays usable on its own: no FERRUM crate and nothing but
//! `rustix` (Unix) among its dependencies (ADR 0015 §1).

// covers 23.1-b
#[test]
fn depends_on_no_ferrum_crate_and_only_rustix() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    let mut section = String::new();
    let mut deps = Vec::new();
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line.to_owned();
        } else if section.contains("dependencies") && !section.contains("dev-dependencies") {
            if let Some((name, _)) = line.split_once('=') {
                deps.push((section.clone(), name.trim().to_owned()));
            }
        }
    }
    assert_eq!(deps, [("[target.'cfg(unix)'.dependencies]".to_owned(), "rustix".to_owned())]);
    assert!(manifest.contains("license.workspace = true"));
}
