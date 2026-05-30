use std::collections::{HashMap, HashSet};

use crate::models::{DeviceInfo, LanTrustRecord};

pub fn reconcile_devices(
    mut incoming: Vec<DeviceInfo>,
    previous: &[DeviceInfo],
    lan_peers: &HashSet<String>,
    trusted_devices: &[LanTrustRecord],
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
    let mut devices = incoming
        .into_iter()
        .map(|mut device| {
            if local_device_id == Some(device.device_id.as_str()) {
                device.online = true;
                device.lan_available = false;
                device.active_route = None;
                device.security_state = "verified".to_string();
                device.device_sources = merge_sources(&device.device_sources, true, false);
                return device;
            }

            if let Some(existing) = previous_by_id.get(device.device_id.as_str()) {
                if device.security_state == "unverified" {
                    device.security_state = existing.security_state.clone();
                }
            }

            let lan_available = lan_peers.contains(&device.device_id);
            device.lan_available = lan_available;
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
        if local_device_id == Some(record.device_id.as_str())
            || known_ids.contains(&record.device_id)
        {
            continue;
        }

        let lan_available = lan_peers.contains(&record.device_id);
        devices.push(DeviceInfo {
            device_id: record.device_id.clone(),
            name: record.name.clone(),
            device_type: "unknown".to_string(),
            online: lan_available,
            cloud_available: false,
            last_seen: None,
            public_key: record.public_key.clone(),
            public_key_updated_at: None,
            lan_available,
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

fn merge_sources(current: &[String], local: bool, lan_trust: bool) -> Vec<String> {
    let mut sources = Vec::new();
    if local {
        push_source(&mut sources, "local");
    }
    for source in current {
        match source.as_str() {
            "local" | "cloud" | "lan_trust" => push_source(&mut sources, source),
            _ => {}
        }
    }
    if lan_trust {
        push_source(&mut sources, "lan_trust");
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
    use std::collections::HashSet;

    use crate::models::DeviceInfo;

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
            active_route: Some("lan".to_string()),
            device_sources: vec!["cloud".to_string()],
            security_state: "verified".to_string(),
        }];
        let lan_peers = HashSet::from(["d1".to_string()]);

        let reconciled = reconcile_devices(incoming, &previous, &lan_peers, &[], None);

        assert_eq!(reconciled[0].active_route.as_deref(), Some("lan"));
        assert!(reconciled[0].lan_available);
        assert!(reconciled[0].cloud_available);
        assert_eq!(reconciled[0].security_state, "verified");
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
            active_route: None,
            device_sources: vec!["local".to_string()],
            security_state: "verified".to_string(),
        }];
        mark_cloud_sources(&mut incoming);

        let reconciled = reconcile_devices(incoming, &[], &HashSet::new(), &[], Some("local"));

        assert!(reconciled[0].online);
        assert!(reconciled[0].cloud_available);
        assert_eq!(reconciled[0].device_sources, vec!["local", "cloud"]);
    }
}
