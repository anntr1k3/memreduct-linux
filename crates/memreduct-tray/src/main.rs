use clap::Parser;
use ksni::menu::{MenuItem, StandardItem, SubMenu};
use ksni::{Icon, ToolTip, Tray, TrayMethods};
use memreduct_core::cleaner::{CleanResult, CleanTarget, Cleaner};
use memreduct_core::ipc::{resolve_socket_path, send_ipc_request, IpcRequest, IpcResponse};
use memreduct_core::metrics::{format_bytes, MemoryMetrics};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "memreduct-tray")]
#[command(about = "System Tray Indicator for Mem Reduct Linux")]
struct Args {
    /// Update interval in seconds (default: 1.5)
    #[arg(short, long, default_value_t = 1.5)]
    interval: f64,

    /// Custom daemon socket path
    #[arg(long)]
    socket: Option<PathBuf>,
}

const DIGITS: [u16; 10] = [
    0b111_101_101_101_111, // 0
    0b010_110_010_010_111, // 1
    0b111_001_111_100_111, // 2
    0b111_001_111_001_111, // 3
    0b101_101_111_001_001, // 4
    0b111_100_111_001_111, // 5
    0b111_100_111_101_111, // 6
    0b111_001_010_010_010, // 7
    0b111_101_111_101_111, // 8
    0b111_101_111_001_111, // 9
];

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TrayConfig {
    pub threshold_enabled: bool,
    pub threshold_pct: f64,
    pub timer_enabled: bool,
    pub timer_minutes: u64,
    pub notify_on_clean: bool,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            threshold_enabled: false,
            threshold_pct: 80.0,
            timer_enabled: false,
            timer_minutes: 30,
            notify_on_clean: true,
        }
    }
}

impl TrayConfig {
    fn config_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".config/memreduct/tray_config.json")
    }

    fn load() -> Self {
        let path = Self::config_path();
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str(&content) {
                return cfg;
            }
        }
        Self::default()
    }

    fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, json);
        }
    }
}

struct MemReductTray {
    metrics: Option<MemoryMetrics>,
    socket_path: PathBuf,
    config: Arc<Mutex<TrayConfig>>,
    last_cleaned_msg: Option<String>,
    last_clean_time: Arc<Mutex<Instant>>,
}

impl MemReductTray {
    fn new(socket_path: PathBuf) -> Self {
        let metrics = MemoryMetrics::fetch().ok();
        let config = Arc::new(Mutex::new(TrayConfig::load()));
        Self {
            metrics,
            socket_path,
            config,
            last_cleaned_msg: None,
            last_clean_time: Arc::new(Mutex::new(Instant::now())),
        }
    }

    fn update_metrics(&mut self) {
        if let Ok(m) = MemoryMetrics::fetch() {
            let cfg = { self.config.lock().unwrap().clone() };

            // 1. Check Threshold Autoclean
            if cfg.threshold_enabled && m.used_percent() >= cfg.threshold_pct {
                info!(
                    "RAM usage ({:.1}%) exceeded threshold ({:.1}%). Triggering autoclean...",
                    m.used_percent(),
                    cfg.threshold_pct
                );
                let _ = self.trigger_clean(CleanTarget::AllCaches, true);
            }

            // 2. Check Timer Autoclean
            if cfg.timer_enabled {
                let elapsed_mins = {
                    let last = self.last_clean_time.lock().unwrap();
                    last.elapsed().as_secs() / 60
                };
                if elapsed_mins >= cfg.timer_minutes {
                    info!("Timer autoclean triggered (every {} min)...", cfg.timer_minutes);
                    let _ = self.trigger_clean(CleanTarget::AllCaches, true);
                }
            }

            self.metrics = Some(m);
        }
    }

    fn trigger_clean(&mut self, target: CleanTarget, is_auto: bool) -> Result<CleanResult, String> {
        let res = if Cleaner::is_privileged() {
            Cleaner::clean(target).map_err(|e| e.to_string())
        } else {
            match send_ipc_request(&self.socket_path, &IpcRequest::Clean(target)) {
                Ok(IpcResponse::CleanResult(res)) => Ok(res),
                Ok(IpcResponse::Error(e)) => Err(e),
                Ok(_) => Err("Unexpected response from daemon".into()),
                Err(e) => Err(format!("Daemon unreachable: {}", e)),
            }
        };

        if let Ok(ref clean_res) = res {
            *self.last_clean_time.lock().unwrap() = Instant::now();
            let freed_str = format_bytes(clean_res.freed_bytes.max(0) as u64);
            self.last_cleaned_msg = Some(format!("Freed {}", freed_str));

            let cfg = { self.config.lock().unwrap().clone() };
            if cfg.notify_on_clean {
                let prefix = if is_auto { "Автоочистка памяти" } else { "Очистка памяти" };
                let body = format!(
                    "Освобождено: {}\nЗанято: {:.1}% ({})",
                    freed_str,
                    clean_res.after.used_percent(),
                    format_bytes(clean_res.after.used_bytes)
                );
                send_desktop_notification(prefix, &body);
            }
        }

        res
    }

