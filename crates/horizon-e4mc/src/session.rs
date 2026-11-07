//! The relay session, on its own thread inside a small tokio runtime: fetch
//! a relay from the broker, connect over QUIC, open the control stream,
//! publish the assigned domain, then bridge every stream the relay opens
//! (one per guest) to the world's loopback port, byte for byte. e4mc's
//! `QuiclimeSession` is the reference; the shape is the same as
//! horizon-steam's share and horizon-friends' host: started in the
//! background, polled by the game.
use crate::control::{self, Clientbound, Decoder, Serverbound};
use quinn::{Endpoint, RecvStream, SendStream};
use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc::Sender,
    },
    time::Duration,
};
use tokio::{io::AsyncWriteExt, net::TcpStream, sync::watch};

/// The e4mc broker hands out the least loaded relay.
pub const BROKER_URL: &str = "https://broker.e4mc.link/getBestRelay";
/// The ALPN both ends of the relay session speak.
const ALPN: &[u8] = b"quiclime";
/// How many guest streams the session advertises (the Java client's 512;
/// the integrated server's own limit of 8 is what actually gates).
const MAX_STREAMS: u32 = 512;
/// QUIC keep-alive: the relay's own is 5 s, so ours must be shorter or the
/// session dies during quiet periods.
const KEEP_ALIVE: Duration = Duration::from_secs(3);

pub struct SessionConfig {
    /// The world's loopback port (Open to LAN's).
    pub lan: SocketAddr,
    /// A specific relay, skipping the broker (a self-hosted quiclime, tests).
    pub relay: Option<(String, u16)>,
}

#[derive(Default)]
pub struct SessionState {
    pub domain: Option<String>,
    pub guests: usize,
    pub problem: Option<String>,
}

pub(crate) struct Shared {
    pub config: SessionConfig,
    pub events: Sender<crate::PublishEvent>,
    pub state: Arc<Mutex<SessionState>>,
    pub guests: Arc<AtomicUsize>,
    pub policy: Arc<Mutex<crate::GuestPolicy>>,
    pub stop: Arc<watch::Sender<bool>>,
}

impl Shared {
    fn emit(&self, event: crate::PublishEvent) {
        let _ = self.events.send(event);
    }
    pub fn stopped(&self) -> bool {
        *self.stop.borrow()
    }
    fn set_problem(&self, problem: String) {
        self.state.lock().unwrap().problem = Some(problem.clone());
        self.emit(crate::PublishEvent::Problem(problem));
    }
}

/// The thread body: one runtime, one session, until stopped or failed.
pub(crate) fn run(shared: Arc<Shared>) {
    // The broker is plain HTTPS and blocking; it runs before the runtime.
    let relay = shared.config.relay.clone().map(Ok).unwrap_or_else(|| {
        broker_relay().map_err(|e| {
            if !shared.stopped() {
                shared.set_problem(format!("the relay broker didn't answer: {e}"));
            }
            e
        })
    });
    let relay = match relay {
        Ok(relay) => relay,
        Err(_) => return,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("e4mc: tokio runtime");
    let stop = shared.stop.subscribe();
    runtime.block_on(async {
        // The endpoint must outlive the session: dropping it kills the
        // connections silently, without the close frame that tells the relay
        // to end its guests. It is released only after wait_idle.
        // The relay's address decides the socket family: on IPv6-only mobile
        // networks (DNS64/NAT64, as Jio hands out) the name resolves to an IPv6
        // address and an IPv4 socket can't reach it ("invalid remote address").
        let address = match resolve_relay(&relay).await {
            Ok(address) => address,
            Err(error) => {
                if !shared.stopped() {
                    shared.set_problem(error.to_string());
                }
                return;
            }
        };
        let endpoint = match make_endpoint(address.is_ipv6()) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                if !shared.stopped() {
                    shared.set_problem(error.to_string());
                }
                return;
            }
        };
        let result = session(&shared, relay, address, stop.clone(), &endpoint).await;
        if let Err(error) = result
            && !shared.stopped()
        {
            shared.set_problem(error.to_string());
        }
        if shared.stopped() {
            // The close frame has to reach the relay before the socket dies.
            let _ = tokio::time::timeout(Duration::from_secs(3), endpoint.wait_idle()).await;
        }
    });
    log::info!("e4mc: session ended, shutting the runtime down");
    runtime.shutdown_timeout(Duration::from_secs(2));
}

