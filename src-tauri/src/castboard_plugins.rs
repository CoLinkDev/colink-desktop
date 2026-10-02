use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::{self, Write},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{
    http::{header, Method, Request, Response, StatusCode},
    AppHandle, Manager, Runtime, UriSchemeContext,
};
use tracing::warn;
use uuid::Uuid;
use zip::ZipArchive;

use crate::castboard_ipc::WINDOW_LABEL;

const PLUGINS_DIRECTORY: &str = "castboard-plugins";
const STATE_FILE: &str = ".colink-plugin.json";
const MAX_ARCHIVE_FILES: usize = 512;
const MAX_ARCHIVE_FILE_SIZE: u64 = 16 * 1024 * 1024;
const MAX_ARCHIVE_TOTAL_SIZE: u64 = 64 * 1024 * 1024;
const MAX_MANIFEST_SIZE: u64 = 1024 * 1024;
const CASTBOARD_VERSION: &str = env!("COLINK_CASTBOARD_VERSION");

pub const ERROR_INVALID_ARCHIVE: &str = "castboard_plugin_invalid_archive";
pub const ERROR_INVALID_MANIFEST: &str = "castboard_plugin_invalid_manifest";
pub const ERROR_INCOMPATIBLE: &str = "castboard_plugin_incompatible";
pub const ERROR_STORAGE: &str = "castboard_plugin_storage_error";
pub const ERROR_NOT_FOUND: &str = "castboard_plugin_not_found";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    schema_version: String,
    id: String,
    name: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<BTreeMap<String, String>>,
    version: String,
    min_cast_board_version: String,
    #[serde(rename = "type")]
    plugin_type: String,
    entry: String,
    #[serde(flatten)]
    additional: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    id: String,
    name: BTreeMap<String, String>,
    description: Option<BTreeMap<String, String>>,
    version: String,
    min_cast_board_version: String,
    #[serde(rename = "type")]
    plugin_type: String,
    entry: String,
    enabled: bool,
    installed_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PluginState {
    enabled: bool,
    installed_at: u64,
}

#[derive(Clone, Debug)]
struct InstalledPlugin {
    manifest: PluginManifest,
    state: PluginState,
    directory_name: String,
}

impl InstalledPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: self.manifest.id.clone(),
            name: self.manifest.name.clone(),
            description: self.manifest.description.clone(),
            version: self.manifest.version.clone(),
            min_cast_board_version: self.manifest.min_cast_board_version.clone(),
            plugin_type: self.manifest.plugin_type.clone(),
            entry: self.manifest.entry.clone(),
            enabled: self.state.enabled,
            installed_at: self.state.installed_at,
        }
    }

    fn registration(&self) -> Value {
        serde_json::json!({
            "manifest": self.manifest,
            "baseUrl": plugin_base_url(&self.directory_name),
        })
    }
}

pub fn list(app: &AppHandle) -> Result<Vec<PluginInfo>, String> {
    let mut plugins = load_plugins(&plugins_root(app)?)?;
    sort_plugins(&mut plugins);
    Ok(plugins.into_iter().map(|plugin| plugin.info()).collect())
}

pub fn registrations(app: &AppHandle) -> Result<Vec<Value>, String> {
    let mut plugins = load_plugins(&plugins_root(app)?)?;
    sort_plugins(&mut plugins);
    Ok(plugins
        .into_iter()
        .filter(|plugin| plugin.state.enabled)
        .map(|plugin| plugin.registration())
        .collect())
}

fn sort_plugins(plugins: &mut [InstalledPlugin]) {
    plugins.sort_by(|left, right| {
        left.state
            .installed_at
            .cmp(&right.state.installed_at)
            .then_with(|| left.manifest.id.cmp(&right.manifest.id))
    });
}

pub fn import(app: &AppHandle, archive_path: &Path) -> Result<PluginInfo, String> {
    let root = plugins_root(app)?;
    fs::create_dir_all(&root).map_err(|_| ERROR_STORAGE.to_string())?;
    import_archive(&root, archive_path, timestamp_millis())
}

