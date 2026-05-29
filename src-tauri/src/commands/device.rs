use tauri::State;

use crate::{
    models::{
        DeviceDeletePayload, DeviceInfo, DeviceNameUpdatePayload, LanPairingCandidate,
        LanPairingDecisionPayload, RotateDeviceKeyPayload, StartLanPairingPayload,
    },
    service,
    state::AppState,
};

#[tauri::command]
pub async fn list_devices(state: State<'_, AppState>) -> Result<Vec<DeviceInfo>, String> {
    service::list_devices(state.inner())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn update_device_name(
    state: State<'_, AppState>,
    payload: DeviceNameUpdatePayload,
) -> Result<Vec<DeviceInfo>, String> {
    service::update_device_name(state.inner(), payload)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_device(
    state: State<'_, AppState>,
    payload: DeviceDeletePayload,
) -> Result<Vec<DeviceInfo>, String> {
    service::delete_device(state.inner(), payload)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn rotate_device_key(
    state: State<'_, AppState>,
    payload: RotateDeviceKeyPayload,
) -> Result<Vec<DeviceInfo>, String> {
    service::rotate_device_key(state.inner(), payload)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_lan_pairing_candidates(
    state: State<'_, AppState>,
) -> Result<Vec<LanPairingCandidate>, String> {
    Ok(state.runtime.list_lan_pairing_candidates())
}

#[tauri::command]
pub async fn start_lan_pairing(
    state: State<'_, AppState>,
    payload: StartLanPairingPayload,
) -> Result<(), String> {
    state
        .runtime
        .start_lan_pairing(payload)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn respond_lan_pairing(
    state: State<'_, AppState>,
    payload: LanPairingDecisionPayload,
) -> Result<(), String> {
    state
        .runtime
        .respond_lan_pairing(payload)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn forget_lan_trust(
    state: State<'_, AppState>,
    payload: DeviceDeletePayload,
) -> Result<Vec<DeviceInfo>, String> {
    state
        .runtime
        .forget_lan_trust(&payload.device_id)
        .map_err(|error| error.to_string())
}
