use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::future::try_join;
use futures::StreamExt;
use log::*;
use std::collections::HashMap;
use std::io::ErrorKind::Other;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::net::UdpSocket;
use tokio::time;

use tokio_util::sync::CancellationToken;

use crate::dns::DNSResolve;
use rproxy::Settings;

#[allow(dead_code)]
enum MessageType {
    Data,
    Terminate,
    DNS,
}

type Tx = UnboundedSender<(SocketAddr, Vec<u8>, MessageType)>;
type Rx = UnboundedReceiver<(SocketAddr, Vec<u8>, MessageType)>;

struct UDPPeerPair {
    client: SocketAddr,
    remote: SocketAddr,
    send: Tx,
    recv: Rx,
}

impl UDPPeerPair {
    async fn run(mut self) -> Result<(), std::io::Error> {
        // The relay socket must match the address family of the remote,
        // otherwise send_to() fails with EAFNOSUPPORT ("Address family not
        // supported by protocol") — e.g. an IPv4 socket cannot send to an
        // IPv6 destination.
        let bind_addr = if self.remote.is_ipv6() {
            "[::]:0"
        } else {
            "0.0.0.0:0"
        };
        let socket = Arc::new(UdpSocket::bind(bind_addr).await?);
        // let (mut socket_recv, mut socket_send) = socket.split();
        let socket_recv = socket.clone();
        let socket_send = socket.clone();
        let client_peer = self.client;
        let _tx = self.send.clone();
        let remote_addr = self.remote;
        let (ctrl_tx, mut ctrl_rx) = unbounded::<MessageType>();

        let client_to_remote_proc = async move {
            // let mut buf: Vec<u8> = vec![0;1024*10];
            loop {
                if let Some((_peer, buf, msg_type)) = self.recv.next().await {
                    match msg_type {
                        MessageType::Terminate => {
                            debug!(
                                "{}:{} sends TERMINATE signal",
                                client_peer.ip(),
                                client_peer.port()
                            );
                            let _ = ctrl_tx.unbounded_send(MessageType::Terminate);
                            break;
                        }
                        _ => {}
                    }
                    // debug!("Forward {} bytes from {}", buf.len(), _peer);

                    match socket_send.send_to(&buf[..], &remote_addr).await {
                        Ok(_sz) => {}
                        Err(e) => {
                            warn!(
                                "{}:{} send_to failed: {:?}",
                                client_peer.ip(),
                                client_peer.port(),
                                e
                            );
                            let _ = ctrl_tx.unbounded_send(MessageType::Terminate);
                            break;
                        }
                    }
                } else {
                    // Channel closed — sender was dropped (client timed out or cleaned up)
                    debug!("{}:{} channel closed", client_peer.ip(), client_peer.port());
                    let _ = ctrl_tx.unbounded_send(MessageType::Terminate);
                    break;
                }
            }
            Ok(())
        };
        let remote_to_client_proc = async move {
            let mut buf: Vec<u8> = vec![0; 1024 * 10];
            loop {
                tokio::select! {
                    x = socket_recv.recv_from(&mut buf) => {
                        if let Ok((_size, _peer)) = x {
                            // debug!("Recv {} bytes to {}", _size, client_peer);
                            match _tx.unbounded_send((client_peer, Vec::from(&buf[.._size]), MessageType::Data)) {
                                Ok(_sz) => {

                                },
                                Err(_e) => {
                                    return Err(std::io::Error::from(Other));
                                }
                            }
                        }
                    },
                    y = ctrl_rx.next() => {
                        if let Some(msg_type) = y{
                            match msg_type{
                                MessageType::Terminate => {
                                    debug!("{}:{} recvs TERMINATE signal", client_peer.ip(), client_peer.port());
                                    break;
                                },
                                _ =>{

                                }
                            }
                        } else {
                            // ctrl channel closed — other side exited
                            debug!("{}:{} ctrl channel closed", client_peer.ip(), client_peer.port());
                            break;
                        }
                    }
                }
            }
            Ok(())
        };
        let _ = try_join(client_to_remote_proc, remote_to_client_proc).await;
        debug!("{}:{} exits", client_peer.ip(), client_peer.port());
        Ok(())
    }
}
struct UDPProxy<'a> {
    addr: &'a String,
    remote: &'a String,
    dns: Vec<String>,
    client_tunnels: HashMap<SocketAddr, (Tx, SystemTime)>,
    settings: Arc<Settings>,
    cancel: CancellationToken,
}