pub fn toggle(app: &AppHandle, id: &str, enabled: bool) -> Result<(), String> {
    let root = plugins_root(app)?;
    let plugin = find_plugin(&root, id)?;
    let next = PluginState {
        enabled,
        installed_at: plugin.state.installed_at,
    };
    write_state_atomic(&root.join(plugin.directory_name), &next)
}

pub fn delete(app: &AppHandle, id: &str) -> Result<(), String> {
    let root = plugins_root(app)?;
    let plugin = find_plugin(&root, id)?;
    fs::remove_dir_all(root.join(plugin.directory_name)).map_err(|_| ERROR_STORAGE.to_string())
}

pub fn protocol_response<R: Runtime>(
    context: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    if context.webview_label() != WINDOW_LABEL {
        return response(StatusCode::FORBIDDEN, "text/plain; charset=utf-8", Vec::new());
    }
    if request.method() == Method::OPTIONS {
        return response(StatusCode::NO_CONTENT, "text/plain; charset=utf-8", Vec::new());
    }
    if request.method() != Method::GET {
        return response(StatusCode::METHOD_NOT_ALLOWED, "text/plain; charset=utf-8", Vec::new());
    }

    let Some(relative_path) = safe_protocol_path(request.uri().path()) else {
        return response(StatusCode::BAD_REQUEST, "text/plain; charset=utf-8", Vec::new());
    };
    if relative_path.file_name().and_then(|name| name.to_str()) == Some(STATE_FILE) {
        return response(StatusCode::NOT_FOUND, "text/plain; charset=utf-8", Vec::new());
    }

    let Ok(root) = plugins_root(context.app_handle()) else {
        return response(StatusCode::INTERNAL_SERVER_ERROR, "text/plain; charset=utf-8", Vec::new());
    };
    let Ok(canonical_root) = root.canonicalize() else {
        return response(StatusCode::NOT_FOUND, "text/plain; charset=utf-8", Vec::new());
    };
    let requested = root.join(relative_path);
    let Ok(canonical_requested) = requested.canonicalize() else {
        return response(StatusCode::NOT_FOUND, "text/plain; charset=utf-8", Vec::new());
    };
    if !canonical_requested.starts_with(&canonical_root) || !canonical_requested.is_file() {
        return response(StatusCode::NOT_FOUND, "text/plain; charset=utf-8", Vec::new());
    }

    match fs::read(&canonical_requested) {
        Ok(body) => response(StatusCode::OK, mime_type(&canonical_requested), body),
        Err(_) => response(StatusCode::NOT_FOUND, "text/plain; charset=utf-8", Vec::new()),
    }
}

fn plugins_root<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join(PLUGINS_DIRECTORY))
        .map_err(|_| ERROR_STORAGE.to_string())
}

fn import_archive(root: &Path, archive_path: &Path, installed_at: u64) -> Result<PluginInfo, String> {
    if archive_path.extension().and_then(|extension| extension.to_str()).map(str::to_ascii_lowercase)
        != Some("zip".to_string())
    {
        return Err(ERROR_INVALID_ARCHIVE.to_string());
    }

    let stage = root.join(format!(".import-{}", Uuid::new_v4()));
    fs::create_dir(&stage).map_err(|_| ERROR_STORAGE.to_string())?;
    let result = (|| {
        extract_archive(archive_path, &stage)?;
        let package_root = resolve_package_root(&stage)?;
        let manifest = read_manifest(&package_root)?;
        validate_manifest(&manifest, &package_root)?;

        let directory_name = plugin_directory_name(&manifest.id);
        let target = root.join(&directory_name);
        let existing = if target.is_dir() {
            read_installed_plugin(&target, directory_name.clone()).ok()
        } else {
            None
        };
        let state = PluginState {
            enabled: existing.as_ref().map(|plugin| plugin.state.enabled).unwrap_or(true),
            installed_at: existing
                .as_ref()
                .map(|plugin| plugin.state.installed_at)
                .unwrap_or(installed_at),
        };
        write_state(&package_root, &state)?;
        replace_directory(&package_root, &target)?;

        Ok(InstalledPlugin {
            manifest,
            state,
            directory_name,
        }
        .info())
    })();
    if stage.exists() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}

