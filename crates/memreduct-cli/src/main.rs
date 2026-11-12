use clap::{Parser, Subcommand};
use colored::*;
use memreduct_core::cleaner::{CleanResult, CleanTarget, Cleaner};
use memreduct_core::ipc::{resolve_socket_path, send_ipc_request, IpcRequest, IpcResponse};
use memreduct_core::metrics::{format_bytes, MemoryMetrics};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use tokio::time::sleep;

#[derive(Parser, Debug)]
#[command(name = "memreduct")]
#[command(about = "Mem Reduct for Linux - System Memory Monitor & Cleaner")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Custom socket path for daemon IPC
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    /// Output in JSON format
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Show current memory statistics (RAM, Caches, Swap, PSI)
    Status,

    /// Live memory monitor in terminal
    Watch {
        /// Refresh interval in seconds (default: 1.5)
        #[arg(short, long, default_value_t = 1.5)]
        interval: f64,
    },

    /// Clean system memory caches and compact memory
    Clean {
        /// Drop both pagecache and dentries/inodes (default if no target specified)
        #[arg(short = 'c', long)]
        caches: bool,

        /// Drop pagecache only
        #[arg(long)]
        pagecache: bool,

        /// Drop dentries and inodes only
        #[arg(long)]
        dentries: bool,

        /// Compact physical memory (HugePages defragmentation)
        #[arg(short = 'm', long)]
        compact: bool,

        /// Safely flush and reset swap space into RAM
        #[arg(short = 's', long)]
        swap: bool,

        /// Full clean: all caches, compaction, and safe swap flush
        #[arg(short = 'a', long)]
        all: bool,

        /// Send desktop notification with result
        #[arg(short = 'n', long)]
        notify: bool,
    },

    /// Ping the memreduct-daemon to check status
    Ping,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let socket_path = cli.socket.unwrap_or_else(resolve_socket_path);

    match cli.command.unwrap_or(Commands::Status) {
        Commands::Status => {
            let metrics = MemoryMetrics::fetch()?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&metrics)?);
            } else {
                print_status(&metrics);
            }
        }
        Commands::Watch { interval } => {
            run_watch(interval).await?;
        }
        Commands::Ping => {
            match send_ipc_request(&socket_path, &IpcRequest::Ping) {
                Ok(IpcResponse::Pong) => {
                    println!("{}", "Daemon is running and responsive (PONG).".green().bold());
                }
                Ok(other) => {
                    println!("Unexpected daemon response: {:?}", other);
                }
                Err(e) => {
                    println!("{}: {}", "Daemon not reachable".red().bold(), e);
                    println!("Check if memreduct-daemon service is running.");
                }
            }
        }
        Commands::Clean {
            caches,
            pagecache,
            dentries,
            compact,
            swap,
            all,
            notify,
        } => {
            let target = determine_target(caches, pagecache, dentries, compact, swap, all);
            execute_clean(target, &socket_path, cli.json, notify).await?;
        }
    }

    Ok(())
}

fn determine_target(
    caches: bool,
    pagecache: bool,
    dentries: bool,
    compact: bool,
    swap: bool,
    all: bool,
) -> CleanTarget {
    if all {
        CleanTarget::All
    } else if pagecache {
        CleanTarget::PageCache
    } else if dentries {
        CleanTarget::DentriesInodes
    } else if compact {
        CleanTarget::CompactMemory
    } else if swap {
        CleanTarget::Swap
    } else if caches {
        CleanTarget::AllCaches
    } else {
        // Default action: All Caches
        CleanTarget::AllCaches
    }
}

