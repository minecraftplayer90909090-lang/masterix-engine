//! CS Friends inside the game (CS Launcher Plus): the pause menu's and the
//! title's Friends button open `cs_friends_screen` (its Friends, Chat, Host
//! World and Voice Chat pages; key V opens Voice Chat); this file ties it to the
//! backend client (`horizon_client::cs_friends`), the world (presence, Join,
//! Invite, the Host tab's publish / MOTD / icon) and the player's attention
//! (chat lines, toasts, the launcher's system notifications).
use super::{Game, Screen};
use horizon_client::{
    cs_friends::{Client, Notice},
    cs_friends_screen::{self as screen, Action, Focus, GroupRow, HostView, Knob, Tab, View},
    hud::HudBatch,
    text::Text,
    toasts::SystemToast,
};
use winit::{
    event::{ElementState, KeyEvent},
    keyboard::{KeyCode, PhysicalKey},
};

/// `cs-server.json` in the game folder: what `cs_server_branding` reads when
/// the world opens to LAN.
#[derive(Default)]
struct ServerBranding {
    motd: String,
    icon: usize,
    /// 0: no limit.
    max_players: usize,
    /// 0 anyone, 1 friends only, 2 invited only (`access` in the file).
    access: u8,
    /// Guests may use commands (Open to LAN's "Allow Cheats").
    cheats: bool,
    /// A relay `host:port` instead of the broker's choice; empty: automatic.
    relay: String,
}
pub(super) const ACCESS_KEYS: [&str; 3] = ["anyone", "friends", "invited"];
const ICON_FILES: [&str; 3] = ["cs-server-icon.png", "", "cs-server-icon-custom.png"];

fn branding_path() -> std::path::PathBuf {
    std::env::current_dir()
        .unwrap_or_default()
        .join("cs-server.json")
}
fn read_branding() -> ServerBranding {
    let mut b = ServerBranding {
        max_players: horizon_client::lan::MAX_PLAYERS,
        ..ServerBranding::default()
    };
    if let Ok(text) = std::fs::read_to_string(branding_path())
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
    {
        b.motd = json["motd"].as_str().unwrap_or_default().to_owned();
        b.icon = match json["icon"].as_str().unwrap_or_default() {
            "" | "cs-server-icon.png" => 0,
            "cs-server-icon-custom.png" => 2,
            _ => 1,
        };
        if let Some(n) = json["max_players"].as_u64() {
            b.max_players = if n == 0 { 0 } else { (n as usize).clamp(2, 64) };
        }
        b.access = ACCESS_KEYS
            .iter()
            .position(|k| Some(*k) == json["access"].as_str())
            .unwrap_or(0) as u8;
        b.cheats = json["cheats"].as_bool().unwrap_or(false);
        b.relay = json["relay"].as_str().unwrap_or_default().trim().to_owned();
    }
    b
}

