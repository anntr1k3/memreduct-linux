use clap::Parser;
use memreduct_core::cleaner::{CleanTarget, Cleaner};
use memreduct_core::ipc::{IpcRequest, IpcResponse, DEFAULT_SYSTEM_SOCKET, FALLBACK_SOCKET};
use memreduct_core::metrics::MemoryMetrics;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal;
use tracing::{error, info, warn};

#[derive(Parser, Debug)]
#[command(name = "memreduct-daemon")]
#[command(about = "Privileged memory management daemon and helper for Mem Reduct Linux")]
struct Args {
    /// Socket path to bind to
    #[arg(short, long)]
    socket: Option<PathBuf>,

    /// Run as a one-shot helper to clean memory directly and exit
    #[arg(long)]
    clean: Option<String>,

    /// Print output in JSON when running one-shot
    #[arg(long)]
    json: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    // Check if one-shot clean mode is requested (e.g. called via pkexec)
    if let Some(target_str) = args.clean {
        let target = match target_str.to_lowercase().as_str() {
            "pagecache" | "page-cache" => CleanTarget::PageCache,
            "dentries" | "inodes" | "dentries-inodes" => CleanTarget::DentriesInodes,
            "caches" | "all-caches" => CleanTarget::AllCaches,
            "compact" | "compact-memory" => CleanTarget::CompactMemory,
            "swap" => CleanTarget::Swap,
            "all" => CleanTarget::All,
            other => {
                eprintln!("Unknown clean target '{}'. Valid options: pagecache, dentries-inodes, caches, compact, swap, all", other);
                std::process::exit(1);
            }
        };

        match Cleaner::clean(target) {
            Ok(res) => {
                if args.json {
                    println!("{}", serde_json::to_string_pretty(&res)?);
                } else {
                    println!("Memory cleaned successfully ({:?}):", target);
                    for action in &res.actions_performed {
                        println!(" - {}", action);
                    }
                    for warn in &res.warnings {
                        println!(" ! Warning: {}", warn);
                    }
                    let freed = res.freed_bytes;
                    if freed >= 0 {
                        println!("Freed: {}", memreduct_core::format_bytes(freed as u64));
                    } else {
                        println!("Freed: -{}", memreduct_core::format_bytes((-freed) as u64));
                    }
                }
                return Ok(());
            }
            Err(e) => {
                eprintln!("Cleaning failed: {}", e);
                std::process::exit(1);
            }
        }
    }

    // Otherwise run in daemon mode
    let socket_path = determine_socket_path(args.socket.as_deref());
    info!("Starting memreduct-daemon on {:?}", socket_path);

    if !Cleaner::is_privileged() {
        warn!("memreduct-daemon is not running as root. Cleanup operations will fail unless privileges are granted!");
    }

    // Ensure socket directory exists
    if let Some(parent) = socket_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }

    // Remove stale socket if exists
    if socket_path.exists() {
        let _ = fs::remove_file(&socket_path);
    }

    let listener = UnixListener::bind(&socket_path)?;

    // Set permissive socket permissions (0666) so local unprivileged clients can connect
    if let Ok(metadata) = fs::metadata(&socket_path) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o666);
        let _ = fs::set_permissions(&socket_path, permissions);
    }

    info!("Listening for IPC requests on {:?}", socket_path);

    let socket_cleanup = socket_path.clone();
    let shutdown_signal = async {
        let _ = signal::ctrl_c().await;
        info!("Received shutdown signal. Exiting...");
    };

    tokio::select! {
        _ = run_server(listener) => {},
        _ = shutdown_signal => {},
    }

    if socket_cleanup.exists() {
        let _ = fs::remove_file(socket_cleanup);
    }

    info!("memreduct-daemon stopped.");
    Ok(())
}

fn determine_socket_path(user_path: Option<&Path>) -> PathBuf {
    if let Some(p) = user_path {
        return p.to_path_buf();
    }

    let sys_path = Path::new(DEFAULT_SYSTEM_SOCKET);
    if let Some(parent) = sys_path.parent() {
        if fs::create_dir_all(parent).is_ok() {
            return sys_path.to_path_buf();
        }
    }

    PathBuf::from(FALLBACK_SOCKET)
}

async fn run_server(listener: UnixListener) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream).await {
                        warn!("Client error: {}", e);
                    }
                });
            }
            Err(e) => {
                error!("Accept error: {}", e);
                break;
            }
        }
    }
}

async fn handle_client(mut stream: UnixStream) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (reader, mut writer) = stream.split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    while buf_reader.read_line(&mut line).await? > 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }

        let request: Result<IpcRequest, _> = serde_json::from_str(trimmed);
        let response = match request {
            Ok(IpcRequest::Ping) => IpcResponse::Pong,
            Ok(IpcRequest::GetMetrics) => match MemoryMetrics::fetch() {
                Ok(metrics) => IpcResponse::Metrics(metrics),
                Err(e) => IpcResponse::Error(format!("Failed to read metrics: {}", e)),
            },
            Ok(IpcRequest::Clean(target)) => match Cleaner::clean(target) {
                Ok(result) => IpcResponse::CleanResult(result),
                Err(e) => IpcResponse::Error(format!("Cleaning failed: {}", e)),
            },
            Err(e) => IpcResponse::Error(format!("Invalid request JSON: {}", e)),
        };

        let mut res_bytes = serde_json::to_vec(&response)?;
        res_bytes.push(b'\n');
        writer.write_all(&res_bytes).await?;
        writer.flush().await?;

        line.clear();
    }

    Ok(())
}
