use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::net::TcpListener;
use log::*;
use tokio::time;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use socket2::TcpKeepalive;

use crate::dns::DNSResolve;

const MAX_CONNECTIONS: usize = 1024;
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
const KEEPALIVE_IDLE: Duration = Duration::from_secs(60);

fn set_keepalive(stream: &TcpStream) {
    let sock = socket2::SockRef::from(stream);
    let keepalive = TcpKeepalive::new()
        .with_time(KEEPALIVE_IDLE)
        .with_interval(KEEPALIVE_INTERVAL);
    if let Err(e) = sock.set_tcp_keepalive(&keepalive) {
        warn!("Failed to set TCP keepalive: {:?}", e);
    }
}

struct TCPPeerPair {
    client: TcpStream,
    remote: String,
}

impl TCPPeerPair {
    async fn run(mut self) {
        set_keepalive(&self.client);

        let mut outbound = match TcpStream::connect(self.remote.clone()).await {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to connect to {}: {:?}", self.remote, e);
                let _ = self.client.shutdown().await;
                return;
            }
        };

        set_keepalive(&outbound);

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
    dns: Vec<String>
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
        let sem = Arc::new(Semaphore::new(MAX_CONNECTIONS));
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
                                            warn!("Connection limit reached ({}), rejecting {}", MAX_CONNECTIONS, peer_addr);
                                            drop(inbound);
                                            continue;
                                        }
                                    };
                                    let client = TCPPeerPair{
                                        client: inbound,
                                        remote: host.clone()
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
    remote:&String) -> Result<(), std::io::Error>
{
    let mut server = TCPProxy {
        addr: &local,
        remote: &remote,
        dns: vec![]
    };
    info!("Start service in TCP mode {}->{}", server.addr, server.remote);
    return server.run().await;
}