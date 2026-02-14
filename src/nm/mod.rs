pub mod proxy;

use std::collections::HashMap;

use proxy::{
    AccessPointProxy, ActiveConnectionProxy, DeviceProxy, NetworkManagerProxy,
    SettingsConnectionProxy, SettingsProxy, WirelessProxy,
};

#[derive(Debug, Clone)]
pub struct WifiDevice {
    pub path: String,
    pub interface: String,
}

impl std::fmt::Display for WifiDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.interface)
    }
}

impl PartialEq for WifiDevice {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

#[derive(Debug, Clone)]
pub struct Network {
    pub ssid: String,
    pub strength: u8,
    pub security: String,
    pub is_connected: bool,
    pub is_saved: bool,
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

fn get_wifi_ssid(
    s: &HashMap<String, HashMap<String, zbus::zvariant::OwnedValue>>,
) -> Option<String> {
    let conn_type = s.get("connection")?.get("type")?;
    let conn_type: &str = conn_type.try_into().ok()?;
    if conn_type != "802-11-wireless" {
        return None;
    }
    let ssid_val = s.get("802-11-wireless")?.get("ssid")?;
    let ssid_array: &zbus::zvariant::Array<'_> = ssid_val.try_into().ok()?;
    let ssid_bytes: Vec<u8> = ssid_array
        .iter()
        .filter_map(|v| u8::try_from(v).ok())
        .collect();
    let ssid = String::from_utf8_lossy(&ssid_bytes).to_string();
    if ssid.is_empty() { None } else { Some(ssid) }
}

async fn saved_wifi_ssids(connection: &zbus::Connection) -> std::collections::HashSet<String> {
    let mut ssids = std::collections::HashSet::new();

    let Ok(settings) = SettingsProxy::new(connection).await else {
        return ssids;
    };
    let Ok(conn_paths) = settings.list_connections().await else {
        return ssids;
    };

    for path in &conn_paths {
        let Ok(builder) = SettingsConnectionProxy::builder(connection).path(path) else {
            continue;
        };
        let Ok(conn) = builder.build().await else {
            continue;
        };
        let Ok(s) = conn.get_settings().await else {
            continue;
        };

        if let Some(ssid) = get_wifi_ssid(&s) {
            ssids.insert(ssid);
        }
    }

    ssids
}

pub async fn list_wifi_devices() -> Result<Vec<WifiDevice>, String> {
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

    let mut wifi_devices = Vec::new();
    for path in &devices {
        let device = DeviceProxy::builder(&connection)
            .path(path)
            .map_err(|e| format!("Invalid device path: {e}"))?
            .build()
            .await
            .map_err(|e| format!("Failed to create device proxy: {e}"))?;

        if device.device_type().await.unwrap_or(0) == 2 {
            let interface = device.interface().await.unwrap_or_default();
            wifi_devices.push(WifiDevice {
                path: path.to_string(),
                interface,
            });
        }
    }

    if wifi_devices.is_empty() {
        return Err("No WiFi devices found".to_string());
    }

    Ok(wifi_devices)
}

pub async fn scan_networks(device_path: &str) -> Result<Vec<Network>, String> {
    let connection = zbus::Connection::system()
        .await
        .map_err(|e| format!("Failed to connect to system D-Bus: {e}"))?;

    let wifi_path = zbus::zvariant::ObjectPath::try_from(device_path)
        .map_err(|e| format!("Invalid device path: {e}"))?;

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

    // Collect saved WiFi SSIDs
    let saved_ssids = saved_wifi_ssids(&connection).await;

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

        let is_saved = saved_ssids.contains(&ssid);

        networks.push(Network {
            ssid,
            strength,
            security: security_from_flags(flags, wpa_flags, rsn_flags),
            is_connected,
            is_saved,
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

/// Find a saved connection profile matching the given SSID.
/// Returns the connection object path if found.
async fn find_saved_connection(
    connection: &zbus::Connection,
    ssid: &str,
) -> Result<Option<zbus::zvariant::OwnedObjectPath>, String> {
    let settings = SettingsProxy::new(connection)
        .await
        .map_err(|e| format!("Failed to create Settings proxy: {e}"))?;

    let connections = settings
        .list_connections()
        .await
        .map_err(|e| format!("Failed to list connections: {e}"))?;

    for path in connections {
        let conn = SettingsConnectionProxy::builder(connection)
            .path(&path)
            .map_err(|e| format!("Invalid connection path: {e}"))?
            .build()
            .await
            .map_err(|e| format!("Failed to create connection proxy: {e}"))?;

        let Ok(s) = conn.get_settings().await else {
            continue;
        };

        if get_wifi_ssid(&s).as_deref() == Some(ssid) {
            return Ok(Some(path));
        }
    }

    Ok(None)
}

/// Poll an active connection until it reaches Activated or fails.
async fn wait_for_activation(
    connection: &zbus::Connection,
    active_path: &zbus::zvariant::OwnedObjectPath,
) -> Result<(), String> {
    let ac = ActiveConnectionProxy::builder(connection)
        .path(active_path)
        .map_err(|e| format!("Invalid active connection path: {e}"))?
        .build()
        .await
        .map_err(|e| format!("Failed to create active connection proxy: {e}"))?;

    for _ in 0..15 {
        match ac.state().await {
            Ok(2) => return Ok(()), // Activated
            Ok(3) | Ok(4) => {
                // Deactivating / Deactivated
                return Err("Connection failed (wrong password?)".to_string());
            }
            Ok(1) => {} // Activating — keep waiting
            Ok(_) | Err(_) => {
                // Unknown or proxy error (object removed)
                return Err("Connection failed".to_string());
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    Err("Connection timed out".to_string())
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

    // Check if there's a saved connection profile for this SSID
    if let Some(saved_path) = find_saved_connection(&connection, &network.ssid).await? {
        let saved_obj = zbus::zvariant::ObjectPath::try_from(saved_path.as_str())
            .map_err(|e| format!("Invalid saved connection path: {e}"))?;
        let active_path = nm
            .activate_connection(&saved_obj, &device_path, &ap_path)
            .await
            .map_err(|e| format!("Failed to connect: {e}"))?;
        let result = wait_for_activation(&connection, &active_path).await;
        if result.is_err() {
            // Delete the saved profile so the user can retry with a new password
            if let Ok(conn_proxy) = SettingsConnectionProxy::builder(&connection)
                .path(saved_path.as_ref())
                .unwrap()
                .build()
                .await
            {
                let _ = conn_proxy.delete().await;
            }
        }
        return result;
    }

    // No saved connection — build settings and create a new one
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

    let (active_path, settings_path) = nm
        .add_and_activate_connection(settings, &device_path, &ap_path)
        .await
        .map_err(|e| format!("Failed to connect: {e}"))?;

    let result = wait_for_activation(&connection, &active_path).await;
    if result.is_err() {
        // Delete the saved profile so the user can retry with a new password
        if let Ok(conn_proxy) = SettingsConnectionProxy::builder(&connection)
            .path(settings_path.as_ref())
            .unwrap()
            .build()
            .await
        {
            let _ = conn_proxy.delete().await;
        }
    }
    result
}

/// Check if the given device has an active WiFi connection.
async fn has_active_wifi_on_device(
    nm: &NetworkManagerProxy<'_>,
    connection: &zbus::Connection,
    device_path: &str,
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
            let devices = ac.devices().await.unwrap_or_default();
            if devices.iter().any(|d| d.as_str() == device_path) {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

pub async fn disconnect(device_path: &str) -> Result<(), String> {
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
            let devices = ac.devices().await.unwrap_or_default();
            if devices.iter().any(|d| d.as_str() == device_path) {
                nm.deactivate_connection(path)
                    .await
                    .map_err(|e| format!("Failed to disconnect: {e}"))?;
                found = true;
                break;
            }
        }
    }

    if !found {
        return Err("No active WiFi connection found on this device".to_string());
    }

    // Poll until NetworkManager confirms no active WiFi on this device
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if !has_active_wifi_on_device(&nm, &connection, device_path).await? {
            return Ok(());
        }
    }

    Err("Disconnect timed out".to_string())
}
