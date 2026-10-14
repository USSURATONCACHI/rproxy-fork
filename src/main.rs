#![recursion_limit = "512"]
#![warn(rust_2018_idioms)]

use chrono::Local;
use clap::{Parser, ValueEnum};
use log::*;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

mod dns;
mod tcp;
mod udp;
use rproxy::{Proxy, Settings};

/// Transport protocol for the proxied service.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Protocol {
    Udp,
    Tcp,
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Protocol::Udp => write!(f, "UDP"),
            Protocol::Tcp => write!(f, "TCP"),
        }
    }
}

impl FromStr for Protocol {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_uppercase().as_str() {
            "UDP" => Ok(Protocol::Udp),
            "TCP" => Ok(Protocol::Tcp),
            _ => Err(format!(
                "unknown protocol '{s}' (expected 'UDP' or 'TCP', case-insensitive)"
            )),
        }
    }
}

/// Validate a local bind address. Must be a concrete `IP:PORT` socket
/// address — you cannot bind a hostname.
fn parse_bind(s: &str) -> Result<String, String> {
    s.parse::<SocketAddr>().map(|_| s.to_string()).map_err(|_| {
        format!(
            "'{s}' is not a valid bind address — expected IP:PORT, \
             e.g. 0.0.0.0:8080 or [::]:8080"
        )
    })
}

/// Validate a remote endpoint. Accepts an IPv4/IPv6 socket-address literal
/// or a `hostname:port` (resolved at runtime). IPv6 literals must be
/// bracketed: `[2001:db8::1]:443`.
fn parse_endpoint(s: &str) -> Result<String, String> {
    // Fast path: a literal socket address (IPv4, or bracketed IPv6).
    if let Ok(sa) = s.parse::<SocketAddr>() {
        if sa.port() == 0 {
            return Err(format!("port must be 1-65535 in '{s}'"));
        }
        return Ok(s.to_string());
    }

    // Split host:port, bracket-aware so IPv6 colons aren't misread.
    let (host, port) = if let Some(end) = s.rfind(']') {
        let host = s
            .get(1..end)
            .filter(|_| s.starts_with('['))
            .ok_or_else(|| format!("malformed IPv6 literal: '{s}'"))?;
        // Brackets unambiguously denote an IPv6 literal — validate it as one.
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(format!("'{host}' is not a valid IPv6 address in '{s}'"));
        }
        let port = s[end + 1..]
            .strip_prefix(':')
            .ok_or_else(|| format!("missing ':PORT' after IPv6 literal in '{s}'"))?;
        (host, port)
    } else {
        match s.rsplit_once(':') {
            Some((h, _)) if h.contains(':') => {
                return Err(format!(
                    "'{s}' looks like a bare IPv6 address — wrap it in brackets, \
                     e.g. [{h}]:PORT"
                ));
            }
            Some((h, p)) => (h, p),
            None => return Err(format!("expected 'host:port', got '{s}'")),
        }
    };

    if host.is_empty() {
        return Err(format!("missing host in '{s}'"));
    }
    let port_num: u32 = port
        .parse()
        .map_err(|_| format!("invalid port '{port}' in '{s}'"))?;
    if port_num == 0 || port_num > 65535 {
        return Err(format!("port must be 1-65535 in '{s}'"));
    }
    Ok(s.to_string())
}

fn pid_file_path() -> PathBuf {
    std::env::temp_dir().join("rproxy.pid")
}

fn reload_signal_path() -> PathBuf {
    std::env::temp_dir().join("rproxy.reload")
}

#[derive(Parser, Debug)]
#[command(
    name = "rproxy",
    version,
    about = "Platform independent high-performance async UDP/TCP proxy"
)]
struct Options {
    /// Remote service endpoint: IP:PORT, [IPv6]:PORT, or hostname:PORT.
    /// Required unless --config is given.
    #[arg(short = 'r', long, value_parser = parse_endpoint)]
    remote: Option<String>,

    /// Local endpoint for rproxy to bind: IP:PORT (e.g. 0.0.0.0:8080 or [::]:8080).
    /// Required unless --config is given.
    #[arg(short = 'b', long, value_parser = parse_bind)]
    bind: Option<String>,

    /// Protocol of the remote service (case-insensitive).
    #[arg(short = 'p', long, value_enum, ignore_case = true, default_value_t = Protocol::Udp)]
    protocol: Protocol,

    /// Set logger level to debug.
    #[arg(short = 'd', long)]
    debug: bool,

    /// Logger settings file (rotating etc...).
    #[arg(short = 'l', long, default_value = "rproxy.logger.yaml")]
    logger_settings: String,

