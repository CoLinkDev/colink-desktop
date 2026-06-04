use std::collections::{HashMap, HashSet};

use chrono::{DateTime, SecondsFormat, Utc};

use crate::models::{DeviceInfo, TrustedPeerKeyRecord};

pub fn reconcile_devices(
    mut incoming: Vec<DeviceInfo>,
    previous: &[DeviceInfo],
    lan_peers: &HashMap<String, String>,
    lan_peer_types: &HashMap<String, String>,
    lan_peer_endpoints: &HashMap<String, (String, u16)>,
    trusted_devices: &[TrustedPeerKeyRecord],
    local_device_id: Option<&str>,
) -> Vec<DeviceInfo> {
    let alive_seen_at = current_rfc3339_millis();

    if let Some(local_device_id) = local_device_id {
        if !incoming
            .iter()
            .any(|device| device.device_id == local_device_id)
        {
            if let Some(local_device) = previous
                .iter()
                .find(|device| device.device_id == local_device_id)
            {
                incoming.push(local_device.clone());
            }
        }
    }

    let previous_by_id = previous
        .iter()
        .map(|device| (device.device_id.as_str(), device))
        .collect::<HashMap<_, _>>();
    let trusted_by_id = trusted_devices
        .iter()
        .filter(|record| is_trusted(record))
        .map(|record| (record.device_id.as_str(), record))
        .collect::<HashMap<_, _>>();
    incoming.retain(|device| {
        if local_device_id == Some(device.device_id.as_str()) {
            return true;
        }
        if trusted_by_id.contains_key(device.device_id.as_str()) {
            return true;
        }
        let has_persistent_source = device
            .device_sources
            .iter()
            .any(|source| matches!(source.as_str(), "local" | "cloud"));
        has_persistent_source
            || !device
                .device_sources
                .iter()
                .any(|source| source == "trusted_peer_key")
    });
    let mut devices = incoming
        .into_iter()
        .map(|mut device| {
            if local_device_id == Some(device.device_id.as_str()) {
                device.online = true;
                device.lan_available = false;
                device.lan_state = "unavailable".to_string();
                device.local_ip = None;
                device.local_port = None;
                device.active_route = None;
                device.security_state = "verified".to_string();
                device.trusted_by_lan = false;
                device.trusted_by_cloud = false;
                device.device_sources = merge_sources(&device.device_sources, true, false);
                return device;
            }

            let trust = trusted_by_id.get(device.device_id.as_str()).copied();
            let trusted = trust.is_some_and(is_trusted);
            let lan_trusted = trust.is_some_and(|record| record.trusted_by_lan);

            let lan_state = lan_peers
                .get(&device.device_id)
                .cloned()
                .unwrap_or_else(|| "unavailable".to_string());
            device.device_type = reconcile_device_type(
                &device.device_type,
                previous_by_id
                    .get(device.device_id.as_str())
                    .map(|existing| existing.device_type.as_str()),
                lan_peer_types.get(&device.device_id).map(String::as_str),
            );
            let lan_available = matches!(lan_state.as_str(), "alive" | "suspect");
            device.lan_available = lan_available;
            device.lan_state = lan_state;
            if lan_available {
                if let Some((ip, port)) = lan_peer_endpoints.get(&device.device_id) {
                    device.local_ip = Some(ip.clone());
                    device.local_port = Some(*port);
                }
            } else {
                device.local_ip = None;
                device.local_port = None;
            }
            if device.last_seen.is_none() {
                device.last_seen = previous_by_id
                    .get(device.device_id.as_str())
                    .and_then(|existing| existing.last_seen.clone());
            }
            if device.lan_state == "alive" {
                device.last_seen = Some(alive_seen_at.clone());
            }
            device.online = device.cloud_available || lan_available;
            device.trusted_by_lan = lan_trusted;
            device.trusted_by_cloud = trust.is_some_and(|record| record.trusted_by_cloud);
            if trusted || lan_available {
                device.security_state = "verified".to_string();
            } else if device.security_state == "verified" {
                device.security_state = "unverified".to_string();
            }
            device.active_route = if lan_available {
                Some("lan".to_string())
            } else if device.cloud_available {
                Some("cloud".to_string())
            } else {
                None
            };
            device.device_sources = merge_sources(&device.device_sources, false, lan_trusted);

            device
        })
        .collect::<Vec<_>>();

    let known_ids = devices
        .iter()
        .map(|device| device.device_id.clone())
        .collect::<HashSet<_>>();
    for record in trusted_devices {
        if !is_trusted(record) {
            continue;
        }
        if local_device_id == Some(record.device_id.as_str())
            || known_ids.contains(&record.device_id)
        {
            continue;
        }

        let lan_state = lan_peers
            .get(&record.device_id)
            .cloned()
            .unwrap_or_else(|| "unavailable".to_string());
        let lan_available = matches!(lan_state.as_str(), "alive" | "suspect");
        let endpoint = lan_peer_endpoints.get(&record.device_id);
        let last_seen = if lan_state == "alive" {
            Some(alive_seen_at.clone())
        } else {
            previous_by_id
                .get(record.device_id.as_str())
                .and_then(|device| device.last_seen.clone())
        };
        let device_type = reconcile_device_type(
            previous_by_id
                .get(record.device_id.as_str())
                .map(|device| device.device_type.as_str())
                .unwrap_or("unknown"),
            None,
            lan_peer_types.get(&record.device_id).map(String::as_str),
        );
        devices.push(DeviceInfo {
            device_id: record.device_id.clone(),
            name: record.name.clone(),
            device_type,
            online: lan_available,
            cloud_available: false,
            last_seen,
            public_key: record.public_key.clone(),
            public_key_updated_at: Some(record.key_updated_at),
            local_ip: endpoint.map(|(ip, _)| ip.clone()),
            local_port: endpoint.map(|(_, port)| *port),
            lan_available,
            lan_state,
            active_route: lan_available.then(|| "lan".to_string()),
            device_sources: trusted_sources(record),
            trusted_by_lan: record.trusted_by_lan,
            trusted_by_cloud: record.trusted_by_cloud,
            security_state: "verified".to_string(),
        });
    }

    devices
}