    fn generate_icon(&self) -> Icon {
        const WIDTH: usize = 24;
        const HEIGHT: usize = 24;
        let mut pixels = vec![0u8; WIDTH * HEIGHT * 4];

        let pct = self
            .metrics
            .as_ref()
            .map(|m| m.used_percent().round() as u32)
            .unwrap_or(0)
            .min(99);

        // Colors (ARGB)
        let (gauge_r, gauge_g, gauge_b) = if pct < 60 {
            (46, 204, 113) // Green
        } else if pct < 85 {
            (241, 196, 15) // Yellow
        } else {
            (231, 76, 60) // Red
        };

        // Dark rounded background
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let idx = (y * WIDTH + x) * 4;
                let is_corner = (x < 3 && y < 3 && (3 - x) * (3 - x) + (3 - y) * (3 - y) > 9)
                    || (x >= 21 && y < 3 && (x - 20) * (x - 20) + (3 - y) * (3 - y) > 9)
                    || (x < 3 && y >= 21 && (3 - x) * (3 - x) + (y - 20) * (y - 20) > 9)
                    || (x >= 21 && y >= 21 && (x - 20) * (x - 20) + (y - 20) * (y - 20) > 9);

                if !is_corner {
                    pixels[idx] = 235; // Alpha
                    pixels[idx + 1] = 24; // R
                    pixels[idx + 2] = 24; // G
                    pixels[idx + 3] = 37; // B (dark slate)
                }
            }
        }

        // Draw bottom gauge bar (y = 20..22)
        let fill_w = ((pct as f64 / 100.0) * (WIDTH - 4) as f64).round() as usize;
        for y in 20..22 {
            for x in 2..(2 + fill_w).min(WIDTH - 2) {
                let idx = (y * WIDTH + x) * 4;
                pixels[idx] = 255;
                pixels[idx + 1] = gauge_r;
                pixels[idx + 2] = gauge_g;
                pixels[idx + 3] = gauge_b;
            }
        }

        // Render digits
        let d1 = (pct / 10) as usize % 10;
        let d2 = (pct % 10) as usize;

        draw_digit_scaled_2x(&mut pixels, WIDTH, 4, 5, d1);
        draw_digit_scaled_2x(&mut pixels, WIDTH, 13, 5, d2);

        Icon {
            width: WIDTH as i32,
            height: HEIGHT as i32,
            data: pixels,
        }
    }
}

fn draw_digit_scaled_2x(pixels: &mut [u8], stride: usize, start_x: usize, start_y: usize, digit: usize) {
    let font_bits = DIGITS[digit.min(9)];
    for row in 0..5 {
        for col in 0..3 {
            let bit_pos = 14 - (row * 3 + col);
            if (font_bits >> bit_pos) & 1 == 1 {
                for dy in 0..2 {
                    for dx in 0..2 {
                        let px = start_x + col * 2 + dx;
                        let py = start_y + row * 2 + dy;
                        if px < stride && py < 24 {
                            let idx = (py * stride + px) * 4;
                            pixels[idx] = 255;     // Alpha
                            pixels[idx + 1] = 255; // R
                            pixels[idx + 2] = 255; // G
                            pixels[idx + 3] = 255; // B
                        }
                    }
                }
            }
        }
    }
}

fn send_desktop_notification(summary: &str, body: &str) {
    let _ = Command::new("notify-send")
        .arg("-a")
        .arg("Mem Reduct")
        .arg(summary)
        .arg(body)
        .arg("-i")
        .arg("utilities-system-monitor")
        .spawn();
}

impl Tray for MemReductTray {
    fn id(&self) -> String {
        "org.memreduct.MemReduct".into()
    }

    fn title(&self) -> String {
        if let Some(m) = &self.metrics {
            format!("Mem Reduct: {:.1}%", m.used_percent())
        } else {
            "Mem Reduct".into()
        }
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![self.generate_icon()]
    }