fn broker_relay() -> Result<(String, u16), String> {
    let response = ureq::get(BROKER_URL)
        .set("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|e| e.to_string())?;
    let broker: BrokerResponse = response
        .into_json()
        .map_err(|e| format!("the broker's answer made no sense: {e}"))?;
    Ok((broker.host, broker.port))
}

#[derive(serde::Deserialize)]
struct BrokerResponse {
    #[allow(dead_code)]
    id: String,
    host: String,
    port: u16,
}

/// The relay's socket address: IPv4 when the name offers one (the common case), else
/// the first IPv6 one (IPv6-only networks reach IPv4 relays through NAT64).
async fn resolve_relay((host, port): &(String, u16)) -> anyhow::Result<SocketAddr> {
    let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), *port))
        .await?
        .collect();
    addresses
        .iter()
        .copied()
        .find(SocketAddr::is_ipv4)
        .or_else(|| addresses.first().copied())
        .ok_or_else(|| anyhow::anyhow!("the relay's name didn't resolve"))
}

fn make_endpoint(ipv6: bool) -> anyhow::Result<Endpoint> {
    let mut root = rustls::RootCertStore::empty();
    root.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut tls = rustls::ClientConfig::builder()
        .with_root_certificates(root)
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(KEEP_ALIVE));
    transport.max_concurrent_bidi_streams(MAX_STREAMS.into());
    transport
        .stream_receive_window((1_250_000u32).into())
        .receive_window((8_000_000u32).into());
    let mut client = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(tls)?,
    ));
    client.transport_config(Arc::new(transport));
    let bind = if ipv6 { "[::]:0" } else { "0.0.0.0:0" };
    let mut endpoint = Endpoint::client(bind.parse()?)?;
    endpoint.set_default_client_config(client);
    Ok(endpoint)
}

async fn session(
    shared: &Arc<Shared>,
    (host, port): (String, u16),
    address: SocketAddr,
    mut stop: watch::Receiver<bool>,
    endpoint: &Endpoint,
) -> anyhow::Result<()> {
    log::info!("e4mc: using relay {host}:{port} at {address}");

    let connecting = endpoint.connect(address, host.as_str())?;
    // A relay that doesn't answer hangs the handshake forever without this.
    let connection = tokio::select! {
        _ = stop.changed() => return Ok(()),
        connection = tokio::time::timeout(Duration::from_secs(10), connecting) => {
            connection.map_err(|_| anyhow::anyhow!("the relay didn't answer in 10 seconds"))??
        }
    };
    log::info!("e4mc: relay session open");

    // The control channel: one bidirectional stream carries the domain
    // negotiation and the relay's broadcasts. It stays open.
    let (mut send_control, mut recv_control) = connection.open_bi().await?;
    write_message(&mut send_control, &Serverbound::ProbeCapabilities).await?;
    write_message(&mut send_control, &Serverbound::RequestDomainAssignment).await?;
    let mut decoder = Decoder::default();
    let mut buf = vec![0u8; 4096];
    let domain = 'domain: loop {
        let read = tokio::select! {
            _ = stop.changed() => return Ok(()),
            read = recv_control.read(&mut buf) => match read {
                Ok(Some(read)) => read,
                Ok(None) => anyhow::bail!("the relay closed the control channel before assigning a domain"),
                Err(e) => return Err(e.into()),
            },
        };
        for message in decoder.feed(&buf[..read]).map_err(anyhow::Error::msg)? {
            if let Clientbound::DomainAssignmentComplete(domain) = message {
                break 'domain domain;
            }
        }
    };
    log::info!("e4mc: domain assigned: {domain}");
    shared.state.lock().unwrap().domain = Some(domain.clone());
    shared.emit(crate::PublishEvent::Published { domain });

    // Serving: the relay's broadcasts on the control channel, and one
    // bidirectional stream per guest.
    loop {
        tokio::select! {
            _ = stop.changed() => {
                log::info!("e4mc: stop asked: closing the relay session");
                connection.close(0u32.into(), b"horizon stopped sharing");
                return Ok(());
            }
            read = recv_control.read(&mut buf) => {
                let read = match read {
                    Ok(Some(read)) => read,
                    Ok(None) => anyhow::bail!("the relay closed the session"),
                    Err(e) => return Err(e.into()),
                };
                for message in decoder.feed(&buf[..read]).map_err(anyhow::Error::msg)? {
                    match message {
                        Clientbound::RequestMessageBroadcast(message) => {
                            shared.emit(crate::PublishEvent::Broadcast(message))
                        }
                        Clientbound::DomainAssignmentComplete(domain) => {
                            shared.state.lock().unwrap().domain = Some(domain);
                        }
                        _ => {}
                    }
                }
            }
            stream = connection.accept_bi() => {
                let (send, recv) = stream?;
                tokio::spawn(bridge(shared.clone(), send, recv));
            }
        }
    }
}

async fn write_message(send: &mut SendStream, message: &Serverbound) -> anyhow::Result<()> {
    send.write_all(&control::frame(&message.to_json())).await?;
    Ok(())
}

