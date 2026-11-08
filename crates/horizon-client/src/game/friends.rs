//! Play with friends, host side: "Start Sharing" on the Play with Friends
//! screen (`friends_screen`, the pause menu's Friends button) opens the world to LAN if
//! it isn't (with Open to LAN's defaults) and shares that port behind an
//! invite code (horizon-friends). The code holds this computer's addresses
//! and the key, so friends join with no server of ours; a relay the player
//! configured adds a short code and a fallback. The integrated server is the
//! LAN one, unchanged: tunnelled guests are LAN guests that connect from the
//! loopback address.
use super::{Game, e4mc_friends, lan::LanSettings};
use horizon_client::{
    Result,
    friends_screen::{self, Action, FriendsView},
    hud::HudBatch,
    text::Text,
};
use horizon_friends::{Host, HostConfig, HostEvent, PathMode, host::DEFAULT_PORT, reach, stun};
use horizon_protocol::{GAME_VERSION, PROTOCOL_VERSION};
use std::time::{Duration, Instant};

pub(super) struct Sharing {
    host: Host,
    /// Sharing opened the world to LAN, so stopping closes it again.
    opened_lan: bool,
    code: Option<String>,
    /// Wrong codes tried so far.
    wrong: u32,
    notice: Option<String>,
    copied: Option<Instant>,
    /// Sessions carried directly and through the relay so far.
    paths: (u32, u32),
}

/// e4mc publishing: the world public on the relay's domain, and whether
/// publishing opened the world to LAN.
pub(super) struct E4mcSharing {
    pub(super) inner: e4mc_friends::Sharing,
    /// Publishing opened the world to LAN, so stopping closes it again.
    opened_lan: bool,
}

pub const HELP: &[&str] = &[
    "friends host                   share the open world behind an invite code",
    "friends stop                   stop sharing",
    "friends status                 the code, how friends reach this host, the guests",
];

/// `HORIZON_STUN_SERVERS`, else `friendsStun` in horizon.txt, else the defaults.
fn stun_servers() -> Vec<String> {
    if std::env::var_os("HORIZON_STUN_SERVERS").is_some() {
        return stun::servers_from_env();
    }
    let dir = std::env::current_dir().unwrap_or_default();
    match horizon_client::ui_theme::preference(&dir, "friendsStun") {
        Some(list) => stun::parse_servers(&list),
        None => stun::servers_from_env(),
    }
}

/// Open to LAN publishes the public address by itself unless `e4mcAutoPublish` in
/// horizon.txt (or `E4MC_AUTO_PUBLISH=0`) says otherwise.
fn e4mc_auto_publish() -> bool {
    if let Ok(v) = std::env::var("E4MC_AUTO_PUBLISH") {
        return v != "0";
    }
    let dir = std::env::current_dir().unwrap_or_default();
    horizon_client::ui_theme::preference(&dir, "e4mcAutoPublish").is_none_or(|v| v != "false")
}

fn env_flag(name: &str, default: bool) -> bool {
    std::env::var(name).map_or(default, |v| v != "0")
}

