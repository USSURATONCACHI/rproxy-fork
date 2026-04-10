use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::net::TcpListener;
use log::*;
use tokio::time;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use socket2::TcpKeepalive;

use tokio_util::sync::CancellationToken;

use crate::dns::DNSResolve;
use rproxy::Settings;

fn set_keepalive(stream: &TcpStream, settings: &Settings) {
    let sock = socket2::SockRef::from(stream);
    let keepalive = TcpKeepalive::new()
        .with_time(Duration::from_secs(settings.keepalive_idle))
        .with_interval(Duration::from_secs(settings.keepalive_interval));
    if let Err(e) = sock.set_tcp_keepalive(&keepalive) {
        warn!("Failed to set TCP keepalive: {:?}", e);
    }
}

struct TCPPeerPair {
    client: TcpStream,
    remote: String,
    settings: Arc<Settings>,
}

impl TCPPeerPair {
    async fn run(mut self) {
        set_keepalive(&self.client, &self.settings);

        let mut outbound = match TcpStream::connect(self.remote.clone()).await {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to connect to {}: {:?}", self.remote, e);
                let _ = self.client.shutdown().await;
                return;
            }
        };

        set_keepalive(&outbound, &self.settings);

        match tokio::io::copy_bidirectional(&mut self.client, &mut outbound).await {
            Ok((tx, rx)) => {
                debug!("Connection to {} closed (tx={}, rx={})", self.remote, tx, rx);
            }
            Err(e) => {
                debug!("Connection to {} ended: {:?}", self.remote, e);
            }
        }

        let _ = outbound.shutdown().await;
        let _ = self.client.shutdown().await;
    }
}
struct TCPProxy<'a> {
    addr: &'a String,
    remote: &'a String,
    dns: Vec<String>,
    settings: Arc<Settings>,
    cancel: CancellationToken,
}

impl<'a> DNSResolve<'a> for TCPProxy<'a> {
    fn remote(&self) -> &String{
        self.remote
    }
    fn dns(&self) -> &Vec<String>{
        &self.dns
    }
    fn reset_dns(&mut self,d: &Vec<String>) -> usize {
        self.dns = d.to_vec();
        self.dns.len()
    }

}
impl<'a> TCPProxy<'a> {

    async fn run(& mut self) -> Result<(), std::io::Error> {
        self.resolve().await.unwrap();
        let mut time_out1 = time::interval(tokio::time::Duration::from_secs(30));
        let mut host = self.dns[0].clone();
        let max_conn = self.settings.max_connections;
        let sem = Arc::new(Semaphore::new(max_conn));
        match TcpListener::bind(self.addr).await {
            Ok(listener) => {
                loop{
                    tokio::select!{
                        x = listener.accept() => {
                            match x {
                                Ok((inbound, peer_addr)) => {
                                    let permit = match sem.clone().try_acquire_owned() {
                                        Ok(p) => p,
                                        Err(_) => {
                                            warn!("Connection limit reached ({}), rejecting {}", max_conn, peer_addr);
                                            drop(inbound);
                                            continue;
                                        }
                                    };
                                    let client = TCPPeerPair{
                                        client: inbound,
                                        remote: host.clone(),
                                        settings: self.settings.clone(),
                                    };
                                    tokio::spawn(async move {
                                        client.run().await;
                                        drop(permit);
                                    });
                                },
                                Err(e1) => {
                                    error!("Failed to accept new connection from {}, err={:?}", self.addr, e1);
                                }
                            }
                        },
                        _ = time_out1.tick() => {
                            self.resolve().await.unwrap();
                            host = self.dns[0].clone();
                        },
                        _ = self.cancel.cancelled() => {
                            info!("TCP proxy {} shutting down (existing connections will drain)", self.addr);
                            break;
                        }
                    }
                }
            },
            Err(e) => {
                error!("Failed to bind interface {}, err={:?}", self.addr, e);
            }
        }

        Ok(())
    }
}

pub async fn tcp_proxy(local: &String,
    remote: &String, settings: Arc<Settings>, cancel: CancellationToken) -> Result<(), std::io::Error>
{
    info!("Start service in TCP mode {}->{} (max_connections={}, keepalive_idle={}s, keepalive_interval={}s)",
        local, remote, settings.max_connections, settings.keepalive_idle, settings.keepalive_interval);
    let mut server = TCPProxy {
        addr: &local,
        remote: &remote,
        dns: vec![],
        settings,
        cancel,
    };
    return server.run().await;
}
