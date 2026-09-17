use std::{fs, path::Path, process::Stdio, time::Duration};

use icepod_core::{Command, CommandResult, FaultKind, Request, Response, SessionManifest, ipc};
use tempfile::TempDir;
use tokio::{
    process::{Child, Command as ProcessCommand},
    time::sleep,
};
use uuid::Uuid;

async fn spawn_service(runtime: &Path, sessions: &Path) -> Child {
    let child = ProcessCommand::new(env!("CARGO_BIN_EXE_icepod-audio-service"))
        .arg("--simulate")
        .arg("--runtime-dir")
        .arg(runtime)
        .arg("--sessions-dir")
        .arg(sessions)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..200 {
        if runtime.join(ipc::TOKEN_BASENAME).is_file() {
            return child;
        }
        sleep(Duration::from_millis(10)).await;
    }
    panic!("service did not create its token");
}

async fn request(runtime: &Path, command: Command, request_id: Uuid) -> CommandResult {
    let token = fs::read_to_string(runtime.join(ipc::TOKEN_BASENAME)).unwrap();
    let response: Response = ipc::round_trip(
        runtime,
        &Request {
            request_id,
            token,
            command,
        },
    )
    .await
    .unwrap();
    response.result.unwrap()
}

async fn create_recording(runtime: &Path) {
    request(
        runtime,
        Command::Configure {
            session_name: "integration".into(),
            sample_rate: 48_000,
            channel_labels: vec!["host".into(), "guest".into()],
        },
        Uuid::new_v4(),
    )
    .await;
    request(runtime, Command::Start, Uuid::new_v4()).await;
    request(runtime, Command::Pause, Uuid::new_v4()).await;
    request(runtime, Command::Resume, Uuid::new_v4()).await;
    request(
        runtime,
        Command::Simulate {
            frames: 1025,
            callback_frames: 127,
        },
        Uuid::new_v4(),
    )
    .await;
}

#[tokio::test]
async fn service_survives_clients_and_finalizes_exact_bwf_channels() {
    let runtime = TempDir::new().unwrap();
    let sessions = TempDir::new().unwrap();
    let mut child = spawn_service(runtime.path(), sessions.path()).await;
    create_recording(runtime.path()).await;

    let marker_id = Uuid::new_v4();
    request(
        runtime.path(),
        Command::Mark {
            note: "chapter".into(),
        },
        marker_id,
    )
    .await;
    request(
        runtime.path(),
        Command::Mark {
            note: "chapter".into(),
        },
        marker_id,
    )
    .await;
    request(
        runtime.path(),
        Command::InjectFault {
            fault: FaultKind::BackendXrun,
        },
        Uuid::new_v4(),
    )
    .await;
    request(
        runtime.path(),
        Command::InjectFault {
            fault: FaultKind::QueueOverflow {
                missing_frames: None,
            },
        },
        Uuid::new_v4(),
    )
    .await;
    request(
        runtime.path(),
        Command::InjectFault {
            fault: FaultKind::MonitorDeviceLost,
        },
        Uuid::new_v4(),
    )
    .await;
    let CommandResult::Snapshot(snapshot) =
        request(runtime.path(), Command::Status, Uuid::new_v4()).await
    else {
        panic!("status did not return snapshot");
    };
    assert_eq!(snapshot.frames_committed, 1025);
    assert_eq!(snapshot.markers, 1);
    assert_eq!(snapshot.incidents, 3);

    request(runtime.path(), Command::Stop, Uuid::new_v4()).await;
    let CommandResult::Sessions(manifests) =
        request(runtime.path(), Command::List, Uuid::new_v4()).await
    else {
        panic!("list did not return sessions");
    };
    assert_eq!(manifests.len(), 1);
    let manifest = &manifests[0];
    assert_eq!(manifest.frames_committed, 1025);
    let directory = find_directory(sessions.path(), manifest.session_id);
    for channel in &manifest.channels {
        let path = directory
            .join("audio")
            .join(channel.final_file.as_ref().unwrap());
        let mut reader = bwavfile::WaveReader::open(path).unwrap();
        reader.validate_broadcast_wave().unwrap();
        assert_eq!(reader.frame_length().unwrap(), 1025);
    }
    child.kill().await.unwrap();
}

#[tokio::test]
async fn hard_kill_recovery_preserves_original_parts() {
    let runtime = TempDir::new().unwrap();
    let sessions = TempDir::new().unwrap();
    let mut child = spawn_service(runtime.path(), sessions.path()).await;
    create_recording(runtime.path()).await;
    let CommandResult::Snapshot(snapshot) =
        request(runtime.path(), Command::Status, Uuid::new_v4()).await
    else {
        panic!("status did not return snapshot");
    };
    child.kill().await.unwrap();
    child.wait().await.unwrap();

    fs::remove_file(runtime.path().join(ipc::TOKEN_BASENAME)).unwrap();
    let mut replacement = spawn_service(runtime.path(), sessions.path()).await;
    request(
        runtime.path(),
        Command::Recover {
            session: snapshot.session_id,
            all: false,
        },
        Uuid::new_v4(),
    )
    .await;
    let directory = find_directory(sessions.path(), snapshot.session_id.unwrap());
    let manifest = SessionManifest::load(&directory.join("session.json")).unwrap();
    assert_eq!(manifest.frames_committed, 1025);
    assert!(directory.join("audio").read_dir().unwrap().all(|entry| {
        entry
            .unwrap()
            .path()
            .extension()
            .is_some_and(|extension| extension == "part")
    }));
    for entry in directory.join("recovery").read_dir().unwrap() {
        let path = entry.unwrap().path();
        let mut reader = bwavfile::WaveReader::open(path).unwrap();
        reader.validate_broadcast_wave().unwrap();
        assert_eq!(reader.frame_length().unwrap(), 1025);
    }
    replacement.kill().await.unwrap();
}

fn find_directory(root: &Path, session_id: Uuid) -> std::path::PathBuf {
    root.read_dir()
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|directory| {
            SessionManifest::load(&directory.join("session.json"))
                .is_ok_and(|manifest| manifest.session_id == session_id)
        })
        .unwrap()
}
