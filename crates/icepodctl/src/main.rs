#![forbid(unsafe_code)]

use std::{
    fs,
    path::{Path, PathBuf},
};

use clap::{Parser, Subcommand};
use directories::BaseDirs;
use icepod_core::{Command, CommandResult, FaultKind, Request, Response, ipc};
use miette::{IntoDiagnostic, Result, miette};
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "icepodctl")]
struct Cli {
    #[arg(long)]
    runtime_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand)]
enum CliCommand {
    Status,
    List,
    Devices,
    Configure {
        #[arg(long, default_value = "Icepod recording")]
        name: String,
        #[arg(long, default_value_t = 48_000)]
        sample_rate: u32,
        #[arg(long, value_delimiter = ',', default_value = "host,guest")]
        channels: Vec<String>,
    },
    Start,
    Pause,
    Resume,
    Mark {
        note: Option<String>,
    },
    Stop,
    Simulate {
        frames: u64,
        #[arg(long, default_value_t = 256)]
        callback_frames: usize,
    },
    Inject {
        #[arg(value_enum)]
        fault: Fault,
        #[arg(long)]
        missing_frames: Option<u64>,
    },
    Recover {
        session: Option<Uuid>,
        #[arg(long)]
        all: bool,
    },
    Inspect {
        session: Uuid,
    },
    Soak {
        #[arg(long, default_value_t = 4)]
        hours: u64,
        #[arg(long, default_value_t = 4)]
        channels: usize,
        #[arg(long)]
        realtime: bool,
        #[arg(long, default_value = "icepod-soak-report.json")]
        report: PathBuf,
    },
}

#[derive(Clone, clap::ValueEnum)]
enum Fault {
    Xrun,
    QueueOverflow,
    TimestampJump,
    InputLost,
    MonitorLost,
    DiskCritical,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let runtime_dir = cli.runtime_dir.unwrap_or_else(default_runtime_dir);
    let token = read_token(&runtime_dir)?;
    if let CliCommand::Soak {
        hours,
        channels,
        realtime,
        report,
    } = &cli.command
    {
        return run_soak(&runtime_dir, &token, *hours, *channels, *realtime, report).await;
    }
    let command = match cli.command {
        CliCommand::Status => Command::Status,
        CliCommand::List => Command::List,
        CliCommand::Devices => Command::Devices,
        CliCommand::Configure {
            name,
            sample_rate,
            channels,
        } => Command::Configure {
            session_name: name,
            sample_rate,
            channel_labels: channels,
        },
        CliCommand::Start => Command::Start,
        CliCommand::Pause => Command::Pause,
        CliCommand::Resume => Command::Resume,
        CliCommand::Mark { note } => Command::Mark {
            note: note.unwrap_or_default(),
        },
        CliCommand::Stop => Command::Stop,
        CliCommand::Simulate {
            frames,
            callback_frames,
        } => Command::Simulate {
            frames,
            callback_frames,
        },
        CliCommand::Inject {
            fault,
            missing_frames,
        } => Command::InjectFault {
            fault: match fault {
                Fault::Xrun => FaultKind::BackendXrun,
                Fault::QueueOverflow => FaultKind::QueueOverflow { missing_frames },
                Fault::TimestampJump => FaultKind::TimestampJump { missing_frames },
                Fault::InputLost => FaultKind::InputDeviceLost,
                Fault::MonitorLost => FaultKind::MonitorDeviceLost,
                Fault::DiskCritical => FaultKind::DiskCritical,
            },
        },
        CliCommand::Recover { session, all } => Command::Recover { session, all },
        CliCommand::Inspect { session } => Command::Inspect { session },
        CliCommand::Soak { .. } => unreachable!("soak returned before command mapping"),
    };
    let result = execute(&runtime_dir, &token, command).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).into_diagnostic()?
    );
    Ok(())
}

async fn execute(runtime_dir: &Path, token: &str, command: Command) -> Result<CommandResult> {
    let request = Request {
        request_id: Uuid::new_v4(),
        token: token.into(),
        command,
    };
    let response: Response = ipc::round_trip(runtime_dir, &request)
        .await
        .into_diagnostic()?;
    response.result.map_err(|error| miette!(error.to_string()))
}

async fn run_soak(
    runtime_dir: &Path,
    token: &str,
    hours: u64,
    channels: usize,
    realtime: bool,
    report_path: &Path,
) -> Result<()> {
    if hours == 0 || channels == 0 {
        return Err(miette!("soak hours and channels must be nonzero"));
    }
    let sample_rate = if realtime { 48_000 } else { 100 };
    let labels = (1..=channels)
        .map(|channel| format!("soak-{channel:02}"))
        .collect();
    execute(
        runtime_dir,
        token,
        Command::Configure {
            session_name: format!("{}-hour-{}-channel-soak", hours, channels),
            sample_rate,
            channel_labels: labels,
        },
    )
    .await?;
    execute(runtime_dir, token, Command::Start).await?;
    let seconds = hours.saturating_mul(3600);
    if realtime {
        for _ in 0..seconds {
            execute(
                runtime_dir,
                token,
                Command::Simulate {
                    frames: u64::from(sample_rate),
                    callback_frames: 1024,
                },
            )
            .await?;
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    } else {
        execute(
            runtime_dir,
            token,
            Command::Simulate {
                frames: seconds.saturating_mul(u64::from(sample_rate)),
                callback_frames: 4096,
            },
        )
        .await?;
    }
    execute(runtime_dir, token, Command::Stop).await?;
    let CommandResult::Sessions(sessions) = execute(runtime_dir, token, Command::List).await?
    else {
        return Err(miette!("service did not return soak session list"));
    };
    let session = sessions
        .first()
        .ok_or_else(|| miette!("soak session missing after finalization"))?;
    let report = serde_json::json!({
        "schemaVersion": 1,
        "mode": if realtime { "wall-clock" } else { "accelerated" },
        "requestedHours": hours,
        "channels": channels,
        "sampleRate": sample_rate,
        "totalFrames": session.frames_written,
        "committedFrames": session.frames_committed,
        "queueHighWaterBlocks": null,
        "writerLatencyMicros": null,
        "xruns": session.incidents.iter().filter(|incident| matches!(incident.kind, icepod_core::session::IncidentKind::BackendXrun)).count(),
        "discontinuities": session.incidents.iter().filter(|incident| matches!(incident.kind, icepod_core::session::IncidentKind::QueueOverflow | icepod_core::session::IncidentKind::TimestampDiscontinuity | icepod_core::session::IncidentKind::UnknownDiscontinuity)).count(),
        "memoryBytes": null,
        "filesValidated": session.channels.iter().all(|channel| channel.final_file.is_some()),
        "sessionId": session.session_id,
    });
    fs::write(
        report_path,
        serde_json::to_vec_pretty(&report).into_diagnostic()?,
    )
    .into_diagnostic()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).into_diagnostic()?
    );
    Ok(())
}

fn read_token(runtime_dir: &Path) -> Result<String> {
    fs::read_to_string(runtime_dir.join(ipc::TOKEN_BASENAME))
        .into_diagnostic()
        .map(|token| token.trim().to_owned())
        .map_err(|error| {
            error.wrap_err("Icepod service token unavailable; start icepod-audio-service")
        })
}

fn default_runtime_dir() -> PathBuf {
    BaseDirs::new()
        .and_then(|dirs| dirs.runtime_dir().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
        .join("icepod")
}