    /// Configuration file for multiple proxy instances.
    #[arg(short = 'c', long)]
    config: Option<PathBuf>,

    /// Send a signal to a running rproxy process. Supported: reload.
    #[arg(short = 's', long)]
    signal: Option<String>,

    /// Max concurrent TCP connections (default: 1024).
    #[arg(long)]
    max_connections: Option<usize>,

    /// Max concurrent UDP client tunnels (default: 1024).
    #[arg(long)]
    max_client_tunnels: Option<usize>,

    /// TCP keepalive idle time in seconds (default: 7200).
    #[arg(long)]
    keepalive_idle: Option<u64>,

    /// TCP keepalive probe interval in seconds (default: 75).
    #[arg(long)]
    keepalive_interval: Option<u64>,
}

static MY_LOGGER: MyLogger = MyLogger;

struct MyLogger;

impl log::Log for MyLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            println!("[{}][{}] - {}", record.level(), Local::now(), record.args());
        }
    }
    fn flush(&self) {}
}

fn build_settings(options: &Options, base: Settings) -> Settings {
    Settings {
        max_connections: options.max_connections.unwrap_or(base.max_connections),
        max_client_tunnels: options
            .max_client_tunnels
            .unwrap_or(base.max_client_tunnels),
        keepalive_idle: options.keepalive_idle.unwrap_or(base.keepalive_idle),
        keepalive_interval: options
            .keepalive_interval
            .unwrap_or(base.keepalive_interval),
    }
}

fn write_pid_file() {
    let path = pid_file_path();
    let pid = std::process::id();
    if let Err(e) = std::fs::write(&path, pid.to_string()) {
        warn!("Failed to write PID file {}: {}", path.display(), e);
    } else {
        info!("PID {} written to {}", pid, path.display());
    }
}

fn remove_pid_file() {
    let _ = std::fs::remove_file(pid_file_path());
    let _ = std::fs::remove_file(reload_signal_path());
}

