#![forbid(unsafe_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
};

use clap::Parser;
use directories::BaseDirs;
use iced::{
    Element, Event, Length, Subscription, Task, event,
    keyboard::{Key, key::Named},
    widget::{button, column, container, row, rule, scrollable, text, text_input},
    window,
};
use icepod_core::{
    Command, CommandResult, DeviceInfo, RecorderState, Request, Response, SessionManifest,
    StateSnapshot, ipc,
};
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "icepod")]
struct Args {
    #[arg(long)]
    simulate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Setup,
    Recording,
    Takes,
}

struct App {
    view: View,
    snapshot: Option<StateSnapshot>,
    sessions: Vec<SessionManifest>,
    devices: Vec<DeviceInfo>,
    session_name: String,
    channels: String,
    marker_note: String,
    error: Option<String>,
    stop_armed: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            view: View::Setup,
            snapshot: None,
            sessions: Vec::new(),
            devices: Vec::new(),
            session_name: "Icepod recording".into(),
            channels: "host,guest".into(),
            marker_note: String::new(),
            error: None,
            stop_armed: false,
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    SessionName(String),
    Channels(String),
    MarkerNote(String),
    Send(Command),
    Received(Result<CommandResult, String>),
    Navigate(View),
    StopIntent,
    StopConfirmed,
    Shortcut(Shortcut),
}

#[derive(Debug, Clone, Copy)]
enum Shortcut {
    Start,
    PauseResume,
    Marker,
    Stop,
}

fn boot() -> (App, Task<Message>) {
    (App::default(), send(Command::Status))
}

fn update(app: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::SessionName(value) => app.session_name = value,
        Message::Channels(value) => app.channels = value,
        Message::MarkerNote(value) => app.marker_note = value,
        Message::Navigate(view) => {
            app.view = view;
            if view == View::Takes {
                return send(Command::List);
            }
        }
        Message::Send(command) => return send(command),
        Message::Received(Ok(result)) => {
            app.error = None;
            match result {
                CommandResult::Snapshot(snapshot) | CommandResult::Accepted(snapshot) => {
                    if matches!(
                        snapshot.state,
                        RecorderState::Recording | RecorderState::Paused
                    ) {
                        app.view = View::Recording;
                    }
                    app.snapshot = Some(snapshot);
                }
                CommandResult::Devices(devices) => app.devices = devices,
                CommandResult::Sessions(sessions) => app.sessions = sessions,
                CommandResult::Session(session) => app.sessions = vec![session],
            }
        }
        Message::Received(Err(error)) => app.error = Some(error),
        Message::StopIntent => app.stop_armed = true,
        Message::StopConfirmed => {
            app.stop_armed = false;
            return send(Command::Stop);
        }
        Message::Shortcut(shortcut) => match shortcut {
            Shortcut::Start => {
                if app
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.state == RecorderState::Armed)
                {
                    return send(Command::Start);
                }
            }
            Shortcut::PauseResume => {
                if let Some(snapshot) = &app.snapshot {
                    return match snapshot.state {
                        RecorderState::Recording => send(Command::Pause),
                        RecorderState::Paused => send(Command::Resume),
                        _ => Task::none(),
                    };
                }
            }
            Shortcut::Marker => {
                if app
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.state == RecorderState::Recording)
                {
                    let note = std::mem::take(&mut app.marker_note);
                    return send(Command::Mark { note });
                }
            }
            Shortcut::Stop => app.stop_armed = true,
        },
    }
    Task::none()
}

fn view(app: &App) -> Element<'_, Message> {
    let navigation = row![
        button("Setup").on_press(Message::Navigate(View::Setup)),
        button("Recording").on_press(Message::Navigate(View::Recording)),
        button("Takes").on_press(Message::Navigate(View::Takes)),
        button("Reconnect").on_press(Message::Send(Command::Status)),
    ]
    .spacing(8);
    let status = app.snapshot.as_ref().map_or_else(
        || "Service disconnected".into(),
        |snapshot| format!("Service connected | {:?}", snapshot.state),
    );
    let body = match app.view {
        View::Setup => setup_view(app),
        View::Recording => recording_view(app),
        View::Takes => takes_view(app),
    };
    let mut content = column![navigation, text(status).size(16), rule::horizontal(1), body]
        .spacing(14)
        .padding(20);
    if let Some(error) = &app.error {
        content = content.push(text(format!("FAULT: {error}")));
    }
    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn setup_view(app: &App) -> Element<'_, Message> {
    let labels: Vec<_> = app
        .channels
        .split(',')
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_owned)
        .collect();
    column![
        text("SETUP").size(30),
        text("Deterministic simulator | 48 kHz | 32-bit float | backend-default buffer"),
        button("Refresh input devices").on_press(Message::Send(Command::Devices)),
        text(format!(
            "Supported input configurations: {}",
            app.devices.len()
        )),
        text_input("Session name", &app.session_name).on_input(Message::SessionName),
        text_input("Armed channel labels, comma separated", &app.channels)
            .on_input(Message::Channels),
        text(format!("Armed channels: {}", labels.len())),
        text("Input-level test and software monitoring require a hardware-enabled service build."),
        row![
            button("Configure and arm").on_press_maybe((!labels.is_empty()).then(
                || Message::Send(Command::Configure {
                    session_name: app.session_name.clone(),
                    sample_rate: 48_000,
                    channel_labels: labels,
                })
            )),
            button("Record").on_press_maybe(
                app.snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.state == RecorderState::Armed)
                    .then_some(Message::Send(Command::Start))
            ),
        ]
        .spacing(8),
        text("Keyboard: R start, Space pause/resume, M marker, S arm stop confirmation"),
    ]
    .spacing(12)
    .into()
}

