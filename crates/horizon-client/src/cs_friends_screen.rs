//! The CS Friends screen (`game/cs_friends_ui.rs` drives it): three tabs —
//! Friends (your code, add a friend, requests, who is online, Join / Invite),
//! Chat (one-to-one messages) and Host (publish the world, server name and
//! icon friends see, invite everyone). Drawn like the other Horizon screens:
//! menu background, vanilla buttons, Done in the footer. Pure layout here:
//! the game fills a `View`, keeps the `Ui` state and acts on the `Action`.
use crate::{
    cs_friends::{Friend, Invite, Message, Request},
    hud::{HudAtlas, HudBatch, rgba},
    multiplayer::wrap,
    text::Text,
    ui,
};

pub const TITLE: &str = "Masterix Friends";
const ROW: f32 = 22.0;
const WHITE: u32 = u32::MAX;
fn grey() -> u32 {
    rgba(160, 160, 160, 255)
}
fn dim() -> u32 {
    rgba(110, 110, 110, 255)
}
fn green() -> u32 {
    rgba(94, 214, 140, 255)
}
fn blue() -> u32 {
    rgba(94, 200, 255, 255)
}
fn yellow() -> u32 {
    rgba(242, 201, 76, 255)
}
fn red() -> u32 {
    rgba(255, 110, 110, 255)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Friends,
    Chat,
    Host,
    Voice,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    None,
    AddCode,
    Chat,
    Motd,
    /// The voice menu's group name.
    Group,
}

/// The screen's own state (what the player typed and where they are).
#[derive(Debug, Default)]
pub struct Ui {
    pub tab: Tab,
    /// Rows scrolled past in the current list.
    pub scroll: i32,
    pub add_code: String,
    /// The friend whose conversation is open.
    pub chat_with: Option<String>,
    pub chat_text: String,
    pub motd: String,
    /// The voice menu: a group name typed to create or join.
    pub group_name: String,
    pub focus: Focus,
    /// 0 = CS Launcher icon, 1 = the world's icon, 2 = a custom file.
    pub icon: usize,
    /// Players the published world takes, host included.
    pub max_players: usize,
    /// A friend's Remove was pressed once; the next press removes.
    pub confirm_remove: Option<String>,
}
impl Ui {
    pub fn field_mut(&mut self) -> Option<&mut String> {
        match self.focus {
            Focus::None => None,
            Focus::AddCode => Some(&mut self.add_code),
            Focus::Chat => Some(&mut self.chat_text),
            Focus::Motd => Some(&mut self.motd),
            Focus::Group => Some(&mut self.group_name),
        }
    }
    pub fn field(&self) -> Option<&String> {
        match self.focus {
            Focus::None => None,
            Focus::AddCode => Some(&self.add_code),
            Focus::Chat => Some(&self.chat_text),
            Focus::Motd => Some(&self.motd),
            Focus::Group => Some(&self.group_name),
        }
    }
    pub fn max_len(&self) -> usize {
        match self.focus {
            Focus::AddCode => 12,
            Focus::Chat => 256,
            Focus::Motd => 59,
            Focus::Group => 24,
            Focus::None => 0,
        }
    }
}

/// What the host tab shows.
#[derive(Clone, Debug, Default)]
pub struct HostView {
    /// A world is open (not the title screen).
    pub in_world: bool,
    /// A singleplayer world: it can be published.
    pub can_publish: bool,
    pub publishing: bool,
    pub address: Option<String>,
    pub guests: usize,
    pub problem: Option<String>,
    pub copied: bool,
    pub world: String,
}

/// The Voice tab (CS built-in voice chat, game/cs_voice.rs).
#[derive(Clone, Debug, Default)]
pub struct VoiceView {
    pub enabled: bool,
    pub ptt: bool,
    pub volume: u32,
    pub mic: u32,
    pub range: u32,
    /// Voice can travel here (a CS Engine world or server).
    pub available: bool,
    pub in_world: bool,
    pub mic_problem: Option<String>,
    pub talking: bool,
    /// Microphone level 0 to 1.
    pub level: f32,
    /// Players in the world: name, muted, heard right now.
    pub players: Vec<(String, bool, bool)>,
    /// The group the player is in.
    pub group: Option<String>,
    /// The world's groups: name, members' names, the player's own.
    pub groups: Vec<(String, Vec<String>, bool)>,
}

