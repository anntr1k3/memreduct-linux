use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use crate::cleaner::{CleanResult, CleanTarget};
use crate::error::CoreError;
use crate::metrics::MemoryMetrics;

pub const DEFAULT_SYSTEM_SOCKET: &str = "/run/memreduct/memreduct.sock";
pub const FALLBACK_SOCKET: &str = "/tmp/memreduct-daemon.sock";

pub fn resolve_socket_path() -> PathBuf {
    if Path::new(DEFAULT_SYSTEM_SOCKET).exists() {
        PathBuf::from(DEFAULT_SYSTEM_SOCKET)
    } else {
        PathBuf::from(FALLBACK_SOCKET)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum IpcRequest {
    Ping,
    GetMetrics,
    Clean(CleanTarget),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum IpcResponse {
    Pong,
    Metrics(MemoryMetrics),
    CleanResult(CleanResult),
    Error(String),
}

pub fn send_ipc_request<P: AsRef<Path>>(
    socket_path: P,
    request: &IpcRequest,
) -> Result<IpcResponse, CoreError> {
    let mut stream = UnixStream::connect(socket_path.as_ref())
        .map_err(|e| CoreError::Ipc(format!("Failed to connect to daemon at {:?}: {}", socket_path.as_ref(), e)))?;

    let mut payload = serde_json::to_vec(request)?;
    payload.push(b'\n');

    stream.write_all(&payload)?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut response_line = String::new();
    reader.read_line(&mut response_line)?;

    if response_line.trim().is_empty() {
        return Err(CoreError::Ipc("Empty response from daemon".into()));
    }

    let response: IpcResponse = serde_json::from_str(&response_line)?;
    Ok(response)
}
