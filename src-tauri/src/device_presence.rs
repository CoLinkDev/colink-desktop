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
    database.ensure_trusted_peer_keys_for_devices(&devices, local_device_id.as_deref())?;
    let trusted = database.load_trusted_peer_keys()?;
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
    let trusted = database.load_trusted_peer_keys()?;
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
    let trusted = database.load_trusted_peer_keys()?;
    let reconciled = reconcile_devices(
        devices.clone(),
        &devices,
        lan_peers,
        &trusted,
        local_device_id.as_deref(),
    );
    save_and_publish(database, app, reconciled)
}

pub fn mark_cloud_unavailable(
    database: &Database,
    app: &AppHandle,
    lan_peers: &HashSet<String>,
) -> AppResult<Vec<DeviceInfo>> {
    let mut devices = database.load_cached_devices()?;
    for device in &mut devices {
        device.cloud_available = false;
    }
    let previous = devices.clone();
    let local_device_id = database
        .load_device_identity()?
        .map(|identity| identity.device_id);
    let trusted = database.load_trusted_peer_keys()?;
    let reconciled = reconcile_devices(
        devices,
        &previous,
        lan_peers,
        &trusted,
        local_device_id.as_deref(),
    );
    save_and_publish(database, app, reconciled)
}

fn save_and_publish(
    database: &Database,
    app: &AppHandle,
    mut devices: Vec<DeviceInfo>,
) -> AppResult<Vec<DeviceInfo>> {
    align_local_identity(database, &mut devices)?;
    database.save_cached_devices(&devices)?;
    let _ = app.emit(DEVICES_UPDATED_EVENT, devices.clone());
    let _ = shell::refresh_tray(app);
    Ok(devices)
}

fn align_local_identity(database: &Database, devices: &mut Vec<DeviceInfo>) -> AppResult<()> {
    let Some(identity) = database.load_device_identity()? else {
        return Ok(());
    };

    let Some(device) = devices
        .iter_mut()
        .find(|device| device.device_id == identity.device_id)
    else {
        devices.push(DeviceInfo {
            device_id: identity.device_id,
            name: identity.name,
            device_type: identity.device_type,
            online: true,
            cloud_available: false,
            last_seen: None,
            public_key: identity.public_key,
            public_key_updated_at: None,
            lan_available: false,
            active_route: None,
            device_sources: vec!["local".to_string()],
            security_state: "verified".to_string(),
        });
        return Ok(());
    };

    device.name = identity.name;
    device.device_type = identity.device_type;
    device.public_key = identity.public_key;
    device.online = true;
    device.lan_available = false;
    device.active_route = None;
    device.security_state = "verified".to_string();
    if !device.device_sources.iter().any(|source| source == "local") {
        device.device_sources.insert(0, "local".to_string());
    }

    Ok(())
}
