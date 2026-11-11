use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::error::CoreError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PsiWindow {
    pub avg10: f64,
    pub avg60: f64,
    pub avg300: f64,
    pub total_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PsiMetrics {
    pub some: PsiWindow,
    pub full: PsiWindow,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ZramMetrics {
    pub device: String,
    pub disk_size_bytes: u64,
    pub orig_data_bytes: u64,
    pub compr_data_bytes: u64,
    pub mem_used_bytes: u64,
}

impl ZramMetrics {
    pub fn compression_ratio(&self) -> f64 {
        if self.compr_data_bytes == 0 {
            1.0
        } else {
            self.orig_data_bytes as f64 / self.compr_data_bytes as f64
        }
    }

    pub fn space_saved_bytes(&self) -> u64 {
        self.orig_data_bytes.saturating_sub(self.compr_data_bytes)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryMetrics {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,

    pub buffers_bytes: u64,
    pub cached_bytes: u64,
    pub swap_cached_bytes: u64,
    pub active_bytes: u64,
    pub inactive_bytes: u64,
    pub shmem_bytes: u64,
    pub slab_bytes: u64,
    pub sreclaimable_bytes: u64,
    pub sunreclaim_bytes: u64,
    pub dirty_bytes: u64,
    pub writeback_bytes: u64,
    pub anon_pages_bytes: u64,
    pub mapped_bytes: u64,
    pub kreclaimable_bytes: u64,

    pub swap_total_bytes: u64,
    pub swap_free_bytes: u64,
    pub swap_used_bytes: u64,

    pub reclaimable_cache_bytes: u64,

    pub psi: Option<PsiMetrics>,
    pub zram: Vec<ZramMetrics>,
}

impl MemoryMetrics {
    pub fn used_percent(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.used_bytes as f64 / self.total_bytes as f64) * 100.0
        }
    }

    pub fn available_percent(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.available_bytes as f64 / self.total_bytes as f64) * 100.0
        }
    }

    pub fn cached_percent(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.cached_bytes as f64 / self.total_bytes as f64) * 100.0
        }
    }

    pub fn swap_used_percent(&self) -> f64 {
        if self.swap_total_bytes == 0 {
            0.0
        } else {
            (self.swap_used_bytes as f64 / self.swap_total_bytes as f64) * 100.0
        }
    }

    pub fn fetch() -> Result<Self, CoreError> {
        let meminfo_str = fs::read_to_string("/proc/meminfo")?;
        let mut metrics = Self::parse_meminfo(&meminfo_str)?;

        metrics.psi = parse_psi_file("/proc/pressure/memory").ok();
        metrics.zram = scan_zram_devices();

        Ok(metrics)
    }

    pub fn parse_meminfo(content: &str) -> Result<Self, CoreError> {
        let mut raw: HashMap<&str, u64> = HashMap::new();

        for line in content.lines() {
            let mut parts = line.splitn(2, ':');
            if let (Some(key), Some(rest)) = (parts.next(), parts.next()) {
                let key = key.trim();
                let num_str = rest.trim_start().split_whitespace().next().unwrap_or("0");
                if let Ok(val) = num_str.parse::<u64>() {
                    // /proc/meminfo gives numbers in kB unless specified otherwise
                    raw.insert(key, val * 1024);
                }
            }
        }

        let total_bytes = *raw.get("MemTotal").ok_or(CoreError::MissingField("MemTotal"))?;
        let free_bytes = *raw.get("MemFree").unwrap_or(&0);
        
        // MemAvailable was added in Linux 3.14. Fallback calculation if missing:
        let available_bytes = match raw.get("MemAvailable") {
            Some(&avail) => avail,
            None => {
                let buffers = *raw.get("Buffers").unwrap_or(&0);
                let cached = *raw.get("Cached").unwrap_or(&0);
                free_bytes + buffers + cached
            }
        };

        let used_bytes = total_bytes.saturating_sub(available_bytes);

        let buffers_bytes = *raw.get("Buffers").unwrap_or(&0);
        let cached_bytes = *raw.get("Cached").unwrap_or(&0);
        let swap_cached_bytes = *raw.get("SwapCached").unwrap_or(&0);
        let active_bytes = *raw.get("Active").unwrap_or(&0);
        let inactive_bytes = *raw.get("Inactive").unwrap_or(&0);
        let shmem_bytes = *raw.get("Shmem").unwrap_or(&0);
        let slab_bytes = *raw.get("Slab").unwrap_or(&0);
        let sreclaimable_bytes = *raw.get("SReclaimable").unwrap_or(&0);
        let sunreclaim_bytes = *raw.get("SUnreclaim").unwrap_or(&0);
        let dirty_bytes = *raw.get("Dirty").unwrap_or(&0);
        let writeback_bytes = *raw.get("Writeback").unwrap_or(&0);
        let anon_pages_bytes = *raw.get("AnonPages").unwrap_or(&0);
        let mapped_bytes = *raw.get("Mapped").unwrap_or(&0);
        let kreclaimable_bytes = *raw.get("KReclaimable").unwrap_or(&sreclaimable_bytes);

        let swap_total_bytes = *raw.get("SwapTotal").unwrap_or(&0);
        let swap_free_bytes = *raw.get("SwapFree").unwrap_or(&0);
        let swap_used_bytes = swap_total_bytes.saturating_sub(swap_free_bytes);

        // Reclaimable cache = Buffers + Cached (minus Shmem) + SReclaimable
        let file_cache = cached_bytes.saturating_sub(shmem_bytes);
        let reclaimable_cache_bytes = buffers_bytes + file_cache + sreclaimable_bytes;

        Ok(MemoryMetrics {
            total_bytes,
            free_bytes,
            available_bytes,
            used_bytes,
            buffers_bytes,
            cached_bytes,
            swap_cached_bytes,
            active_bytes,
            inactive_bytes,
            shmem_bytes,
            slab_bytes,
            sreclaimable_bytes,
            sunreclaim_bytes,
            dirty_bytes,
            writeback_bytes,
            anon_pages_bytes,
            mapped_bytes,
            kreclaimable_bytes,
            swap_total_bytes,
            swap_free_bytes,
            swap_used_bytes,
            reclaimable_cache_bytes,
            psi: None,
            zram: Vec::new(),
        })
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    const TIB: f64 = GIB * 1024.0;

    let b = bytes as f64;
    if b >= TIB {
        format!("{:.2} TiB", b / TIB)
    } else if b >= GIB {
        format!("{:.2} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.1} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{} B", bytes)
    }
}

