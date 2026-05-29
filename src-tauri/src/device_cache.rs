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
                return device;
            }

            if let Some(existing) = previous_by_id.get(device.device_id.as_str()) {
                if device.security_state == "unverified" {
                    device.security_state = existing.security_state.clone();
                }
            }

            let lan_available = lan_peers.contains(&device.device_id);
            device.lan_available = lan_available;
            if lan_available {
                device.security_state = "verified".to_string();
            }
            device.active_route = if lan_available {
                Some("lan".to_string())
            } else if device.online {
                Some("cloud".to_string())
            } else {
                None
            };

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
            last_seen: None,
            public_key: record.public_key.clone(),
            lan_available,
            active_route: lan_available.then(|| "lan".to_string()),
            security_state: "verified".to_string(),
        });
    }

    devices
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::models::DeviceInfo;

    use super::reconcile_devices;

    #[test]
    fn keeps_real_lan_route_after_cloud_refresh() {
        let incoming = vec![DeviceInfo {
            device_id: "d1".to_string(),
            name: "peer".to_string(),
            device_type: "windows".to_string(),
            online: true,
            last_seen: None,
            public_key: "pk".to_string(),
            lan_available: false,
            active_route: None,
            security_state: "unverified".to_string(),
        }];
        let previous = vec![DeviceInfo {
            device_id: "d1".to_string(),
            name: "peer".to_string(),
            device_type: "windows".to_string(),
            online: true,
            last_seen: None,
            public_key: "pk".to_string(),
            lan_available: true,
            active_route: Some("lan".to_string()),
            security_state: "verified".to_string(),
        }];
        let lan_peers = HashSet::from(["d1".to_string()]);

        let reconciled = reconcile_devices(incoming, &previous, &lan_peers, &[], None);

        assert_eq!(reconciled[0].active_route.as_deref(), Some("lan"));
        assert!(reconciled[0].lan_available);
        assert_eq!(reconciled[0].security_state, "verified");
    }
}
