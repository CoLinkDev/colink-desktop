use std::collections::HashSet;

use tauri::{AppHandle, Emitter};

use crate::{
    device_cache::{mark_cloud_sources, reconcile_devices},
    error::AppResult,
    models::DeviceInfo,
    network::cloud::DEVICES_UPDATED_EVENT,
    protocol::DeviceOnlinePayload,
    shell,
    store::db::Database,
};

pub fn replace_all(
    database: &Database,
    app: &AppHandle,
    lan_peers: &HashSet<String>,
    devices: Vec<DeviceInfo>,
    cloud_snapshot: bool,
) -> AppResult<Vec<DeviceInfo>> {
    let previous = database.load_cached_devices()?;
    let mut devices = devices;
    if cloud_snapshot {
        mark_cloud_sources(&mut devices);
    }
    let local_device_id = database
        .load_device_identity()?
        .map(|identity| identity.device_id);
    database.ensure_lan_trusts_for_devices(&devices, local_device_id.as_deref())?;
    let trusted = database.load_lan_trusts()?;
    let reconciled = reconcile_devices(
        devices,
        &previous,
        lan_peers,
        &trusted,
        local_device_id.as_deref(),
    );
    save_and_publish(database, app, reconciled)
}

pub fn update_one(
    database: &Database,
    app: &AppHandle,
    lan_peers: &HashSet<String>,
    device_id: &str,
    online: bool,
    payload: Option<DeviceOnlinePayload>,
) -> AppResult<Option<Vec<DeviceInfo>>> {
    let mut devices = database.load_cached_devices()?;
    let previous = devices.clone();

    let Some(device) = devices.iter_mut().find(|item| item.device_id == device_id) else {
        return Ok(None);
    };

    device.cloud_available = online;
    if let Some(payload) = payload {
        device.name = payload.name;
        device.device_type = payload.device_type;
    }

    let local_device_id = database
        .load_device_identity()?
        .map(|identity| identity.device_id);
    let trusted = database.load_lan_trusts()?;
    let reconciled = reconcile_devices(
        devices,
        &previous,
        lan_peers,
        &trusted,
        local_device_id.as_deref(),
    );
    save_and_publish(database, app, reconciled).map(Some)
}

pub fn reconcile_routes(
    database: &Database,
    app: &AppHandle,
    lan_peers: &HashSet<String>,
) -> AppResult<Vec<DeviceInfo>> {
    let devices = database.load_cached_devices()?;
    let local_device_id = database
        .load_device_identity()?
        .map(|identity| identity.device_id);
    let trusted = database.load_lan_trusts()?;
    let reconciled = reconcile_devices(
        devices.clone(),
        &devices,
        lan_peers,
        &trusted,
        local_device_id.as_deref(),
    );
    save_and_publish(database, app, reconciled)
}

fn save_and_publish(
    database: &Database,
    app: &AppHandle,
    devices: Vec<DeviceInfo>,
) -> AppResult<Vec<DeviceInfo>> {
    database.save_cached_devices(&devices)?;
    let _ = app.emit(DEVICES_UPDATED_EVENT, devices.clone());
    let _ = shell::refresh_tray(app);
    Ok(devices)
}
