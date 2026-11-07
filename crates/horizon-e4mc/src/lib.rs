//! Open the world to anyone, anywhere: e4mc ported to Horizon.
//!
//! e4mc is the Minecraft mod that publishes an open-to-LAN world through its
//! public relay: the host connects out to a quiclime relay over QUIC, the
//! relay assigns the world a public domain, and every guest who joins that
//! domain arrives as one bidirectional stream on the host's session, which
//! this crate bridges to the integrated server's loopback port byte for
//! byte. No port forwarding, no Steam, no account: the address is the
//! world's, and anyone with it can join — e4mc's own design ("Open a LAN
//! server to anyone, anywhere, anytime"), so the player is told what
//! publishing means before they press the button.
//!
//! Guests need nothing from this crate: the domain is a plain server
//! address that any client understands, Horizon's and vanilla's alike.
//! The reference implementation is e4mc-minecraft-architectury's
//! `QuiclimeSession` with [e4mc-quiclime] on the relay side; the wire
//! protocol (ALPN `quiclime`, varint-framed JSON control messages, one
//! stream per guest) is the same.
//!
//! [e4mc-quiclime]: https://github.com/vgskye/e4mc-quiclime
pub mod control;
pub mod session;

use session::{SessionConfig, SessionState, Shared};
use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread::JoinHandle,
};

/// How many guests the world carries at once (the integrated server's own
/// limit; the relay's stream budget is 512).
pub const MAX_GUESTS: usize = 8;

/// How publishing is set up.
pub struct PublishConfig {
    /// The integrated server's loopback address (Open to LAN's port).
    pub lan: SocketAddr,
    /// A specific relay, skipping the broker (a self-hosted quiclime or a
    /// test). None: the broker's least loaded relay.
    pub relay: Option<(String, u16)>,
}

/// Who the relay lets into the world (CS Launcher Plus "World hosting
/// settings"). The guest's login start names the player before any byte
/// reaches the world, so a refusal costs the world nothing: the guest sees
/// the reason on the login screen, the integrated server never hears of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuestPolicy {
    /// Players the relay carries at once (None: no limit here; the world's
    /// own player limit still applies).
    pub max_players: Option<usize>,
    /// None: anyone with the address. Some: only these names (compared case
    /// insensitively) log in; everyone else is refused with `closed_reason`.
    pub allowed: Option<std::collections::BTreeSet<String>>,
    /// What a guest the list keeps out is told.
    pub closed_reason: String,
}

impl GuestPolicy {
    /// The reason a guest refused by the list sees when none was set.
    pub const DEFAULT_CLOSED: &'static str = "This world is open to the host's friends only.";
    /// The reason a guest over the player limit sees.
    pub const FULL: &'static str = "The world is full.";

    /// Whether `name` may log in now, else the refusal text.
    pub fn admit(&self, name: &str, playing: usize) -> Result<(), String> {
        if self.max_players.is_some_and(|max| playing >= max) {
            return Err(Self::FULL.to_owned());
        }
        if let Some(allowed) = &self.allowed
            && !allowed.iter().any(|n| n.eq_ignore_ascii_case(name))
        {
            return Err(if self.closed_reason.is_empty() {
                Self::DEFAULT_CLOSED.to_owned()
            } else {
                self.closed_reason.clone()
            });
        }
        Ok(())
    }
}

/// What the game shows.
#[derive(Clone, Debug, Default)]
pub struct PublishState {
    /// The world's public address (`<name>.e4mc.link`), once assigned.
    pub domain: Option<String>,
    /// Guests being carried right now.
    pub guests: usize,
    /// Why publishing isn't serving, when the player asked for it.
    pub problem: Option<String>,
}

/// What the publisher reports, polled like a tick.
#[derive(Debug)]
pub enum PublishEvent {
    /// The world's public address is ready.
    Published { domain: String },
    /// A note the relay asked every host to show its players.
    Broadcast(String),
    /// A guest's stream is being carried.
    GuestJoined,
    /// A guest was refused at the relay: who, and the reason they saw.
    GuestRefused { name: String, reason: String },
    /// The guest's stream ended.
    GuestLeft,
    /// The session failed (text for the player).
    Problem(String),
}

