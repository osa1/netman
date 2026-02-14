mod proxy;

use std::collections::HashMap;

use proxy::{
    AccessPointProxy, ActiveConnectionProxy, DeviceProxy, NetworkManagerProxy, WirelessProxy,
};

#[derive(Debug, Clone)]
pub struct Network {
    pub ssid: String,
    pub strength: u8,
    pub security: String,
    pub is_connected: bool,
    pub ap_path: String,
    pub device_path: String,
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
            ap_path: ap_path.to_string(),
            device_path: wifi_path.to_string(),
        });
    }

    // Deduplicate by SSID: prefer connected, then strongest signal
    networks.sort_by(|a, b| {
        b.is_connected
            .cmp(&a.is_connected)
            .then(b.strength.cmp(&a.strength))
    });
    let mut seen = std::collections::HashSet::new();
    networks.retain(|n| seen.insert(n.ssid.clone()));

    Ok(networks)
}

pub async fn connect(network: Network, password: String) -> Result<(), String> {
    let connection = zbus::Connection::system()
        .await
        .map_err(|e| format!("Failed to connect to system D-Bus: {e}"))?;

    let nm = NetworkManagerProxy::new(&connection)
        .await
        .map_err(|e| format!("Failed to create NetworkManager proxy: {e}"))?;

    let device_path = zbus::zvariant::ObjectPath::try_from(network.device_path.as_str())
        .map_err(|e| format!("Invalid device path: {e}"))?;
    let ap_path = zbus::zvariant::ObjectPath::try_from(network.ap_path.as_str())
        .map_err(|e| format!("Invalid AP path: {e}"))?;

    let mut settings: HashMap<&str, HashMap<&str, zbus::zvariant::Value<'_>>> = HashMap::new();

    let mut conn_section: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    conn_section.insert("type", "802-11-wireless".into());
    conn_section.insert("id", network.ssid.as_str().into());
    settings.insert("connection", conn_section);

    let mut wireless_section: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    wireless_section.insert("ssid", zbus::zvariant::Value::from(network.ssid.as_bytes()));
    wireless_section.insert("mode", "infrastructure".into());
    settings.insert("802-11-wireless", wireless_section);

    if network.security != "Open" {
        let mut security_section: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
        let key_mgmt = if network.security == "WPA3" {
            "sae"
        } else {
            "wpa-psk"
        };
        security_section.insert("key-mgmt", key_mgmt.into());
        security_section.insert("psk", password.as_str().into());
        settings.insert("802-11-wireless-security", security_section);
    }

    nm.add_and_activate_connection(settings, &device_path, &ap_path)
        .await
        .map_err(|e| format!("Failed to connect: {e}"))?;

    Ok(())
}

async fn has_active_wifi(
    nm: &NetworkManagerProxy<'_>,
    connection: &zbus::Connection,
) -> Result<bool, String> {
    let active_connections = nm
        .active_connections()
        .await
        .map_err(|e| format!("Failed to get active connections: {e}"))?;

    for path in &active_connections {
        let ac = ActiveConnectionProxy::builder(connection)
            .path(path)
            .map_err(|e| format!("Invalid active connection path: {e}"))?
            .build()
            .await
            .map_err(|e| format!("Failed to create active connection proxy: {e}"))?;

        if ac.connection_type().await.unwrap_or_default() == "802-11-wireless" {
            return Ok(true);
        }
    }

    Ok(false)
}

pub async fn disconnect() -> Result<(), String> {
    let connection = zbus::Connection::system()
        .await
        .map_err(|e| format!("Failed to connect to system D-Bus: {e}"))?;

    let nm = NetworkManagerProxy::new(&connection)
        .await
        .map_err(|e| format!("Failed to create NetworkManager proxy: {e}"))?;

    let active_connections = nm
        .active_connections()
        .await
        .map_err(|e| format!("Failed to get active connections: {e}"))?;

    let mut found = false;
    for path in &active_connections {
        let ac = ActiveConnectionProxy::builder(&connection)
            .path(path)
            .map_err(|e| format!("Invalid active connection path: {e}"))?
            .build()
            .await
            .map_err(|e| format!("Failed to create active connection proxy: {e}"))?;

        if ac.connection_type().await.unwrap_or_default() == "802-11-wireless" {
            nm.deactivate_connection(path)
                .await
                .map_err(|e| format!("Failed to disconnect: {e}"))?;
            found = true;
            break;
        }
    }

    if !found {
        return Err("No active WiFi connection found".to_string());
    }

    // Poll until NetworkManager confirms there's no active WiFi connection
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if !has_active_wifi(&nm, &connection).await? {
            return Ok(());
        }
    }

    Err("Disconnect timed out".to_string())
}