pub fn parse_psi_file<P: AsRef<Path>>(path: P) -> Result<PsiMetrics, CoreError> {
    let content = fs::read_to_string(path)?;
    parse_psi_str(&content)
}

pub fn parse_psi_str(content: &str) -> Result<PsiMetrics, CoreError> {
    let mut some = None;
    let mut full = None;

    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        let prefix = parts[0];
        let mut avg10 = 0.0;
        let mut avg60 = 0.0;
        let mut avg300 = 0.0;
        let mut total_us = 0;

        for part in &parts[1..] {
            let mut kv = part.split('=');
            if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
                match k {
                    "avg10" => avg10 = v.parse::<f64>().unwrap_or(0.0),
                    "avg60" => avg60 = v.parse::<f64>().unwrap_or(0.0),
                    "avg300" => avg300 = v.parse::<f64>().unwrap_or(0.0),
                    "total" => total_us = v.parse::<u64>().unwrap_or(0),
                    _ => {}
                }
            }
        }

        let window = PsiWindow {
            avg10,
            avg60,
            avg300,
            total_us,
        };

        if prefix == "some" {
            some = Some(window);
        } else if prefix == "full" {
            full = Some(window);
        }
    }

    Ok(PsiMetrics {
        some: some.unwrap_or(PsiWindow {
            avg10: 0.0,
            avg60: 0.0,
            avg300: 0.0,
            total_us: 0,
        }),
        full: full.unwrap_or(PsiWindow {
            avg10: 0.0,
            avg60: 0.0,
            avg300: 0.0,
            total_us: 0,
        }),
    })
}