fn current_rfc3339_millis() -> String {
    let now: DateTime<Utc> = std::time::SystemTime::now().into();
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn mark_cloud_sources(devices: &mut [DeviceInfo]) {
    for device in devices {
        device.cloud_available = device.online;
        device.device_sources = merge_sources(&device.device_sources, false, false);
        push_source(&mut device.device_sources, "cloud");
    }
}

fn merge_sources(current: &[String], local: bool, trusted_peer_key: bool) -> Vec<String> {
    let mut sources = Vec::new();
    if local {
        push_source(&mut sources, "local");
    }
    for source in current {
        match source.as_str() {
            "local" | "cloud" => push_source(&mut sources, source),
            "trusted_peer_key" if trusted_peer_key => push_source(&mut sources, source),
            _ => {}
        }
    }
    if trusted_peer_key {
        push_source(&mut sources, "trusted_peer_key");
    }
    sources
}

fn trusted_sources(record: &TrustedPeerKeyRecord) -> Vec<String> {
    let mut sources = Vec::new();
    if record.trusted_by_cloud {
        push_source(&mut sources, "cloud");
    }
    if record.trusted_by_lan {
        push_source(&mut sources, "trusted_peer_key");
    }
    sources
}

fn is_trusted(record: &TrustedPeerKeyRecord) -> bool {
    record.trusted_by_lan || record.trusted_by_cloud
}

fn push_source(sources: &mut Vec<String>, source: &str) {
    if !sources.iter().any(|item| item == source) {
        sources.push(source.to_string());
    }
}

fn reconcile_device_type(incoming: &str, previous: Option<&str>, lan_type: Option<&str>) -> String {
    if !is_unknown_device_type(incoming) {
        return incoming.trim().to_string();
    }
    if let Some(lan_type) = lan_type.and_then(normalized_device_type) {
        return lan_type;
    }
    if let Some(previous) = previous.filter(|value| !is_unknown_device_type(value)) {
        return previous.trim().to_string();
    }
    if incoming.trim().is_empty() {
        "unknown".to_string()
    } else {
        incoming.trim().to_string()
    }
}

fn normalized_device_type(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "windows" | "macos" | "linux" | "android" | "ios" => Some(value),
        _ => None,
    }
}

