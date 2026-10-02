use std::{env, fs, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let package_json_path = manifest_dir
        .parent()
        .expect("src-tauri must have a parent directory")
        .join("package.json");
    let package_json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&package_json_path).expect("read desktop package.json"),
    )
    .expect("parse desktop package.json");
    let castboard_version = package_json["castboardVersion"]
        .as_str()
        .filter(|version| is_release_version(version))
        .expect("package.json castboardVersion must be a semantic version");

    println!("cargo:rerun-if-changed={}", package_json_path.display());
    println!("cargo:rustc-env=COLINK_CASTBOARD_VERSION={castboard_version}");
    tauri_build::build()
}

fn is_release_version(version: &str) -> bool {
    let components = version.split('.').collect::<Vec<_>>();
    components.len() == 3
        && components
            .iter()
            .all(|component| !component.is_empty() && component.chars().all(|character| character.is_ascii_digit()))
}
