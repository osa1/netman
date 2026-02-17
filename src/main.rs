mod nm;

use iced::futures::{SinkExt, StreamExt};
use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_input};
use iced::{Element, Subscription, Task, Theme, event, keyboard, window};

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title("netman")
        .subscription(App::subscription)
        .theme(Theme::Dark)
        .window(window::Settings {
            size: iced::Size::new(480.0, 500.0),
            platform_specific: window::settings::PlatformSpecific {
                application_id: "netman".to_string(),
                ..Default::default()
            },
            ..Default::default()
        })
        .run()
}

enum App {
    Loading,
    Loaded {
        devices: Vec<nm::WifiDevice>,
        selected_device: usize,
        networks: Vec<nm::Network>,
        connecting_ssid: Option<String>,
        password: String,
        wifi_enabled: bool,
    },
    Connecting {
        devices: Vec<nm::WifiDevice>,
        selected_device: usize,
    },
    Disconnecting {
        devices: Vec<nm::WifiDevice>,
        selected_device: usize,
    },
    Error {
        message: String,
        devices: Option<Vec<nm::WifiDevice>>,
        selected_device: usize,
    },
}

#[derive(Debug, Clone)]
enum Message {
    DevicesLoaded(Result<Vec<nm::WifiDevice>, String>),
    DeviceSelected(nm::WifiDevice),
    NetworksLoaded(Result<Vec<nm::Network>, String>, String),
    NetworkChanged,
    DevicesChanged,
    Refresh,
    Back,
    Disconnect,
    Disconnected(Result<(), String>),
    Connect(String),
    PasswordChanged(String),
    SubmitConnect,
    CancelConnect,
    Connected(Result<(), String>),
    WifiEnabledChanged,
    WifiEnabledLoaded(Result<bool, String>),
    ToggleWifi(bool),
    WifiToggled(Result<bool, String>),
}

#[allow(clippy::ptr_arg)]
fn nm_signals(device_path: &String) -> iced::futures::stream::BoxStream<'static, Message> {
    let device_path = device_path.clone();
    Box::pin(iced::stream::channel(
        10,
        async move |mut output: iced::futures::channel::mpsc::Sender<Message>| {
            use nm::proxy::{NetworkManagerProxy, WirelessProxy};

            let Ok(conn) = zbus::Connection::system().await else {
                return;
            };
            let Ok(nm): Result<NetworkManagerProxy, _> = NetworkManagerProxy::new(&conn).await
            else {
                return;
            };
            let Ok(wireless): Result<WirelessProxy, _> = WirelessProxy::builder(&conn)
                .path(device_path.as_str())
                .unwrap()
                .build()
                .await
            else {
                return;
            };

            let Ok(ap_added) = wireless.receive_access_point_added().await else {
                return;
            };
            let Ok(ap_removed) = wireless.receive_access_point_removed().await else {
                return;
            };
            let active_changed = nm.receive_active_connections_changed().await;

            let mut merged = iced::futures::stream::select(
                iced::futures::stream::select(ap_added.map(|_| ()), ap_removed.map(|_| ())),
                active_changed.map(|_| ()),
            );

            while merged.next().await.is_some() {
                let _ = output.send(Message::NetworkChanged).await;
            }
        },
    ))
}

fn nm_device_signal_stream() -> iced::futures::stream::BoxStream<'static, Message> {
    Box::pin(iced::stream::channel(
        10,
        async move |mut output: iced::futures::channel::mpsc::Sender<Message>| {
            use nm::proxy::NetworkManagerProxy;

            let Ok(conn) = zbus::Connection::system().await else {
                return;
            };
            let Ok(nm): Result<NetworkManagerProxy, _> = NetworkManagerProxy::new(&conn).await
            else {
                return;
            };
            let Ok(dev_added) = nm.receive_device_added().await else {
                return;
            };
            let Ok(dev_removed) = nm.receive_device_removed().await else {
                return;
            };
            let wifi_changed = nm.receive_wireless_enabled_changed().await;

            let mut merged = iced::futures::stream::select(
                iced::futures::stream::select(
                    dev_added.map(|_| Message::DevicesChanged),
                    dev_removed.map(|_| Message::DevicesChanged),
                ),
                wifi_changed.map(|_| Message::WifiEnabledChanged),
            );

            while let Some(msg) = merged.next().await {
                let _ = output.send(msg).await;
            }
        },
    ))
}