async fn execute_clean(
    target: CleanTarget,
    socket_path: &PathBuf,
    json_output: bool,
    notify: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // If running as root, clean directly!
    if Cleaner::is_privileged() {
        println!("{}", "Running with root privileges. Executing directly...".cyan());
        let res = Cleaner::clean(target)?;
        if notify {
            send_desktop_notification(&res);
        }
        display_clean_result(&res, json_output)?;
        return Ok(());
    }

    // Otherwise, try connecting to the daemon
    match send_ipc_request(socket_path, &IpcRequest::Clean(target)) {
        Ok(IpcResponse::CleanResult(res)) => {
            if notify {
                send_desktop_notification(&res);
            }
            display_clean_result(&res, json_output)?;
            return Ok(());
        }
        Ok(IpcResponse::Error(err)) => {
            eprintln!("{}: {}", "Daemon returned error".red().bold(), err);
            std::process::exit(1);
        }
        Ok(other) => {
            eprintln!("Unexpected response from daemon: {:?}", other);
            std::process::exit(1);
        }
        Err(ipc_err) => {
            // Daemon is not running or not reachable.
            // Check if pkexec is available to elevate privileges on demand!
            if which_bin("pkexec") && which_bin("memreduct-daemon") {
                println!(
                    "{} (Socket: {:?}). Attempting elevation via pkexec...",
                    "Daemon not running".yellow(),
                    socket_path
                );

                let target_arg = match target {
                    CleanTarget::PageCache => "pagecache",
                    CleanTarget::DentriesInodes => "dentries-inodes",
                    CleanTarget::AllCaches => "caches",
                    CleanTarget::CompactMemory => "compact",
                    CleanTarget::Swap => "swap",
                    CleanTarget::All => "all",
                };

                let mut cmd = Command::new("pkexec");
                cmd.arg("memreduct-daemon")
                    .arg("--clean")
                    .arg(target_arg);

                if json_output {
                    cmd.arg("--json");
                }

                let status = cmd.status()?;
                if !status.success() {
                    eprintln!("{}", "Elevation cancelled or failed.".red());
                    std::process::exit(status.code().unwrap_or(1));
                }
                return Ok(());
            }

            eprintln!("{}: {}", "Cannot clean memory as non-root user".red().bold(), ipc_err);
            eprintln!("\nOptions to enable memory cleaning:");
            eprintln!(" 1. Start memreduct-daemon as root:");
            eprintln!("    {} sudo memreduct-daemon", "$".cyan());
            eprintln!(" 2. Or run CLI with sudo:");
            eprintln!("    {} sudo memreduct clean --all", "$".cyan());
            std::process::exit(1);
        }
    }
}

fn is_ru() -> bool {
    std::env::var("LANG")
        .or_else(|_| std::env::var("LC_ALL"))
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .map(|l| l.to_lowercase().starts_with("ru"))
        .unwrap_or(false)
}

fn translate_action(act: &str) -> String {
    if !is_ru() {
        return act.to_string();
    }
    match act {
        "Flushed dirty pages and dropped Page Cache (1)" => "Сброшены грязные страницы и кэш файлов (1)".into(),
        "Flushed dirty pages and dropped Dentries & Inodes (2)" => "Сброшены грязные страницы и кэш метаданных (2)".into(),
        "Flushed dirty pages and dropped All Caches (3)" => "Сброшены грязные страницы и все кэши ядра (3)".into(),
        "Compacted physical memory (HugePages defragmentation)" | "Compacted physical memory" => "Выполнена дефрагментация физической памяти (HugePages)".into(),
        "Safely flushed Swap into RAM and re-enabled Swap" => "Файл подкачки (Swap) безопасно сброшен в ОЗУ и перезапущен".into(),
        "Swap is already empty, skipped swap reset" => "Файл подкачки пуст, сброс пропущен".into(),
        other => other.to_string(),
    }
}

fn send_desktop_notification(res: &CleanResult) {
    let freed = res.freed_bytes;
    let freed_str = if freed >= 0 {
        format_bytes(freed as u64)
    } else {
        format!("-{}", format_bytes((-freed) as u64))
    };
    let ru = is_ru();
    let title = if ru { "Mem Reduct: Память очищена" } else { "Mem Reduct: Memory Cleaned" };
    let body = if ru {
        format!(
            "Освобождено: {}\nЗанято: {:.1}% ({})",
            freed_str,
            res.after.used_percent(),
            format_bytes(res.after.used_bytes)
        )
    } else {
        format!(
            "Freed: {}\nUsed: {:.1}% ({})",
            freed_str,
            res.after.used_percent(),
            format_bytes(res.after.used_bytes)
        )
    };
    let _ = Command::new("notify-send")
        .arg("-a")
        .arg("Mem Reduct")
        .arg(title)
        .arg(body)
        .arg("-i")
        .arg("utilities-system-monitor")
        .spawn();
}