fn send_reload_signal() {
    let pid_path = pid_file_path();

    // Verify rproxy is running by checking PID file
    match std::fs::read_to_string(&pid_path) {
        Ok(content) => {
            match content.trim().parse::<u32>() {
                Ok(pid) => {
                    // On Unix, send SIGHUP directly
                    #[cfg(unix)]
                    {
                        use nix::sys::signal::{kill, Signal};
                        use nix::unistd::Pid;
                        match kill(Pid::from_raw(pid as i32), Signal::SIGHUP) {
                            Ok(()) => {
                                println!("Reload signal sent to rproxy (PID {})", pid);
                                return;
                            }
                            Err(e) => {
                                eprintln!("Failed to send signal to PID {}: {}", pid, e);
                                std::process::exit(1);
                            }
                        }
                    }

                    // On Windows (or fallback), write a signal file that the running process polls
                    #[cfg(not(unix))]
                    {
                        let signal_path = reload_signal_path();
                        match std::fs::write(&signal_path, "reload") {
                            Ok(()) => {
                                println!("Reload signal written for rproxy (PID {})", pid);
                            }
                            Err(e) => {
                                eprintln!("Failed to write reload signal file: {}", e);
                                std::process::exit(1);
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Invalid PID in {}: {}", pid_path.display(), e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!(
                "Failed to read PID file {}: {}. Is rproxy running?",
                pid_path.display(),
                e
            );
            std::process::exit(1);
        }
    }
}

struct ProxyTask {
    #[allow(dead_code)]
    handle: JoinHandle<Result<(), std::io::Error>>,
    cancel: CancellationToken,
    proxy: Proxy,
}

fn spawn_proxy(proxy: &Proxy, settings: &Arc<Settings>) -> ProxyTask {
    let bind = proxy.bind.clone();
    let remote = proxy.remote.clone();
    let protocol = proxy.protocol.clone();
    let s = settings.clone();
    let cancel = CancellationToken::new();
    let c = cancel.clone();

    let proto = <Protocol as FromStr>::from_str(&protocol).unwrap_or_else(|e| {
        warn!("Proxy {} -> {}: {}; defaulting to UDP", bind, remote, e);
        Protocol::Udp
    });

    let handle = match proto {
        Protocol::Udp => tokio::spawn(async move { udp::udp_proxy(&bind, &remote, s, c).await }),
        Protocol::Tcp => tokio::spawn(async move { tcp::tcp_proxy(&bind, &remote, s, c).await }),
    };

    ProxyTask {
        handle,
        cancel,
        proxy: proxy.clone(),
    }
}

fn reload_config(config_path: &Path, options: &Options, running: &mut HashMap<String, ProxyTask>) {
    let config = match rproxy::load_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to reload configuration: {:?}", e);
            return;
        }
    };

    let settings = Arc::new(build_settings(options, config.settings));

    // Build new proxy map keyed by bind address
    let mut new_proxies: HashMap<String, &Proxy> = HashMap::new();
    for proxy in &config.proxies {
        new_proxies.insert(proxy.bind.clone(), proxy);
    }

    // Find removed and changed proxies
    let old_keys: Vec<String> = running.keys().cloned().collect();
    for key in &old_keys {
        match new_proxies.get(key) {
            None => {
                // Removed
                info!("[reload] Stopping proxy on {}", key);
                if let Some(task) = running.remove(key) {
                    task.cancel.cancel();
                }
            }
            Some(new_proxy) => {
                let old_task = running.get(key).unwrap();
                if old_task.proxy.remote != new_proxy.remote
                    || old_task.proxy.protocol != new_proxy.protocol
                {
                    // Changed — cancel old, will spawn new below
                    info!(
                        "[reload] Restarting proxy on {} ({}->{})",
                        key, old_task.proxy.remote, new_proxy.remote
                    );
                    if let Some(task) = running.remove(key) {
                        task.cancel.cancel();
                    }
                } else {
                    // Unchanged — keep running
                    debug!("[reload] Proxy on {} unchanged, keeping", key);
                }
            }
        }
    }

    // Spawn new and changed proxies
    for (bind, proxy) in &new_proxies {
        if !running.contains_key(bind) {
            info!(
                "[reload] Starting proxy on {} -> {} ({})",
                bind, proxy.remote, proxy.protocol
            );
            let task = spawn_proxy(proxy, &settings);
            running.insert(bind.clone(), task);
        }
    }

    info!(
        "[reload] Configuration reloaded: {} proxies active",
        running.len()
    );
}

/// Wait for a reload signal. On Unix uses SIGHUP, on Windows polls a signal file.
async fn wait_for_reload_signal() {
    #[cfg(unix)]
    {
        let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .expect("Failed to register SIGHUP handler");
        sighup.recv().await;
    }

    #[cfg(not(unix))]
    {
        let signal_path = reload_signal_path();
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            if signal_path.exists() {
                let _ = std::fs::remove_file(&signal_path);
                break;
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let options = Options::parse();

    // Handle -s reload
    if let Some(ref sig) = options.signal {
        match sig.as_str() {
            "reload" => {
                send_reload_signal();
                return;
            }
            _ => {
                eprintln!("Unknown signal: {}. Supported: reload", sig);
                std::process::exit(1);
            }
        }
    }

    // Setup logging
    if Path::new(&options.logger_settings).exists() {
        log4rs::init_file(&options.logger_settings, Default::default()).unwrap();
        debug!("NICE");
    } else {
        log::set_logger(&MY_LOGGER).unwrap();
        if options.debug {
            log::set_max_level(log::LevelFilter::Debug);
        } else {
            log::set_max_level(log::LevelFilter::Info);
        }
    }

    match options.config {
        None => {
            // Single proxy mode — no hot reload
            let bind = match options.bind.clone() {
                Some(b) => b,
                None => {
                    error!("--bind is required when --config is not used");
                    std::process::exit(2);
                }
            };
            let remote = match options.remote.clone() {
                Some(r) => r,
                None => {
                    error!("--remote is required when --config is not used");
                    std::process::exit(2);
                }
            };

            let settings = Arc::new(build_settings(&options, Settings::default()));
            let cancel = CancellationToken::new();
            let result = match options.protocol {
                Protocol::Udp => udp::udp_proxy(&bind, &remote, settings, cancel).await,
                Protocol::Tcp => tcp::tcp_proxy(&bind, &remote, settings, cancel).await,
            };
            if let Err(e) = result {
                error!("Proxy exited with error: {}", e);
                std::process::exit(1);
            }
        }
        Some(ref config_path) => {
            if !config_path.as_path().exists() {
                error!(
                    "Invalid configuration file path {}",
                    config_path.as_path().display()
                );
                return;
            }

            // Write PID file for -s reload
            write_pid_file();

            let config = match rproxy::load_config(config_path.as_path()) {
                Ok(c) => c,
                Err(e) => {
                    error!("Failed to parse configuration: {:?}", e);
                    remove_pid_file();
                    return;
                }
            };

            let settings = Arc::new(build_settings(&options, config.settings));
            info!("Settings: max_connections={}, max_client_tunnels={}, keepalive_idle={}s, keepalive_interval={}s",
                settings.max_connections, settings.max_client_tunnels,
                settings.keepalive_idle, settings.keepalive_interval);

            // Spawn initial proxies
            let mut running: HashMap<String, ProxyTask> = HashMap::new();
            for proxy in &config.proxies {
                let task = spawn_proxy(proxy, &settings);
                running.insert(proxy.bind.clone(), task);
            }

            info!(
                "Started {} proxies, listening for reload signal",
                running.len()
            );

            loop {
                tokio::select! {
                    _ = wait_for_reload_signal() => {
                        info!("[reload] Reload signal received, reloading configuration from {}", config_path.display());
                        reload_config(config_path.as_path(), &options, &mut running);
                    },
                    _ = tokio::signal::ctrl_c() => {
                        info!("Ctrl+C received, shutting down all proxies...");
                        for (bind, task) in running.drain() {
                            info!("Stopping proxy on {}", bind);
                            task.cancel.cancel();
                        }
                        break;
                    }
                }
            }

            remove_pid_file();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_parsing_is_case_insensitive() {
        assert_eq!(
            <Protocol as FromStr>::from_str("UDP").unwrap(),
            Protocol::Udp
        );
        assert_eq!(
            <Protocol as FromStr>::from_str("udp").unwrap(),
            Protocol::Udp
        );
        assert_eq!(
            <Protocol as FromStr>::from_str(" Tcp ").unwrap(),
            Protocol::Tcp
        );
        assert_eq!(
            <Protocol as FromStr>::from_str("TCP").unwrap(),
            Protocol::Tcp
        );
    }

    #[test]
    fn protocol_parsing_reports_bad_input() {
        let err = <Protocol as FromStr>::from_str("htcp").unwrap_err();
        assert!(err.contains("htcp"), "error should echo the input: {err}");
        assert!(err.contains("UDP") && err.contains("TCP"));
    }

    #[test]
    fn bind_accepts_ipv4_and_ipv6_literals() {
        assert!(parse_bind("0.0.0.0:8080").is_ok());
        assert!(parse_bind("127.0.0.1:1").is_ok());
        assert!(parse_bind("[::]:8080").is_ok());
        assert!(parse_bind("[::1]:50000").is_ok());
    }

    #[test]
    fn bind_rejects_hostnames_and_garbage() {
        // A hostname cannot be bound.
        assert!(parse_bind("localhost:8080").is_err());
        assert!(parse_bind("not-an-addr").is_err());
        // Bare IPv6 without brackets is invalid as a bind address.
        assert!(parse_bind("::1:8080").is_err());
    }

    #[test]
    fn endpoint_accepts_ipv4_ipv6_and_hostnames() {
        assert!(parse_endpoint("1.2.3.4:80").is_ok());
        assert!(parse_endpoint("[fd00:99:0::3]:50000").is_ok());
        assert!(parse_endpoint("[2001:db8::1]:443").is_ok());
        assert!(parse_endpoint("example.com:8080").is_ok());
    }

    #[test]
    fn endpoint_rejects_invalid_inputs() {
        // Invalid IPv6 hextet (5 hex digits) and unbracketed -> not a literal,
        // and no single host:port split -> rejected.
        assert!(parse_endpoint("fd000:99:0::3:50000").is_err());
        // Missing port.
        assert!(parse_endpoint("example.com").is_err());
        // Port out of range / zero.
        assert!(parse_endpoint("1.2.3.4:0").is_err());
        assert!(parse_endpoint("1.2.3.4:99999").is_err());
        // Non-numeric port.
        assert!(parse_endpoint("1.2.3.4:http").is_err());
    }

    #[test]
    fn endpoint_rejects_invalid_bracketed_ipv6() {
        // 'fd000' is a 5-hex-digit hextet — not a valid IPv6 address.
        // This is the exact input that previously slipped past validation
        // and later panicked the UDP proxy.
        let err = parse_endpoint("[fd000:99:0::3]:50000").unwrap_err();
        assert!(
            err.contains("IPv6"),
            "should reject invalid bracketed IPv6: {err}"
        );
        // The valid form is accepted.
        assert!(parse_endpoint("[fd00:99:0::3]:50000").is_ok());
    }

    #[test]
    fn endpoint_hints_at_brackets_for_bare_ipv6() {
        let err = parse_endpoint("fd00:99:0::3").unwrap_err();
        assert!(
            err.contains("brackets"),
            "bare IPv6 error should hint at brackets: {err}"
        );
    }
}
