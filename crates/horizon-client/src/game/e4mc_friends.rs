//! e4mc publishing: the open world published through e4mc's public relay
//! ([horizon-e4mc]), the "Open to LAN as normal, the address creates
//! itself" flow. Anyone with the domain joins, even from vanilla
//! Minecraft; there is no identity and no invitation. The button says so.
//!
//! [horizon-e4mc]: ../../../horizon_e4mc/index.html

/// What the Play with Friends screen shows about e4mc publishing.
#[derive(Clone, Debug, Default)]
pub struct View {
    /// The world's public domain, once the relay assigned it.
    pub domain: Option<String>,
    /// Guests being carried right now.
    pub guests: usize,
    /// Why publishing isn't serving, when the player asked for it.
    pub problem: Option<String>,
}

/// What a tick reports to the game.
#[derive(Debug)]
pub enum Event {
    /// The world's public address is ready.
    Published(String),
    /// A note the relay asked every host to show its players.
    Broadcast(String),
    /// A guest's stream is being carried.
    GuestJoined,
    /// The guest's stream ended.
    GuestLeft,
    /// A guest was turned away at the relay (the hosting settings): who and why.
    GuestRefused(String, String),
}

pub struct Sharing {
    publish: Option<horizon_e4mc::Publish>,
    copied: Option<std::time::Instant>,
    /// The relay problem was already announced in chat.
    pub problem_shown: bool,
}

impl Sharing {
    /// Start publishing the world already open to LAN on `lan`. `relay` is a
    /// `host:port` the player chose (the hosting settings); empty or
    /// malformed, the `E4MC_RELAY` variable, then the broker's choice.
    pub fn start(lan: std::net::SocketAddr, relay: &str) -> Result<Sharing, String> {
        let parse = |relay: &str| {
            let (host, port) = relay.trim().rsplit_once(':')?;
            Some((host.to_owned(), port.parse().ok()?))
        };
        Ok(Sharing {
            publish: Some(horizon_e4mc::Publish::start(horizon_e4mc::PublishConfig {
                lan,
                relay: parse(relay).or_else(|| std::env::var("E4MC_RELAY").ok().and_then(|r| parse(&r))),
            })?),
            copied: None,
            problem_shown: false,
        })
    }

    /// End the publish: the domain stops answering, guests are dropped.
    pub fn stop(&mut self) {
        if let Some(publish) = self.publish.take() {
            publish.stop();
        }
    }

    /// New guests, relay notes and the domain, once a tick.
    pub fn tick(&mut self) -> Vec<Event> {
        let Some(publish) = &self.publish else {
            return Vec::new();
        };
        publish
            .poll()
            .into_iter()
            .filter_map(|event| match event {
                horizon_e4mc::PublishEvent::Published { domain } => Some(Event::Published(domain)),
                horizon_e4mc::PublishEvent::Broadcast(message) => Some(Event::Broadcast(message)),
                horizon_e4mc::PublishEvent::GuestJoined => Some(Event::GuestJoined),
                horizon_e4mc::PublishEvent::GuestLeft => Some(Event::GuestLeft),
                horizon_e4mc::PublishEvent::GuestRefused { name, reason } => {
                    Some(Event::GuestRefused(name, reason))
                }
                horizon_e4mc::PublishEvent::Problem(_) => None,
            })
            .collect()
    }

    /// Who the relay lets in from now on.
    pub fn set_policy(&self, policy: horizon_e4mc::GuestPolicy) {
        if let Some(publish) = &self.publish
            && publish.policy() != policy
        {
            publish.set_policy(policy);
        }
    }

    /// What the screen shows.
    pub fn view(&self) -> View {
        match &self.publish {
            Some(publish) => {
                let state = publish.state();
                View {
                    domain: state.domain,
                    guests: state.guests,
                    problem: state.problem,
                }
            }
            None => View::default(),
        }
    }

    /// The public domain, for the copy button.
    pub fn address(&self) -> Option<String> {
        self.view().domain
    }

    /// The address was just copied (the button says so for a moment).
    pub fn mark_copied(&mut self) {
        self.copied = Some(std::time::Instant::now());
    }
    pub fn just_copied(&self) -> bool {
        self.copied
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(2))
    }
}