impl Game<'_> {
    /// Friends run for the launcher (host mode); `CS_FRIENDS=1` turns them on
    /// elsewhere, `CS_FRIENDS=0` off.
    fn cs_friends_enabled(&self) -> bool {
        if self.options.frames.is_some() {
            return false;
        }
        match std::env::var("CS_FRIENDS") {
            Ok(v) => v != "0",
            Err(_) => horizon_client::android_runtime::host_mode(),
        }
    }
    fn cs_friends_typing_now(&self) -> bool {
        self.screen == Screen::Friends && self.cs_friends_ui.focus != Focus::None
    }
    /// A CS Friends text field has keyboard focus.
    pub(super) fn cs_friends_typing(&self) -> bool {
        self.cs_friends_typing_now()
    }

    /// Once a frame, title and world alike: the client's news, presence, a
    /// join that waited for the world to close.
    pub(super) fn cs_friends_frame(&mut self) {
        if self.cs_friends.is_none() {
            if !self.cs_friends_enabled() {
                return;
            }
            self.cs_friends = Some(Client::start(&self.options.player_name()));
            let b = read_branding();
            self.cs_friends_ui.motd = b.motd;
            self.cs_friends_ui.icon = b.icon;
            self.cs_friends_ui.max_players = b.max_players;
            self.cs_friends_ui.access = b.access;
            self.cs_friends_ui.cheats = b.cheats;
            self.cs_friends_ui.relay = b.relay;
        }
        let (status, address, world) = self.cs_presence();
        self.cs_host_status();
        let chat_open = self.screen == Screen::Friends
            && self.cs_friends_ui.tab == Tab::Chat
            && self.cs_friends_ui.chat_with.is_some();
        let attention = if chat_open {
            2
        } else if self.screen == Screen::Friends {
            1
        } else {
            0
        };
        let notices = {
            let Some(client) = self.cs_friends.as_mut() else {
                return;
            };
            client.set_presence(status, address.as_deref(), world.as_deref());
            client.attention(attention);
            client.poll();
            if chat_open
                && let Some(id) = self.cs_friends_ui.chat_with.clone()
            {
                client.mark_read(&id);
            }
            client.take_notices()
        };
        for notice in notices {
            self.cs_notice(notice);
        }
        if let Some(address) = self.cs_join_pending.clone()
            && self.title.is_some()
            && self.pending_save.is_none()
            && !self.remote_loading()
        {
            self.cs_join_pending = None;
            self.cs_connect(&address);
        }
    }

    /// What friends see: at the title, in a world, or hosting a public one.
    fn cs_presence(&self) -> (&'static str, Option<String>, Option<String>) {
        if self.title.is_some() {
            return ("online", None, None);
        }
        let world = Some(self.level.name.clone());
        match self.e4mc.as_ref().and_then(|s| s.inner.address()) {
            Some(address) => ("hosting", Some(address), world),
            None => ("in_game", None, world),
        }
    }

    /// The launcher's Discord Rich Presence and play time follow the engine:
    /// what the player is doing, for `nativeHostStatus`.
    fn cs_host_status(&self) {
        if !horizon_client::android_runtime::host_mode() {
            return;
        }
        let (scene, world, server, players) = if self.title.is_some() {
            ("menu", String::new(), String::new(), 0)
        } else {
            let players = self.remote.as_ref().map_or(1, |r| r.player_count().max(1));
            let published = self.e4mc.as_ref().and_then(|s| s.inner.address()).is_some()
                || self.lan.is_some()
                || self.integrated.as_ref().is_some_and(|s| s.published().is_some());
            if self.external_server() {
                (
                    "multiplayer",
                    String::new(),
                    self.server_address().unwrap_or_default().to_owned(),
                    players,
                )
            } else if published {
                ("hosting", self.level.name.clone(), String::new(), players)
            } else {
                ("singleplayer", self.level.name.clone(), String::new(), players)
            }
        };
        let clean = |s: &str| s.replace(['\t', '\n'], " ");
        horizon_client::android_runtime::host_set_status(&format!(
            "{scene}\t{}\t{}\t{players}",
            clean(&world),
            clean(&server)
        ));
    }

    /// A chat line, a toast, and (for invites) the launcher's notification.
    fn cs_notice(&mut self, notice: Notice) {
        let line = if notice.text.is_empty() {
            notice.title.clone()
        } else {
            format!("{}: {}", notice.title, notice.text)
        };
        if self.title.is_none() {
            self.system_message(Text::literal(format!("§b[Friends]§r {line}")));
        }
        let toast = SystemToast::new(
            notice.title.clone(),
            Some(notice.text.clone()).filter(|t| !t.is_empty()),
        );
        match self.server_packs.cs_toast.as_mut() {
            Some(shown) => shown.reset_from(toast),
            None => self.server_packs.cs_toast = Some(toast),
        }
        if notice.system {
            horizon_client::android_runtime::host_push_notice(&notice.title, &notice.text);
        }
    }

    /// Open the screen (pause menu's or title's Friends button).
    pub(super) fn open_cs_friends(&mut self) {
        if self.screen != Screen::Friends {
            self.cs_friends_ui.anim.page_changed();
        }
        self.screen = Screen::Friends;
        self.cs_friends_ui.focus = Focus::None;
        self.cs_friends_ui.confirm_remove = None;
        if self.cs_friends.is_none() {
            self.cs_friends_frame();
        }
        if let Some(client) = &self.cs_friends {
            client.sync_now();
        }
        if self.title.is_none() {
            self.capture(false);
        }
    }
    /// The voice menu (key V, `key.cs_voice_menu`): the Voice Chat page —
    /// groups to create, join or leave, invites, players to mute.
    pub(super) fn open_cs_voice_menu(&mut self) {
        self.open_cs_friends();
        self.cs_friends_ui.go(Tab::Voice);
    }
    /// Done / Escape: back to the pause menu, or the title.
    fn close_cs_friends(&mut self) {
        self.cs_friends_ui.focus = Focus::None;
        self.cs_friends_ui.confirm_remove = None;
        self.screen = if self.title.is_some() {
            Screen::Playing
        } else {
            Screen::Paused
        };
    }

    fn cs_view(&mut self) -> View {
        let mut view = View {
            blink: self.started.elapsed().as_millis() / 500 % 2 == 0,
            copied: self
                .cs_code_copied
                .is_some_and(|t| t.elapsed().as_secs() < 2),
            ..View::default()
        };
        if let Some(client) = &self.cs_friends {
            let st = &client.state;
            view.me_code = st.me_code.clone();
            view.me_name = st.me_name.clone();
            view.connected = st.connected;
            view.error = st.error.as_ref().map(|(e, _)| e.clone());
            view.info = st.info.as_ref().map(|(i, _)| i.clone());
            view.friends = st.friends.clone();
            view.requests = st.requests.clone();
            view.invites = st.invites.clone();
            view.unread = st
                .unread
                .iter()
                .filter(|(_, n)| **n > 0)
                .map(|(k, n)| (k.clone(), *n))
                .collect();
            if let Some(id) = &self.cs_friends_ui.chat_with {
                view.chat = st.chats.get(id).cloned().unwrap_or_default();
            }
        }
        let (address, guests, problem, copied) = match &self.e4mc {
            Some(s) => {
                let v = s.inner.view();
                (v.domain, v.guests, v.problem, s.inner.just_copied())
            }
            None => (None, 0, None, false),
        };
        {
            self.cs_voice_tidy();
            let in_world = self.title.is_none();
            let groups: Vec<GroupRow> = self
                .cs_voice_groups()
                .into_iter()
                .map(|(name, locked, members, mine)| GroupRow {
                    name,
                    locked,
                    members,
                    mine,
                })
                .collect();
            // The players list leaves out the player's own group (they are on its card).
            let members: Vec<String> = groups
                .iter()
                .find(|g| g.mine)
                .map(|g| g.members.iter().map(|m| m.0.to_lowercase()).collect())
                .unwrap_or_default();
            let players: Vec<(String, bool, bool)> = self
                .cs_voice_players()
                .into_iter()
                .filter(|(n, _, _)| !members.contains(&n.to_lowercase()))
                .collect();
            let invitable = self.cs_voice_invitable();
            let v = &self.cs_voice;
            view.voice = screen::VoiceView {
                enabled: v.settings.enabled,
                ptt: v.settings.ptt,
                volume: v.settings.volume,
                mic: v.settings.mic,
                range: v.settings.range,
                available: in_world
                    && self
                        .remote
                        .as_ref()
                        .is_some_and(|r| r.joined && r.mod_net),
                in_world,
                mic_problem: v.mic_problem.clone(),
                talking: v.talking,
                level: v.level_smooth,
                players,
                group: self.cs_voice_my_group().map(str::to_owned),
                groups,
                invites: v
                    .invites
                    .iter()
                    .map(|i| (i.from_name.clone(), i.group.clone()))
                    .collect(),
                invitable,
                notice: v.notice.as_ref().map(|(t, p, _)| (t.clone(), *p)),
            };
        }
        view.host = HostView {
            in_world: self.title.is_none(),
            can_publish: self.title.is_none() && !self.external_server(),
            publishing: self.e4mc.is_some(),
            address,
            guests,
            problem,
            copied,
            world: if self.title.is_none() {
                self.level.name.clone()
            } else {
                String::new()
            },
        };
        view
    }

    pub(super) fn draw_cs_friends(&mut self, batch: &mut HudBatch) {
        let view = self.cs_view();
        // The animations: the eased values chase the voice state, the hover
        // glow follows the cursor from button to button.
        {
            let (enabled, ptt, talking, level) = (
                view.voice.enabled,
                view.voice.ptt,
                view.voice.talking,
                view.voice.level,
            );
            self.cs_friends_ui.anim.tick(enabled, ptt, talking, level);
            let over = screen::hovered_box(&self.gui.atlas, self.gui_size(), self.cursor, &self.cs_friends_ui, &view);
            let anim = &mut self.cs_friends_ui.anim;
            match (over, anim.hover) {
                (Some(b), Some((was, _))) if was == b => {}
                (Some(b), _) => anim.hover = Some((b, 0.0)),
                (None, _) => anim.hover = None,
            }
        }
        screen::draw(
            batch,
            &self.gui.atlas,
            self.cursor,
            &self.cs_friends_ui,
            &view,
            self.title.is_none(),
        );
    }

    pub(super) fn cs_friends_click(&mut self) {
        let view = self.cs_view();
        let Some((action, rect)) = screen::click(
            &self.gui.atlas,
            self.gui_size(),
            self.cursor,
            &self.cs_friends_ui,
            &view,
        ) else {
            return;
        };
        if !matches!(action, Action::Focus(_)) {
            self.audio.play_button_click();
            self.cs_friends_ui.anim.press(rect);
        }
        self.cs_friends_action(action, &view);
    }

    fn cs_friends_action(&mut self, action: Action, view: &View) {
        let ui = &mut self.cs_friends_ui;
        if !matches!(action, Action::Remove(_)) {
            ui.confirm_remove = None;
        }
        match action {
            Action::Focus(focus) => ui.focus = focus,
            Action::Tab(tab) => ui.go(tab),
            Action::Scroll(by) => {
                let range = screen::scroll_range(&self.gui.atlas, self.gui_size(), &self.cs_friends_ui, view);
                let ui = &mut self.cs_friends_ui;
                ui.scroll = (ui.scroll + by).clamp(0, range);
            }
            Action::Done => self.close_cs_friends(),
            Action::CopyCode => {
                if !view.me_code.is_empty() {
                    use horizon_client::clipboard::Clipboard;
                    self.clipboard.set(&view.me_code);
                    self.cs_code_copied = Some(std::time::Instant::now());
                }
            }
            Action::AddFriend => {
                let code = ui.add_code.trim().to_uppercase();
                if let Some(client) = &self.cs_friends {
                    client.add_friend(&code);
                }
                self.cs_friends_ui.add_code.clear();
                self.cs_friends_ui.focus = Focus::None;
            }
            Action::Accept(id) => {
                if let Some(client) = &self.cs_friends {
                    client.respond(&id, true);
                }
            }
            Action::Decline(id) => {
                if let Some(client) = &self.cs_friends {
                    client.respond(&id, false);
                }
            }
            Action::Remove(id) => {
                if ui.confirm_remove.as_deref() == Some(id.as_str()) || !view.friends.iter().any(|f| f.id == id) {
                    ui.confirm_remove = None;
                    if ui.chat_with.as_deref() == Some(id.as_str()) {
                        ui.chat_with = None;
                    }
                    if let Some(client) = self.cs_friends.as_mut() {
                        client.remove_friend(&id);
                    }
                } else {
                    ui.confirm_remove = Some(id);
                }
            }
            Action::Invite(id) => {
                if let Some(address) = &view.host.address
                    && let Some(client) = &self.cs_friends
                {
                    client.invite(Some(&id), address, &view.host.world);
                    if let Some(f) = client.state.friend(&id) {
                        self.cs_friends_ui.invited.insert(f.name.clone());
                    }
                    self.e4mc_apply_policy(true);
                }
            }
            Action::InviteAll => {
                if let Some(address) = &view.host.address
                    && let Some(client) = &self.cs_friends
                {
                    client.invite(None, address, &view.host.world);
                    for f in client.state.friends.iter().filter(|f| f.online()) {
                        self.cs_friends_ui.invited.insert(f.name.clone());
                    }
                    self.e4mc_apply_policy(true);
                }
            }
            Action::InviteJoin(id, address) => {
                if let Some(client) = self.cs_friends.as_mut() {
                    client.dismiss_invite(id);
                }
                self.cs_join(&address);
            }
            Action::InviteDismiss(id) => {
                if let Some(client) = self.cs_friends.as_mut() {
                    client.dismiss_invite(id);
                }
            }
            Action::Join(address) => self.cs_join(&address),
            Action::OpenChat(id) => {
                ui.go(Tab::Chat);
                ui.chat_with = Some(id.clone());
                ui.focus = Focus::Chat;
                ui.scroll = 0;
                if let Some(client) = self.cs_friends.as_mut() {
                    client.mark_read(&id);
                    client.sync_now();
                }
            }
            Action::ChatBack => {
                ui.chat_with = None;
                ui.focus = Focus::None;
                ui.chat_text.clear();
            }
            Action::Send => self.cs_send_chat(),
            Action::Publish => {
                if let Err(e) = self.e4mc_start() {
                    self.system_message(Text::literal(format!("Couldn't publish the world: {e}")));
                }
            }
            Action::StopPublish => self.e4mc_stop(),
            Action::CopyAddress => self.e4mc_copy(),
            Action::SaveMotd => {
                self.cs_friends_ui.focus = Focus::None;
                self.cs_save_branding();
            }
            Action::CycleIcon => {
                ui.icon = (ui.icon + 1) % 3;
                self.cs_save_branding();
            }
            Action::VoiceEnabled => {
                self.cs_voice.settings.enabled = !self.cs_voice.settings.enabled;
                self.cs_voice_save();
            }
            Action::VoiceMode => {
                self.cs_voice.settings.ptt = !self.cs_voice.settings.ptt;
                self.cs_voice_save();
            }
            Action::VoiceSet(knob, value) => {
                let s = &mut self.cs_voice.settings;
                let value = value.clamp(0.0, 1.0);
                match knob {
                    // Steps of 5 %, 5 % and 4 blocks: the handle snaps as it goes.
                    Knob::Volume => s.volume = ((value * 200.0 / 5.0).round() * 5.0) as u32,
                    Knob::Mic => s.mic = (50.0 + (value * 250.0 / 5.0).round() * 5.0) as u32,
                    Knob::Range => s.range = (8.0 + (value * 248.0 / 4.0).round() * 4.0) as u32,
                }
                s.volume = s.volume.clamp(0, 200);
                s.mic = s.mic.clamp(50, 300);
                s.range = s.range.clamp(8, 256);
                self.cs_voice_save();
            }
            Action::VoiceMute(name) => self.cs_voice_toggle_mute(&name),
            Action::VoiceGroupCreate => {
                let name = std::mem::take(&mut self.cs_friends_ui.group_name);
                let password = std::mem::take(&mut self.cs_friends_ui.password);
                self.cs_friends_ui.focus = Focus::None;
                self.cs_voice_group_create(&name, &password);
            }
            Action::VoiceGroupJoin(name) => {
                // The password typed goes with a locked group's join; an open
                // group needs none.
                let password = if self.cs_friends_ui.join_target.as_deref() == Some(name.as_str()) {
                    std::mem::take(&mut self.cs_friends_ui.password)
                } else {
                    String::new()
                };
                self.cs_friends_ui.join_target = None;
                self.cs_friends_ui.focus = Focus::None;
                self.cs_voice_group_join(&name, &password);
            }
            Action::VoiceAskPassword(name) => {
                ui.join_target = Some(name);
                ui.password.clear();
                ui.focus = Focus::Password;
                ui.inviting = false;
            }
            Action::VoiceCancelPassword => {
                ui.join_target = None;
                ui.password.clear();
                ui.focus = Focus::None;
            }
            Action::VoiceGroupLeave => {
                ui.inviting = false;
                self.cs_voice_group_leave();
            }
            Action::VoiceInviteToggle => {
                ui.inviting = !ui.inviting;
                ui.focus = Focus::None;
            }
            Action::VoiceInvite(name) => self.cs_voice_invite(&name),
            Action::VoiceInviteAccept(i) => {
                ui.inviting = false;
                ui.join_target = None;
                self.cs_voice_invite_accept(i);
            }
            Action::VoiceInviteDismiss(i) => self.cs_voice_invite_dismiss(i),
            Action::Players(step) => {
                // 2, 4, 6, 8, 10, 12, 16, 20, 24, 32, 64, then no limit (0).
                const STEPS: [usize; 12] = [2, 4, 6, 8, 10, 12, 16, 20, 24, 32, 64, 0];
                let cur = ui.max_players;
                let i = if cur == 0 {
                    STEPS.len() - 1
                } else {
                    STEPS.iter().position(|&n| n >= cur).unwrap_or(STEPS.len() - 2)
                };
                let next = (i as i32 + step).clamp(0, STEPS.len() as i32 - 1) as usize;
                ui.max_players = STEPS[next];
                self.cs_save_branding();
            }
            Action::Access => {
                ui.access = (ui.access + 1) % 3;
                self.cs_save_branding();
            }
            Action::Cheats => {
                ui.cheats = !ui.cheats;
                self.cs_save_branding();
                let cheats = self.cs_friends_ui.cheats;
                if self.lan.is_some() || self.integrated.is_some() {
                    self.lan_apply_settings(cheats, true);
                }
            }
            Action::SaveRelay => {
                self.cs_friends_ui.focus = Focus::None;
                self.cs_save_branding();
                if self.e4mc.is_some() {
                    self.system_message(Text::literal(
                        "Relay saved: it is used the next time the world is published.",
                    ));
                }
            }
        }
    }

    fn cs_send_chat(&mut self) {
        let text = self.cs_friends_ui.chat_text.trim().to_owned();
        let Some(to) = self.cs_friends_ui.chat_with.clone() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if let Some(client) = self.cs_friends.as_mut() {
            client.send_message(&to, &text);
        }
        self.cs_friends_ui.chat_text.clear();
    }

    /// The invite banner's Accept (game/cs_invite_banner.rs).
    pub(super) fn cs_join_from_banner(&mut self, address: &str) {
        self.cs_join(address);
    }
    /// Join a friend's world: now from the title, else once this world has closed.
    fn cs_join(&mut self, address: &str) {
        if address.is_empty() {
            return;
        }
        if self.title.is_some() {
            self.screen = Screen::Playing;
            self.cs_connect(address);
            return;
        }
        self.cs_join_pending = Some(address.to_owned());
        self.system_message(Text::literal(format!("§b[Friends]§r Joining {address}…")));
        self.screen = Screen::Playing;
        if let Err(e) = self.close_world() {
            self.cs_join_pending = None;
            self.system_message(Text::literal(format!("Couldn't leave the world: {e}")));
        }
    }
    fn cs_connect(&mut self, address: &str) {
        let name = self.options.player_name();
        if let Err(e) = self.connect_server_with_options(address, &name, None, false) {
            self.cs_notice(Notice {
                title: "Couldn't join".to_owned(),
                text: format!("{address}: {e}"),
                system: false,
            });
        }
    }

    /// The Host tab's MOTD and icon: saved for the next Open to LAN and
    /// applied to the running one.
    fn cs_save_branding(&mut self) {
        let mut motd = self.cs_friends_ui.motd.trim().to_owned();
        motd.truncate(59 * 4);
        let max_players = match self.cs_friends_ui.max_players {
            0 => 0,
            n => n.clamp(2, 64),
        };
        let icon_file = match self.cs_friends_ui.icon {
            1 => {
                // The world's own icon (saves/<world>/icon.png), copied so the
                // path in cs-server.json stays inside the game folder.
                let from = self.world_root.join("icon.png");
                let to = std::env::current_dir()
                    .unwrap_or_default()
                    .join("cs-server-icon-world.png");
                if std::fs::copy(&from, &to).is_ok() {
                    "cs-server-icon-world.png"
                } else {
                    self.system_message(Text::literal(
                        "This world has no icon yet (it is taken on the first save); using the Masterix icon.",
                    ));
                    ICON_FILES[0]
                }
            }
            2 => ICON_FILES[2],
            _ => ICON_FILES[0],
        };
        let relay = self.cs_friends_ui.relay.trim().to_owned();
        let json = serde_json::json!({
            "motd": motd,
            "icon": icon_file,
            "max_players": max_players,
            "access": ACCESS_KEYS[usize::from(self.cs_friends_ui.access.min(2))],
            "cheats": self.cs_friends_ui.cheats,
            "relay": relay,
        });
        if let Err(e) = std::fs::write(branding_path(), json.to_string()) {
            self.system_message(Text::literal(format!("Couldn't save the server name: {e}")));
            return;
        }
        // Live, while the world is open: the integrated server's host (on its
        // thread) or the window's own LAN host.
        let applied = if let Some(server) = &self.integrated {
            server.call(|g| g.cs_apply_branding()).flatten()
        } else {
            self.cs_apply_branding()
        };
        if let Some((_, _, max)) = applied {
            self.cs_friends_ui.max_players = if max >= super::lan::NO_PLAYER_LIMIT { 0 } else { max };
        }
        self.e4mc_apply_policy(true);
        if icon_file != ICON_FILES[0] && ui_icon_missing(icon_file) {
            self.system_message(Text::literal(format!(
                "Icon file {icon_file} not found in the game folder; friends see no icon until it exists."
            )));
        }
    }

    /// Escape / Back: leave the field, then the conversation, then the screen.
    pub(super) fn cs_friends_escape(&mut self) {
        let ui = &mut self.cs_friends_ui;
        if ui.focus != Focus::None {
            ui.focus = Focus::None;
        } else if ui.join_target.is_some() || ui.inviting {
            ui.join_target = None;
            ui.inviting = false;
        } else if ui.chat_with.is_some() && ui.tab == Tab::Chat {
            ui.chat_with = None;
        } else {
            self.close_cs_friends();
        }
    }
    /// A key (or typed text) while the screen is open.
    pub(super) fn cs_friends_key(&mut self, event: &KeyEvent) {
        let key = match event.physical_key {
            PhysicalKey::Code(code) => Some(code),
            PhysicalKey::Unidentified(_) => None,
        };
        // The voice keys work over this screen too (it is the voice menu):
        // push to talk is held while no field has the focus, and the menu key
        // closes the Voice tab it opened.
        let bound = |mapping: &str| {
            key.and_then(horizon_client::keybinds::key_name)
                .is_some_and(|n| self.settings.keys.get(mapping) == n)
        };
        let (ptt, menu) = (bound("key.cs_voice"), bound("key.cs_voice_menu"));
        if ptt {
            if event.state == ElementState::Pressed && self.cs_friends_ui.focus == Focus::None {
                self.keys.insert(super::cs_voice::PTT_KEY);
            } else if event.state != ElementState::Pressed {
                self.keys.remove(&super::cs_voice::PTT_KEY);
            }
            if self.cs_friends_ui.focus == Focus::None {
                return;
            }
        }
        if event.state != ElementState::Pressed {
            return;
        }
        if key == Some(KeyCode::Escape)
            || (menu
                && !event.repeat
                && self.cs_friends_ui.focus == Focus::None
                && self.cs_friends_ui.tab == Tab::Voice)
        {
            self.cs_friends_escape();
            return;
        }
        if self.cs_friends_ui.focus == Focus::None {
            return;
        }
        let text = if self.types_text() {
            event.text.as_deref()
        } else {
            None
        };
        self.cs_friends_input(key, text);
    }

    /// Typing into the focused field: the on-screen keyboard, the IME and
    /// real keys all end here.
    pub(super) fn cs_friends_input(&mut self, key: Option<KeyCode>, text: Option<&str>) {
        let ui = &mut self.cs_friends_ui;
        match key {
            Some(KeyCode::Backspace) => {
                if let Some(field) = ui.field_mut() {
                    field.pop();
                }
                return;
            }
            Some(KeyCode::Enter | KeyCode::NumpadEnter) => {
                match ui.focus {
                    Focus::AddCode => {
                        let view = self.cs_view();
                        self.cs_friends_action(Action::AddFriend, &view);
                    }
                    Focus::Chat => self.cs_send_chat(),
                    Focus::Motd => {
                        let view = self.cs_view();
                        self.cs_friends_action(Action::SaveMotd, &view);
                    }
                    Focus::Relay => {
                        let view = self.cs_view();
                        self.cs_friends_action(Action::SaveRelay, &view);
                    }
                    Focus::Group => {
                        let view = self.cs_view();
                        self.cs_friends_action(Action::VoiceGroupCreate, &view);
                    }
                    Focus::Password => {
                        let action = match ui.join_target.clone() {
                            Some(name) => Action::VoiceGroupJoin(name),
                            None => Action::VoiceGroupCreate,
                        };
                        let view = self.cs_view();
                        self.cs_friends_action(action, &view);
                    }
                    Focus::None => {}
                }
                return;
            }
            Some(KeyCode::Tab) => return,
            _ => {}
        }
        let Some(text) = text else {
            return;
        };
        let max = ui.max_len();
        let upper = ui.focus == Focus::AddCode;
        if let Some(field) = ui.field_mut() {
            for c in text.chars().filter(|c| !c.is_control()) {
                if field.chars().count() >= max {
                    break;
                }
                if upper {
                    field.extend(c.to_uppercase());
                } else {
                    field.push(c);
                }
            }
        }
    }

    /// The focused field's text (the Android keyboard starts from it).
    pub(super) fn cs_friends_edit_value(&self) -> String {
        self.cs_friends_ui.field().cloned().unwrap_or_default()
    }

    /// The wheel over a list.
    pub(super) fn cs_friends_scroll(&mut self, d: f32) {
        if d == 0.0 {
            return;
        }
        let view = self.cs_view();
        let range = screen::scroll_range(&self.gui.atlas, self.gui_size(), &self.cs_friends_ui, &view);
        let ui = &mut self.cs_friends_ui;
        ui.scroll = (ui.scroll - d.signum() as i32).clamp(0, range);
    }

    /// The focused field's box for the IME.
    pub(super) fn cs_friends_focused_box(&mut self) -> Option<[f32; 4]> {
        let view = self.cs_view();
        screen::focused_box(&self.gui.atlas, self.gui_size(), &self.cs_friends_ui, &view)
    }
}

fn ui_icon_missing(icon_file: &str) -> bool {
    !std::env::current_dir()
        .unwrap_or_default()
        .join(icon_file)
        .is_file()
}