    fn tool_tip(&self) -> ToolTip {
        if let Some(m) = &self.metrics {
            ToolTip {
                title: format!("Mem Reduct: {:.1}% Used", m.used_percent()),
                description: format!(
                    "Used: {} / {}\nAvailable: {}\nCaches: {}\nSwap: {}",
                    format_bytes(m.used_bytes),
                    format_bytes(m.total_bytes),
                    format_bytes(m.available_bytes),
                    format_bytes(m.reclaimable_cache_bytes),
                    format_bytes(m.swap_used_bytes),
                ),
                icon_name: "utilities-system-monitor".into(),
                icon_pixmap: Vec::new(),
            }
        } else {
            ToolTip {
                title: "Mem Reduct (Linux)".into(),
                description: "Monitoring system memory...".into(),
                icon_name: "utilities-system-monitor".into(),
                icon_pixmap: Vec::new(),
            }
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = Vec::new();

        if let Some(m) = &self.metrics {
            let header = format!(
                "RAM: {:.1}% ({} / {})",
                m.used_percent(),
                format_bytes(m.used_bytes),
                format_bytes(m.total_bytes)
            );
            items.push(
                StandardItem {
                    label: header,
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );

            items.push(
                StandardItem {
                    label: format!("Available: {}", format_bytes(m.available_bytes)),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );

            items.push(
                StandardItem {
                    label: format!("Caches: {}", format_bytes(m.reclaimable_cache_bytes)),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );

            if m.swap_total_bytes > 0 {
                items.push(
                    StandardItem {
                        label: format!(
                            "Swap: {} / {}",
                            format_bytes(m.swap_used_bytes),
                            format_bytes(m.swap_total_bytes)
                        ),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
            }
        }

        if let Some(msg) = &self.last_cleaned_msg {
            items.push(MenuItem::Separator);
            items.push(
                StandardItem {
                    label: format!("Last clean: {}", msg),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
        }

        items.push(MenuItem::Separator);

        // Actions
        items.push(
            StandardItem {
                label: "⚡ Clean All Caches".into(),
                icon_name: "edit-clear".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.trigger_clean(CleanTarget::AllCaches, false);
                }),
                ..Default::default()
            }
            .into(),
        );

        items.push(
            StandardItem {
                label: "🧹 Full Clean (Caches + Compact + Swap)".into(),
                icon_name: "edit-clear-all".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.trigger_clean(CleanTarget::All, false);
                }),
                ..Default::default()
            }
            .into(),
        );

        items.push(
            StandardItem {
                label: "🗜 Compact Memory".into(),
                icon_name: "system-run".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.trigger_clean(CleanTarget::CompactMemory, false);
                }),
                ..Default::default()
            }
            .into(),
        );

        items.push(
            StandardItem {
                label: "🔄 Safe Swap Reset".into(),
                icon_name: "view-refresh".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.trigger_clean(CleanTarget::Swap, false);
                }),
                ..Default::default()
            }
            .into(),
        );

        items.push(MenuItem::Separator);

        // Autoclean Submenu
        let cfg = { self.config.lock().unwrap().clone() };

        let threshold_label = if cfg.threshold_enabled {
            format!("✔ Очистка при RAM > {}% (Вкл)", cfg.threshold_pct as u32)
        } else {
            format!("  Очистка при RAM > {}% (Выкл)", cfg.threshold_pct as u32)
        };

        let timer_label = if cfg.timer_enabled {
            format!("✔ Очистка каждые {} мин (Вкл)", cfg.timer_minutes)
        } else {
            format!("  Очистка каждые {} мин (Выкл)", cfg.timer_minutes)
        };

        let notify_label = if cfg.notify_on_clean {
            "✔ Всплывающие уведомления (Вкл)".to_string()
        } else {
            "  Всплывающие уведомления (Выкл)".to_string()
        };

        let cfg_clone1 = self.config.clone();
        let cfg_clone2 = self.config.clone();
        let cfg_clone3 = self.config.clone();

        items.push(
            SubMenu {
                label: "⚙ Настройки автоочистки".into(),
                submenu: vec![
                    StandardItem {
                        label: threshold_label,
                        activate: Box::new(move |_| {
                            let mut c = cfg_clone1.lock().unwrap();
                            c.threshold_enabled = !c.threshold_enabled;
                            c.save();
                        }),
                        ..Default::default()
                    }
                    .into(),
                    StandardItem {
                        label: timer_label,
                        activate: Box::new(move |_| {
                            let mut c = cfg_clone2.lock().unwrap();
                            c.timer_enabled = !c.timer_enabled;
                            c.save();
                        }),
                        ..Default::default()
                    }
                    .into(),
                    StandardItem {
                        label: notify_label,
                        activate: Box::new(move |_| {
                            let mut c = cfg_clone3.lock().unwrap();
                            c.notify_on_clean = !c.notify_on_clean;
                            c.save();
                        }),
                        ..Default::default()
                    }
                    .into(),
                ],
                ..Default::default()
            }
            .into(),
        );

        items.push(MenuItem::Separator);

        items.push(
            StandardItem {
                label: "Quit Mem Reduct".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|_| {
                    info!("Quit requested from tray menu. Exiting.");
                    std::process::exit(0);
                }),
                ..Default::default()
            }
            .into(),
        );

        items
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let socket_path = args.socket.unwrap_or_else(resolve_socket_path);
    info!("Starting memreduct-tray (socket: {:?})", socket_path);

    let tray = MemReductTray::new(socket_path);
    let handle = tray.spawn().await?;

    let dur = Duration::from_secs_f64(args.interval.max(0.5));

    loop {
        sleep(dur).await;
        handle.update(|t| {
            t.update_metrics();
        }).await;
    }
}