struct Inner {
    config: PublishConfig,
    events: Mutex<Sender<PublishEvent>>,
    state: Arc<Mutex<SessionState>>,
    guests: Arc<std::sync::atomic::AtomicUsize>,
    policy: Arc<Mutex<GuestPolicy>>,
    stop: Arc<tokio::sync::watch::Sender<bool>>,
    stopped: AtomicBool,
    thread: Mutex<Option<JoinHandle<()>>>,
}

/// The publisher, like `horizon_friends::Host`: started in the background,
/// the game polls it.
pub struct Publish {
    inner: Arc<Inner>,
    events: Receiver<PublishEvent>,
}

impl Publish {
    /// Start publishing: reach the relay, publish the domain (a few seconds
    /// at most), start serving guests.
    pub fn start(config: PublishConfig) -> Result<Self, String> {
        let (tx, events) = channel();
        let (stop, _) = tokio::sync::watch::channel(false);
        let state = Arc::new(Mutex::new(SessionState::default()));
        let inner = Arc::new(Inner {
            config,
            events: Mutex::new(tx),
            state: state.clone(),
            guests: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            policy: Arc::new(Mutex::new(GuestPolicy::default())),
            stop: Arc::new(stop),
            stopped: AtomicBool::new(false),
            thread: Mutex::new(None),
        });
        let shared = Arc::new(Shared {
            config: SessionConfig {
                lan: inner.config.lan,
                relay: inner.config.relay.clone(),
            },
            events: inner.events.lock().unwrap().clone(),
            state,
            guests: inner.guests.clone(),
            policy: inner.policy.clone(),
            stop: inner.stop.clone(),
        });
        let thread = std::thread::Builder::new()
            .name("horizon-e4mc".into())
            .spawn({
                let shared = shared.clone();
                move || session::run(shared)
            })
            .map_err(|e| format!("e4mc publishing's thread could not start: {e}"))?;
        *inner.thread.lock().unwrap() = Some(thread);
        Ok(Self { inner, events })
    }

    pub fn state(&self) -> PublishState {
        let session = self.inner.state.lock().unwrap();
        PublishState {
            domain: session.domain.clone(),
            guests: self.inner.guests.load(Ordering::Relaxed),
            problem: session.problem.clone(),
        }
    }

    pub fn poll(&self) -> Vec<PublishEvent> {
        self.events.try_iter().collect()
    }

    /// The guests being carried right now.
    pub fn sessions(&self) -> usize {
        self.inner.guests.load(Ordering::Relaxed)
    }

    /// Who the relay lets in from now on; guests already playing stay.
    pub fn set_policy(&self, policy: GuestPolicy) {
        *self
            .inner
            .policy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = policy;
    }

