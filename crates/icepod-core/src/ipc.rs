use std::{
    io,
    path::{Path, PathBuf},
};

use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced,
    tokio::{Stream, prelude::*},
};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::protocol::{
    MAX_FRAME_LENGTH, PROTOCOL_MAGIC, PROTOCOL_MAJOR, PROTOCOL_MINOR, decode_payload, encode_frame,
};

pub const SOCKET_BASENAME: &str = "service.sock";
pub const TOKEN_BASENAME: &str = "service.token";

pub fn socket_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(SOCKET_BASENAME)
}

pub async fn connect(runtime_dir: &Path) -> io::Result<Stream> {
    if GenericFilePath::is_supported() {
        let path = socket_path(runtime_dir);
        Stream::connect(path.as_path().to_fs_name::<GenericFilePath>()?).await
    } else {
        Stream::connect("icepod-service".to_ns_name::<GenericNamespaced>()?).await
    }
}

pub async fn write_async<T: Serialize>(stream: &mut &Stream, value: &T) -> io::Result<()> {
    stream.write_all(&encode_frame(value)?).await
}

pub async fn read_async<T: DeserializeOwned>(stream: &mut &Stream) -> io::Result<T> {
    let mut header = [0; 16];
    stream.read_exact(&mut header).await?;
    if header[..8] != PROTOCOL_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid IPC magic",
        ));
    }
    let major = u16::from_le_bytes([header[8], header[9]]);
    let minor = u16::from_le_bytes([header[10], header[11]]);
    if major != PROTOCOL_MAJOR || minor > PROTOCOL_MINOR {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "incompatible IPC version",
        ));
    }
    let length = u32::from_le_bytes(header[12..16].try_into().unwrap_or_default()) as usize;
    if length > MAX_FRAME_LENGTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "IPC frame too large",
        ));
    }
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).await?;
    decode_payload(&payload)
}

pub async fn round_trip<Req: Serialize, Resp: DeserializeOwned>(
    runtime_dir: &Path,
    request: &Req,
) -> io::Result<Resp> {
    let stream = connect(runtime_dir).await?;
    let mut sender = &stream;
    write_async(&mut sender, request).await?;
    let mut receiver = &stream;
    read_async(&mut receiver).await
}
