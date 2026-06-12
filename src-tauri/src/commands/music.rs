use tauri::State;

use crate::{
    models::{MusicProviderConfig, MusicProviderMeta},
    service,
    state::AppState,
};

#[tauri::command]
pub fn get_music_providers(state: State<'_, AppState>) -> Result<Vec<MusicProviderConfig>, String> {
    service::get_music_providers(state.inner()).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn update_music_providers(
    state: State<'_, AppState>,
    providers: Vec<MusicProviderConfig>,
) -> Result<(), String> {
    service::update_music_providers(state.inner(), providers).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn list_available_music_providers() -> Vec<MusicProviderMeta> {
    service::list_available_music_providers()
}