fn recording_view(app: &App) -> Element<'_, Message> {
    let Some(snapshot) = &app.snapshot else {
        return column![
            text("RECORDING"),
            text("Reconnect to retrieve authoritative state.")
        ]
        .into();
    };
    let elapsed = snapshot.frames_written / 48_000;
    let transport = match snapshot.state {
        RecorderState::Recording => button("Pause").on_press(Message::Send(Command::Pause)),
        RecorderState::Paused => button("Resume").on_press(Message::Send(Command::Resume)),
        _ => button("Pause"),
    };
    let stop = if app.stop_armed {
        button("Confirm stop and finalize").on_press(Message::StopConfirmed)
    } else {
        button("Stop...").on_press(Message::StopIntent)
    };
    column![
        text(format!(
            "{:02}:{:02}:{:02}",
            elapsed / 3600,
            elapsed / 60 % 60,
            elapsed % 60
        ))
        .size(48),
        text(format!("STATE: {:?}", snapshot.state)),
        row![transport, stop].spacing(8),
        text(format!(
            "Channels: {} | live meter updates: service pending",
            snapshot.active_channels
        )),
        text(format!(
            "Frames captured: {} | durable: {}",
            snapshot.frames_written, snapshot.frames_committed
        )),
        text(format!(
            "Markers: {} | incidents/xruns/discontinuities: {}",
            snapshot.markers, snapshot.incidents
        )),
        text(format!(
            "Available disk: {} bytes",
            snapshot
                .available_bytes
                .map_or_else(|| "unknown".into(), |value| value.to_string())
        )),
        text_input("Marker note", &app.marker_note).on_input(Message::MarkerNote),
        button("Add marker").on_press(Message::Send(Command::Mark {
            note: app.marker_note.clone()
        })),
        text("Monitoring is disabled by default and never backpressures recording."),
    ]
    .spacing(12)
    .into()
}

fn takes_view(app: &App) -> Element<'_, Message> {
    let mut takes = column![
        text("TAKE BROWSER").size(30),
        button("Refresh").on_press(Message::Send(Command::List))
    ]
    .spacing(10);
    for session in &app.sessions {
        takes = takes.push(
            column![
                text(format!("{} | {}", session.name, session.session_id)),
                text(format!(
                    "{:?} | {} Hz | {} channels | {} frames",
                    session.status,
                    session.sample_rate,
                    session.channels.len(),
                    session.frames_committed
                )),
                text(format!(
                    "Incidents: {} | markers: {}",
                    session.incidents.len(),
                    session.markers.len()
                )),
            ]
            .spacing(3),
        );
    }
    scrollable(takes).into()
}

fn subscription(_app: &App) -> Subscription<Message> {
    event::listen_with(shortcut_event)
}

fn shortcut_event(event: Event, status: event::Status, _window: window::Id) -> Option<Message> {
    if status == event::Status::Captured {
        return None;
    }
    match event {
        Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. })
            if !modifiers.command() =>
        {
            match key.as_ref() {
                Key::Character("r") => Some(Message::Shortcut(Shortcut::Start)),
                Key::Character("m") => Some(Message::Shortcut(Shortcut::Marker)),
                Key::Character("s") => Some(Message::Shortcut(Shortcut::Stop)),
                Key::Named(Named::Space) => Some(Message::Shortcut(Shortcut::PauseResume)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn send(command: Command) -> Task<Message> {
    Task::perform(send_command(command), Message::Received)
}

async fn send_command(command: Command) -> Result<CommandResult, String> {
    let runtime_dir = default_runtime_dir();
    let token = fs::read_to_string(runtime_dir.join(ipc::TOKEN_BASENAME))
        .map_err(|error| format!("service token unavailable: {error}"))?;
    let request = Request {
        request_id: Uuid::new_v4(),
        token: token.trim().into(),
        command,
    };
    let response: Response = ipc::round_trip(&runtime_dir, &request)
        .await
        .map_err(|error| error.to_string())?;
    response.result.map_err(|error| error.to_string())
}

fn default_runtime_dir() -> PathBuf {
    BaseDirs::new()
        .and_then(|dirs| dirs.runtime_dir().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
        .join("icepod")
}

fn spawn_simulated_service() {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let Some(directory) = executable.parent() else {
        return;
    };
    let service = directory.join(if cfg!(windows) {
        "icepod-audio-service.exe"
    } else {
        "icepod-audio-service"
    });
    let _ = ProcessCommand::new(service)
        .arg("--simulate")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn main() -> iced::Result {
    if Args::parse().simulate {
        spawn_simulated_service();
    }
    iced::application(boot, update, view)
        .title("Icepod")
        .subscription(subscription)
        .run()
}
