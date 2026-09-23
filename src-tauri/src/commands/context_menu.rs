use std::path::Path;

use serde::Serialize;
use tauri::State;

use crate::state::AppState;

pub const SYSTEM_SHARE_FILES_EVENT: &str = "system-share-files";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemShareFile {
    pub path: String,
    pub name: String,
    pub size: u64,
}

fn describe_share_files(paths: &[String]) -> Vec<SystemShareFile> {
    paths
        .iter()
        .filter(|path| Path::new(path).is_file())
        .map(|path| {
            let path_ref = Path::new(path);
            let name = path_ref
                .file_name()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty())
                .unwrap_or(path)
                .to_string();
            let size = std::fs::metadata(path_ref).map(|metadata| metadata.len()).unwrap_or(0);
            SystemShareFile {
                path: path.clone(),
                name,
                size,
            }
        })
        .collect()
}

pub fn parse_send_args(args: &[String]) -> Option<Vec<String>> {
    let send_position = args.iter().position(|arg| arg == "--send")?;
    let paths = args
        .iter()
        .skip(send_position + 1)
        .filter(|arg| !arg.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>();
    (!paths.is_empty()).then_some(paths)
}

#[tauri::command]
pub fn get_pending_share_files(state: State<'_, AppState>) -> Vec<SystemShareFile> {
    describe_share_files(&state.take_share_files())
}