fn resolve_package_root(stage: &Path) -> Result<PathBuf, String> {
    if stage.join("manifest.json").is_file() {
        return Ok(stage.to_path_buf());
    }

    let entries = fs::read_dir(stage).map_err(|_| ERROR_STORAGE.to_string())?;
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| ERROR_STORAGE.to_string())?;
        let name = entry.file_name();
        if matches!(name.to_str(), Some("__MACOSX" | ".DS_Store")) {
            continue;
        }
        candidates.push(entry);
    }

    if candidates.len() != 1 {
        return Err(ERROR_INVALID_MANIFEST.to_string());
    }
    let package_root = candidates.pop().expect("candidate count was checked").path();
    if !package_root.is_dir() || !package_root.join("manifest.json").is_file() {
        return Err(ERROR_INVALID_MANIFEST.to_string());
    }
    Ok(package_root)
}

fn extract_archive(archive_path: &Path, destination: &Path) -> Result<(), String> {
    let file = File::open(archive_path).map_err(|_| ERROR_INVALID_ARCHIVE.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|_| ERROR_INVALID_ARCHIVE.to_string())?;
    if archive.len() > MAX_ARCHIVE_FILES {
        return Err(ERROR_INVALID_ARCHIVE.to_string());
    }

    let mut total_size = 0_u64;
    let mut visited = HashSet::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|_| ERROR_INVALID_ARCHIVE.to_string())?;
        let entry_name = entry.name().to_string();
        let relative = safe_package_path(&entry_name).ok_or_else(|| ERROR_INVALID_ARCHIVE.to_string())?;
        if entry.enclosed_name().is_none() {
            return Err(ERROR_INVALID_ARCHIVE.to_string());
        }
        if relative.as_os_str().is_empty() {
            continue;
        }
        if !visited.insert(relative.clone()) {
            return Err(ERROR_INVALID_ARCHIVE.to_string());
        }
        if entry.unix_mode().is_some_and(|mode| mode & 0o170000 == 0o120000) {
            return Err(ERROR_INVALID_ARCHIVE.to_string());
        }
        if entry.size() > MAX_ARCHIVE_FILE_SIZE {
            return Err(ERROR_INVALID_ARCHIVE.to_string());
        }
        total_size = total_size
            .checked_add(entry.size())
            .filter(|size| *size <= MAX_ARCHIVE_TOTAL_SIZE)
            .ok_or_else(|| ERROR_INVALID_ARCHIVE.to_string())?;

        let target = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|_| ERROR_STORAGE.to_string())?;
            continue;
        }
        if !entry.is_file() {
            return Err(ERROR_INVALID_ARCHIVE.to_string());
        }
        let parent = target.parent().ok_or_else(|| ERROR_INVALID_ARCHIVE.to_string())?;
        fs::create_dir_all(parent).map_err(|_| ERROR_STORAGE.to_string())?;
        let mut output = File::create(&target).map_err(|_| ERROR_STORAGE.to_string())?;
        let copied = io::copy(&mut entry, &mut output).map_err(|_| ERROR_INVALID_ARCHIVE.to_string())?;
        if copied != entry.size() {
            return Err(ERROR_INVALID_ARCHIVE.to_string());
        }
    }
    Ok(())
}