fn display_clean_result(res: &CleanResult, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    if json {
        println!("{}", serde_json::to_string_pretty(res)?);
        return Ok(());
    }

    let ru = is_ru();
    let header = if ru { "=== Очистка памяти завершена ===" } else { "=== Memory Clean Completed ===" };
    println!("\n{}", header.green().bold());
    for action in &res.actions_performed {
        println!(" {} {}", "✔".green(), translate_action(action));
    }
    for warning in &res.warnings {
        println!(" {} {}", "⚠".yellow(), warning);
    }

    let stats_title = if ru { "Статистика:" } else { "Statistics:" };
    println!("\n{}", stats_title.bold());

    let before_label = if ru { "Занято RAM до:   " } else { "Used RAM before: " };
    let after_label = if ru { "Занято RAM после:" } else { "Used RAM after:  " };
    println!(
        " {}{} ({:.1}%)",
        before_label,
        format_bytes(res.before.used_bytes),
        res.before.used_percent()
    );
    println!(
        " {}{} ({:.1}%)",
        after_label,
        format_bytes(res.after.used_bytes),
        res.after.used_percent()
    );

    let diff = res.freed_bytes;
    if diff >= 0 {
        let freed_label = if ru { "Всего освобождено памяти:" } else { "Total memory freed:" };
        println!(" {} {}", freed_label, format_bytes(diff as u64).green().bold());
    } else {
        let change_label = if ru { "Изменение памяти:" } else { "Memory change:" };
        println!(" {} -{}", change_label, format_bytes((-diff) as u64).yellow());
    }
    println!();

    Ok(())
}