fn is_unknown_device_type(value: &str) -> bool {
    let value = value.trim();
    value.is_empty() || value.eq_ignore_ascii_case("unknown")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::models::{DeviceInfo, TrustedPeerKeyRecord};

    use super::{mark_cloud_sources, reconcile_devices};

    #[test]
    fn keeps_real_lan_route_after_cloud_refresh() {
        let mut incoming = vec![DeviceInfo {
            device_id: "d1".to_string(),
            name: "peer".to_string(),
            device_type: "windows".to_string(),
            online: true,
            cloud_available: false,
            last_seen: None,
            public_key: "pk".to_string(),
            public_key_updated_at: None,
            local_ip: None,
            local_port: None,
            lan_available: false,
            lan_state: "unavailable".to_string(),
            active_route: None,
            device_sources: Vec::new(),
            trusted_by_lan: false,
            trusted_by_cloud: false,
            security_state: "unverified".to_string(),
        }];
        mark_cloud_sources(&mut incoming);
        let previous = vec![DeviceInfo {
            device_id: "d1".to_string(),
            name: "peer".to_string(),
            device_type: "windows".to_string(),
            online: true,
            cloud_available: true,
            last_seen: None,
            public_key: "pk".to_string(),
            public_key_updated_at: None,
            local_ip: None,
            local_port: None,
            lan_available: true,
            lan_state: "alive".to_string(),
            active_route: Some("lan".to_string()),
            device_sources: vec!["cloud".to_string()],
            trusted_by_lan: false,
            trusted_by_cloud: false,
            security_state: "verified".to_string(),
        }];
        let lan_peers = HashMap::from([("d1".to_string(), "alive".to_string())]);

        let reconciled = reconcile_devices(
            incoming,
            &previous,
            &lan_peers,
            &HashMap::new(),
            &HashMap::new(),
            &[],
            None,
        );

        assert_eq!(reconciled[0].active_route.as_deref(), Some("lan"));
        assert!(reconciled[0].lan_available);
        assert_eq!(reconciled[0].lan_state, "alive");
        assert!(reconciled[0].cloud_available);
        assert_eq!(reconciled[0].security_state, "verified");
    }

    #[test]
    fn keeps_suspect_lan_route_visible() {
        let incoming = vec![DeviceInfo {
            device_id: "d1".to_string(),
            name: "peer".to_string(),
            device_type: "windows".to_string(),
            online: false,
            cloud_available: false,
            last_seen: None,
            public_key: "pk".to_string(),
            public_key_updated_at: None,
            local_ip: None,
            local_port: None,
            lan_available: false,
            lan_state: "unavailable".to_string(),
            active_route: None,
            device_sources: Vec::new(),
            trusted_by_lan: false,
            trusted_by_cloud: false,
            security_state: "unverified".to_string(),
        }];
        let trusted = vec![TrustedPeerKeyRecord {
            device_id: "d1".to_string(),
            name: "peer".to_string(),
            public_key: "pk".to_string(),
            key_updated_at: 1,
            trusted_by_lan: true,
            trusted_by_cloud: false,
        }];
        let lan_peers = HashMap::from([("d1".to_string(), "suspect".to_string())]);

        let reconciled = reconcile_devices(
            incoming,
            &[],
            &lan_peers,
            &HashMap::new(),
            &HashMap::new(),
            &trusted,
            None,
        );

        assert!(reconciled[0].online);
        assert!(reconciled[0].lan_available);
        assert_eq!(reconciled[0].lan_state, "suspect");
        assert_eq!(reconciled[0].active_route.as_deref(), Some("lan"));
    }

    #[test]
    fn keeps_local_device_source_local_only() {
        let mut incoming = vec![DeviceInfo {
            device_id: "local".to_string(),
            name: "desktop".to_string(),
            device_type: "windows".to_string(),
            online: true,
            cloud_available: false,
            last_seen: None,
            public_key: "pk".to_string(),
            public_key_updated_at: None,
            local_ip: None,
            local_port: None,
            lan_available: false,
            lan_state: "unavailable".to_string(),
            active_route: None,
            device_sources: vec!["local".to_string()],
            trusted_by_lan: false,
            trusted_by_cloud: false,
            security_state: "verified".to_string(),
        }];
        mark_cloud_sources(&mut incoming);

        let reconciled = reconcile_devices(
            incoming,
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &[],
            Some("local"),
        );

        assert!(reconciled[0].online);
        assert!(reconciled[0].cloud_available);
        assert_eq!(reconciled[0].device_sources, vec!["local", "cloud"]);
    }

    #[test]
    fn clears_verified_state_when_lan_pairing_is_revoked() {
        let devices = vec![DeviceInfo {
            device_id: "d1".to_string(),
            name: "peer".to_string(),
            device_type: "windows".to_string(),
            online: true,
            cloud_available: true,
            last_seen: None,
            public_key: "pk".to_string(),
            public_key_updated_at: None,
            local_ip: None,
            local_port: None,
            lan_available: false,
            lan_state: "unavailable".to_string(),
            active_route: Some("lan".to_string()),
            device_sources: vec!["cloud".to_string()],
            trusted_by_lan: false,
            trusted_by_cloud: false,
            security_state: "verified".to_string(),
        }];
        let trusted = vec![TrustedPeerKeyRecord {
            device_id: "d1".to_string(),
            name: "peer".to_string(),
            public_key: "pk".to_string(),
            key_updated_at: 1,
            trusted_by_lan: false,
            trusted_by_cloud: false,
        }];

        let reconciled = reconcile_devices(
            devices.clone(),
            &devices,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &trusted,
            None,
        );

        assert_eq!(reconciled[0].security_state, "unverified");
        assert_eq!(reconciled[0].active_route.as_deref(), Some("cloud"));
    }

    #[test]
    fn fills_unknown_device_type_from_lan_peer_type() {
        let incoming = vec![DeviceInfo {
            device_id: "d1".to_string(),
            name: "peer".to_string(),
            device_type: "unknown".to_string(),
            online: false,
            cloud_available: false,
            last_seen: None,
            public_key: "pk".to_string(),
            public_key_updated_at: None,
            local_ip: None,
            local_port: None,
            lan_available: false,
            lan_state: "unavailable".to_string(),
            active_route: None,
            device_sources: Vec::new(),
            trusted_by_lan: false,
            trusted_by_cloud: false,
            security_state: "unverified".to_string(),
        }];
        let lan_peers = HashMap::from([("d1".to_string(), "alive".to_string())]);
        let lan_peer_types = HashMap::from([("d1".to_string(), "android".to_string())]);

        let reconciled = reconcile_devices(
            incoming,
            &[],
            &lan_peers,
            &lan_peer_types,
            &HashMap::new(),
            &[],
            None,
        );

        assert_eq!(reconciled[0].device_type, "android");
    }

    #[test]
    fn fills_trusted_lan_only_device_type_from_lan_peer_type() {
        let trusted = vec![TrustedPeerKeyRecord {
            device_id: "d1".to_string(),
            name: "peer".to_string(),
            public_key: "pk".to_string(),
            key_updated_at: 1,
            trusted_by_lan: true,
            trusted_by_cloud: false,
        }];
        let lan_peers = HashMap::from([("d1".to_string(), "alive".to_string())]);
        let lan_peer_types = HashMap::from([("d1".to_string(), "android".to_string())]);

        let reconciled = reconcile_devices(
            Vec::new(),
            &[],
            &lan_peers,
            &lan_peer_types,
            &HashMap::new(),
            &trusted,
            None,
        );

        assert_eq!(reconciled[0].device_type, "android");
    }

    #[test]
    fn fills_lan_endpoint_for_reachable_device() {
        let incoming = vec![DeviceInfo {
            device_id: "d1".to_string(),
            name: "peer".to_string(),
            device_type: "android".to_string(),
            online: false,
            cloud_available: false,
            last_seen: None,
            public_key: "pk".to_string(),
            public_key_updated_at: None,
            local_ip: None,
            local_port: None,
            lan_available: false,
            lan_state: "unavailable".to_string(),
            active_route: None,
            device_sources: Vec::new(),
            trusted_by_lan: false,
            trusted_by_cloud: false,
            security_state: "unverified".to_string(),
        }];
        let lan_peers = HashMap::from([("d1".to_string(), "alive".to_string())]);
        let lan_peer_endpoints = HashMap::from([("d1".to_string(), ("192.168.1.5".to_string(), 27777))]);

        let reconciled = reconcile_devices(
            incoming,
            &[],
            &lan_peers,
            &HashMap::new(),
            &lan_peer_endpoints,
            &[],
            None,
        );

        assert_eq!(reconciled[0].local_ip.as_deref(), Some("192.168.1.5"));
        assert_eq!(reconciled[0].local_port, Some(27777));
    }
}