fn read_manifest(directory: &Path) -> Result<PluginManifest, String> {
    let path = directory.join("manifest.json");
    let metadata = fs::metadata(&path).map_err(|_| ERROR_INVALID_MANIFEST.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_MANIFEST_SIZE {
        return Err(ERROR_INVALID_MANIFEST.to_string());
    }
    let contents = fs::read_to_string(path).map_err(|_| ERROR_INVALID_MANIFEST.to_string())?;
    serde_json::from_str(&contents).map_err(|_| ERROR_INVALID_MANIFEST.to_string())
}

fn validate_manifest(manifest: &PluginManifest, directory: &Path) -> Result<(), String> {
    if manifest.schema_version != "1.0.0"
        || manifest.id.trim().is_empty()
        || !valid_localized_strings(&manifest.name)
        || manifest
            .description
            .as_ref()
            .is_some_and(|description| !valid_localized_strings(description))
        || parse_version(&manifest.version).is_none()
        || !matches!(manifest.plugin_type.as_str(), "navigable" | "transient")
    {
        return Err(ERROR_INVALID_MANIFEST.to_string());
    }
    let minimum = parse_version(&manifest.min_cast_board_version)
        .ok_or_else(|| ERROR_INVALID_MANIFEST.to_string())?;
    let current = parse_version(CASTBOARD_VERSION).expect("build CastBoard version is valid");
    if minimum > current {
        return Err(ERROR_INCOMPATIBLE.to_string());
    }

    let entry = safe_package_path(&manifest.entry).ok_or_else(|| ERROR_INVALID_MANIFEST.to_string())?;
    let entry_path = directory.join(entry);
    let canonical_directory = directory.canonicalize().map_err(|_| ERROR_STORAGE.to_string())?;
    let canonical_entry = entry_path
        .canonicalize()
        .map_err(|_| ERROR_INVALID_MANIFEST.to_string())?;
    if !canonical_entry.starts_with(canonical_directory) || !canonical_entry.is_file() {
        return Err(ERROR_INVALID_MANIFEST.to_string());
    }
    Ok(())
}

fn valid_localized_strings(strings: &BTreeMap<String, String>) -> bool {
    !strings.is_empty()
        && strings
            .iter()
            .all(|(language, value)| !language.trim().is_empty() && !value.trim().is_empty())
}

fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.');
    let parse_component = |component: &str| {
        if component.is_empty() || !component.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        component.parse().ok()
    };
    let parsed = (
        parse_component(parts.next()?)?,
        parse_component(parts.next()?)?,
        parse_component(parts.next()?)?,
    );
    if parts.next().is_some() {
        return None;
    }
    Some(parsed)
}

fn safe_package_path(path: &str) -> Option<PathBuf> {
    let bytes = path.as_bytes();
    if path.trim().is_empty()
        || path.starts_with('/')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || path.bytes().any(|byte| matches!(byte, b':' | b'?' | b'#'))
        || path.contains('\\')
        || path.contains('\0')
    {
        return None;
    }
    let path = Path::new(path);
    if path.is_absolute() || !path.components().all(|component| matches!(component, Component::Normal(_))) {
        return None;
    }
    Some(path.to_path_buf())
}

fn safe_protocol_path(path: &str) -> Option<PathBuf> {
    let decoded = percent_decode_str(path.trim_start_matches('/')).decode_utf8().ok()?;
    let relative = safe_package_path(&decoded)?;
    let mut components = relative.components();
    let first = components.next()?.as_os_str().to_str()?;
    if !first.starts_with("plugin-") {
        return None;
    }
    Some(relative)
}

fn plugin_directory_name(id: &str) -> String {
    let digest = Sha256::digest(id.as_bytes());
    format!("plugin-{digest:x}")
}

fn plugin_base_url(directory_name: &str) -> String {
    if cfg!(any(target_os = "windows", target_os = "android")) {
        format!("http://colink-plugin.localhost/{directory_name}/")
    } else {
        format!("colink-plugin://localhost/{directory_name}/")
    }
}

fn load_plugins(root: &Path) -> Result<Vec<InstalledPlugin>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(root).map_err(|_| ERROR_STORAGE.to_string())?;
    let mut plugins = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| ERROR_STORAGE.to_string())?;
        let file_type = entry.file_type().map_err(|_| ERROR_STORAGE.to_string())?;
        let directory_name = entry.file_name().to_string_lossy().into_owned();
        if !file_type.is_dir() || !directory_name.starts_with("plugin-") {
            continue;
        }
        match read_installed_plugin(&entry.path(), directory_name) {
            Ok(plugin) => plugins.push(plugin),
            Err(error) => warn!(path = %entry.path().display(), %error, "ignoring invalid CastBoard plugin directory"),
        }
    }
    Ok(plugins)
}