    /// The policy in force.
    pub fn policy(&self) -> GuestPolicy {
        self.inner
            .policy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// End the publish: the session closes and the domain stops answering.
    /// Guests already playing are dropped with it: the domain's routing is
    /// the session, there is nothing to hand over.
    pub fn stop(&self) {
        if self.inner.stopped.swap(true, Ordering::Relaxed) {
            return;
        }
        let _ = self.inner.stop.send(true);
        if let Some(thread) = self.inner.thread.lock().unwrap().take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Publish {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    struct PrintLogger;
    impl log::Log for PrintLogger {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            eprintln!("[log] {}", record.args());
        }
        fn flush(&self) {}
    }
    use super::*;
    use std::time::Duration;

    /// Stop before anything happened: the state stays empty, no panic.
    #[test]
    fn a_stopped_publish_reports_nothing() {
        let publish = Publish::start(PublishConfig {
            lan: "127.0.0.1:25565".parse().unwrap(),
            // The broker is never reached: stopped first thing.
            relay: Some(("127.0.0.1".into(), 1)),
        })
        .unwrap();
        drop(publish);
    }

    /// The full chain against the real broker and relay: the domain is
    /// assigned, a guest joining it arrives at the world's port, bytes flow
    /// both ways, and stopping tears the session down. `--ignored`: only
    /// for manual runs, it needs the network and e4mc's public service.
    #[test]
    #[ignore = "reaches e4mc's public broker and relay"]
    fn the_relay_publishes_and_serves_a_guest_end_to_end() {
        use std::io::{Read, Write};
        let lan = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let lan_address = lan.local_addr().unwrap();
        let publish = Publish::start(PublishConfig {
            lan: lan_address,
            relay: None,
        })
        .unwrap();
        let domain = (|| {
            for _ in 0..150 {
                for event in publish.poll() {
                    if let PublishEvent::Published { domain } = event {
                        return Some(domain);
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            None
        })()
        .expect("no domain arrived");
        eprintln!("e4mc-test: domain {domain}, joining as a guest...");
        let _ = log::set_boxed_logger(Box::new(PrintLogger));
        log::set_max_level(log::LevelFilter::Info);
        assert!(domain.ends_with(".e4mc.link"), "{domain}");
        assert_eq!(publish.state().domain.as_deref(), Some(domain.as_str()));

        // A guest joins the domain like any vanilla client would: its first
        // bytes are the Minecraft handshake, whose virtual host names the
        // domain; the relay routes on it and forwards the packet as-is.
        let handshake = {
            fn varint(mut value: usize, out: &mut Vec<u8>) {
                loop {
                    let byte = (value & 0x7f) as u8;
                    value >>= 7;
                    if value == 0 {
                        out.push(byte);
                        break;
                    }
                    out.push(byte | 0x80);
                }
            }
            fn string(value: &str, out: &mut Vec<u8>) {
                varint(value.len(), out);
                out.extend_from_slice(value.as_bytes());
            }
            let mut body = vec![0x00]; // packet id: handshake
            varint(767, &mut body); // protocol
            string(&domain, &mut body); // the virtual host the client dialed
            body.extend_from_slice(&25565u16.to_be_bytes());
            varint(2, &mut body); // next state: login
            let mut packet = Vec::new();
            varint(body.len(), &mut packet);
            packet.extend_from_slice(&body);
            packet
        };
        use std::net::ToSocketAddrs;
        let guest_address = (domain.as_str(), 25565)
            .to_socket_addrs()
            .expect("dns")
            .next()
            .expect("the domain didn't resolve");
        eprintln!("e4mc-test: guest dials {guest_address}");
        let mut guest = None;
        for _ in 0..30 {
            match std::net::TcpStream::connect_timeout(&guest_address, Duration::from_secs(1)) {
                Ok(stream) => {
                    guest = Some(stream);
                    break;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(500)),
            }
        }
        let mut guest = guest.expect("the domain didn't answer");
        eprintln!("e4mc-test: guest connected, sending the handshake");
        guest.write_all(&handshake).unwrap();
        let mut buffer = vec![0u8; handshake.len()];
        lan.set_nonblocking(false).unwrap();
        let (mut world, _) = lan.accept().unwrap();
        world.read_exact(&mut buffer).unwrap();
        assert_eq!(
            buffer, handshake,
            "the relay must forward the handshake verbatim"
        );
        eprintln!("e4mc-test: handshake reached the world, playing bytes...");
        guest.write_all(b"hello").unwrap();
        let mut rest = [0u8; 5];
        world.read_exact(&mut rest).unwrap();
        assert_eq!(&rest, b"hello");
        world.write_all(b"world").unwrap();
        let mut back = [0u8; 5];
        guest.read_exact(&mut back).unwrap();
        assert_eq!(&back, b"world");
        assert_eq!(publish.sessions(), 1);

        // Stopping ends the session: the guest's stream closes.
        publish.stop();
        eprintln!("e4mc-test: stopped, waiting for the guest's EOF...");
        guest
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let mut ended = [0u8; 1];
        assert_eq!(guest.read(&mut ended).unwrap(), 0);
        for _ in 0..50 {
            if publish.sessions() == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(publish.sessions(), 0);
    }
}