impl<'a> DNSResolve<'a> for UDPProxy<'a> {
    fn remote(&self) -> &String {
        &self.remote
    }

    fn dns(&self) -> &Vec<String> {
        &self.dns
    }

    fn reset_dns(&mut self, d: &Vec<String>) -> usize {
        self.dns = d.clone();
        d.len()
    }
}

impl<'a> UDPProxy<'a> {
    async fn run(&mut self) -> Result<(), std::io::Error> {
        // Pre-flight: the remote must resolve to a usable IP:port before we
        // start serving. UDP forwarding needs a concrete SocketAddr, so a
        // hostname that doesn't resolve (or a bad literal) is a fatal startup
        // error rather than an endless retry loop.
        self.resolve().await?;
        let initial_remote = self.dns.first().cloned().unwrap_or_default();
        if initial_remote.parse::<SocketAddr>().is_err() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "remote '{}' could not be resolved to a usable IP:port address",
                    self.remote
                ),
            ));
        }

        // Refuse to start if the local address can't be bound (port in use,
        // permission denied, ...) instead of panicking.
        let socket = Arc::new(UdpSocket::bind(&self.addr).await.map_err(|e| {
            std::io::Error::new(e.kind(), format!("cannot bind {}: {}", self.addr, e))
        })?);
        info!("Listening on {}", socket.local_addr()?);

        let mut dns_timeout = time::interval(tokio::time::Duration::from_secs(30));
        let mut _remote = initial_remote;
        // let (mut socket_recv, mut socket_send) = socket.split();
        let socket_recv = socket.clone();
        let socket_send = socket.clone();
        let cancel_token = self.cancel.clone();
        let (tx, mut rx) = unbounded::<(SocketAddr, Vec<u8>, MessageType)>();
        let remote_to_client_proc = async move {
            loop {
                if let Some((peer, buf, _msg_type)) = rx.next().await {
                    // debug!("Forward {} bytes to {}", buf.len(), peer);
                    match socket_send.send_to(&buf[..], &peer).await {
                        Ok(_sz) => {}
                        Err(e) => {
                            return Err(e);
                        }
                    }
                } else {
                    break;
                }
            }
            Ok(())
        };
        // let mut client_run_procs: Vec<JoinHandle<Result<(), io::Error>> > = Vec::new();

        let client_to_proxy_proc = async move {
            let mut buf: Vec<u8> = vec![0; 1024 * 256];
            let empty: Vec<u8> = vec![0; 0];
            // let mut client_tunnels:HashMap<SocketAddr, (Tx, SystemTime)> = HashMap::new();
            let mut time_out1 = time::interval(tokio::time::Duration::from_secs(5));
            loop {
                tokio::select! {
                    data = socket_recv.recv_from(&mut buf) => {
                        match data {
                            Ok((size, peer)) => {
                                // Check if tunnel still has a working channel
                                let needs_new_tunnel = match self.client_tunnels.get(&peer) {
                                    Some((tx, _)) => tx.is_closed(),
                                    None => true,
                                };

                                if needs_new_tunnel {
                                    // Remove stale entry if it exists
                                    if self.client_tunnels.contains_key(&peer) {
                                        info!("Replacing stale tunnel for {}:{}", peer.ip(), peer.port());
                                        self.client_tunnels.remove(&peer);
                                    }

                                    let max_tunnels = self.settings.max_client_tunnels;
                                    if self.client_tunnels.len() >= max_tunnels {
                                        warn!("Client tunnel limit reached ({}), rejecting {}:{}", max_tunnels, peer.ip(), peer.port());
                                        continue;
                                    }

                                    // The UDP path needs a resolved IP:port. If the
                                    // remote isn't (yet) a usable SocketAddr, drop this
                                    // client instead of panicking the whole proxy.
                                    let remote_sa = match _remote.parse::<SocketAddr>() {
                                        Ok(sa) => sa,
                                        Err(e) => {
                                            warn!("Cannot route {}:{} — remote {:?} is not a usable address: {}",
                                                peer.ip(), peer.port(), _remote, e);
                                            continue;
                                        }
                                    };

                                    info!("New client {}:{} is added", peer.ip(), peer.port());
                                    let (_s, _r) = unbounded::<(SocketAddr, Vec<u8>,  MessageType)>();
                                    self.client_tunnels.insert(peer, (_s, SystemTime::now()));
                                    let c = UDPPeerPair {
                                        client : peer,
                                        remote: remote_sa,
                                        send: tx.clone(),
                                        recv: _r
                                    };
                                    tokio::spawn(c.run());
                                }

                                if let Some((tx, tm)) = self.client_tunnels.get_mut(&peer) {
                                    if let Err(_) = tx.unbounded_send((peer, Vec::from(&buf[..size]), MessageType::Data)) {
                                        warn!("Failed to send data to tunnel for {}:{}, removing", peer.ip(), peer.port());
                                        self.client_tunnels.remove(&peer);
                                    } else {
                                        *tm = SystemTime::now();
                                    }
                                }
                            },
                            Err(e) => {
                                warn!("recv_from {:?} returned error {}, {:?}", socket_recv, e, e);
                                break;
                            }
                        }
                    },
                    _ = dns_timeout.tick() => {
                        self.resolve().await.unwrap();
                        _remote = self.dns[0].clone();
                    },
                    _ = time_out1.tick() =>{
                        debug!("Tick");
                        let mut tbd: Vec<SocketAddr> = Vec::new();
                        for (k, v) in (&mut self.client_tunnels).iter(){
                            let sec = v.1.elapsed().unwrap().as_secs();
                            if sec > 120{
                                info!("Client {}:{} is timeout({}s)", k.ip(), k.port(), sec);
                                let _ = v.0.unbounded_send((k.clone(), empty.clone(), MessageType::Terminate));
                                tbd.push(k.to_owned());
                            } else {
                                debug!("Client {}:{} is good. ({}s)", k.ip(), k.port(), sec);
                            }
                        }

                        for k in tbd{
                            self.client_tunnels.remove(&k);
                        }

                    },
                    _ = cancel_token.cancelled() => {
                        info!("UDP proxy {} shutting down (existing tunnels will drain)", self.addr);
                        // Send terminate to all active tunnels
                        for (k, (tx, _)) in self.client_tunnels.iter() {
                            let _ = tx.unbounded_send((k.clone(), empty.clone(), MessageType::Terminate));
                        }
                        self.client_tunnels.clear();
                        break;
                    }
                }
            }
            Ok(())
        };
        // client_to_proxy_proc.await;
        try_join(client_to_proxy_proc, remote_to_client_proc)
            .await
            .unwrap();
        Ok(())
    }
}

pub async fn udp_proxy(
    local: &String,
    remote: &String,
    settings: Arc<Settings>,
    cancel: CancellationToken,
) -> Result<(), std::io::Error> {
    info!(
        "Start service in UDP mode {}->{} (max_client_tunnels={})",
        local, remote, settings.max_client_tunnels
    );
    let mut server = UDPProxy {
        addr: &local,
        remote: &remote,
        dns: vec![],
        client_tunnels: HashMap::new(),
        settings,
        cancel,
    };
    return server.run().await;
}
