mod proxy;

use std::collections::HashMap;

use proxy::{AccessPointProxy, DeviceProxy, NetworkManagerProxy, WirelessProxy};

#[derive(Debug, Clone)]
pub struct Network {
    pub ssid: String,
    pub strength: u8,
    pub security: String,
    pub is_connected: bool,
}

fn security_from_flags(flags: u32, wpa_flags: u32, rsn_flags: u32) -> String {
    // NM_802_11_AP_FLAGS_PRIVACY = 0x1
    let privacy = flags & 0x1 != 0;

    let has_wpa = wpa_flags != 0;
    let has_rsn = rsn_flags != 0;

    // NM_802_11_AP_SEC_KEY_MGMT_SAE = 0x400 (WPA3)
    let has_wpa3 = rsn_flags & 0x400 != 0;
    // NM_802_11_AP_SEC_KEY_MGMT_802_1X = 0x200 (Enterprise)
    let has_enterprise = (wpa_flags & 0x200 != 0) || (rsn_flags & 0x200 != 0);

    if has_enterprise {
        return "Enterprise".to_string();
    }
    if has_wpa3 && has_rsn {
        return "WPA3".to_string();
    }
    if has_rsn {
        return "WPA2".to_string();
    }
    if has_wpa {
        return "WPA".to_string();
    }
    if privacy {
        return "WEP".to_string();
    }
    "Open".to_string()
}

pub async fn scan_networks() -> Result<Vec<Network>, String> {
    let connection = zbus::Connection::system()
        .await
        .map_err(|e| format!("Failed to connect to system D-Bus: {e}"))?;

    let nm = NetworkManagerProxy::new(&connection)
        .await
        .map_err(|e| format!("Failed to create NetworkManager proxy: {e}"))?;

    let devices = nm
        .get_devices()
        .await
        .map_err(|e| format!("Failed to get devices: {e}"))?;

    // Find the WiFi device (DeviceType == 2)
    let mut wifi_device_path = None;
    for path in &devices {
        let device = DeviceProxy::builder(&connection)
            .path(path)
            .map_err(|e| format!("Invalid device path: {e}"))?
            .build()
            .await
            .map_err(|e| format!("Failed to create device proxy: {e}"))?;

        if device.device_type().await.unwrap_or(0) == 2 {
            wifi_device_path = Some(path.clone());
            break;
        }
    }

    let wifi_path = wifi_device_path.ok_or("No WiFi device found")?;

    let wireless = WirelessProxy::builder(&connection)
        .path(&wifi_path)
        .map_err(|e| format!("Invalid wireless path: {e}"))?
        .build()
        .await
        .map_err(|e| format!("Failed to create wireless proxy: {e}"))?;

    // Trigger a scan (best-effort, may fail due to permissions or rate limiting)
    let _ = wireless.request_scan(HashMap::new()).await;

    let active_ap = wireless.active_access_point().await.ok();

    let ap_paths = wireless
        .get_all_access_points()
        .await
        .map_err(|e| format!("Failed to get access points: {e}"))?;

    let mut networks: Vec<Network> = Vec::new();

    for ap_path in &ap_paths {
        let ap = AccessPointProxy::builder(&connection)
            .path(ap_path)
            .map_err(|e| format!("Invalid AP path: {e}"))?
            .build()
            .await
            .map_err(|e| format!("Failed to create AP proxy: {e}"))?;

        let ssid_bytes = ap.ssid().await.unwrap_or_default();
        let ssid = String::from_utf8_lossy(&ssid_bytes).to_string();

        // Skip hidden networks (empty SSID)
        if ssid.is_empty() {
            continue;
        }

        let strength = ap.strength().await.unwrap_or(0);
        let flags = ap.flags().await.unwrap_or(0);
        let wpa_flags = ap.wpa_flags().await.unwrap_or(0);
        let rsn_flags = ap.rsn_flags().await.unwrap_or(0);

        let is_connected = active_ap.as_ref().is_some_and(|active| active == ap_path);

        networks.push(Network {
            ssid,
            strength,
            security: security_from_flags(flags, wpa_flags, rsn_flags),
            is_connected,
        });
    }

    // Deduplicate by SSID, keeping the strongest signal
    networks.sort_by(|a, b| b.strength.cmp(&a.strength));
    let mut seen = std::collections::HashSet::new();
    networks.retain(|n| seen.insert(n.ssid.clone()));

    Ok(networks)
}
