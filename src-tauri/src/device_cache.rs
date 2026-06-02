use std::collections::{HashMap, HashSet};

use crate::models::{DeviceInfo, TrustedPeerKeyRecord};

pub fn reconcile_devices(
    mut incoming: Vec<DeviceInfo>,
    previous: &[DeviceInfo],
    lan_peers: &HashMap<String, String>,
    trusted_devices: &[TrustedPeerKeyRecord],
    local_device_id: Option<&str>,
) -> Vec<DeviceInfo> {
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
    let lan_paired_ids = trusted_devices
        .iter()
        .filter(|record| record.lan_paired)
        .map(|record| record.device_id.as_str())
        .collect::<HashSet<_>>();
    let mut devices = incoming
        .into_iter()
        .map(|mut device| {
            if local_device_id == Some(device.device_id.as_str()) {
                device.online = true;
                device.lan_available = false;
                device.lan_state = "unavailable".to_string();
                device.active_route = None;
                device.security_state = "verified".to_string();
                device.device_sources = merge_sources(&device.device_sources, true, false);
                return device;
            }

            let lan_paired = lan_paired_ids.contains(device.device_id.as_str());
            if let Some(existing) = previous_by_id.get(device.device_id.as_str()) {
                if lan_paired && device.security_state == "unverified" {
                    device.security_state = existing.security_state.clone();
                }
            }
            if !lan_paired && device.security_state == "verified" {
                device.security_state = "unverified".to_string();
            }

            let lan_state = lan_peers
                .get(&device.device_id)
                .cloned()
                .unwrap_or_else(|| "unavailable".to_string());
            let lan_available = matches!(lan_state.as_str(), "alive" | "suspect");
            device.lan_available = lan_available;
            device.lan_state = lan_state;
            device.online = device.cloud_available || lan_available;
            if lan_available {
                device.security_state = "verified".to_string();
            }
            device.active_route = if lan_available {
                Some("lan".to_string())
            } else if device.cloud_available {
                Some("cloud".to_string())
            } else {
                None
            };
            device.device_sources = merge_sources(&device.device_sources, false, false);

            device
        })
        .collect::<Vec<_>>();

    let known_ids = devices
        .iter()
        .map(|device| device.device_id.clone())
        .collect::<HashSet<_>>();
    for record in trusted_devices {
        if !record.lan_paired {
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
        devices.push(DeviceInfo {
            device_id: record.device_id.clone(),
            name: record.name.clone(),
            device_type: "unknown".to_string(),
            online: lan_available,
            cloud_available: false,
            last_seen: None,
            public_key: record.public_key.clone(),
            public_key_updated_at: Some(record.key_updated_at),
            lan_available,
            lan_state,
            active_route: lan_available.then(|| "lan".to_string()),
            device_sources: merge_sources(&[], false, true),
            security_state: "verified".to_string(),
        });
    }

    devices
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
            "local" | "cloud" | "trusted_peer_key" => push_source(&mut sources, source),
            _ => {}
        }
    }
    if trusted_peer_key {
        push_source(&mut sources, "trusted_peer_key");
    }
    sources
}

fn push_source(sources: &mut Vec<String>, source: &str) {
    if !sources.iter().any(|item| item == source) {
        sources.push(source.to_string());
    }
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
            lan_available: false,
            lan_state: "unavailable".to_string(),
            active_route: None,
            device_sources: Vec::new(),
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
            lan_available: true,
            lan_state: "alive".to_string(),
            active_route: Some("lan".to_string()),
            device_sources: vec!["cloud".to_string()],
            security_state: "verified".to_string(),
        }];
        let lan_peers = HashMap::from([("d1".to_string(), "alive".to_string())]);

        let reconciled = reconcile_devices(incoming, &previous, &lan_peers, &[], None);

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
            lan_available: false,
            lan_state: "unavailable".to_string(),
            active_route: None,
            device_sources: Vec::new(),
            security_state: "unverified".to_string(),
        }];
        let trusted = vec![TrustedPeerKeyRecord {
            device_id: "d1".to_string(),
            name: "peer".to_string(),
            public_key: "pk".to_string(),
            key_updated_at: 1,
            trusted_at: Some(1),
            lan_paired: true,
        }];
        let lan_peers = HashMap::from([("d1".to_string(), "suspect".to_string())]);

        let reconciled = reconcile_devices(incoming, &[], &lan_peers, &trusted, None);

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
            lan_available: false,
            lan_state: "unavailable".to_string(),
            active_route: None,
            device_sources: vec!["local".to_string()],
            security_state: "verified".to_string(),
        }];
        mark_cloud_sources(&mut incoming);

        let reconciled = reconcile_devices(incoming, &[], &HashMap::new(), &[], Some("local"));

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
            lan_available: false,
            lan_state: "unavailable".to_string(),
            active_route: Some("lan".to_string()),
            device_sources: vec!["cloud".to_string()],
            security_state: "verified".to_string(),
        }];
        let trusted = vec![TrustedPeerKeyRecord {
            device_id: "d1".to_string(),
            name: "peer".to_string(),
            public_key: "pk".to_string(),
            key_updated_at: 1,
            trusted_at: None,
            lan_paired: false,
        }];

        let reconciled =
            reconcile_devices(devices.clone(), &devices, &HashMap::new(), &trusted, None);

        assert_eq!(reconciled[0].security_state, "unverified");
        assert_eq!(reconciled[0].active_route.as_deref(), Some("cloud"));
    }
}