impl App {
    fn new() -> (Self, Task<Message>) {
        (
            App::Loading,
            Task::perform(nm::list_wifi_devices(), Message::DevicesLoaded),
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        let kbd = event::listen_with(|event, _status, _window| match event {
            event::Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            }) => Some(Message::CancelConnect),
            event::Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Enter),
                ..
            }) => Some(Message::Back),
            _ => None,
        });

        let dev_signals = Subscription::run(nm_device_signal_stream);

        if let App::Loaded {
            devices,
            selected_device,
            wifi_enabled,
            ..
        } = self
            && *wifi_enabled
        {
            let device_path = devices[*selected_device].path.clone();
            Subscription::batch([
                kbd,
                dev_signals,
                Subscription::run_with(device_path, nm_signals),
            ])
        } else {
            Subscription::batch([kbd, dev_signals])
        }
    }

    /// Helper: get devices and selected index from current state (for state transitions).
    fn device_info(&self) -> Option<(Vec<nm::WifiDevice>, usize)> {
        match self {
            App::Loaded {
                devices,
                selected_device,
                ..
            }
            | App::Connecting {
                devices,
                selected_device,
            }
            | App::Disconnecting {
                devices,
                selected_device,
            } => Some((devices.clone(), *selected_device)),
            App::Error {
                devices: Some(devices),
                selected_device,
                ..
            } => Some((devices.clone(), *selected_device)),
            _ => None,
        }
    }

    /// Transition to error state, preserving device info if available.
    fn goto_error(&mut self, e: String) {
        let info = self.device_info();
        *self = App::Error {
            message: e,
            devices: info.as_ref().map(|(d, _)| d.clone()),
            selected_device: info.map(|(_, s)| s).unwrap_or(0),
        };
    }

    /// Helper: scan networks for the currently selected device.
    fn scan_selected(&self, devices: &[nm::WifiDevice], selected: usize) -> Task<Message> {
        let path = devices[selected].path.clone();
        Task::perform(
            async move {
                let result = nm::scan_networks(&path).await;
                (result, path)
            },
            |(result, path)| Message::NetworksLoaded(result, path),
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::DevicesLoaded(result) => match result {
                Ok(devices) => {
                    // Preserve previous selection if the device still exists
                    let prev_path = self.device_info().map(|(d, s)| d[s].path.clone());
                    let selected = prev_path
                        .and_then(|p| devices.iter().position(|d| d.path == p))
                        .unwrap_or(0);
                    let task = self.scan_selected(&devices, selected);
                    *self = App::Loaded {
                        devices,
                        selected_device: selected,
                        networks: Vec::new(),
                        connecting_ssid: None,
                        password: String::new(),
                        wifi_enabled: true,
                    };
                    Task::batch([
                        task,
                        Task::perform(nm::get_wifi_enabled(), Message::WifiEnabledLoaded),
                    ])
                }
                Err(e) => {
                    *self = App::Error {
                        message: e,
                        devices: None,
                        selected_device: 0,
                    };
                    Task::none()
                }
            },
            Message::DeviceSelected(device) => {
                if let App::Loaded {
                    devices,
                    selected_device,
                    networks,
                    connecting_ssid,
                    password,
                    ..
                } = self
                    && let Some(idx) = devices.iter().position(|d| d == &device)
                {
                    *selected_device = idx;
                    *networks = Vec::new();
                    *connecting_ssid = None;
                    *password = String::new();
                    let path = devices[idx].path.clone();
                    return Task::perform(
                        async move {
                            let result = nm::scan_networks(&path).await;
                            (result, path)
                        },
                        |(result, path)| Message::NetworksLoaded(result, path),
                    );
                }
                Task::none()
            }
            Message::NetworksLoaded(result, for_device) => {
                match result {
                    Ok(nets) => {
                        if let App::Loaded {
                            devices,
                            selected_device,
                            networks,
                            ..
                        } = self
                        {
                            if devices[*selected_device].path == for_device {
                                *networks = nets;
                            }
                        } else if let Some((devices, selected_device)) = self.device_info()
                            && devices[selected_device].path == for_device
                        {
                            *self = App::Loaded {
                                devices,
                                selected_device,
                                networks: nets,
                                connecting_ssid: None,
                                password: String::new(),
                                wifi_enabled: true,
                            };
                        }
                    }
                    Err(e) => self.goto_error(e),
                }
                Task::none()
            }
            Message::NetworkChanged => {
                if let App::Loaded {
                    devices,
                    selected_device,
                    ..
                } = self
                {
                    let path = devices[*selected_device].path.clone();
                    return Task::perform(
                        async move {
                            let result = nm::scan_networks(&path).await;
                            (result, path)
                        },
                        |(result, path)| Message::NetworksLoaded(result, path),
                    );
                }
                Task::none()
            }
            Message::DevicesChanged => {
                *self = App::Loading;
                Task::perform(nm::list_wifi_devices(), Message::DevicesLoaded)
            }
            Message::Back => {
                if let App::Error { .. } = self
                    && let Some((devices, selected)) = self.device_info()
                {
                    let task = self.scan_selected(&devices, selected);
                    *self = App::Loaded {
                        devices,
                        selected_device: selected,
                        networks: Vec::new(),
                        connecting_ssid: None,
                        password: String::new(),
                        wifi_enabled: true,
                    };
                    return task;
                }
                Task::none()
            }
            Message::Refresh => {
                if let Some((devices, selected)) = self.device_info() {
                    let task = self.scan_selected(&devices, selected);
                    *self = App::Loaded {
                        devices,
                        selected_device: selected,
                        networks: Vec::new(),
                        connecting_ssid: None,
                        password: String::new(),
                        wifi_enabled: true,
                    };
                    return task;
                }
                Task::none()
            }
            Message::Disconnect => {
                if let Some((devices, selected)) = self.device_info() {
                    let path = devices[selected].path.clone();
                    *self = App::Disconnecting {
                        devices,
                        selected_device: selected,
                    };
                    return Task::perform(
                        async move { nm::disconnect(&path).await },
                        Message::Disconnected,
                    );
                }
                Task::none()
            }
            Message::Disconnected(result) => {
                if let Err(e) = result {
                    self.goto_error(e);
                    return Task::none();
                }
                if let Some((devices, selected)) = self.device_info() {
                    let task = self.scan_selected(&devices, selected);
                    *self = App::Loaded {
                        devices,
                        selected_device: selected,
                        networks: Vec::new(),
                        connecting_ssid: None,
                        password: String::new(),
                        wifi_enabled: true,
                    };
                    return task;
                }
                Task::none()
            }
            Message::Connect(ssid) => {
                if let App::Loaded {
                    devices,
                    selected_device,
                    networks,
                    connecting_ssid,
                    password,
                    ..
                } = self
                {
                    // Open or saved networks: connect immediately (no password needed)
                    if let Some(net) = networks.iter().find(|n| n.ssid == ssid)
                        && (net.security == "Open" || net.is_saved)
                    {
                        let net = net.clone();
                        let devs = devices.clone();
                        let sel = *selected_device;
                        *self = App::Connecting {
                            devices: devs,
                            selected_device: sel,
                        };
                        return Task::perform(nm::connect(net, String::new()), Message::Connected);
                    }
                    *connecting_ssid = Some(ssid);
                    *password = String::new();
                    return iced::widget::operation::focus("password-input");
                }
                Task::none()
            }
            Message::PasswordChanged(pw) => {
                if let App::Loaded { password, .. } = self {
                    *password = pw;
                }
                Task::none()
            }
            Message::SubmitConnect => {
                if let App::Loaded {
                    devices,
                    selected_device,
                    networks,
                    connecting_ssid: Some(ssid),
                    password,
                    ..
                } = self
                    && let Some(net) = networks.iter().find(|n| n.ssid == *ssid)
                {
                    let net = net.clone();
                    let pw = password.clone();
                    let devs = devices.clone();
                    let sel = *selected_device;
                    *self = App::Connecting {
                        devices: devs,
                        selected_device: sel,
                    };
                    return Task::perform(nm::connect(net, pw), Message::Connected);
                }
                Task::none()
            }
            Message::CancelConnect => {
                if let App::Loaded {
                    connecting_ssid,
                    password,
                    ..
                } = self
                {
                    if connecting_ssid.is_some() {
                        *connecting_ssid = None;
                        *password = String::new();
                        return Task::none();
                    }
                    return iced::exit();
                }
                if let App::Error { .. } = self
                    && let Some((devices, selected)) = self.device_info()
                {
                    let task = self.scan_selected(&devices, selected);
                    *self = App::Loaded {
                        devices,
                        selected_device: selected,
                        networks: Vec::new(),
                        connecting_ssid: None,
                        password: String::new(),
                        wifi_enabled: true,
                    };
                    return task;
                }
                Task::none()
            }
            Message::Connected(result) => {
                if let Err(e) = result {
                    self.goto_error(e);
                    return Task::none();
                }
                if let Some((devices, selected)) = self.device_info() {
                    let task = self.scan_selected(&devices, selected);
                    *self = App::Loaded {
                        devices,
                        selected_device: selected,
                        networks: Vec::new(),
                        connecting_ssid: None,
                        password: String::new(),
                        wifi_enabled: true,
                    };
                    return task;
                }
                Task::none()
            }
            Message::WifiEnabledChanged => {
                Task::perform(nm::get_wifi_enabled(), Message::WifiEnabledLoaded)
            }
            Message::WifiEnabledLoaded(result) => {
                match result {
                    Ok(enabled) => {
                        if let App::Loaded { wifi_enabled, .. } = self {
                            *wifi_enabled = enabled;
                        }
                    }
                    Err(e) => self.goto_error(e),
                }
                Task::none()
            }
            Message::ToggleWifi(enabled) => {
                Task::perform(nm::set_wifi_enabled(enabled), Message::WifiToggled)
            }
            Message::WifiToggled(result) => {
                match result {
                    Ok(enabled) => {
                        if enabled {
                            // WiFi turned on — reload devices and networks
                            *self = App::Loading;
                            return Task::perform(nm::list_wifi_devices(), Message::DevicesLoaded);
                        }
                        if let App::Loaded {
                            wifi_enabled,
                            networks,
                            connecting_ssid,
                            password,
                            ..
                        } = self
                        {
                            *wifi_enabled = false;
                            *networks = Vec::new();
                            *connecting_ssid = None;
                            *password = String::new();
                        }
                    }
                    Err(e) => self.goto_error(e),
                }
                Task::none()
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let content: Element<Message> = match self {
            App::Loading => column![text("Scanning...").size(18)].into(),
            App::Connecting { .. } => column![text("Connecting...").size(18)].into(),
            App::Disconnecting { .. } => column![text("Disconnecting...").size(18)].into(),
            App::Loaded {
                devices,
                selected_device,
                networks,
                connecting_ssid,
                password,
                wifi_enabled,
            } => {
                let mut header = row![text("WiFi Networks").size(22),]
                    .align_y(iced::Alignment::Center)
                    .spacing(10)
                    .padding(6);

                if devices.len() > 1 {
                    header = header.push(
                        pick_list(
                            devices.as_slice(),
                            Some(&devices[*selected_device]),
                            Message::DeviceSelected,
                        )
                        .text_size(14),
                    );
                }

                header = header.push(iced::widget::space::horizontal());

                if *wifi_enabled {
                    header = header
                        .push(button("Refresh").on_press(Message::Refresh))
                        .push(button("Turn off").on_press(Message::ToggleWifi(false)));
                } else {
                    header = header.push(button("Turn on").on_press(Message::ToggleWifi(true)));
                }

                if !wifi_enabled {
                    column![header, text("WiFi is disabled").size(16)]
                        .spacing(15)
                        .into()
                } else if networks.is_empty() {
                    column![header, text("Scanning...").size(16)]
                        .spacing(15)
                        .into()
                } else {
                    let list = networks.iter().fold(column![].spacing(4), |col, network| {
                        let is_entering_password =
                            connecting_ssid.as_deref() == Some(&network.ssid);

                        let ssid_text = text(&network.ssid).size(16);
                        let info =
                            text(format!("{}%  {}", network.strength, network.security)).size(13);

                        let network_row = if is_entering_password {
                            // Password input row
                            let input = text_input("Password", password)
                                .id("password-input")
                                .on_input(Message::PasswordChanged)
                                .on_submit(Message::SubmitConnect)
                                .secure(true)
                                .size(14)
                                .width(iced::Fill);

                            row![input].align_y(iced::Alignment::Center).padding(6)
                        } else {
                            let mut r = row![
                                column![ssid_text, info].spacing(2),
                                iced::widget::space::horizontal(),
                            ]
                            .align_y(iced::Alignment::Center)
                            .padding(6);

                            if network.is_connected {
                                r = r.push(button("Disconnect").on_press(Message::Disconnect));
                            } else {
                                r = r.push(
                                    button("Connect")
                                        .on_press(Message::Connect(network.ssid.clone())),
                                );
                            }

                            r
                        };

                        col.push(network_row)
                            .push(iced::widget::rule::horizontal(1))
                    });

                    let thin_scrollbar = scrollable::Scrollbar::new()
                        .width(6)
                        .scroller_width(6)
                        .spacing(0);

                    column![
                        header,
                        scrollable(list).direction(scrollable::Direction::Vertical(thin_scrollbar)),
                    ]
                    .spacing(15)
                    .into()
                }
            }
            App::Error {
                message, devices, ..
            } => {
                let mut col = column![text("Error").size(22), text(message).size(14),].spacing(10);

                if devices.is_some() {
                    col = col.push(button("Back").on_press(Message::Back));
                }

                col.into()
            }
        };

        container(content).padding(20).width(iced::Fill).into()
    }
}