fn print_status(m: &MemoryMetrics) {
    let ru = is_ru();
    let title = if ru { "Mem Reduct (Linux Версия)" } else { "Mem Reduct (Linux Edition)" };
    println!("\n{}", title.bold().cyan());
    println!("{}", "=========================================".cyan());

    // RAM Bar
    let used_pct = m.used_percent();
    let bar = render_bar(used_pct);
    let pct_colored = if used_pct < 60.0 {
        format!("{:.1}%", used_pct).green().bold()
    } else if used_pct < 85.0 {
        format!("{:.1}%", used_pct).yellow().bold()
    } else {
        format!("{:.1}%", used_pct).red().bold()
    };

    let ram_title = if ru { "Физическая память (ОЗУ):" } else { "Physical Memory (RAM):" };
    println!("\n{}", ram_title.bold());
    println!(" {} {}", bar, pct_colored);

    let (l_used, l_avail, l_free, l_total) = if ru {
        ("Занято:     ", "Доступно:  ", "Свободно:   ", "Всего:     ")
    } else {
        ("Used:       ", "Available: ", "Free:       ", "Total:     ")
    };

    println!(
        " {} {:>10}  |  {} {:>10}",
        l_used,
        format_bytes(m.used_bytes).yellow(),
        l_avail,
        format_bytes(m.available_bytes).green()
    );
    println!(
        " {} {:>10}  |  {} {:>10}",
        l_free,
        format_bytes(m.free_bytes),
        l_total,
        format_bytes(m.total_bytes).bold()
    );

    // Caches & Buffers
    let cache_title = if ru { "Кэши и буферы:" } else { "Caches & Buffers:" };
    println!("\n{}", cache_title.bold());

    let (l_page, l_buf, l_rec, l_dirty, l_slab, l_srec) = if ru {
        ("Кэш файлов: ", "Буферы:    ", "Очищаемый:  ", "Грязные:   ", "Кэш ядра:   ", "Очищ. Slab:")
    } else {
        ("Page Cache: ", "Buffers:   ", "Reclaimable:", "Dirty:     ", "Slab Total: ", "SReclaim:  ")
    };

    println!(
        " {} {:>10}  |  {} {:>10}",
        l_page,
        format_bytes(m.cached_bytes),
        l_buf,
        format_bytes(m.buffers_bytes)
    );
    println!(
        " {} {:>10}  |  {} {:>10}",
        l_rec,
        format_bytes(m.reclaimable_cache_bytes).cyan(),
        l_dirty,
        format_bytes(m.dirty_bytes)
    );
    println!(
        " {} {:>10}  |  {} {:>10}",
        l_slab,
        format_bytes(m.slab_bytes),
        l_srec,
        format_bytes(m.sreclaimable_bytes)
    );

    // Swap / Zram
    let swap_title = if ru { "Раздел подкачки (Swap):" } else { "Swap Space:" };
    println!("\n{}", swap_title.bold());
    if m.swap_total_bytes > 0 {
        let swap_pct = m.swap_used_percent();
        let (l_su, l_st) = if ru { ("Swap занято: ", "Swap всего: ") } else { ("Swap Used:  ", "Swap Total: ") };
        println!(
            " {} {:>10} ({:.1}%) | {} {:>10}",
            l_su,
            format_bytes(m.swap_used_bytes).yellow(),
            swap_pct,
            l_st,
            format_bytes(m.swap_total_bytes)
        );
    } else {
        let no_swap = if ru { " Swap не настроен или отключен." } else { " Swap is not configured or disabled." };
        println!("{}", no_swap);
    }

    for z in &m.zram {
        let (l_raw, l_compr, l_saved) = if ru {
            ("Исходный", "Сжатый", "Сэкономлено")
        } else {
            ("Raw", "Compr", "Saved")
        };
        println!(
            " ZRAM ({}):  {} {} -> {} {} (Ratio: {:.2}x, {}: {})",
            z.device,
            l_raw,
            format_bytes(z.orig_data_bytes),
            l_compr,
            format_bytes(z.compr_data_bytes),
            z.compression_ratio(),
            l_saved,
            format_bytes(z.space_saved_bytes()).green()
        );
    }

    // Pressure Stall Information (PSI)
    if let Some(psi) = &m.psi {
        let psi_title = if ru { "Давление на память ядра (PSI):" } else { "Memory Pressure (PSI):" };
        println!("\n{}", psi_title.bold());
        let (l_some, l_full) = if ru { ("Задержка (Some):", "Простой (Full): ") } else { ("Some:           ", "Full:           ") };
        println!(
            " {} 10s={:.2}%, 60s={:.2}%, 300s={:.2}%",
            l_some, psi.some.avg10, psi.some.avg60, psi.some.avg300
        );
        println!(
            " {} 10s={:.2}%, 60s={:.2}%, 300s={:.2}%",
            l_full, psi.full.avg10, psi.full.avg60, psi.full.avg300
        );
    }
    println!();
}

fn render_bar(percentage: f64) -> String {
    let width = 30;
    let filled = ((percentage / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width.saturating_sub(filled);

    let fill_str = "=".repeat(filled);
    let empty_str = " ".repeat(empty);

    let colored_fill = if percentage < 60.0 {
        fill_str.green()
    } else if percentage < 85.0 {
        fill_str.yellow()
    } else {
        fill_str.red()
    };

    format!("[{}{}]", colored_fill, empty_str)
}

async fn run_watch(interval_secs: f64) -> Result<(), Box<dyn std::error::Error>> {
    let dur = Duration::from_secs_f64(interval_secs.max(0.2));

    loop {
        // Clear terminal screen
        print!("\x1B[2J\x1B[1;1H");
        let metrics = MemoryMetrics::fetch()?;
        print_status(&metrics);
        println!("{}", format!("(Refreshing every {:.1}s. Press Ctrl+C to exit)", interval_secs).dimmed());

        tokio::select! {
            _ = sleep(dur) => {},
            _ = tokio::signal::ctrl_c() => {
                println!("\nExiting monitor.");
                break;
            }
        }
    }

    Ok(())
}

fn which_bin(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
