mod nm;

use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Element, Task, Theme, window};

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title("netman")
        .theme(Theme::Dark)
        .window(window::Settings {
            size: iced::Size::new(400.0, 500.0),
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
        networks: Vec<nm::Network>,
        connecting_ssid: Option<String>,
        password: String,
    },
    Connecting,
    Disconnecting,
    Error(String),
}

#[derive(Debug, Clone)]
enum Message {
    NetworksLoaded(Result<Vec<nm::Network>, String>),
    Refresh,
    Disconnect,
    Disconnected(Result<(), String>),
    Connect(String),
    PasswordChanged(String),
    SubmitConnect,
    CancelConnect,
    Connected(Result<(), String>),
}

impl App {
    fn new() -> (Self, Task<Message>) {
        (
            App::Loading,
            Task::perform(nm::scan_networks(), Message::NetworksLoaded),
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::NetworksLoaded(result) => {
                *self = match result {
                    Ok(networks) => App::Loaded {
                        networks,
                        connecting_ssid: None,
                        password: String::new(),
                    },
                    Err(e) => App::Error(e),
                };
                Task::none()
            }
            Message::Refresh => {
                *self = App::Loading;
                Task::perform(nm::scan_networks(), Message::NetworksLoaded)
            }
            Message::Disconnect => {
                *self = App::Disconnecting;
                Task::perform(nm::disconnect(), Message::Disconnected)
            }
            Message::Disconnected(result) => {
                if let Err(e) = result {
                    *self = App::Error(e);
                    return Task::none();
                }
                *self = App::Loading;
                Task::perform(nm::scan_networks(), Message::NetworksLoaded)
            }
            Message::Connect(ssid) => {
                if let App::Loaded {
                    networks,
                    connecting_ssid,
                    password,
                } = self
                {
                    // Open or saved networks: connect immediately (no password needed)
                    if let Some(net) = networks.iter().find(|n| n.ssid == ssid)
                        && (net.security == "Open" || net.is_saved)
                    {
                        let net = net.clone();
                        *self = App::Connecting;
                        return Task::perform(nm::connect(net, String::new()), Message::Connected);
                    }
                    *connecting_ssid = Some(ssid);
                    *password = String::new();
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
                    networks,
                    connecting_ssid: Some(ssid),
                    password,
                } = self
                    && let Some(net) = networks.iter().find(|n| n.ssid == *ssid)
                {
                    let net = net.clone();
                    let pw = password.clone();
                    *self = App::Connecting;
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
                    *connecting_ssid = None;
                    *password = String::new();
                }
                Task::none()
            }
            Message::Connected(result) => {
                if let Err(e) = result {
                    *self = App::Error(e);
                    return Task::none();
                }
                *self = App::Loading;
                Task::perform(nm::scan_networks(), Message::NetworksLoaded)
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let content: Element<Message> = match self {
            App::Loading => column![text("Scanning...").size(18)].into(),
            App::Connecting => column![text("Connecting...").size(18)].into(),
            App::Disconnecting => column![text("Disconnecting...").size(18)].into(),
            App::Loaded {
                networks,
                connecting_ssid,
                password,
            } => {
                let header = row![
                    text("WiFi Networks").size(22),
                    iced::widget::space::horizontal(),
                    button("Refresh").on_press(Message::Refresh),
                ]
                .align_y(iced::Alignment::Center)
                .spacing(10)
                .padding(6);

                if networks.is_empty() {
                    column![header, text("No networks found.").size(16)]
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
                                .on_input(Message::PasswordChanged)
                                .on_submit(Message::SubmitConnect)
                                .secure(true)
                                .size(14)
                                .width(150);

                            row![
                                column![ssid_text, info].spacing(2),
                                iced::widget::space::horizontal(),
                                input,
                                button("Go").on_press(Message::SubmitConnect),
                                button("X").on_press(Message::CancelConnect),
                            ]
                            .align_y(iced::Alignment::Center)
                            .spacing(4)
                            .padding(6)
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
            App::Error(e) => column![
                text("Error").size(22),
                text(e).size(14),
                button("Retry").on_press(Message::Refresh),
            ]
            .spacing(10)
            .into(),
        };

        container(content).padding(20).width(iced::Fill).into()
    }
}
