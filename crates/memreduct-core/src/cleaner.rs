use nix::unistd::Uid;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::process::Command;
use tracing::{info, warn};

use crate::error::CoreError;
use crate::metrics::{format_bytes, MemoryMetrics};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanTarget {
    PageCache,
    DentriesInodes,
    AllCaches,
    CompactMemory,
    Swap,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanResult {
    pub freed_bytes: i64,
    pub before: MemoryMetrics,
    pub after: MemoryMetrics,
    pub actions_performed: Vec<String>,
    pub warnings: Vec<String>,
}

pub struct Cleaner;

impl Cleaner {
    pub fn is_privileged() -> bool {
        Uid::current().is_root()
    }

    pub fn clean(target: CleanTarget) -> Result<CleanResult, CoreError> {
        if !Self::is_privileged() {
            return Err(CoreError::PermissionDenied(
                "Cleaning operations require root privileges or CAP_SYS_ADMIN".into(),
            ));
        }

        let before = MemoryMetrics::fetch()?;
        let mut actions = Vec::new();
        let mut warnings = Vec::new();

        match target {
            CleanTarget::PageCache => {
                Self::sync_disks();
                Self::drop_caches(1)?;
                actions.push("Flushed dirty pages and dropped Page Cache (1)".into());
            }
            CleanTarget::DentriesInodes => {
                Self::sync_disks();
                Self::drop_caches(2)?;
                actions.push("Flushed dirty pages and dropped Dentries & Inodes (2)".into());
            }
            CleanTarget::AllCaches => {
                Self::sync_disks();
                Self::drop_caches(3)?;
                actions.push("Flushed dirty pages and dropped All Caches (3)".into());
            }
            CleanTarget::CompactMemory => {
                Self::compact_memory()?;
                actions.push("Compacted physical memory (HugePages defragmentation)".into());
            }
            CleanTarget::Swap => {
                match Self::clear_swap(&before) {
                    Ok(action) => actions.push(action),
                    Err(warn_msg) => warnings.push(warn_msg),
                }
            }
            CleanTarget::All => {
                Self::sync_disks();
                Self::drop_caches(3)?;
                actions.push("Flushed dirty pages and dropped All Caches (3)".into());

                if let Err(e) = Self::compact_memory() {
                    warnings.push(format!("Memory compaction skipped or failed: {}", e));
                } else {
                    actions.push("Compacted physical memory".into());
                }

                // Check swap
                let mid = MemoryMetrics::fetch().unwrap_or_else(|_| before.clone());
                match Self::clear_swap(&mid) {
                    Ok(action) => actions.push(action),
                    Err(warn_msg) => warnings.push(warn_msg),
                }
            }
        }

        let after = MemoryMetrics::fetch()?;
        // Positive freed_bytes means memory was freed (used_bytes decreased)
        let freed_bytes = before.used_bytes as i64 - after.used_bytes as i64;

        info!(
            "Memory clean completed for {:?}. Freed: {} (before: {}, after: {})",
            target,
            if freed_bytes >= 0 {
                format_bytes(freed_bytes as u64)
            } else {
                format!("-{}", format_bytes((-freed_bytes) as u64))
            },
            format_bytes(before.used_bytes),
            format_bytes(after.used_bytes)
        );

        Ok(CleanResult {
            freed_bytes,
            before,
            after,
            actions_performed: actions,
            warnings,
        })
    }

    fn sync_disks() {
        unsafe {
            libc::sync();
        }
    }

    fn drop_caches(level: u8) -> Result<(), CoreError> {
        let mut file = OpenOptions::new()
            .write(true)
            .open("/proc/sys/vm/drop_caches")
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    CoreError::PermissionDenied("Cannot write to /proc/sys/vm/drop_caches".into())
                } else {
                    CoreError::Io(e)
                }
            })?;

        let val = format!("{}\n", level);
        file.write_all(val.as_bytes())?;
        file.flush()?;
        Ok(())
    }

    fn compact_memory() -> Result<(), CoreError> {
        let mut file = OpenOptions::new()
            .write(true)
            .open("/proc/sys/vm/compact_memory")
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    CoreError::PermissionDenied("Cannot write to /proc/sys/vm/compact_memory".into())
                } else {
                    CoreError::Io(e)
                }
            })?;

        file.write_all(b"1\n")?;
        file.flush()?;
        Ok(())
    }

    fn clear_swap(current: &MemoryMetrics) -> Result<String, String> {
        if current.swap_used_bytes == 0 {
            return Ok("Swap is already empty, skipped swap reset".into());
        }

        // Safety check: Available RAM must be at least SwapUsed * 1.2
        let required_headroom = (current.swap_used_bytes as f64 * 1.2) as u64;
        if current.available_bytes < required_headroom {
            let msg = format!(
                "Swap reset skipped for safety: Available RAM ({}) is less than required headroom ({}) for {} of swap data",
                format_bytes(current.available_bytes),
                format_bytes(required_headroom),
                format_bytes(current.swap_used_bytes)
            );
            warn!("{}", msg);
            return Err(msg);
        }

        info!("Resetting swap (swapoff -a && swapon -a)...");
        let status_off = Command::new("swapoff")
            .arg("-a")
            .status()
            .map_err(|e| format!("Failed to execute swapoff: {}", e))?;

        if !status_off.success() {
            return Err(format!("swapoff -a failed with exit code {:?}", status_off.code()));
        }

        let status_on = Command::new("swapon")
            .arg("-a")
            .status()
            .map_err(|e| format!("Failed to execute swapon: {}", e))?;

        if !status_on.success() {
            return Err(format!("swapon -a failed with exit code {:?}", status_on.code()));
        }

        Ok("Safely flushed Swap into RAM and re-enabled Swap".into())
    }
}