fn read_installed_plugin(directory: &Path, directory_name: String) -> Result<InstalledPlugin, String> {
    let manifest = read_manifest(directory)?;
    validate_manifest(&manifest, directory)?;
    if plugin_directory_name(&manifest.id) != directory_name {
        return Err(ERROR_INVALID_MANIFEST.to_string());
    }
    let state: PluginState = serde_json::from_slice(
        &fs::read(directory.join(STATE_FILE)).map_err(|_| ERROR_STORAGE.to_string())?,
    )
    .map_err(|_| ERROR_STORAGE.to_string())?;
    Ok(InstalledPlugin {
        manifest,
        state,
        directory_name,
    })
}

fn find_plugin(root: &Path, id: &str) -> Result<InstalledPlugin, String> {
    let directory_name = plugin_directory_name(id);
    let target = root.join(&directory_name);
    if !target.is_dir() {
        return Err(ERROR_NOT_FOUND.to_string());
    }
    let plugin = read_installed_plugin(&target, directory_name)?;
    if plugin.manifest.id != id {
        return Err(ERROR_NOT_FOUND.to_string());
    }
    Ok(plugin)
}

fn write_state(directory: &Path, state: &PluginState) -> Result<(), String> {
    let contents = serde_json::to_vec(state).map_err(|_| ERROR_STORAGE.to_string())?;
    let mut file = File::create(directory.join(STATE_FILE)).map_err(|_| ERROR_STORAGE.to_string())?;
    file.write_all(&contents).map_err(|_| ERROR_STORAGE.to_string())?;
    file.sync_all().map_err(|_| ERROR_STORAGE.to_string())
}

fn write_state_atomic(directory: &Path, state: &PluginState) -> Result<(), String> {
    let temporary = directory.join(format!("{STATE_FILE}.{}.tmp", Uuid::new_v4()));
    let current = directory.join(STATE_FILE);
    let backup = directory.join(format!("{STATE_FILE}.{}.backup", Uuid::new_v4()));
    let contents = serde_json::to_vec(state).map_err(|_| ERROR_STORAGE.to_string())?;
    let result = (|| {
        let mut file = File::create(&temporary).map_err(|_| ERROR_STORAGE.to_string())?;
        file.write_all(&contents).map_err(|_| ERROR_STORAGE.to_string())?;
        file.sync_all().map_err(|_| ERROR_STORAGE.to_string())?;
        if current.exists() {
            fs::rename(&current, &backup).map_err(|_| ERROR_STORAGE.to_string())?;
        }
        if fs::rename(&temporary, &current).is_err() {
            if backup.exists() {
                let _ = fs::rename(&backup, &current);
            }
            return Err(ERROR_STORAGE.to_string());
        }
        if backup.exists() {
            let _ = fs::remove_file(&backup);
        }
        Ok(())
    })();
    if temporary.exists() {
        let _ = fs::remove_file(temporary);
    }
    if backup.exists() && current.exists() {
        let _ = fs::remove_file(backup);
    }
    result
}

fn replace_directory(stage: &Path, target: &Path) -> Result<(), String> {
    if !target.exists() {
        return fs::rename(stage, target).map_err(|_| ERROR_STORAGE.to_string());
    }

    let parent = target.parent().ok_or_else(|| ERROR_STORAGE.to_string())?;
    let backup = parent.join(format!(".backup-{}", Uuid::new_v4()));
    fs::rename(target, &backup).map_err(|_| ERROR_STORAGE.to_string())?;
    if fs::rename(stage, target).is_err() {
        let _ = fs::rename(&backup, target);
        return Err(ERROR_STORAGE.to_string());
    }
    if let Err(error) = fs::remove_dir_all(&backup) {
        warn!(path = %backup.display(), %error, "failed to remove replaced CastBoard plugin backup");
    }
    Ok(())
}

fn timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn response(status: StatusCode, content_type: &'static str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, OPTIONS")
        .header("X-Content-Type-Options", "nosniff")
        .body(body)
        .expect("valid CastBoard plugin response")
}