pub fn scan_zram_devices() -> Vec<ZramMetrics> {
    let mut devices = Vec::new();
    let sys_block = Path::new("/sys/block");
    if !sys_block.exists() {
        return devices;
    }

    if let Ok(entries) = fs::read_dir(sys_block) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("zram") {
                let dev_dir = entry.path();
                let disk_size_bytes = read_sysfs_u64(&dev_dir.join("disksize")).unwrap_or(0);
                let orig_data_bytes = read_sysfs_u64(&dev_dir.join("orig_data_size")).unwrap_or(0);
                let compr_data_bytes = read_sysfs_u64(&dev_dir.join("compr_data_size")).unwrap_or(0);
                let mem_used_bytes = read_sysfs_u64(&dev_dir.join("mem_used_total")).unwrap_or(0);

                if disk_size_bytes > 0 {
                    devices.push(ZramMetrics {
                        device: name,
                        disk_size_bytes,
                        orig_data_bytes,
                        compr_data_bytes,
                        mem_used_bytes,
                    });
                }
            }
        }
    }

    devices
}

fn read_sysfs_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_MEMINFO: &str = r#"
MemTotal:       16237200 kB
MemFree:         1298600 kB
MemAvailable:   11518164 kB
Buffers:          360228 kB
Cached:         10844844 kB
SwapCached:        84412 kB
Active:          3882072 kB
Inactive:        9548764 kB
Active(anon):    2477348 kB
Inactive(anon):   866996 kB
Active(file):    1404724 kB
Inactive(file):  8681768 kB
Unevictable:          48 kB
Mlocked:              48 kB
SwapTotal:       4194300 kB
SwapFree:        4051908 kB
Zswap:                 0 kB
Zswapped:              0 kB
Dirty:             13340 kB
Writeback:             0 kB
AnonPages:       2212816 kB
Mapped:          1201128 kB
Shmem:           1118772 kB
KReclaimable:     476936 kB
Slab:             675308 kB
SReclaimable:     476936 kB
SUnreclaim:       198372 kB
"#;

    const SAMPLE_PSI: &str = r#"
some avg10=1.23 avg60=0.50 avg300=0.10 total=368630
full avg10=0.45 avg60=0.12 avg300=0.02 total=367496
"#;

    #[test]
    fn test_parse_meminfo() {
        let metrics = MemoryMetrics::parse_meminfo(SAMPLE_MEMINFO).expect("parsing failed");
        assert_eq!(metrics.total_bytes, 16237200 * 1024);
        assert_eq!(metrics.available_bytes, 11518164 * 1024);
        assert_eq!(metrics.used_bytes, (16237200 - 11518164) * 1024);
        assert_eq!(metrics.swap_total_bytes, 4194300 * 1024);
        assert_eq!(metrics.swap_free_bytes, 4051908 * 1024);
        assert_eq!(metrics.swap_used_bytes, (4194300 - 4051908) * 1024);

        assert!(metrics.used_percent() > 28.0 && metrics.used_percent() < 30.0);
        assert_eq!(format_bytes(metrics.total_bytes), "15.49 GiB");
    }

    #[test]
    fn test_parse_psi() {
        let psi = parse_psi_str(SAMPLE_PSI).expect("psi parsing failed");
        assert_eq!(psi.some.avg10, 1.23);
        assert_eq!(psi.some.avg60, 0.50);
        assert_eq!(psi.full.avg10, 0.45);
        assert_eq!(psi.full.total_us, 367496);
    }
}