/// Returns the guest count to zero when the bridge ends, however it ends:
/// the runtime cancels the task on shutdown, and a plain decrement after
/// the pump would be skipped with it.
struct GuestGuard {
    shared: Arc<Shared>,
}

impl Drop for GuestGuard {
    fn drop(&mut self) {
        self.shared.guests.fetch_sub(1, Ordering::Relaxed);
        self.shared.state.lock().unwrap().guests -= 1;
        self.shared.emit(crate::PublishEvent::GuestLeft);
    }
}

/// How long a guest has to say who they are before the stream is dropped.
const OPENING_TIMEOUT: Duration = Duration::from_secs(10);
/// A handshake and a login start never need more (a 255-char name, a
/// 255-char host): anything longer is not a Minecraft client.
const OPENING_LIMIT: usize = 4096;

/// What a guest's first bytes are.
enum Opening {
    /// A status ping (the server list): passed through, never counted.
    Status,
    /// A login (or a transfer) by this player name.
    Login(String),
    /// Not a Minecraft handshake.
    Other,
}

fn read_varint(buf: &[u8], pos: &mut usize) -> Option<i32> {
    let mut value = 0i32;
    for i in 0..5 {
        let b = *buf.get(*pos)?;
        *pos += 1;
        value |= i32::from(b & 0x7F) << (7 * i);
        if b & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}
fn write_varint(mut v: u32, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}
fn read_string(buf: &[u8], pos: &mut usize) -> Option<String> {
    let len = usize::try_from(read_varint(buf, pos)?).ok()?;
    let bytes = buf.get(*pos..*pos + len)?;
    *pos += len;
    String::from_utf8(bytes.to_vec()).ok()
}
/// The next framed packet: (id, body). None until the bytes are all there.
fn take_packet(buf: &[u8], pos: &mut usize) -> Option<(i32, Vec<u8>)> {
    let mut p = *pos;
    let len = usize::try_from(read_varint(buf, &mut p).filter(|&l| l > 0)?).ok()?;
    let body = buf.get(p..p + len)?;
    let mut q = 0;
    let id = read_varint(body, &mut q)?;
    *pos = p + len;
    Some((id, body[q..].to_vec()))
}

/// Parse what `buf` holds so far: None while more bytes are needed.
fn parse_opening(buf: &[u8]) -> Option<Opening> {
    let mut pos = 0;
    // Before the whole handshake is in: a first packet that is not id 0 is
    // not a Minecraft client (an HTTP probe, say), no need to wait for it.
    {
        let mut p = 0;
        match read_varint(buf, &mut p) {
            Some(len) if len <= 0 => return Some(Opening::Other),
            Some(_) if buf.get(p).is_some_and(|&id| id != 0) => return Some(Opening::Other),
            _ => {}
        }
    }
    let (id, body) = take_packet(buf, &mut pos)?;
    if id != 0 {
        return Some(Opening::Other);
    }
    let mut p = 0;
    let _protocol = read_varint(&body, &mut p)?;
    let _host = read_string(&body, &mut p)?;
    if body.len() < p + 2 {
        return Some(Opening::Other);
    }
    p += 2;
    match read_varint(&body, &mut p) {
        Some(1) => Some(Opening::Status),
        Some(2 | 3) => {
            let (id, body) = take_packet(buf, &mut pos)?;
            if id != 0 {
                return Some(Opening::Other);
            }
            let mut p = 0;
            let name = read_string(&body, &mut p)?;
            Some(Opening::Login(name))
        }
        _ => Some(Opening::Other),
    }
}

/// Read the guest's opening packets into `buf` (kept to replay to the world).
async fn read_opening(recv: &mut RecvStream, buf: &mut Vec<u8>) -> Opening {
    let read = async {
        let mut chunk = [0u8; 1024];
        loop {
            if let Some(opening) = parse_opening(buf) {
                return opening;
            }
            if buf.len() > OPENING_LIMIT {
                return Opening::Other;
            }
            match recv.read(&mut chunk).await {
                Ok(Some(n)) => buf.extend_from_slice(&chunk[..n]),
                _ => return Opening::Other,
            }
        }
    };
    tokio::time::timeout(OPENING_TIMEOUT, read)
        .await
        .unwrap_or(Opening::Other)
}

/// A login-state disconnect with `reason` (ClientboundLoginDisconnectPacket:
/// the text as JSON, every version alike).
fn login_disconnect(reason: &str) -> Vec<u8> {
    let json = serde_json::json!({ "text": reason, "color": "red" }).to_string();
    let mut body = vec![0u8];
    write_varint(json.len() as u32, &mut body);
    body.extend_from_slice(json.as_bytes());
    let mut out = Vec::with_capacity(body.len() + 3);
    write_varint(body.len() as u32, &mut out);
    out.extend_from_slice(&body);
    out
}

/// One guest: the relay opened a stream, bridge it to the world's port
/// until either side ends it. Half-closes carry through: a guest's EOF
/// reaches the server and the server's EOF reaches the guest. A login is
/// held at the door first: the policy says who gets in, status pings pass
/// without counting as guests.
async fn bridge(shared: Arc<Shared>, mut send: SendStream, mut recv: RecvStream) {
    let mut opening = Vec::new();
    let _guard = match read_opening(&mut recv, &mut opening).await {
        Opening::Status => None,
        Opening::Other => {
            let _ = send.finish();
            return;
        }
        Opening::Login(name) => {
            let playing = shared.guests.load(Ordering::Relaxed);
            let verdict = shared
                .policy
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .admit(&name, playing);
            if let Err(reason) = verdict {
                log::info!("e4mc: refused {name}: {reason}");
                let _ = send.write_all(&login_disconnect(&reason)).await;
                let _ = send.finish();
                // The relay delivers what was written before the stream
                // closes; a moment lets the client read it.
                tokio::time::sleep(Duration::from_millis(400)).await;
                shared.emit(crate::PublishEvent::GuestRefused { name, reason });
                return;
            }
            shared.guests.fetch_add(1, Ordering::Relaxed);
            shared.state.lock().unwrap().guests += 1;
            shared.emit(crate::PublishEvent::GuestJoined);
            Some(GuestGuard {
                shared: shared.clone(),
            })
        }
    };
    let result = match TcpStream::connect(shared.config.lan).await {
        Ok(server) => {
            let (mut server_read, mut server_write) = server.into_split();
            let to_server = async {
                let copied = match server_write.write_all(&opening).await {
                    Ok(()) => tokio::io::copy(&mut recv, &mut server_write)
                        .await
                        .map(|n| n + opening.len() as u64),
                    Err(e) => Err(e),
                };
                let _ = server_write.shutdown().await;
                copied
            };
            let to_guest = tokio::io::copy(&mut server_read, &mut send);
            let (a, b) = tokio::join!(to_server, to_guest);
            let _ = send.finish();
            match (a, b) {
                (Ok(_), Ok(_)) => Ok(()),
                (Err(e), _) | (_, Err(e)) => Err(anyhow::anyhow!(e)),
            }
        }
        Err(e) => {
            let _ = send.finish();
            Err(anyhow::anyhow!(e))
        }
    };
    if let Err(e) = result {
        log::info!("e4mc: a guest's bridge ended: {e}");
    }
}

#[cfg(test)]
mod opening_tests {
    use super::*;

    fn packet(id: u8, body: &[u8]) -> Vec<u8> {
        let mut inner = vec![id];
        inner.extend_from_slice(body);
        let mut out = Vec::new();
        write_varint(inner.len() as u32, &mut out);
        out.extend_from_slice(&inner);
        out
    }
    fn string(s: &str, out: &mut Vec<u8>) {
        write_varint(s.len() as u32, out);
        out.extend_from_slice(s.as_bytes());
    }
    fn handshake(next: u8) -> Vec<u8> {
        let mut body = Vec::new();
        write_varint(773, &mut body);
        string("abc.e4mc.link", &mut body);
        body.extend_from_slice(&25565u16.to_be_bytes());
        body.push(next);
        packet(0, &body)
    }

    #[test]
    fn a_login_names_the_player_and_a_ping_passes() {
        let mut bytes = handshake(2);
        let mut login = Vec::new();
        string("Steve", &mut login);
        login.extend_from_slice(&[0; 16]);
        bytes.extend(packet(0, &login));
        assert!(matches!(parse_opening(&bytes), Some(Opening::Login(n)) if n == "Steve"));
        // Half a login start: wait for more.
        assert!(parse_opening(&bytes[..bytes.len() - 10]).is_none());
        assert!(matches!(parse_opening(&handshake(1)), Some(Opening::Status)));
        assert!(matches!(parse_opening(b"GET / HTTP/1.1\r\n"), Some(Opening::Other)));
    }

    #[test]
    fn the_policy_keeps_the_list_and_the_limit() {
        let mut policy = crate::GuestPolicy {
            max_players: Some(2),
            ..Default::default()
        };
        assert!(policy.admit("Alex", 1).is_ok());
        assert_eq!(policy.admit("Alex", 2), Err(crate::GuestPolicy::FULL.to_owned()));
        policy.allowed = Some(["Steve".to_owned()].into_iter().collect());
        assert!(policy.admit("steve", 0).is_ok());
        assert!(policy.admit("Alex", 0).is_err());
        let d = login_disconnect("no");
        assert_eq!(d[1], 0, "login disconnect is packet 0");
    }
}