/// What the screen shows (the game fills it from `cs_friends::State`).
#[derive(Clone, Debug, Default)]
pub struct View {
    pub me_code: String,
    pub me_name: String,
    pub connected: bool,
    pub error: Option<String>,
    pub info: Option<String>,
    pub friends: Vec<Friend>,
    pub requests: Vec<Request>,
    pub invites: Vec<Invite>,
    pub unread: Vec<(String, usize)>,
    /// The open conversation, oldest first.
    pub chat: Vec<Message>,
    pub host: HostView,
    pub voice: VoiceView,
    /// The code was just copied.
    pub copied: bool,
    /// Caret blink phase.
    pub blink: bool,
}
impl View {
    fn name_of(&self, id: &str) -> String {
        self.friends
            .iter()
            .find(|f| f.id == id)
            .map_or_else(|| "Friend".to_owned(), |f| f.name.clone())
    }
    fn unread_of(&self, id: &str) -> usize {
        self.unread.iter().find(|(k, _)| k == id).map_or(0, |(_, n)| *n)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Tab(Tab),
    Focus(Focus),
    AddFriend,
    CopyCode,
    InviteJoin(i64, String),
    InviteDismiss(i64),
    Accept(String),
    Decline(String),
    Remove(String),
    Invite(String),
    Join(String),
    OpenChat(String),
    ChatBack,
    Send,
    Scroll(i32),
    Publish,
    StopPublish,
    CopyAddress,
    InviteAll,
    SaveMotd,
    CycleIcon,
    /// Player limit up (+1) or down (-1) a step.
    Players(i32),
    /// Voice chat on/off, push-to-talk/open mic, volume, mic gain, range steps, mute a player.
    VoiceEnabled,
    VoiceMode,
    VoiceVolume(i32),
    VoiceMic(i32),
    VoiceRange(i32),
    VoiceMute(String),
    /// Create or join the group typed (empty: the named one), leave the group.
    VoiceGroupJoin(String),
    VoiceGroupLeave,
    Done,
}

struct Button {
    x: f32,
    y: f32,
    w: f32,
    label: String,
    action: Action,
    active: bool,
}
struct Field {
    x: f32,
    y: f32,
    w: f32,
    focus: Focus,
    value: String,
    placeholder: &'static str,
    focused: bool,
}
struct Layout {
    lines: Vec<(String, u32, f32, f32)>,
    buttons: Vec<Button>,
    fields: Vec<Field>,
    /// Rows the list has and rows that fit (for the scroll arrows).
    overflow: Option<(i32, i32)>,
}

/// `text` cut to `width` with an ellipsis.
fn fit(atlas: &HudAtlas, text: &str, width: f32) -> String {
    if HudBatch::text_width(atlas, text) <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    for c in text.chars() {
        let next = format!("{out}{c}…");
        if HudBatch::text_width(atlas, &next) > width {
            break;
        }
        out.push(c);
    }
    format!("{out}…")
}

fn status_text(f: &Friend) -> (&'static str, u32) {
    match f.status.as_str() {
        "hosting" => ("Hosting", blue()),
        "in_game" => ("In game", green()),
        "online" => ("Online", green()),
        _ => ("Offline", dim()),
    }
}

fn layout(atlas: &HudAtlas, (w, h): (f32, f32), ui: &Ui, view: &View) -> Layout {
    let cx = (w / 2.0).floor();
    let width = (w - 20.0).min(300.0);
    let left = cx - (width / 2.0).floor();
    let right = left + width;
    let mut lines = Vec::new();
    let mut buttons = Vec::new();
    let mut fields = Vec::new();
    let mut overflow = None;
    // Header: who we are.
    let sub = if !view.connected {
        "Connecting to Masterix Friends…".to_owned()
    } else if view.me_code.is_empty() {
        "Signing in…".to_owned()
    } else {
        format!("{} · your code: {}", view.me_name, view.me_code)
    };
    lines.push((fit(atlas, &sub, width - 70.0), grey(), left, 21.0));
    buttons.push(Button {
        x: right - 60.0,
        y: 16.0,
        w: 60.0,
        label: if view.copied { "Copied" } else { "Copy Code" }.to_owned(),
        action: Action::CopyCode,
        active: !view.me_code.is_empty(),
    });
    // Tabs.
    let tabs = [
        (
            Tab::Friends,
            {
                let n = view.requests.iter().filter(|r| r.incoming).count() + view.invites.len();
                if n > 0 {
                    format!("Friends ({n})")
                } else {
                    "Friends".to_owned()
                }
            },
        ),
        (
            Tab::Chat,
            {
                let n: usize = view.unread.iter().map(|(_, n)| n).sum();
                if n > 0 {
                    format!("Chat ({n})")
                } else {
                    "Chat".to_owned()
                }
            },
        ),
        (Tab::Host, "Host".to_owned()),
        (
            Tab::Voice,
            if view.voice.talking || view.voice.players.iter().any(|p| p.2) {
                "Voice ●".to_owned()
            } else {
                "Voice".to_owned()
            },
        ),
    ];
    let tab_w = ((width - 12.0) / 4.0).floor();
    for (i, (tab, label)) in tabs.into_iter().enumerate() {
        buttons.push(Button {
            x: left + i as f32 * (tab_w + 4.0),
            y: 34.0,
            w: tab_w,
            label,
            action: Action::Tab(tab),
            active: ui.tab != tab,
        });
    }
    let top = 60.0;
    let bottom = h - 44.0;
    let status_y = h - 40.0;
    match ui.tab {
        Tab::Friends => {
            fields.push(Field {
                x: left,
                y: top,
                w: width - 64.0,
                focus: Focus::AddCode,
                value: ui.add_code.clone(),
                placeholder: "Friend code (MX-XXXXXX)",
                focused: ui.focus == Focus::AddCode,
            });
            buttons.push(Button {
                x: right - 60.0,
                y: top,
                w: 60.0,
                label: "Add".to_owned(),
                action: Action::AddFriend,
                active: ui.add_code.trim().len() >= 4 && view.connected,
            });
            // The rows: invites, requests, friends.
            enum Row<'a> {
                Invite(&'a Invite),
                Request(&'a Request),
                Friend(&'a Friend),
            }
            let mut rows: Vec<Row> = Vec::new();
            rows.extend(view.invites.iter().map(Row::Invite));
            rows.extend(view.requests.iter().filter(|r| r.incoming).map(Row::Request));
            rows.extend(view.friends.iter().map(Row::Friend));
            rows.extend(view.requests.iter().filter(|r| !r.incoming).map(Row::Request));
            let list_top = top + 26.0;
            let fit_rows = ((bottom - list_top) / ROW).floor().max(1.0) as i32;
            let total = rows.len() as i32;
            let scroll = ui.scroll.clamp(0, (total - fit_rows).max(0));
            if total > fit_rows {
                overflow = Some((total, fit_rows));
            }
            if rows.is_empty() {
                let text = if view.connected {
                    "No friends yet. Share your code with a friend, or type theirs above — they get a request to accept."
                } else {
                    "Friends appear here once the connection is up."
                };
                let mut y = list_top + 4.0;
                for line in wrap(text, width, atlas) {
                    lines.push((line, grey(), left, y));
                    y += 10.0;
                }
            }
            let name_w = width - 150.0;
            for (i, row) in rows.iter().enumerate().skip(scroll as usize).take(fit_rows as usize) {
                let y = list_top + (i as i32 - scroll) as f32 * ROW;
                let ty = y + 6.0;
                match row {
                    Row::Invite(inv) => {
                        lines.push((
                            fit(atlas, &format!("{} invited you to {}", inv.from, inv.world), width - 112.0),
                            yellow(),
                            left,
                            ty,
                        ));
                        buttons.push(Button {
                            x: right - 108.0,
                            y,
                            w: 60.0,
                            label: "Join".to_owned(),
                            action: Action::InviteJoin(inv.id, inv.address.clone()),
                            active: true,
                        });
                        buttons.push(Button {
                            x: right - 44.0,
                            y,
                            w: 44.0,
                            label: "Later".to_owned(),
                            action: Action::InviteDismiss(inv.id),
                            active: true,
                        });
                    }
                    Row::Request(r) if r.incoming => {
                        lines.push((
                            fit(atlas, &format!("{} ({}) wants to be friends", r.name, r.code), width - 124.0),
                            yellow(),
                            left,
                            ty,
                        ));
                        buttons.push(Button {
                            x: right - 120.0,
                            y,
                            w: 60.0,
                            label: "Accept".to_owned(),
                            action: Action::Accept(r.id.clone()),
                            active: true,
                        });
                        buttons.push(Button {
                            x: right - 56.0,
                            y,
                            w: 56.0,
                            label: "Decline".to_owned(),
                            action: Action::Decline(r.id.clone()),
                            active: true,
                        });
                    }
                    Row::Request(r) => {
                        lines.push((
                            fit(atlas, &format!("{} · request sent", r.name), width - 64.0),
                            grey(),
                            left,
                            ty,
                        ));
                        buttons.push(Button {
                            x: right - 56.0,
                            y,
                            w: 56.0,
                            label: "Cancel".to_owned(),
                            action: Action::Remove(r.id.clone()),
                            active: true,
                        });
                    }
                    Row::Friend(f) => {
                        let (status, color) = status_text(f);
                        lines.push(("●".to_owned(), color, left, ty));
                        lines.push((fit(atlas, &f.name, name_w - 10.0), WHITE, left + 10.0, ty));
                        let detail = match (&f.status[..], &f.world) {
                            ("hosting", Some(world)) => format!("{status} · {world}"),
                            _ => status.to_owned(),
                        };
                        lines.push((fit(atlas, &detail, 70.0), color, left + name_w, ty));
                        let removing = ui.confirm_remove.as_deref() == Some(f.id.as_str());
                        if removing {
                            buttons.push(Button {
                                x: right - 120.0,
                                y,
                                w: 96.0,
                                label: "Remove friend?".to_owned(),
                                action: Action::Remove(f.id.clone()),
                                active: true,
                            });
                        } else if f.hosting() {
                            buttons.push(Button {
                                x: right - 120.0,
                                y,
                                w: 50.0,
                                label: "Join".to_owned(),
                                action: Action::Join(f.address.clone().unwrap_or_default()),
                                active: true,
                            });
                        } else {
                            buttons.push(Button {
                                x: right - 120.0,
                                y,
                                w: 50.0,
                                label: "Invite".to_owned(),
                                action: Action::Invite(f.id.clone()),
                                active: f.online() && view.host.address.is_some(),
                            });
                        }
                        if !removing {
                            let unread = view.unread_of(&f.id);
                            buttons.push(Button {
                                x: right - 66.0,
                                y,
                                w: 42.0,
                                label: if unread > 0 { format!("Chat {unread}") } else { "Chat".to_owned() },
                                action: Action::OpenChat(f.id.clone()),
                                active: true,
                            });
                        }
                        buttons.push(Button {
                            x: right - 20.0,
                            y,
                            w: 20.0,
                            label: if removing { "✓" } else { "✕" }.to_owned(),
                            action: Action::Remove(f.id.clone()),
                            active: true,
                        });
                    }
                }
            }
        }
        Tab::Chat => match &ui.chat_with {
            None => {
                let list_top = top;
                let fit_rows = ((bottom - list_top) / ROW).floor().max(1.0) as i32;
                let total = view.friends.len() as i32;
                let scroll = ui.scroll.clamp(0, (total - fit_rows).max(0));
                if total > fit_rows {
                    overflow = Some((total, fit_rows));
                }
                if view.friends.is_empty() {
                    lines.push(("Add a friend first — then chat here.".to_owned(), grey(), left, top + 4.0));
                }
                for (i, f) in view.friends.iter().enumerate().skip(scroll as usize).take(fit_rows as usize) {
                    let y = list_top + (i as i32 - scroll) as f32 * ROW;
                    let (_, color) = status_text(f);
                    lines.push(("●".to_owned(), color, left, y + 6.0));
                    lines.push((fit(atlas, &f.name, width - 140.0), WHITE, left + 10.0, y + 6.0));
                    let unread = view.unread_of(&f.id);
                    if unread > 0 {
                        lines.push((format!("{unread} new"), yellow(), right - 110.0, y + 6.0));
                    }
                    buttons.push(Button {
                        x: right - 60.0,
                        y,
                        w: 60.0,
                        label: "Open".to_owned(),
                        action: Action::OpenChat(f.id.clone()),
                        active: true,
                    });
                }
            }
            Some(id) => {
                let name = view.name_of(id);
                let online = view.friends.iter().find(|f| &f.id == id).is_some_and(Friend::online);
                lines.push((
                    fit(atlas, &format!("Chat with {name}{}", if online { "" } else { " (offline)" }), width - 64.0),
                    WHITE,
                    left,
                    top + 2.0,
                ));
                buttons.push(Button {
                    x: right - 56.0,
                    y: top - 4.0,
                    w: 56.0,
                    label: "Back".to_owned(),
                    action: Action::ChatBack,
                    active: true,
                });
                // Messages, newest at the bottom, as many as fit above the input.
                let input_y = bottom - 20.0;
                let area_top = top + 16.0;
                let mut rendered: Vec<(String, u32)> = Vec::new();
                for m in &view.chat {
                    let prefix = if m.mine { "You: " } else { "" };
                    let color = if m.mine { grey() } else { WHITE };
                    for line in wrap(&format!("{prefix}{}", m.body), width, atlas) {
                        rendered.push((line, color));
                    }
                }
                if rendered.is_empty() {
                    rendered.push((format!("Say hi to {name}."), dim()));
                }
                let fit_lines = ((input_y - 4.0 - area_top) / 10.0).floor().max(1.0) as usize;
                let start = rendered.len().saturating_sub(fit_lines);
                for (i, (line, color)) in rendered.into_iter().skip(start).enumerate() {
                    lines.push((line, color, left, area_top + i as f32 * 10.0));
                }
                fields.push(Field {
                    x: left,
                    y: input_y,
                    w: width - 64.0,
                    focus: Focus::Chat,
                    value: ui.chat_text.clone(),
                    placeholder: "Message…",
                    focused: ui.focus == Focus::Chat,
                });
                buttons.push(Button {
                    x: right - 60.0,
                    y: input_y,
                    w: 60.0,
                    label: "Send".to_owned(),
                    action: Action::Send,
                    active: !ui.chat_text.trim().is_empty(),
                });
            }
        },
        Tab::Host => {
            let hv = &view.host;
            let mut y = top;
            let add = |lines: &mut Vec<(String, u32, f32, f32)>, text: &str, color: u32, y: &mut f32| {
                for line in wrap(text, width, atlas) {
                    lines.push((line, color, left, *y));
                    *y += 10.0;
                }
            };
            if !hv.in_world {
                add(&mut lines, "Open a world to host it: friends join through its public address (e4mc, no VPN, no port forwarding).", grey(), &mut y);
            } else if hv.publishing {
                match &hv.address {
                    Some(a) => add(&mut lines, &format!("Your world is public: {a}"), WHITE, &mut y),
                    None => add(&mut lines, "Reaching the relay…", WHITE, &mut y),
                }
                if hv.guests > 0 {
                    add(&mut lines, &format!("{} guest(s) connected.", hv.guests), grey(), &mut y);
                }
                if let Some(p) = &hv.problem {
                    add(&mut lines, p, red(), &mut y);
                }
                y += 2.0;
                let bw = ((width - 8.0) / 3.0).floor();
                buttons.push(Button {
                    x: left,
                    y,
                    w: bw,
                    label: if hv.copied { "Copied" } else { "Copy Address" }.to_owned(),
                    action: Action::CopyAddress,
                    active: hv.address.is_some(),
                });
                buttons.push(Button {
                    x: left + bw + 4.0,
                    y,
                    w: bw,
                    label: "Invite All".to_owned(),
                    action: Action::InviteAll,
                    active: hv.address.is_some(),
                });
                buttons.push(Button {
                    x: left + 2.0 * (bw + 4.0),
                    y,
                    w: bw,
                    label: "Stop".to_owned(),
                    action: Action::StopPublish,
                    active: true,
                });
                y += 24.0;
            } else {
                add(&mut lines, "Publish your world: it opens to LAN and gets a public address friends join from the Friends tab (Join) or an invite.", WHITE, &mut y);
                y += 2.0;
                buttons.push(Button {
                    x: left,
                    y,
                    w: width,
                    label: "Publish World".to_owned(),
                    action: Action::Publish,
                    active: hv.can_publish,
                });
                y += 24.0;
            }
            if hv.in_world {
                y += 2.0;
                lines.push(("Server name friends see (MOTD)".to_owned(), grey(), left, y));
                y += 11.0;
                fields.push(Field {
                    x: left,
                    y,
                    w: width - 64.0,
                    focus: Focus::Motd,
                    value: ui.motd.clone(),
                    placeholder: "{world} - hosted by {name} / Masterix Plus",
                    focused: ui.focus == Focus::Motd,
                });
                buttons.push(Button {
                    x: right - 60.0,
                    y,
                    w: 60.0,
                    label: "Save".to_owned(),
                    action: Action::SaveMotd,
                    active: true,
                });
                y += 24.0;
                let icon = ["Masterix icon", "This world's icon", "Custom file"][ui.icon.min(2)];
                lines.push((format!("Server icon: {icon}"), grey(), left, y + 6.0));
                buttons.push(Button {
                    x: right - 60.0,
                    y,
                    w: 60.0,
                    label: "Change".to_owned(),
                    action: Action::CycleIcon,
                    active: true,
                });
                y += 22.0;
                if ui.icon == 2 && y + 44.0 < bottom {
                    add(&mut lines, "Custom: a 64x64 cs-server-icon-custom.png in this profile's game folder.", dim(), &mut y);
                    y += 2.0;
                }
                lines.push((
                    format!("Players (you included): {}", ui.max_players.max(2)),
                    grey(),
                    left,
                    y + 6.0,
                ));
                buttons.push(Button {
                    x: right - 60.0,
                    y,
                    w: 28.0,
                    label: "-".to_owned(),
                    action: Action::Players(-1),
                    active: ui.max_players > 2,
                });
                buttons.push(Button {
                    x: right - 28.0,
                    y,
                    w: 28.0,
                    label: "+".to_owned(),
                    action: Action::Players(1),
                    active: ui.max_players < 32,
                });
                let _ = y;
            }
        }
        Tab::Voice => {
            let vv = &view.voice;
            let mut y = top;
            let add = |lines: &mut Vec<(String, u32, f32, f32)>, text: &str, color: u32, y: &mut f32| {
                for line in wrap(text, width, atlas) {
                    lines.push((line, color, left, *y));
                    *y += 10.0;
                }
            };
            buttons.push(Button {
                x: left,
                y,
                w: width,
                label: if vv.enabled { "Voice Chat: ON" } else { "Voice Chat: OFF" }.to_owned(),
                action: Action::VoiceEnabled,
                active: true,
            });
            y += 24.0;
            if !vv.enabled {
                add(&mut lines, "Built-in proximity voice chat for Masterix Engine worlds: talk to the players near you in a world you host or join (e4mc, LAN, a Masterix server). No account, no extra app.", grey(), &mut y);
            } else {
                // Status.
                if !vv.in_world {
                    add(&mut lines, "Join or host a world to talk. Voice works in Masterix Engine worlds and servers.", grey(), &mut y);
                } else if !vv.available {
                    add(&mut lines, "This server doesn't carry Masterix voice (not a Masterix Engine / Horizon host).", red(), &mut y);
                } else if let Some(p) = &vv.mic_problem {
                    add(&mut lines, &format!("Microphone: {p}. Allow the microphone permission, then come back."), red(), &mut y);
                } else if vv.talking {
                    let bars = (vv.level * 10.0).round() as usize;
                    add(&mut lines, &format!("Transmitting  {}{}", "|".repeat(bars.min(10)), "·".repeat(10 - bars.min(10))), green(), &mut y);
                } else if vv.ptt {
                    add(&mut lines, "Ready. Hold the Push to Talk key (B, or the Masterix control button bound to it in Key Binds). V opens this menu.", WHITE, &mut y);
                } else {
                    add(&mut lines, "Ready. Open mic: sends when you speak.", WHITE, &mut y);
                }
                y += 2.0;
                // Settings rows.
                let row = |lines: &mut Vec<(String, u32, f32, f32)>, buttons: &mut Vec<Button>, y: f32, text: String, minus: Action, plus: Action, lo: bool, hi: bool| {
                    lines.push((text, grey(), left, y + 6.0));
                    buttons.push(Button { x: right - 60.0, y, w: 28.0, label: "-".to_owned(), action: minus, active: lo });
                    buttons.push(Button { x: right - 28.0, y, w: 28.0, label: "+".to_owned(), action: plus, active: hi });
                };
                lines.push((format!("Mode: {}", if vv.ptt { "Push to talk" } else { "Open mic" }), grey(), left, y + 6.0));
                buttons.push(Button { x: right - 60.0, y, w: 60.0, label: "Change".to_owned(), action: Action::VoiceMode, active: true });
                y += 22.0;
                row(&mut lines, &mut buttons, y, format!("Voice volume: {}%", vv.volume), Action::VoiceVolume(-1), Action::VoiceVolume(1), vv.volume > 0, vv.volume < 200);
                y += 22.0;
                row(&mut lines, &mut buttons, y, format!("Microphone gain: {}%", vv.mic), Action::VoiceMic(-1), Action::VoiceMic(1), vv.mic > 50, vv.mic < 300);
                y += 22.0;
                row(&mut lines, &mut buttons, y, format!("Range (your worlds): {} blocks", vv.range), Action::VoiceRange(-1), Action::VoiceRange(1), vv.range > 8, vv.range < 256);
                y += 24.0;
                // Groups: members hear each other from anywhere.
                if vv.in_world && vv.available {
                    match &vv.group {
                        Some(g) => {
                            lines.push((fit(atlas, &format!("Your group: {g}"), width - 64.0), green(), left, y + 6.0));
                            buttons.push(Button { x: right - 60.0, y, w: 60.0, label: "Leave".to_owned(), action: Action::VoiceGroupLeave, active: true });
                            y += 22.0;
                            add(&mut lines, "Group members hear each other anywhere in the world, at full volume. Players near you still hear you too.", grey(), &mut y);
                        }
                        None => {
                            lines.push(("Create or join a group (members hear each other anywhere)".to_owned(), grey(), left, y));
                            y += 11.0;
                            fields.push(Field {
                                x: left,
                                y,
                                w: width - 64.0,
                                focus: Focus::Group,
                                value: ui.group_name.clone(),
                                placeholder: "Group name",
                                focused: ui.focus == Focus::Group,
                            });
                            buttons.push(Button {
                                x: right - 60.0,
                                y,
                                w: 60.0,
                                label: "Join".to_owned(),
                                action: Action::VoiceGroupJoin(String::new()),
                                active: !ui.group_name.trim().is_empty(),
                            });
                            y += 24.0;
                        }
                    }
                    y += 2.0;
                }
                // Groups and players, one scrolled list: groups first (join
                // another from here), then everyone in the world (mute).
                let list_top = y;
                let fit_rows = ((bottom - list_top) / ROW).floor().max(1.0) as i32;
                let mut rows: Vec<(String, u32, String, Action, bool)> = Vec::new();
                for (name, members, mine) in &vv.groups {
                    let who = if members.is_empty() { String::new() } else { format!("  ({})", members.join(", ")) };
                    rows.push((
                        format!("[Group] {name}{who}"),
                        if *mine { green() } else { WHITE },
                        if *mine { "Leave" } else { "Join" }.to_owned(),
                        if *mine { Action::VoiceGroupLeave } else { Action::VoiceGroupJoin(name.clone()) },
                        false,
                    ));
                }
                for (name, muted, heard) in &vv.players {
                    rows.push((
                        name.clone(),
                        if *heard { green() } else if *muted { red() } else { WHITE },
                        if *muted { "Unmute" } else { "Mute" }.to_owned(),
                        Action::VoiceMute(name.clone()),
                        true,
                    ));
                }
                let total = rows.len() as i32;
                let scroll = ui.scroll.clamp(0, (total - fit_rows).max(0));
                if total > fit_rows {
                    overflow = Some((total, fit_rows));
                }
                if rows.is_empty() && vv.in_world {
                    lines.push(("Nobody else in the world yet.".to_owned(), dim(), left, y + 4.0));
                }
                for (i, (text, color, label, action, dot)) in rows.into_iter().enumerate().skip(scroll as usize).take(fit_rows as usize) {
                    let y = list_top + (i as i32 - scroll) as f32 * ROW;
                    let x = if dot {
                        lines.push(("●".to_owned(), if color == WHITE { dim() } else { color }, left, y + 6.0));
                        left + 10.0
                    } else {
                        left
                    };
                    lines.push((fit(atlas, &text, width - 90.0), color, x, y + 6.0));
                    buttons.push(Button { x: right - 60.0, y, w: 60.0, label, action, active: true });
                }
            }
        }
    }
    // Scroll arrows for long lists.
    if let Some((total, fit_rows)) = overflow {
        let scroll = ui.scroll.clamp(0, (total - fit_rows).max(0));
        buttons.push(Button {
            x: right + 2.0,
            y: top + if ui.tab == Tab::Friends { 26.0 } else { 0.0 },
            w: 16.0,
            label: "▲".to_owned(),
            action: Action::Scroll(-1),
            active: scroll > 0,
        });
        buttons.push(Button {
            x: right + 2.0,
            y: bottom - 20.0,
            w: 16.0,
            label: "▼".to_owned(),
            action: Action::Scroll(1),
            active: scroll < total - fit_rows,
        });
    }
    // The status line: a problem, or a confirmation.
    if let Some(e) = &view.error {
        lines.push((fit(atlas, e, w - 20.0), red(), 10.0, status_y));
    } else if let Some(i) = &view.info {
        lines.push((fit(atlas, i, w - 20.0), green(), 10.0, status_y));
    }
    buttons.push(Button {
        x: cx - 100.0,
        y: h - 27.0,
        w: 200.0,
        label: "gui.done".to_owned(),
        action: Action::Done,
        active: true,
    });
    Layout {
        lines,
        buttons,
        fields,
        overflow,
    }
}

fn label(key: &str) -> Text {
    if key.starts_with("gui.") {
        Text::translate(key, &[])
    } else {
        Text::literal(key)
    }
}

pub fn draw(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    cursor: (f32, f32),
    ui: &Ui,
    view: &View,
    in_world: bool,
) {
    let (w, h) = (batch.width, batch.height);
    ui::menu_background(batch, atlas, 0.0, 0.0, w, h, in_world);
    let title = Text::literal(TITLE);
    let tw = atlas.font.width_text(&title);
    batch.text_styled(atlas, &title, ((w - tw) / 2.0).floor(), 8.0, WHITE, true);
    let l = layout(atlas, (w, h), ui, view);
    for (text, color, x, y) in l.lines {
        batch.text(atlas, &text, x, y, color, true);
    }
    for f in l.fields {
        let border = if f.focused { WHITE } else { grey() };
        batch.fill(atlas, f.x - 1.0, f.y - 1.0, f.w + 2.0, 22.0, border);
        batch.fill(atlas, f.x, f.y, f.w, 20.0, rgba(0, 0, 0, 255));
        let inner = f.w - 8.0;
        if f.value.is_empty() && !f.focused {
            batch.text(atlas, &fit(atlas, f.placeholder, inner), f.x + 4.0, f.y + 6.0, dim(), true);
        } else {
            // The tail of a long value stays in view.
            let mut shown = f.value.clone();
            while HudBatch::text_width(atlas, &shown) > inner && !shown.is_empty() {
                shown.remove(0);
            }
            let adv = batch.text(atlas, &shown, f.x + 4.0, f.y + 6.0, WHITE, true);
            if f.focused && view.blink {
                batch.text(atlas, "_", f.x + 4.0 + adv, f.y + 6.0, WHITE, true);
            }
        }
    }
    for b in l.buttons {
        let hover = b.active && ui::hovered(cursor, b.x, b.y, b.w, 20.0);
        ui::button(batch, atlas, b.x, b.y, b.w, 20.0, &label(&b.label), b.active, hover);
    }
}

/// The button or field under a click.
pub fn click(atlas: &HudAtlas, size: (f32, f32), cursor: (f32, f32), ui: &Ui, view: &View) -> Option<Action> {
    let l = layout(atlas, size, ui, view);
    if let Some(b) = l
        .buttons
        .into_iter()
        .find(|b| b.active && ui::hovered(cursor, b.x, b.y, b.w, 20.0))
    {
        return Some(b.action);
    }
    if let Some(f) = l
        .fields
        .into_iter()
        .find(|f| ui::hovered(cursor, f.x, f.y, f.w, 20.0))
    {
        return Some(Action::Focus(f.focus));
    }
    Some(Action::Focus(Focus::None))
}

/// The rows the open list can scroll by (for the mouse wheel).
pub fn scroll_range(atlas: &HudAtlas, size: (f32, f32), ui: &Ui, view: &View) -> i32 {
    layout(atlas, size, ui, view)
        .overflow
        .map_or(0, |(total, fit_rows)| (total - fit_rows).max(0))
}

/// The focused field's box in GUI pixels (where the IME composition shows).
pub fn focused_box(atlas: &HudAtlas, size: (f32, f32), ui: &Ui, view: &View) -> Option<[f32; 4]> {
    layout(atlas, size, ui, view)
        .fields
        .into_iter()
        .find(|f| f.focused)
        .map(|f| [f.x, f.y, f.w, 20.0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atlas() -> HudAtlas {
        HudAtlas::build(std::path::Path::new("/nonexistent/horizon-test-assets")).unwrap()
    }

    #[test]
    fn tabs_fields_and_done_are_clickable() {
        let atlas = atlas();
        let size = (427.0, 240.0);
        let ui = Ui::default();
        let view = View {
            connected: true,
            me_code: "MX-ABCDEF".into(),
            me_name: "Steve".into(),
            friends: vec![Friend {
                id: "f1".into(),
                name: "Alex".into(),
                code: "CS-1".into(),
                status: "hosting".into(),
                address: Some("abc.e4mc.link".into()),
                world: Some("World".into()),
            }],
            ..View::default()
        };
        let l = layout(&atlas, size, &ui, &view);
        let done = l.buttons.iter().find(|b| b.action == Action::Done).unwrap();
        assert_eq!(click(&atlas, size, (done.x + 1.0, done.y + 1.0), &ui, &view), Some(Action::Done));
        let join = l.buttons.iter().find(|b| matches!(b.action, Action::Join(_))).unwrap();
        assert_eq!(
            click(&atlas, size, (join.x + 1.0, join.y + 1.0), &ui, &view),
            Some(Action::Join("abc.e4mc.link".into()))
        );
        let field = &l.fields[0];
        assert_eq!(
            click(&atlas, size, (field.x + 2.0, field.y + 2.0), &ui, &view),
            Some(Action::Focus(Focus::AddCode))
        );
        assert!(l.lines.iter().all(|(_, _, _, y)| *y + 9.0 <= size.1 - 27.0));
    }
}