fn mime_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("js" | "mjs") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("json" | "map") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, fs::File, io::Write};

    use uuid::Uuid;
    use zip::{write::SimpleFileOptions, ZipWriter};

    use super::{
        import_archive, parse_version, plugin_directory_name, safe_package_path,
        ERROR_INVALID_ARCHIVE, ERROR_INVALID_MANIFEST,
    };

    #[test]
    fn validates_restricted_versions() {
        assert_eq!(parse_version("2.2.0"), Some((2, 2, 0)));
        assert_eq!(parse_version("2.2"), None);
        assert_eq!(parse_version("2.2.0-beta"), None);
    }

    #[test]
    fn rejects_unsafe_package_paths() {
        assert!(safe_package_path("assets/icon.svg").is_some());
        assert!(safe_package_path("../index.js").is_none());
        assert!(safe_package_path("assets\\index.js").is_none());
        assert!(safe_package_path("/index.js").is_none());
    }

    #[test]
    fn storage_keys_are_fixed_and_id_specific() {
        let first = plugin_directory_name("com.example.one");
        let second = plugin_directory_name("com.example.two");
        assert_eq!(first.len(), 71);
        assert_ne!(first, second);
    }

    #[test]
    fn imports_a_valid_plugin_package() {
        let root = std::env::temp_dir().join(format!("colink-plugin-test-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let archive = root.join("plugin.zip");
        write_zip(
            &archive,
            &[
                (
                    "manifest.json",
                    r#"{"schemaVersion":"1.0.0","id":"com.example.test","name":{"en":"Test"},"version":"1.0.0","minCastBoardVersion":"2.2.0","type":"navigable","entry":"index.js"}"#,
                ),
                ("index.js", "export default { mount() {} }")
            ],
        );

        let plugin = import_archive(&root, &archive, 42).unwrap();
        assert_eq!(plugin.id, "com.example.test");
        assert!(plugin.enabled);
        assert_eq!(plugin.installed_at, 42);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imports_a_plugin_package_from_a_single_wrapper_directory() {
        let root = std::env::temp_dir().join(format!("colink-plugin-test-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let archive = root.join("plugin.zip");
        write_zip(
            &archive,
            &[
                (".DS_Store", "metadata"),
                ("__MACOSX/plugin/._manifest.json", "metadata"),
                (
                    "plugin/manifest.json",
                    r#"{"schemaVersion":"1.0.0","id":"com.example.wrapped","name":{"en":"Wrapped"},"version":"1.0.0","minCastBoardVersion":"2.2.0","type":"transient","entry":"index.js"}"#,
                ),
                ("plugin/index.js", "export default { mount() {} }"),
            ],
        );

        let plugin = import_archive(&root, &archive, 42).unwrap();
        assert_eq!(plugin.id, "com.example.wrapped");
        let installed = root.join(plugin_directory_name("com.example.wrapped"));
        assert!(installed.join("manifest.json").is_file());
        assert!(installed.join("index.js").is_file());
        assert!(!installed.join("plugin").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_ambiguous_or_deeply_nested_package_roots() {
        for entries in [
            vec![
                ("first/manifest.json", "{}"),
                ("second/manifest.json", "{}"),
            ],
            vec![("outer/inner/manifest.json", "{}")],
        ] {
            let root = std::env::temp_dir().join(format!("colink-plugin-test-{}", Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let archive = root.join("plugin.zip");
            write_zip(&archive, &entries);

            assert_eq!(import_archive(&root, &archive, 42).unwrap_err(), ERROR_INVALID_MANIFEST);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn rejects_zip_slip_entries() {
        let root = std::env::temp_dir().join(format!("colink-plugin-test-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let archive = root.join("plugin.zip");
        write_zip(&archive, &[("../outside.js", "unsafe")]);

        assert_eq!(import_archive(&root, &archive, 42).unwrap_err(), ERROR_INVALID_ARCHIVE);
        assert!(!root.parent().unwrap().join("outside.js").exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn write_zip(path: &std::path::Path, entries: &[(&str, &str)]) {
        let mut archive = ZipWriter::new(File::create(path).unwrap());
        for (name, contents) in entries {
            archive.start_file(*name, SimpleFileOptions::default()).unwrap();
            archive.write_all(contents.as_bytes()).unwrap();
        }
        archive.finish().unwrap();
    }
}
