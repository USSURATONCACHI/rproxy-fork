#![recursion_limit="512"]
#![warn(rust_2018_idioms)]

use chrono::Local;
use std::sync::Arc;
use std::collections::HashMap;
use std::path::PathBuf;
use std::path::Path;
use log::*;
use argh::FromArgs;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

mod tcp;
mod udp;
mod dns;
use rproxy::{Proxy, Settings};

const PID_FILE: &str = "/tmp/rproxy.pid";

#[allow(dead_code)]
#[derive(FromArgs)]
#[argh(description = "rproxy is a platform independent UDP TCP high performance async proxy")]
struct Options {
    /// remote service endpoint (UDP & TCP)
    #[argh(option, short='r', default="\"\".to_string()")]
    remote: String,
    /// local endpoint to be binded by rproxy
    #[argh(option, short='b', default="\"\".to_string()")]
    bind: String,
    /// protocol of the remote service(UDP|TCP)
    #[argh(option, short='p', default="\"UDP\".to_string()")]
    protocol: String,
    /// set logger level to debug or not
    #[argh(switch, short='d')]
    debug: bool,

    /// logger settings rotating etc...
    #[argh(option, short = 'l', default = "\"rproxy.logger.yaml\".to_string()")]
    logger_settings: String,

    /// configuration for multiple proxy instances
    #[argh(option, short = 'c')]
    config: Option<PathBuf>,

    /// send signal to running rproxy process (reload)
    #[argh(option, short = 's')]
    signal: Option<String>,

    /// max concurrent TCP connections (default: 1024)
    #[argh(option)]
    max_connections: Option<usize>,

    /// max concurrent UDP client tunnels (default: 1024)
    #[argh(option)]
    max_client_tunnels: Option<usize>,

    /// TCP keepalive idle time in seconds (default: 60)
    #[argh(option)]
    keepalive_idle: Option<u64>,

    /// TCP keepalive probe interval in seconds (default: 30)
    #[argh(option)]
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
        max_client_tunnels: options.max_client_tunnels.unwrap_or(base.max_client_tunnels),
        keepalive_idle: options.keepalive_idle.unwrap_or(base.keepalive_idle),
        keepalive_interval: options.keepalive_interval.unwrap_or(base.keepalive_interval),
    }
}

fn write_pid_file() {
    let pid = std::process::id();
    if let Err(e) = std::fs::write(PID_FILE, pid.to_string()) {
        warn!("Failed to write PID file {}: {}", PID_FILE, e);
    } else {
        info!("PID {} written to {}", pid, PID_FILE);
    }
}

fn remove_pid_file() {
    let _ = std::fs::remove_file(PID_FILE);
}

fn send_reload_signal() {
    match std::fs::read_to_string(PID_FILE) {
        Ok(content) => {
            match content.trim().parse::<i32>() {
                Ok(pid) => {
                    use nix::sys::signal::{kill, Signal};
                    use nix::unistd::Pid;
                    match kill(Pid::from_raw(pid), Signal::SIGHUP) {
                        Ok(()) => {
                            println!("Reload signal sent to rproxy (PID {})", pid);
                        }
                        Err(e) => {
                            eprintln!("Failed to send signal to PID {}: {}", pid, e);
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Invalid PID in {}: {}", PID_FILE, e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("Failed to read PID file {}: {}. Is rproxy running?", PID_FILE, e);
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

    let handle = if protocol == "UDP" {
        tokio::spawn(async move { udp::udp_proxy(&bind, &remote, s, c).await })
    } else {
        tokio::spawn(async move { tcp::tcp_proxy(&bind, &remote, s, c).await })
    };

    ProxyTask {
        handle,
        cancel,
        proxy: proxy.clone(),
    }
}

fn reload_config(
    config_path: &Path,
    options: &Options,
    running: &mut HashMap<String, ProxyTask>,
) {
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
                    // task.handle will complete on its own
                }
            }
            Some(new_proxy) => {
                let old_task = running.get(key).unwrap();
                if old_task.proxy.remote != new_proxy.remote || old_task.proxy.protocol != new_proxy.protocol {
                    // Changed — cancel old, will spawn new below
                    info!("[reload] Restarting proxy on {} ({}→{})",
                        key, old_task.proxy.remote, new_proxy.remote);
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
            info!("[reload] Starting proxy on {} -> {} ({})", bind, proxy.remote, proxy.protocol);
            let task = spawn_proxy(proxy, &settings);
            running.insert(bind.clone(), task);
        }
    }

    info!("[reload] Configuration reloaded: {} proxies active", running.len());
}

#[tokio::main]
async fn main(){
    let options: Options = argh::from_env();

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
    if Path::new(&options.logger_settings).exists(){
        log4rs::init_file(&options.logger_settings,
            Default::default()).unwrap();
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
            let settings = Arc::new(build_settings(&options, Settings::default()));
            let cancel = CancellationToken::new();
            if options.protocol == "UDP"{
                udp::udp_proxy(&options.bind, &options.remote, settings, cancel).await.unwrap();
            } else if options.protocol == "TCP" {
                tcp::tcp_proxy(&options.bind, &options.remote, settings, cancel).await.unwrap();
            }
        },
        Some(ref config_path) => {
            if !config_path.as_path().exists() {
                error!("Invalid configuration file path {}", config_path.as_path().display());
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

            info!("Started {} proxies, listening for SIGHUP to reload config", running.len());

            // Listen for SIGHUP
            let mut sighup = tokio::signal::unix::signal(
                tokio::signal::unix::SignalKind::hangup()
            ).expect("Failed to register SIGHUP handler");

            loop {
                tokio::select! {
                    _ = sighup.recv() => {
                        info!("[reload] SIGHUP received, reloading configuration from {}", config_path.display());
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
