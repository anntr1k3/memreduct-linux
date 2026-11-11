use thiserror::Error;

#[derive(Error, Debug)]
pub enum CoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to parse meminfo field '{0}': {1}")]
    ParseField(&'static str, String),

    #[error("Missing required field in meminfo: '{0}'")]
    MissingField(&'static str),

    #[error("Permission denied: {0}. Root or CAP_SYS_ADMIN required.")]
    PermissionDenied(String),

    #[error("Swap clearing is unsafe: available memory ({available_human}) is less than swap used ({swap_human}) + headroom")]
    UnsafeSwapClear {
        available_human: String,
        swap_human: String,
    },

    #[error("IPC communication error: {0}")]
    Ipc(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