impl Game<'_> {
    /// Start sharing the open world.
    pub(super) fn friends_start(&mut self) -> Result<()> {
        if self.friends.is_some() {
            return Ok(());
        }
        // Single player on its server thread shares that server, as Open to
        // LAN publishes it; another server can't be shared.
        if self.title.is_some() || (self.remote.is_some() && self.integrated.is_none()) {
            return Err("open a singleplayer world first".into());
        }
        let opened_lan = self.lan_port().is_none() && self.e4mc.is_none();
        if self.lan_port().is_none() {
            // ShareToLanScreen's defaults, as the world options screen offers them.
            self.lan_open_quiet(LanSettings {
                port: horizon_client::lan::available_port(),
                guest_commands: false,
                force_game_mode: true,
            })?;
        }
        let port = self.lan_port().ok_or("the world isn't open to LAN")?;
        let host = match Host::start(HostConfig {
            lan: std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            protocol: PROTOCOL_VERSION,
            game: GAME_VERSION.to_owned(),
            relay: horizon_client::net::friends_relay_setting(),
            path: PathMode::from_env(),
            port: std::env::var("HORIZON_FRIENDS_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            reach: reach::Options {
                // HORIZON_FRIENDS_UPNP=0 leaves the router alone.
                map_port: env_flag("HORIZON_FRIENDS_UPNP", true),
                stun: stun_servers(),
                // Both ends on one machine (tests).
                loopback: env_flag("HORIZON_FRIENDS_LOOPBACK", false),
            },
        }) {
            Ok(h) => h,
            Err(e) => {
                if opened_lan {
                    self.lan_close(true);
                }
                return Err(e.into());
            }
        };
        self.friends = Some(Sharing {
            host,
            opened_lan,
            code: None,
            wrong: 0,
            notice: None,
            copied: None,
            paths: (0, 0),
        });
        Ok(())
    }

    /// Stop sharing: the code ends (direct port closed, router mapping
    /// deleted); the world closes to LAN again if sharing opened it.
    pub(super) fn friends_stop(&mut self) {
        let Some(mut sharing) = self.friends.take() else {
            return;
        };
        sharing.host.stop();
        if sharing.opened_lan && self.e4mc.is_none() {
            self.lan_close(true);
        } else {
            self.system_message(Text::literal("Stopped sharing the invite code."));
        }
    }

    /// The LAN server is closing: the code and the public address go with it.
    pub(super) fn friends_lan_closed(&mut self) {
        if let Some(mut sharing) = self.friends.take() {
            sharing.host.stop();
        }
        if let Some(mut sharing) = self.e4mc.take() {
            sharing.inner.stop();
            horizon_client::android_runtime::host_note_published(None);
        }
    }

    /// The world just opened to LAN (Open to LAN, /publish, world options): like the
    /// original e4mc mod, the public address starts with it. Best effort: a problem
    /// becomes a chat line; the LAN open itself stands.
    pub(super) fn friends_auto_publish(&mut self) {
        if self.e4mc.is_some() || !e4mc_auto_publish() {
            return;
        }
        match self.e4mc_start() {
            Ok(()) => self.system_message(Text::literal(
                "Public address: reaching the relay, the address lands here shortly (Game Menu > Play with Friends).",
            )),
            Err(e) => self.system_message(Text::literal(format!("Public address didn't start: {e}"))),
        }
    }

    /// Publish the world through e4mc's public relay: opens the world to LAN if it
    /// isn't, then puts the world's domain on the relay. Anyone with the domain joins.
    pub(super) fn e4mc_start(&mut self) -> Result<()> {
        if self.e4mc.is_some() {
            return Ok(());
        }
        if self.title.is_some() || (self.remote.is_some() && self.integrated.is_none()) {
            return Err("open a singleplayer world first".into());
        }
        let opened_lan = self.lan_port().is_none() && self.friends.is_none();
        // The hosting settings (Game Menu > Play with Friends > Host): cheats
        // for guests, the relay, and who gets in (applied right after).
        let cheats = self.cs_friends_ui.cheats;
        let relay = self.cs_friends_ui.relay.clone();
        if self.lan_port().is_none() {
            self.lan_open_quiet(LanSettings {
                port: horizon_client::lan::available_port(),
                guest_commands: cheats,
                force_game_mode: true,
            })?;
        } else {
            self.lan_apply_settings(cheats, true);
        }
        let port = self.lan_port().ok_or("the world isn't open to LAN")?;
        match e4mc_friends::Sharing::start(std::net::SocketAddr::from(([127, 0, 0, 1], port)), &relay) {
            Ok(inner) => {
                self.e4mc = Some(E4mcSharing { inner, opened_lan });
                self.e4mc_apply_policy(true);
                Ok(())
            }
            Err(e) => {
                if opened_lan {
                    self.lan_close(true);
                }
                Err(e.into())
            }
        }
    }

    /// Stop publishing: the domain stops answering, guests are dropped.
    pub(super) fn e4mc_stop(&mut self) {
        let Some(mut sharing) = self.e4mc.take() else {
            return;
        };
        sharing.inner.stop();
        horizon_client::android_runtime::host_note_published(None);
        if sharing.opened_lan && self.friends.is_none() {
            self.lan_close(true);
        } else {
            self.system_message(Text::literal("Stopped publishing the world."));
        }
    }

    /// The relay's door policy from the hosting settings: the player limit
    /// (the host counted out), and for "friends only" / "invited only" the
    /// names allowed. `force` recomputes even when nothing is known to have
    /// changed; otherwise it runs cheaply once a second from the tick.
    pub(super) fn e4mc_apply_policy(&mut self, force: bool) {
        let Some(sharing) = self.e4mc.as_ref() else {
            return;
        };
        if !force && !self.ticks.is_multiple_of(20) {
            return;
        }
        let ui = &self.cs_friends_ui;
        let max_players = match ui.max_players {
            0 => None,
            n => Some(n.saturating_sub(1).max(1)),
        };
        let allowed = match ui.access {
            1 => Some(
                self.cs_friends
                    .as_ref()
                    .map(|c| c.state.friends.iter().map(|f| f.name.clone()).collect())
                    .unwrap_or_default(),
            ),
            2 => Some(ui.invited.clone()),
            _ => None,
        };
        let closed_reason = match ui.access {
            1 => "This world is open to the host's CS friends only.",
            2 => "This world is open to invited players only.",
            _ => "",
        }
        .to_owned();
        sharing.inner.set_policy(horizon_e4mc::GuestPolicy {
            max_players,
            allowed,
            closed_reason,
            blocked: ui.blocked.clone(),
        });
    }

    /// e4mc's news, once a tick: the domain, relay notes, guests.
    pub(super) fn tick_e4mc(&mut self) {
        self.e4mc_apply_policy(false);
        let Some(sharing) = self.e4mc.as_mut() else {
            return;
        };
        let mut lines = Vec::new();
        for event in sharing.inner.tick() {
            match event {
                e4mc_friends::Event::Published(domain) => {
                    horizon_client::android_runtime::host_note_published(Some(&domain));
                    let who = match self.cs_friends_ui.access {
                        1 => "your CS friends can join",
                        2 => "invited players can join",
                        _ => "anyone with the address can join",
                    };
                    lines.push(format!("Your world is public: {domain} ({who})."));
                }
                e4mc_friends::Event::Broadcast(message) => lines.push(message),
                e4mc_friends::Event::GuestJoined => {}
                e4mc_friends::Event::GuestNamed(name) => {
                    lines.push(format!("{name} joined through the public address."))
                }
                e4mc_friends::Event::GuestLeft => {
                    lines.push("A public-address guest left.".to_owned())
                }
                e4mc_friends::Event::GuestRefused(name, reason) => {
                    lines.push(format!("{name} tried to join and was turned away: {reason}"))
                }
            }
        }
        if let Some(problem) = sharing.inner.view().problem
            && !sharing.inner.problem_shown
        {
            sharing.inner.problem_shown = true;
            lines.push(format!("Public address: {problem}"));
        }
        for line in lines {
            self.system_message(Text::literal(line));
        }
    }

    /// Copy the public domain for a friend to paste.
    pub(super) fn e4mc_copy(&mut self) {
        let Some(address) = self.e4mc.as_ref().and_then(|s| s.inner.address()) else {
            return;
        };
        use horizon_client::clipboard::Clipboard;
        self.clipboard.set(&address);
        if let Some(s) = self.e4mc.as_mut() {
            s.inner.mark_copied();
        }
    }

    /// What the e4mc section of the view shows.
    fn e4mc_view(&self) -> friends_screen::E4mcView {
        let (domain, guests, problem) = match &self.e4mc {
            Some(s) => {
                let v = s.inner.view();
                (v.domain, v.guests, v.problem)
            }
            None => (None, 0, None),
        };
        friends_screen::E4mcView {
            publishing: self.e4mc.is_some(),
            domain,
            guests,
            problem,
            copied: self.e4mc.as_ref().is_some_and(|s| s.inner.just_copied()),
        }
    }

    /// New codes and sessions, once a tick and while the pause menu is drawn.
    pub(super) fn tick_friends(&mut self) {
        let Some(sharing) = self.friends.as_mut() else {
            return;
        };
        let mut announce = false;
        for event in sharing.host.poll() {
            match event {
                HostEvent::Code(code) => {
                    if sharing.code.is_some() && sharing.wrong >= horizon_friends::host::MAX_WRONG {
                        sharing.notice = Some(
                            "Someone kept trying a wrong short code, so it changed.".to_owned(),
                        );
                        sharing.wrong = 0;
                    }
                    announce = sharing.code.is_none();
                    sharing.code = Some(code);
                }
                HostEvent::WrongCode { count } => {
                    sharing.wrong = count;
                    sharing.notice = Some("Someone tried a wrong code.".to_owned());
                }
                HostEvent::Connected { direct: true, .. } => sharing.paths.0 += 1,
                HostEvent::Connected { direct: false, .. } => sharing.paths.1 += 1,
                HostEvent::Disconnected { .. } => {}
            }
        }
        if announce {
            self.system_message(Text::literal(
                "Your invite code is ready: Game Menu, Play with Friends. Copy it and send it to your friends.",
            ));
        }
    }

    pub(super) fn friends_copy(&mut self) {
        let Some(code) = self.friends.as_ref().and_then(|s| s.code.clone()) else {
            return;
        };
        use horizon_client::clipboard::Clipboard;
        self.clipboard.set(&code);
        if let Some(s) = self.friends.as_mut() {
            s.copied = Some(Instant::now());
        }
    }

    /// Play with Friends. The world is paused under it in singleplayer, so
    /// new codes and sessions are read while it is drawn.
    #[allow(dead_code)]
    pub(super) fn draw_legacy_friends_screen(&mut self, batch: &mut HudBatch) {
        self.tick_friends();
        let view = self.friends_view();
        friends_screen::draw(batch, &self.gui.atlas, self.cursor, &view, true);
    }
    #[allow(dead_code)]
    pub(super) fn legacy_friends_screen_click(&mut self) {
        let view = self.friends_view();
        let Some(action) =
            friends_screen::click(&self.gui.atlas, self.gui_size(), self.cursor, &view)
        else {
            return;
        };
        self.audio.play_button_click();
        match action {
            Action::Share => {
                if let Err(e) = self.friends_start() {
                    self.system_message(Text::literal(format!("Couldn't share the world: {e}")));
                }
            }
            Action::Copy => self.friends_copy(),
            Action::Stop => self.friends_stop(),
            Action::E4mcPublish => {
                if let Err(e) = self.e4mc_start() {
                    self.system_message(Text::literal(format!("Couldn't publish the world: {e}")));
                }
            }
            Action::E4mcCopy => self.e4mc_copy(),
            Action::E4mcStop => self.e4mc_stop(),
            // The system browser, as the folder buttons open their folders.
            Action::Help => {
                if let Some(url) = &view.help {
                    super::packs::open_folder(std::path::Path::new(url));
                }
            }
            Action::Done => self.screen = super::Screen::Paused,
        }
    }

    /// What Play with Friends shows.
    pub(super) fn friends_view(&self) -> FriendsView {
        let guests = match &self.lan {
            Some(h) => h.hub.guests.iter().map(|g| g.name.clone()).collect(),
            // On the server thread: the other players the server lists.
            None if self.integrated.is_some() => {
                let me = self.options.player_name();
                let mut names = self.remote_online_names().unwrap_or_default();
                names.retain(|n| *n != me);
                names
            }
            None => Vec::new(),
        };
        let e4mc = Some(self.e4mc_view());
        let Some(s) = &self.friends else {
            return FriendsView {
                shown: self.remote.is_none() || self.integrated.is_some(),
                guests,
                e4mc,
                ..FriendsView::default()
            };
        };
        let state = s.host.state();
        let mut status = if s.code.is_none() {
            "Finding how friends can reach you…".to_owned()
        } else {
            state.summary.clone()
        };
        if let Some(short) = &state.short {
            status.push_str(&format!(" Short code for the relay: {short}."));
        }
        if let Some(problem) = &state.relay_problem {
            status.push(' ');
            status.push_str(problem);
        }
        let reachable = matches!(
            state.verdict,
            Some(reach::Verdict::Mapped(_) | reach::Verdict::Public)
        ) || state.short.is_some();
        FriendsView {
            shown: true,
            sharing: true,
            code: s.code.clone(),
            status,
            reachable,
            help: (!reachable && s.code.is_some()).then(|| reach::HELP_URL.to_owned()),
            guests,
            notice: s.notice.clone(),
            copied: s
                .copied
                .is_some_and(|t| t.elapsed() < Duration::from_secs(2)),
            e4mc,
        }
    }

    /// `horizon-ctl friends ...`.
    pub(super) fn friends_control(&mut self, words: &[&str]) -> Result<Option<serde_json::Value>> {
        use serde_json::json;
        if words[0] != "friends" {
            return Ok(None);
        }
        match words.get(1).copied() {
            Some("host") => {
                self.friends_start()?;
                Ok(Some(json!({ "ok": true, "lan_port": self.lan_port() })))
            }
            Some("stop") => {
                self.friends_stop();
                Ok(Some(json!({ "ok": true })))
            }
            Some("status") => {
                self.tick_friends();
                let view = self.friends_view();
                let state = self.friends.as_ref().map(|s| s.host.state());
                Ok(Some(json!({
                    "sharing": view.sharing,
                    "code": view.code,
                    "short": state.as_ref().and_then(|s| s.short.as_ref().map(ToString::to_string)),
                    "summary": view.status,
                    "reachable": view.reachable,
                    "verdict": state.as_ref().and_then(|s| s.verdict.as_ref().map(|v| format!("{v:?}"))),
                    "direct_port": state.as_ref().map(|s| s.port),
                    "sessions": self.friends.as_ref().map_or(0, |s| s.host.sessions()),
                    "direct": self.friends.as_ref().map_or(0, |s| s.paths.0),
                    "relayed": self.friends.as_ref().map_or(0, |s| s.paths.1),
                    "guests": view.guests,
                    "lan_port": self.lan_port(),
                })))
            }
            _ => Err("friends host|stop|status".into()),
        }
    }
}
