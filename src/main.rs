mod nm;

use iced::widget::{button, center, column, container, row, scrollable, text};
use iced::{Element, Task, Theme};

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title("netman")
        .theme(Theme::Dark)
        .window_size((400.0, 500.0))
        .run()
}

enum App {
    Loading,
    Loaded(Vec<nm::Network>),
    Error(String),
}

#[derive(Debug, Clone)]
enum Message {
    NetworksLoaded(Result<Vec<nm::Network>, String>),
    Refresh,
    Disconnect,
    Disconnected(Result<(), String>),
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
                    Ok(networks) => App::Loaded(networks),
                    Err(e) => App::Error(e),
                };
                Task::none()
            }
            Message::Refresh => {
                *self = App::Loading;
                Task::perform(nm::scan_networks(), Message::NetworksLoaded)
            }
            Message::Disconnect => Task::perform(nm::disconnect(), Message::Disconnected),
            Message::Disconnected(result) => {
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
            App::Loaded(networks) => {
                let header = row![
                    text("WiFi Networks").size(22),
                    iced::widget::space::horizontal(),
                    button("Refresh").on_press(Message::Refresh),
                ]
                .align_y(iced::Alignment::Center)
                .spacing(10);

                if networks.is_empty() {
                    column![header, text("No networks found.").size(16)]
                        .spacing(15)
                        .into()
                } else {
                    let list = networks.iter().fold(column![].spacing(4), |col, network| {
                        let ssid_text = text(&network.ssid).size(16);

                        let info =
                            text(format!("{}%  {}", network.strength, network.security)).size(13);

                        let mut network_row = row![
                            column![ssid_text, info].spacing(2),
                            iced::widget::space::horizontal(),
                        ]
                        .align_y(iced::Alignment::Center)
                        .padding(6);

                        if network.is_connected {
                            network_row = network_row
                                .push(button("Disconnect").on_press(Message::Disconnect));
                        }

                        col.push(network_row)
                            .push(iced::widget::rule::horizontal(1))
                    });

                    column![header, scrollable(list)].spacing(15).into()
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

        center(container(content).padding(20).max_width(380)).into()
    }
}
