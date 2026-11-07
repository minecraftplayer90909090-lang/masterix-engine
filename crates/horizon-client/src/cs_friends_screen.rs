//! The CS pages inside the game (`game/cs_friends_ui.rs` drives them): four
//! pages that share one screen — **Friends** (your code, add a friend,
//! requests, who is online, Join / Invite / Chat), **Chat** (one-to-one
//! messages), **Host World** (publish the world, the server name and icon
//! friends see, invite everyone) and **Voice Chat** (the built-in voice chat:
//! groups with a name and password, invites, who to mute, the microphone).
//! The Friends page opens the others; key V opens Voice Chat directly.
//!
//! Drawn like the other Horizon screens (menu background, vanilla buttons,
//! Done in the footer) with CS cards on top, and animated: pages settle in,
//! rows follow one another, toggles slide, the microphone orb breathes and
//! swells with the voice, speakers' bars dance, a pressed button ripples.
//! Pure layout here: the game fills a `View`, keeps the `Ui` state, ticks
//! the `Anim` once a frame and acts on the `Action`.
use crate::{
    cs_friends::{Friend, Invite, Message, Request},
    hud::{HudAtlas, HudBatch, rgba},
    multiplayer::wrap,
    text::Text,
    ui,
};
use std::time::Instant;

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
/// A card's face and edge.
fn card() -> u32 {
    rgba(0, 0, 0, 96)
}
fn card_edge() -> u32 {
    rgba(255, 255, 255, 28)
}
fn accent() -> u32 {
    rgba(198, 198, 206, 255)
}

/// How long a page takes to settle in, and each row's lag behind the last.
const PAGE_IN: f32 = 0.26;
const STAGGER: f32 = 0.035;
/// A pressed button's ripple.
const RIPPLE: f32 = 0.32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Friends,
    Chat,
    Host,
    Voice,
}
impl Tab {
    pub fn title(self) -> &'static str {
        match self {
            Tab::Friends => TITLE,
            Tab::Chat => "Chat",
            Tab::Host => "Host World",
            Tab::Voice => "Voice Chat",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    None,
    AddCode,
    Chat,
    Motd,
    /// The host page's relay address.
    Relay,
    /// The voice page's group name.
    Group,
    /// The voice page's group password.
    Password,
}

/// Eased values the game ticks once a frame while the screen is open.
#[derive(Debug, Default)]
pub struct Anim {
    last: Option<Instant>,
    /// Seconds the screen has been open.
    pub clock: f32,
    /// Seconds since the page changed (the settle-in).
    pub page: f32,
    /// Voice chat on (0 off, 1 on), push to talk, talking: eased.
    pub enabled: f32,
    pub ptt: f32,
    pub talking: f32,
    /// The microphone level, eased.
    pub level: f32,
    /// A pressed button's box and the seconds since (the ripple).
    pub pressed: Option<([f32; 4], f32)>,
    /// The hovered button's box, eased towards it (the glow follows the cursor).
    pub hover: Option<([f32; 4], f32)>,
}
impl Anim {
    /// Once a frame: the clocks advance, the eased values chase their targets.
    pub fn tick(&mut self, enabled: bool, ptt: bool, talking: bool, level: f32) {
        let now = Instant::now();
        let dt = self
            .last
            .map_or(0.0, |t| now.duration_since(t).as_secs_f32())
            .clamp(0.0, 0.1);
        self.last = Some(now);
        self.clock += dt;
        self.page += dt;
        let chase = |v: &mut f32, target: f32, speed: f32| {
            *v += (target - *v) * (1.0 - (-speed * dt).exp());
        };
        chase(&mut self.enabled, f32::from(u8::from(enabled)), 14.0);
        chase(&mut self.ptt, f32::from(u8::from(ptt)), 14.0);
        chase(&mut self.talking, f32::from(u8::from(talking)), 12.0);
        let rising = level > self.level;
        chase(&mut self.level, level, if rising { 30.0 } else { 8.0 });
        if let Some((_, age)) = &mut self.pressed {
            *age += dt;
            if *age > RIPPLE {
                self.pressed = None;
            }
        }
        if let Some((_, glow)) = &mut self.hover {
            chase(glow, 1.0, 16.0);
        }
    }
    /// The page changed: it settles in again.
    pub fn page_changed(&mut self) {
        self.page = 0.0;
        self.hover = None;
    }
    /// A button was pressed at `rect`: the ripple.
    pub fn press(&mut self, rect: [f32; 4]) {
        self.pressed = Some((rect, 0.0));
    }
}
fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}
/// Where an element is in its settle-in, 0 (not yet) to 1 (in place).
fn settle(anim: &Anim, delay: f32) -> f32 {
    ease_out((anim.page - delay) / PAGE_IN)
}
fn with_alpha(color: u32, alpha: f32) -> u32 {
    let a = ((color >> 24) as f32 * alpha.clamp(0.0, 1.0)).round() as u32;
    (color & 0x00FF_FFFF) | (a << 24)
}
fn lerp_color(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |shift: u32| {
        let x = ((a >> shift) & 0xFF) as f32;
        let y = ((b >> shift) & 0xFF) as f32;
        ((x + (y - x) * t).round() as u32) << shift
    };
    ch(0) | ch(8) | ch(16) | ch(24)
}

/// The screen's own state (what the player typed and where they are).
#[derive(Debug, Default)]
pub struct Ui {
    pub tab: Tab,
    /// Rows scrolled past in the current list.
    pub scroll: i32,
    /// GUI pixels the Host page is scrolled down by (it is a long form, not a list).
    pub page_scroll: f32,
    pub add_code: String,
    /// The friend whose conversation is open.
    pub chat_with: Option<String>,
    pub chat_text: String,
    pub motd: String,
    /// The voice page: a group name typed to create one.
    pub group_name: String,
    /// The voice page: the password typed (to create with, or to join with).
    pub password: String,
    /// The voice page: a locked group the player is joining (the password is asked).
    pub join_target: Option<String>,
    /// The voice page: the invite picker is open.
    pub inviting: bool,
    pub focus: Focus,
    /// 0 = CS Launcher icon, 1 = the world's icon, 2 = a custom file.
    pub icon: usize,
    /// Players the published world takes, host included; 0: no limit.
    pub max_players: usize,
    /// Who may join through the public address: 0 anyone, 1 CS friends, 2 invited players.
    pub access: u8,
    /// Guests may use commands (Allow Cheats).
    pub cheats: bool,
    /// A relay `host:port` instead of the broker's pick; empty: automatic.
    pub relay: String,
    /// Names invited to this world this session ("invited only" lets them in).
    pub invited: std::collections::BTreeSet<String>,
    /// A friend's Remove was pressed once; the next press removes.
    pub confirm_remove: Option<String>,
    pub anim: Anim,
}
impl Ui {
    pub fn field_mut(&mut self) -> Option<&mut String> {
        match self.focus {
            Focus::None => None,
            Focus::AddCode => Some(&mut self.add_code),
            Focus::Chat => Some(&mut self.chat_text),
            Focus::Motd => Some(&mut self.motd),
            Focus::Relay => Some(&mut self.relay),
            Focus::Group => Some(&mut self.group_name),
            Focus::Password => Some(&mut self.password),
        }
    }
    pub fn field(&self) -> Option<&String> {
        match self.focus {
            Focus::None => None,
            Focus::AddCode => Some(&self.add_code),
            Focus::Chat => Some(&self.chat_text),
            Focus::Motd => Some(&self.motd),
            Focus::Relay => Some(&self.relay),
            Focus::Group => Some(&self.group_name),
            Focus::Password => Some(&self.password),
        }
    }
    pub fn max_len(&self) -> usize {
        match self.focus {
            Focus::AddCode => 12,
            Focus::Chat => 256,
            Focus::Motd => 59,
            Focus::Relay => 64,
            Focus::Group => 24,
            Focus::Password => 24,
            Focus::None => 0,
        }
    }
    /// Go to `tab`: the list starts at the top, the page settles in.
    pub fn go(&mut self, tab: Tab) {
        if self.tab != tab {
            self.anim.page_changed();
        }
        self.tab = tab;
        self.scroll = 0;
        self.page_scroll = 0.0;
        self.focus = Focus::None;
        self.join_target = None;
        self.inviting = false;
    }
}

/// What the host page shows.
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

/// A group on the voice page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroupRow {
    pub name: String,
    /// Joining needs the password (or an invite).
    pub locked: bool,
    /// Members: name ("You" first), speaking now, muted.
    pub members: Vec<(String, bool, bool)>,
    /// The player's own group.
    pub mine: bool,
}

/// The voice page (CS built-in voice chat, game/cs_voice.rs).
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
    /// Players in the world (not in the player's group): name, muted, heard right now.
    pub players: Vec<(String, bool, bool)>,
    /// The group the player is in.
    pub group: Option<String>,
    /// The world's groups.
    pub groups: Vec<GroupRow>,
    /// Invites received: from whom, to which group.
    pub invites: Vec<(String, String)>,
    /// Players who can be invited to the player's group.
    pub invitable: Vec<String>,
    /// The server's last word on a request: text, a problem.
    pub notice: Option<(String, bool)>,
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

/// A voice slider: which setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Knob {
    Volume,
    Mic,
    Range,
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
    /// Who may join: anyone -> friends -> invited.
    Access,
    /// Allow Cheats for guests on/off.
    Cheats,
    /// The relay field's Save.
    SaveRelay,
    /// Voice chat on/off, push-to-talk/open mic, a slider set (0 to 1), mute a player.
    VoiceEnabled,
    VoiceMode,
    VoiceSet(Knob, f32),
    VoiceMute(String),
    /// Create the group typed (with the password typed, if any).
    VoiceGroupCreate,
    /// Join the named group (with the password typed when it is locked).
    VoiceGroupJoin(String),
    /// A locked group: ask for its password first.
    VoiceAskPassword(String),
    VoiceCancelPassword,
    VoiceGroupLeave,
    /// Open / close the invite picker; invite the named player.
    VoiceInviteToggle,
    VoiceInvite(String),
    /// An invite received, by index: accept it or let it go.
    VoiceInviteAccept(usize),
    VoiceInviteDismiss(usize),
    Done,
}

struct Line {
    text: String,
    color: u32,
    x: f32,
    y: f32,
    delay: f32,
}
struct Button {
    x: f32,
    y: f32,
    w: f32,
    label: String,
    action: Action,
    active: bool,
    delay: f32,
}
struct Field {
    x: f32,
    y: f32,
    w: f32,
    focus: Focus,
    value: String,
    placeholder: &'static str,
    focused: bool,
    secret: bool,
    delay: f32,
}
struct Slider {
    x: f32,
    y: f32,
    w: f32,
    label: String,
    value: f32,
    knob: Knob,
    active: bool,
    delay: f32,
}
/// A card behind a section.
struct Panel {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    delay: f32,
}
/// A drawn (not typed) decoration.
enum Mark {
    /// Three dancing bars: a speaker (`active`), or quiet.
    Speaking { x: f32, y: f32, active: bool, color: u32 },
    /// A status dot.
    Dot { x: f32, y: f32, color: u32 },
    /// A padlock.
    Lock { x: f32, y: f32, color: u32 },
    /// The voice on/off switch (it follows `Anim::enabled`).
    Toggle { x: f32, y: f32 },
    /// The microphone orb.
    Orb { x: f32, y: f32 },
}
struct Placed {
    mark: Mark,
    delay: f32,
}
struct Layout {
    lines: Vec<Line>,
    buttons: Vec<Button>,
    fields: Vec<Field>,
    sliders: Vec<Slider>,
    panels: Vec<Panel>,
    marks: Vec<Placed>,
    /// Rows the list has and rows that fit (for the scroll arrows).
    overflow: Option<(i32, i32)>,
    /// A pixel-scrolled page: how far it can scroll down.
    page_overflow: Option<f32>,
    /// The voice switch's hit box (a tap on it toggles voice chat).
    toggle: Option<[f32; 4]>,
    /// The settle-in lag stamped on what is added next.
    delay: f32,
}
/// Where a layout stood before a page was added (for `scroll_page`).
struct PageMark(usize, usize, usize, usize, usize, usize);
impl Layout {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            buttons: Vec::new(),
            fields: Vec::new(),
            sliders: Vec::new(),
            panels: Vec::new(),
            marks: Vec::new(),
            overflow: None,
            page_overflow: None,
            toggle: None,
            delay: 0.0,
        }
    }
    fn page_mark(&self) -> PageMark {
        PageMark(
            self.lines.len(),
            self.buttons.len(),
            self.fields.len(),
            self.sliders.len(),
            self.panels.len(),
            self.marks.len(),
        )
    }
    /// Scroll what was added since `mark` (a page laid out from `top` as if
    /// it had all the room) up by `scroll` pixels, and clip it to
    /// `top..bottom`: whole elements only, so nothing half shows. `content`
    /// is the page's full height; the page remembers how far it can go.
    fn scroll_page(&mut self, mark: PageMark, top: f32, bottom: f32, content: f32, scroll: f32) {
        let max = (content - (bottom - top)).max(0.0);
        let dy = scroll.clamp(0.0, max);
        self.page_overflow = Some(max);
        let inside = |y: f32, h: f32| y >= top - 0.5 && y + h <= bottom + 0.5;
        let PageMark(l, b, f, s, p, m) = mark;
        for line in &mut self.lines[l..] {
            line.y -= dy;
        }
        let kept: Vec<Line> = self.lines.drain(l..).filter(|t| inside(t.y, 9.0)).collect();
        self.lines.extend(kept);
        for e in &mut self.buttons[b..] {
            e.y -= dy;
        }
        let kept: Vec<Button> = self.buttons.drain(b..).filter(|e| inside(e.y, 20.0)).collect();
        self.buttons.extend(kept);
        for e in &mut self.fields[f..] {
            e.y -= dy;
        }
        let kept: Vec<Field> = self.fields.drain(f..).filter(|e| inside(e.y, 20.0)).collect();
        self.fields.extend(kept);
        for e in &mut self.sliders[s..] {
            e.y -= dy;
        }
        let kept: Vec<Slider> = self.sliders.drain(s..).filter(|e| inside(e.y, 20.0)).collect();
        self.sliders.extend(kept);
        // Cards clip to the page instead of vanishing.
        let kept: Vec<Panel> = self
            .panels
            .drain(p..)
            .filter_map(|mut e| {
                e.y -= dy;
                let y0 = e.y.max(top - 4.0);
                let y1 = (e.y + e.h).min(bottom + 4.0);
                (y1 - y0 > 8.0).then(|| {
                    e.y = y0;
                    e.h = y1 - y0;
                    e
                })
            })
            .collect();
        self.panels.extend(kept);
        let kept: Vec<Placed> = self
            .marks
            .drain(m..)
            .filter_map(|mut e| {
                let y = match &mut e.mark {
                    Mark::Speaking { y, .. }
                    | Mark::Dot { y, .. }
                    | Mark::Lock { y, .. }
                    | Mark::Toggle { y, .. }
                    | Mark::Orb { y, .. } => {
                        *y -= dy;
                        *y
                    }
                };
                inside(y - 4.0, 8.0).then_some(e)
            })
            .collect();
        self.marks.extend(kept);
        if let Some(t) = &mut self.toggle {
            t[1] -= dy;
            if !inside(t[1], t[3]) {
                self.toggle = None;
            }
        }
    }
    /// The next elements lag one step more.
    fn step(&mut self) {
        self.delay += STAGGER;
    }
    fn text(&mut self, text: impl Into<String>, color: u32, x: f32, y: f32) {
        self.lines.push(Line {
            text: text.into(),
            color,
            x,
            y,
            delay: self.delay,
        });
    }
    /// A paragraph wrapped to `width`, from `y` down; returns the y after it.
    fn para(&mut self, atlas: &HudAtlas, text: &str, color: u32, x: f32, mut y: f32, width: f32) -> f32 {
        for line in wrap(text, width, atlas) {
            self.text(line, color, x, y);
            y += 10.0;
        }
        y
    }
    fn button(&mut self, x: f32, y: f32, w: f32, label: impl Into<String>, action: Action, active: bool) {
        self.buttons.push(Button {
            x,
            y,
            w,
            label: label.into(),
            action,
            active,
            delay: self.delay,
        });
    }
    #[allow(clippy::too_many_arguments)]
    fn field(&mut self, x: f32, y: f32, w: f32, focus: Focus, value: &str, placeholder: &'static str, focused: bool, secret: bool) {
        self.fields.push(Field {
            x,
            y,
            w,
            focus,
            value: value.to_owned(),
            placeholder,
            focused,
            secret,
            delay: self.delay,
        });
    }
    #[allow(clippy::too_many_arguments)]
    fn slider(&mut self, x: f32, y: f32, w: f32, label: String, value: f32, knob: Knob, active: bool) {
        self.sliders.push(Slider {
            x,
            y,
            w,
            label,
            value,
            knob,
            active,
            delay: self.delay,
        });
    }
    fn panel(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.panels.push(Panel {
            x,
            y,
            w,
            h,
            delay: self.delay,
        });
    }
    fn mark(&mut self, mark: Mark) {
        self.marks.push(Placed {
            mark,
            delay: self.delay,
        });
    }
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

/// The page's frame: where the content goes.
#[derive(Clone, Copy)]
struct Frame {
    left: f32,
    right: f32,
    width: f32,
    top: f32,
    bottom: f32,
}

fn layout(atlas: &HudAtlas, (w, h): (f32, f32), ui: &Ui, view: &View) -> Layout {
    let cx = (w / 2.0).floor();
    let width = (w - 20.0).min(320.0);
    let left = cx - (width / 2.0).floor();
    let right = left + width;
    let mut l = Layout::new();
    let status_y = h - 40.0;
    let bottom = h - 44.0;
    let top = match ui.tab {
        Tab::Friends => {
            // Header: who we are, and the other pages.
            let sub = if !view.connected {
                "Connecting to Masterix Friends…".to_owned()
            } else if view.me_code.is_empty() {
                "Signing in…".to_owned()
            } else {
                format!("{} · your code: {}", view.me_name, view.me_code)
            };
            l.text(fit(atlas, &sub, width - 70.0), grey(), left, 21.0);
            l.button(
                right - 60.0,
                16.0,
                60.0,
                if view.copied { "Copied" } else { "Copy Code" },
                Action::CopyCode,
                !view.me_code.is_empty(),
            );
            l.step();
            let unread: usize = view.unread.iter().map(|(_, n)| n).sum();
            let pages = [
                (
                    Tab::Chat,
                    if unread > 0 {
                        format!("Chat ({unread})")
                    } else {
                        "Chat".to_owned()
                    },
                ),
                (Tab::Host, "Host World".to_owned()),
                (
                    Tab::Voice,
                    if view.voice.talking || view.voice.players.iter().any(|p| p.2) {
                        "Voice Chat ●".to_owned()
                    } else {
                        "Voice Chat".to_owned()
                    },
                ),
            ];
            let bw = ((width - 8.0) / 3.0).floor();
            for (i, (tab, label)) in pages.into_iter().enumerate() {
                l.button(left + i as f32 * (bw + 4.0), 34.0, bw, label, Action::Tab(tab), true);
            }
            l.step();
            60.0
        }
        _ => {
            // A page of its own: back to Friends on the left.
            l.button(10.0, 8.0, 60.0, "‹ Friends", Action::Tab(Tab::Friends), true);
            l.step();
            34.0
        }
    };
    let frame = Frame {
        left,
        right,
        width,
        top,
        bottom,
    };
    match ui.tab {
        Tab::Friends => friends_page(atlas, &mut l, &frame, ui, view),
        Tab::Chat => chat_page(atlas, &mut l, &frame, ui, view),
        Tab::Host => {
            let mark = l.page_mark();
            let content = host_page(atlas, &mut l, &frame, ui, view);
            l.scroll_page(mark, top, bottom, content, ui.page_scroll);
        }
        Tab::Voice => voice_page(atlas, &mut l, &frame, ui, view),
    }
    // Scroll arrows for a long page (a finger drag scrolls it too).
    if let Some(max) = l.page_overflow
        && max > 0.0
    {
        let at = ui.page_scroll.clamp(0.0, max);
        l.button(right + 2.0, top, 16.0, "▲", Action::Scroll(-1), at > 0.0);
        l.button(right + 2.0, bottom - 20.0, 16.0, "▼", Action::Scroll(1), at < max);
    }
    // Scroll arrows for long lists.
    if let Some((total, fit_rows)) = l.overflow {
        let scroll = ui.scroll.clamp(0, (total - fit_rows).max(0));
        let list_top = top + if ui.tab == Tab::Friends { 26.0 } else { 0.0 };
        l.button(right + 2.0, list_top, 16.0, "▲", Action::Scroll(-1), scroll > 0);
        l.button(right + 2.0, bottom - 20.0, 16.0, "▼", Action::Scroll(1), scroll < total - fit_rows);
    }
    // The status line: a problem, or a confirmation.
    l.delay = 0.0;
    if let Some(e) = &view.error {
        l.text(fit(atlas, e, w - 20.0), red(), 10.0, status_y);
    } else if let Some(i) = &view.info {
        l.text(fit(atlas, i, w - 20.0), green(), 10.0, status_y);
    }
    l.button(cx - 100.0, h - 27.0, 200.0, "gui.done", Action::Done, true);
    l
}

fn friends_page(atlas: &HudAtlas, l: &mut Layout, f: &Frame, ui: &Ui, view: &View) {
    let Frame {
        left,
        right,
        width,
        top,
        bottom,
    } = *f;
    l.field(
        left,
        top,
        width - 64.0,
        Focus::AddCode,
        &ui.add_code,
        "Friend code (MX-XXXXXX)",
        ui.focus == Focus::AddCode,
        false,
    );
    l.button(
        right - 60.0,
        top,
        60.0,
        "Add",
        Action::AddFriend,
        ui.add_code.trim().len() >= 4 && view.connected,
    );
    l.step();
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
        l.overflow = Some((total, fit_rows));
    }
    if rows.is_empty() {
        let text = if view.connected {
            "No friends yet. Share your code with a friend, or type theirs above — they get a request to accept."
        } else {
            "Friends appear here once the connection is up."
        };
        l.para(atlas, text, grey(), left, list_top + 4.0, width);
    }
    // The name on the left, the status beside it, the buttons on the right —
    // the status ends before the buttons begin.
    let buttons_x = right - 120.0;
    let name_w = (width * 0.42).floor();
    let status_w = buttons_x - 6.0 - (left + 10.0 + name_w);
    for (i, row) in rows.iter().enumerate().skip(scroll as usize).take(fit_rows as usize) {
        let y = list_top + (i as i32 - scroll) as f32 * ROW;
        let ty = y + 6.0;
        match row {
            Row::Invite(inv) => {
                l.text(
                    fit(atlas, &format!("{} invited you to {}", inv.from, inv.world), width - 112.0),
                    yellow(),
                    left,
                    ty,
                );
                l.button(right - 108.0, y, 60.0, "Join", Action::InviteJoin(inv.id, inv.address.clone()), true);
                l.button(right - 44.0, y, 44.0, "Later", Action::InviteDismiss(inv.id), true);
            }
            Row::Request(r) if r.incoming => {
                l.text(
                    fit(atlas, &format!("{} ({}) wants to be friends", r.name, r.code), width - 124.0),
                    yellow(),
                    left,
                    ty,
                );
                l.button(right - 120.0, y, 60.0, "Accept", Action::Accept(r.id.clone()), true);
                l.button(right - 56.0, y, 56.0, "Decline", Action::Decline(r.id.clone()), true);
            }
            Row::Request(r) => {
                l.text(fit(atlas, &format!("{} · request sent", r.name), width - 64.0), grey(), left, ty);
                l.button(right - 56.0, y, 56.0, "Cancel", Action::Remove(r.id.clone()), true);
            }
            Row::Friend(f) => {
                let (status, color) = status_text(f);
                l.mark(Mark::Dot { x: left + 2.0, y: ty + 1.0, color });
                l.text(fit(atlas, &f.name, name_w - 2.0), WHITE, left + 10.0, ty);
                let detail = match (&f.status[..], &f.world) {
                    ("hosting", Some(world)) => format!("{status} · {world}"),
                    _ => status.to_owned(),
                };
                l.text(fit(atlas, &detail, status_w), color, left + 10.0 + name_w, ty);
                let removing = ui.confirm_remove.as_deref() == Some(f.id.as_str());
                if removing {
                    l.button(buttons_x, y, 96.0, "Remove friend?", Action::Remove(f.id.clone()), true);
                } else if f.hosting() {
                    l.button(
                        buttons_x,
                        y,
                        50.0,
                        "Join",
                        Action::Join(f.address.clone().unwrap_or_default()),
                        true,
                    );
                } else {
                    l.button(
                        buttons_x,
                        y,
                        50.0,
                        "Invite",
                        Action::Invite(f.id.clone()),
                        f.online() && view.host.address.is_some(),
                    );
                }
                if !removing {
                    let unread = view.unread_of(&f.id);
                    l.button(
                        right - 66.0,
                        y,
                        42.0,
                        if unread > 0 { format!("Chat {unread}") } else { "Chat".to_owned() },
                        Action::OpenChat(f.id.clone()),
                        true,
                    );
                }
                l.button(
                    right - 20.0,
                    y,
                    20.0,
                    if removing { "✓" } else { "✕" },
                    Action::Remove(f.id.clone()),
                    true,
                );
            }
        }
        l.step();
    }
}

fn chat_page(atlas: &HudAtlas, l: &mut Layout, f: &Frame, ui: &Ui, view: &View) {
    let Frame {
        left,
        right,
        width,
        top,
        bottom,
    } = *f;
    match &ui.chat_with {
        None => {
            let list_top = top;
            let fit_rows = ((bottom - list_top) / ROW).floor().max(1.0) as i32;
            let total = view.friends.len() as i32;
            let scroll = ui.scroll.clamp(0, (total - fit_rows).max(0));
            if total > fit_rows {
                l.overflow = Some((total, fit_rows));
            }
            if view.friends.is_empty() {
                l.text("Add a friend first — then chat here.", grey(), left, top + 4.0);
            }
            for (i, f) in view.friends.iter().enumerate().skip(scroll as usize).take(fit_rows as usize) {
                let y = list_top + (i as i32 - scroll) as f32 * ROW;
                let (_, color) = status_text(f);
                l.mark(Mark::Dot { x: left + 2.0, y: y + 7.0, color });
                l.text(fit(atlas, &f.name, width - 140.0), WHITE, left + 10.0, y + 6.0);
                let unread = view.unread_of(&f.id);
                if unread > 0 {
                    l.text(format!("{unread} new"), yellow(), right - 110.0, y + 6.0);
                }
                l.button(right - 60.0, y, 60.0, "Open", Action::OpenChat(f.id.clone()), true);
                l.step();
            }
        }
        Some(id) => {
            let name = view.name_of(id);
            let online = view.friends.iter().find(|f| &f.id == id).is_some_and(Friend::online);
            l.text(
                fit(atlas, &format!("Chat with {name}{}", if online { "" } else { " (offline)" }), width - 64.0),
                WHITE,
                left,
                top + 2.0,
            );
            l.button(right - 56.0, top - 4.0, 56.0, "Back", Action::ChatBack, true);
            l.step();
            // Messages, newest at the bottom, as many as fit above the input.
            let input_y = bottom - 20.0;
            let area_top = top + 16.0;
            l.panel(left - 4.0, area_top - 4.0, width + 8.0, input_y - area_top);
            // Each line may carry a Join button (a world invite sent in the chat).
            let mut rendered: Vec<(String, u32, Option<String>)> = Vec::new();
            for m in &view.chat {
                let prefix = if m.mine { "You: " } else { "" };
                let color = if m.mine { grey() } else { WHITE };
                let text_width = if m.invite.is_some() && !m.mine { width - 68.0 } else { width };
                let mut join = if m.mine { None } else { m.invite.clone() };
                let lines = wrap(&format!("{prefix}{}", m.body), text_width, atlas);
                for line in lines {
                    rendered.push((line, color, join.take()));
                }
                if m.invite.is_some() && !m.mine {
                    // Room for the button's lower half.
                    rendered.push((String::new(), color, None));
                }
            }
            if rendered.is_empty() {
                rendered.push((format!("Say hi to {name}."), dim(), None));
            }
            let fit_lines = ((input_y - 8.0 - area_top) / 10.0).floor().max(1.0) as usize;
            let start = rendered.len().saturating_sub(fit_lines);
            for (i, (line, color, join)) in rendered.into_iter().skip(start).enumerate() {
                let y = area_top + i as f32 * 10.0;
                l.text(line, color, left, y);
                if let Some(address) = join {
                    l.button(right - 60.0, y - 2.0, 60.0, "Join", Action::Join(address), true);
                }
            }
            l.step();
            l.field(left, input_y, width - 64.0, Focus::Chat, &ui.chat_text, "Message…", ui.focus == Focus::Chat, false);
            l.button(right - 60.0, input_y, 60.0, "Send", Action::Send, !ui.chat_text.trim().is_empty());
        }
    }
}

/// The Host page, laid out from `top` with all the room it wants; returns
/// its height (the caller scrolls and clips it).
fn host_page(atlas: &HudAtlas, l: &mut Layout, f: &Frame, ui: &Ui, view: &View) -> f32 {
    let Frame {
        left,
        right,
        width,
        top,
        ..
    } = *f;
    let hv = &view.host;
    let mut y = top;
    // The world card: its state and the publish buttons.
    let card_top = y;
    y += 6.0;
    if !hv.in_world {
        l.mark(Mark::Dot { x: left + 6.0, y: y + 1.0, color: dim() });
        l.text("No world open", grey(), left + 14.0, y);
        y += 12.0;
        y = l.para(
            atlas,
            "Open a world to host it: friends join through its public address (e4mc, no VPN, no port forwarding).",
            grey(),
            left + 4.0,
            y,
            width - 8.0,
        );
        y += 6.0;
    } else if hv.publishing {
        l.mark(Mark::Dot { x: left + 6.0, y: y + 1.0, color: green() });
        l.text(fit(atlas, &format!("{} · public", hv.world), width - 20.0), WHITE, left + 14.0, y);
        y += 12.0;
        match &hv.address {
            Some(a) => y = l.para(atlas, &format!("Address: {a}"), blue(), left + 4.0, y, width - 8.0),
            None => y = l.para(atlas, "Reaching the relay…", grey(), left + 4.0, y, width - 8.0),
        }
        let guests = match hv.guests {
            0 => "Nobody has joined yet.".to_owned(),
            1 => "1 guest connected.".to_owned(),
            n => format!("{n} guests connected."),
        };
        y = l.para(atlas, &guests, grey(), left + 4.0, y, width - 8.0);
        if let Some(p) = &hv.problem {
            y = l.para(atlas, p, red(), left + 4.0, y, width - 8.0);
        }
        y += 4.0;
        let bw = ((width - 16.0) / 3.0).floor();
        l.button(
            left + 4.0,
            y,
            bw,
            if hv.copied { "Copied" } else { "Copy Address" },
            Action::CopyAddress,
            hv.address.is_some(),
        );
        l.button(left + 4.0 + bw + 4.0, y, bw, "Invite All", Action::InviteAll, hv.address.is_some());
        l.button(left + 4.0 + 2.0 * (bw + 4.0), y, bw, "Stop", Action::StopPublish, true);
        y += 26.0;
    } else {
        l.mark(Mark::Dot { x: left + 6.0, y: y + 1.0, color: yellow() });
        l.text(fit(atlas, &format!("{} · private", hv.world), width - 20.0), WHITE, left + 14.0, y);
        y += 12.0;
        y = l.para(
            atlas,
            "Publish your world: it opens to LAN and gets a public address friends join from the Friends page or an invite.",
            grey(),
            left + 4.0,
            y,
            width - 8.0,
        );
        y += 4.0;
        l.button(left + 4.0, y, width - 8.0, "Publish World", Action::Publish, hv.can_publish);
        y += 26.0;
    }
    l.panel(left - 4.0, card_top - 4.0, width + 8.0, y - card_top + 4.0);
    l.step();
    if !hv.in_world {
        return y - top;
    }
    y += 8.0;
    // The server card: what friends see.
    let card_top = y;
    y += 6.0;
    l.text("What friends see", accent(), left + 4.0, y);
    y += 12.0;
    l.text("Server name (MOTD)", grey(), left + 4.0, y);
    y += 11.0;
    l.field(
        left + 4.0,
        y,
        width - 72.0,
        Focus::Motd,
        &ui.motd,
        "{world} - hosted by {name} / Masterix Plus",
        ui.focus == Focus::Motd,
        false,
    );
    l.button(right - 64.0, y, 60.0, "Save", Action::SaveMotd, true);
    y += 24.0;
    l.step();
    let icon = ["Masterix icon", "This world's icon", "Custom file"][ui.icon.min(2)];
    l.text(format!("Server icon: {icon}"), grey(), left + 4.0, y + 6.0);
    l.button(right - 64.0, y, 60.0, "Change", Action::CycleIcon, true);
    y += 22.0;
    if ui.icon == 2 {
        y = l.para(
            atlas,
            "Custom: a 64x64 cs-server-icon-custom.png in this profile's game folder.",
            dim(),
            left + 4.0,
            y,
            width - 8.0,
        );
        y += 2.0;
    }
    l.panel(left - 4.0, card_top - 4.0, width + 8.0, y - card_top + 4.0);
    l.step();
    y += 8.0;
    // The hosting settings card: who gets in, how many, cheats, the relay.
    let card_top = y;
    y += 6.0;
    l.text("World hosting settings", accent(), left + 4.0, y);
    y += 12.0;
    let players = match ui.max_players {
        0 => "No limit".to_owned(),
        n => n.max(2).to_string(),
    };
    l.text(format!("Players (you included): {players}"), grey(), left + 4.0, y + 6.0);
    l.button(right - 64.0, y, 28.0, "-", Action::Players(-1), ui.max_players != 2);
    l.button(right - 32.0, y, 28.0, "+", Action::Players(1), ui.max_players != 0);
    y += 24.0;
    l.step();
    let access = ["Anyone with the address", "CS friends only", "Invited players only"][usize::from(ui.access.min(2))];
    l.text(fit(atlas, &format!("Who can join: {access}"), width - 76.0), grey(), left + 4.0, y + 6.0);
    l.button(right - 64.0, y, 60.0, "Change", Action::Access, true);
    y += 24.0;
    if ui.access == 2 {
        let n = ui.invited.len();
        y = l.para(
            atlas,
            &if n == 0 {
                "Nobody is invited yet: Invite a friend from the Friends page or Invite All above.".to_owned()
            } else {
                format!("{n} invited this session.")
            },
            dim(),
            left + 4.0,
            y,
            width - 8.0,
        );
        y += 2.0;
    }
    l.step();
    l.text(
        format!("Allow cheats for guests: {}", if ui.cheats { "ON" } else { "OFF" }),
        grey(),
        left + 4.0,
        y + 6.0,
    );
    l.button(right - 64.0, y, 60.0, if ui.cheats { "Turn off" } else { "Turn on" }, Action::Cheats, true);
    y += 24.0;
    l.step();
    l.text("Relay (e4mc): automatic picks the least loaded relay", grey(), left + 4.0, y);
    y += 11.0;
    l.field(
        left + 4.0,
        y,
        width - 72.0,
        Focus::Relay,
        &ui.relay,
        "Automatic (or host:port)",
        ui.focus == Focus::Relay,
        false,
    );
    l.button(right - 64.0, y, 60.0, "Save", Action::SaveRelay, true);
    y += 24.0;
    y = l.para(
        atlas,
        "The relay only carries the connection; the world runs on this device. Same-Wi-Fi players always join over LAN.",
        dim(),
        left + 4.0,
        y,
        width - 8.0,
    );
    y += 2.0;
    l.panel(left - 4.0, card_top - 4.0, width + 8.0, y - card_top + 4.0);
    y + 6.0 - top
}

fn voice_page(atlas: &HudAtlas, l: &mut Layout, f: &Frame, ui: &Ui, view: &View) {
    let Frame {
        left,
        right,
        width,
        top,
        bottom,
    } = *f;
    let vv = &view.voice;
    let mut y = top;
    // The microphone card: the orb, the switch, the mode and the state.
    let card_top = y;
    l.mark(Mark::Orb { x: left + 22.0, y: y + 22.0 });
    l.mark(Mark::Toggle { x: right - 30.0, y: y + 7.0 });
    l.toggle = Some([right - 34.0, y - 2.0, 30.0, 18.0]);
    l.button(
        right - 104.0,
        y + 2.0,
        66.0,
        if vv.enabled { "Voice: ON" } else { "Voice: OFF" },
        Action::VoiceEnabled,
        true,
    );
    let state_x = left + 48.0;
    let state_w = right - 104.0 - state_x;
    let (state, color) = if !vv.enabled {
        ("Voice chat is off", grey())
    } else if !vv.in_world {
        ("Join or host a world to talk", grey())
    } else if !vv.available {
        ("This server has no Masterix voice", red())
    } else if vv.mic_problem.is_some() {
        ("Microphone unavailable", red())
    } else if vv.talking {
        ("Transmitting", green())
    } else if vv.ptt {
        ("Ready · hold B to talk", WHITE)
    } else {
        ("Ready · open mic", WHITE)
    };
    l.text(fit(atlas, state, state_w), color, state_x, y + 8.0);
    let hint = if !vv.enabled {
        "Proximity voice for Masterix Engine worlds: no account, no extra app.".to_owned()
    } else if let Some(p) = &vv.mic_problem {
        format!("{p}. Allow the microphone, then come back.")
    } else if vv.ptt {
        "Push to talk: B, or the Masterix control bound to it. V opens this page.".to_owned()
    } else {
        "Open mic: sends when you speak. V opens this page.".to_owned()
    };
    l.text(fit(atlas, &hint, state_w + 36.0), dim(), state_x, y + 20.0);
    y += 36.0;
    if vv.enabled {
        l.text(
            format!("Mode: {}", if vv.ptt { "Push to talk" } else { "Open mic" }),
            grey(),
            left + 4.0,
            y + 6.0,
        );
        l.button(right - 64.0, y, 60.0, "Change", Action::VoiceMode, true);
        y += 24.0;
        l.step();
        // The sliders, side by side.
        let sw = ((width - 16.0) / 3.0).floor();
        l.slider(
            left + 4.0,
            y,
            sw,
            format!("Volume {}%", vv.volume),
            vv.volume as f32 / 200.0,
            Knob::Volume,
            true,
        );
        l.slider(
            left + 4.0 + sw + 4.0,
            y,
            sw,
            format!("Mic {}%", vv.mic),
            (vv.mic as f32 - 50.0) / 250.0,
            Knob::Mic,
            true,
        );
        l.slider(
            left + 4.0 + 2.0 * (sw + 4.0),
            y,
            sw,
            format!("Range {}", vv.range),
            (vv.range as f32 - 8.0) / 248.0,
            Knob::Range,
            true,
        );
        y += 24.0;
    }
    l.panel(left - 4.0, card_top - 4.0, width + 8.0, y - card_top + 4.0);
    l.step();
    if !vv.enabled || !vv.in_world || !vv.available {
        return;
    }
    y += 8.0;
    // Invites received: a card each.
    for (i, (from, group)) in vv.invites.iter().enumerate().take(3) {
        l.panel(left - 4.0, y - 4.0, width + 8.0, 28.0);
        l.mark(Mark::Dot { x: left + 4.0, y: y + 7.0, color: yellow() });
        l.text(
            fit(atlas, &format!("{from} invited you to {group}"), width - 124.0),
            yellow(),
            left + 12.0,
            y + 6.0,
        );
        l.button(right - 108.0, y, 56.0, "Join", Action::VoiceInviteAccept(i), true);
        l.button(right - 48.0, y, 44.0, "Later", Action::VoiceInviteDismiss(i), true);
        y += 32.0;
        l.step();
    }
    // A word from the server (joined, wrong password…).
    if let Some((text, problem)) = &vv.notice {
        l.text(
            fit(atlas, text, width),
            if *problem { red() } else { green() },
            left,
            y,
        );
        y += 14.0;
    }
    // The group card.
    let card_top = y;
    y += 6.0;
    let mine = vv.groups.iter().find(|g| g.mine);
    match mine {
        Some(g) => {
            if g.locked {
                l.mark(Mark::Lock { x: left + 4.0, y: y - 1.0, color: green() });
            } else {
                l.mark(Mark::Dot { x: left + 5.0, y: y + 1.0, color: green() });
            }
            l.text(fit(atlas, &format!("Group · {}", g.name), width - 140.0), green(), left + 14.0, y);
            l.button(
                right - 128.0,
                y - 6.0,
                60.0,
                if ui.inviting { "Close" } else { "Invite" },
                Action::VoiceInviteToggle,
                true,
            );
            l.button(right - 64.0, y - 6.0, 60.0, "Leave", Action::VoiceGroupLeave, true);
            y += 16.0;
            l.step();
            if ui.inviting {
                l.text("Invite to the group:", grey(), left + 4.0, y);
                y += 11.0;
                if vv.invitable.is_empty() {
                    l.text("Everyone in the world is in your group already.", dim(), left + 4.0, y + 2.0);
                    y += 14.0;
                } else {
                    for name in vv.invitable.iter().take(6) {
                        l.text(fit(atlas, name, width - 80.0), WHITE, left + 4.0, y + 6.0);
                        l.button(right - 64.0, y, 60.0, "Invite", Action::VoiceInvite(name.clone()), true);
                        y += ROW;
                        l.step();
                    }
                }
            } else {
                for (name, speaking, muted) in &g.members {
                    let you = name == "You";
                    let color = if *speaking {
                        green()
                    } else if *muted {
                        red()
                    } else if you {
                        accent()
                    } else {
                        WHITE
                    };
                    l.mark(Mark::Speaking { x: left + 4.0, y: y + 3.0, active: *speaking, color });
                    l.text(fit(atlas, name, width - 90.0), color, left + 18.0, y + 6.0);
                    if !you {
                        l.button(
                            right - 64.0,
                            y,
                            60.0,
                            if *muted { "Unmute" } else { "Mute" },
                            Action::VoiceMute(name.clone()),
                            true,
                        );
                    }
                    y += ROW;
                    l.step();
                }
                l.text(
                    "Members hear each other anywhere in the world. Invited players join without the password.",
                    dim(),
                    left + 4.0,
                    y,
                );
                y += 12.0;
            }
        }
        None => match &ui.join_target {
            Some(name) => {
                l.mark(Mark::Lock { x: left + 4.0, y: y - 1.0, color: yellow() });
                l.text(fit(atlas, &format!("{name} needs a password"), width - 20.0), yellow(), left + 14.0, y);
                y += 14.0;
                l.step();
                l.field(
                    left + 4.0,
                    y,
                    width - 136.0,
                    Focus::Password,
                    &ui.password,
                    "Password",
                    ui.focus == Focus::Password,
                    true,
                );
                l.button(
                    right - 128.0,
                    y,
                    60.0,
                    "Join",
                    Action::VoiceGroupJoin(name.clone()),
                    !ui.password.trim().is_empty(),
                );
                l.button(right - 64.0, y, 60.0, "Cancel", Action::VoiceCancelPassword, true);
                y += 26.0;
            }
            None => {
                l.text("Create a group", accent(), left + 4.0, y);
                y += 12.0;
                l.step();
                let fw = ((width - 80.0) / 2.0).floor();
                l.field(
                    left + 4.0,
                    y,
                    fw,
                    Focus::Group,
                    &ui.group_name,
                    "Group name",
                    ui.focus == Focus::Group,
                    false,
                );
                l.field(
                    left + 4.0 + fw + 4.0,
                    y,
                    fw,
                    Focus::Password,
                    &ui.password,
                    "Password (optional)",
                    ui.focus == Focus::Password,
                    true,
                );
                l.button(
                    right - 64.0,
                    y,
                    60.0,
                    "Create",
                    Action::VoiceGroupCreate,
                    !ui.group_name.trim().is_empty(),
                );
                y += 26.0;
            }
        },
    }
    l.panel(left - 4.0, card_top - 4.0, width + 8.0, y - card_top + 4.0);
    l.step();
    y += 8.0;
    // Other groups to join, then the players nearby to mute: one scrolled list.
    enum Row<'a> {
        Group(&'a GroupRow),
        Player(&'a (String, bool, bool)),
    }
    let mut rows: Vec<Row> = Vec::new();
    rows.extend(vv.groups.iter().filter(|g| !g.mine).map(Row::Group));
    rows.extend(vv.players.iter().map(Row::Player));
    let head_h = 12.0;
    let list_top = y + head_h;
    let fit_rows = ((bottom - list_top) / ROW).floor().max(1.0) as i32;
    let total = rows.len() as i32;
    let scroll = ui.scroll.clamp(0, (total - fit_rows).max(0));
    if total > fit_rows {
        l.overflow = Some((total, fit_rows));
    }
    let other_groups = vv.groups.iter().filter(|g| !g.mine).count();
    let head = match (other_groups, vv.players.len()) {
        (0, 0) => "Nobody else in the world yet".to_owned(),
        (0, n) => format!("Players nearby ({n})"),
        (g, n) => format!("Groups ({g}) · players ({n})"),
    };
    l.text(head, accent(), left, y);
    l.step();
    for (i, row) in rows.iter().enumerate().skip(scroll as usize).take(fit_rows as usize) {
        let y = list_top + (i as i32 - scroll) as f32 * ROW;
        match row {
            Row::Group(g) => {
                if g.locked {
                    l.mark(Mark::Lock { x: left + 2.0, y: y + 5.0, color: grey() });
                } else {
                    l.mark(Mark::Dot { x: left + 3.0, y: y + 7.0, color: grey() });
                }
                let who: Vec<&str> = g.members.iter().map(|m| m.0.as_str()).collect();
                let text = format!("{}  ({})", g.name, who.join(", "));
                l.text(fit(atlas, &text, width - 84.0), WHITE, left + 12.0, y + 6.0);
                let action = if g.locked {
                    Action::VoiceAskPassword(g.name.clone())
                } else {
                    Action::VoiceGroupJoin(g.name.clone())
                };
                l.button(right - 64.0, y, 60.0, "Join", action, true);
            }
            Row::Player((name, muted, heard)) => {
                let color = if *heard {
                    green()
                } else if *muted {
                    red()
                } else {
                    WHITE
                };
                l.mark(Mark::Speaking { x: left, y: y + 3.0, active: *heard, color });
                l.text(fit(atlas, name, width - 84.0), color, left + 14.0, y + 6.0);
                l.button(
                    right - 64.0,
                    y,
                    60.0,
                    if *muted { "Unmute" } else { "Mute" },
                    Action::VoiceMute(name.clone()),
                    true,
                );
            }
        }
        l.step();
    }
}

fn label(key: &str) -> Text {
    if key.starts_with("gui.") {
        Text::translate(key, &[])
    } else {
        Text::literal(key)
    }
}

/// What was drawn since `start` settles in: it rises into place and fades
/// in; `false` when it is not due yet (and was dropped).
fn settle_since(batch: &mut HudBatch, start: usize, anim: &Anim, delay: f32) -> bool {
    let p = settle(anim, delay);
    if p <= 0.0 {
        batch.vertices.truncate(start);
        return false;
    }
    if p >= 1.0 {
        return true;
    }
    let dy = (1.0 - p) * 10.0;
    for v in &mut batch.vertices[start..] {
        v.pos[1] += dy;
        v.color = with_alpha(v.color, p);
    }
    true
}

/// A filled disc, as a fan of quads (each spans two of the sixteen segments).
fn disc(batch: &mut HudBatch, atlas: &HudAtlas, cx: f32, cy: f32, r: f32, color: u32) {
    const N: usize = 16;
    let step = 2.0 * std::f32::consts::PI / N as f32;
    let p = |a: f32| [cx + r * a.cos(), cy + r * a.sin()];
    for i in 0..N / 2 {
        let a0 = i as f32 * 2.0 * step;
        batch.fill_quad(atlas, [[cx, cy], p(a0), p(a0 + step), p(a0 + 2.0 * step)], color);
    }
}
/// A ring between two radii.
fn ring(batch: &mut HudBatch, atlas: &HudAtlas, cx: f32, cy: f32, r0: f32, r1: f32, color: u32) {
    const N: usize = 20;
    for i in 0..N {
        let a0 = i as f32 * 2.0 * std::f32::consts::PI / N as f32;
        let a1 = a0 + 2.0 * std::f32::consts::PI / N as f32;
        let p = |a: f32, r: f32| [cx + r * a.cos(), cy + r * a.sin()];
        batch.fill_quad(atlas, [p(a0, r0), p(a0, r1), p(a1, r1), p(a1, r0)], color);
    }
}
/// A 1px frame round a rectangle.
fn frame(batch: &mut HudBatch, atlas: &HudAtlas, x: f32, y: f32, w: f32, h: f32, color: u32) {
    batch.fill(atlas, x, y, w, 1.0, color);
    batch.fill(atlas, x, y + h - 1.0, w, 1.0, color);
    batch.fill(atlas, x, y, 1.0, h, color);
    batch.fill(atlas, x + w - 1.0, y, 1.0, h, color);
}

fn draw_mark(batch: &mut HudBatch, atlas: &HudAtlas, mark: &Mark, anim: &Anim, view: &View) {
    match *mark {
        Mark::Speaking { x, y, active, color } => {
            for i in 0..3 {
                let h = if active {
                    let phase = anim.clock * 9.0 + i as f32 * 1.4;
                    3.0 + 5.0 * phase.sin().abs()
                } else {
                    2.0
                };
                let c = if active { color } else { with_alpha(color, 0.45) };
                batch.fill(atlas, x + i as f32 * 3.0, y + 8.0 - h, 2.0, h, c);
            }
        }
        Mark::Dot { x, y, color } => {
            let r = 2.5;
            batch.fill_quad(atlas, [[x + r, y], [x + 2.0 * r, y + r], [x + r, y + 2.0 * r], [x, y + r]], color);
        }
        Mark::Lock { x, y, color } => {
            // The shackle, then the body.
            frame(batch, atlas, x + 1.0, y, 5.0, 4.0, color);
            batch.fill(atlas, x, y + 3.0, 7.0, 6.0, color);
            batch.fill(atlas, x + 3.0, y + 5.0, 1.0, 2.0, rgba(0, 0, 0, 200));
        }
        Mark::Toggle { x, y } => {
            let t = anim.enabled.clamp(0.0, 1.0);
            let track = lerp_color(rgba(70, 70, 76, 255), rgba(94, 214, 140, 255), t);
            batch.fill(atlas, x, y, 22.0, 10.0, track);
            frame(batch, atlas, x, y, 22.0, 10.0, rgba(0, 0, 0, 120));
            let kx = x + 1.0 + t * 12.0;
            batch.fill(atlas, kx, y + 1.0, 8.0, 8.0, WHITE);
        }
        Mark::Orb { x, y } => {
            let vv = &view.voice;
            let live = vv.enabled && vv.in_world && vv.available && vv.mic_problem.is_none();
            let breath = (anim.clock * 1.8).sin() * 0.6;
            let level = anim.level.clamp(0.0, 1.0);
            let talk = anim.talking.clamp(0.0, 1.0);
            let base = if !vv.enabled {
                rgba(90, 90, 96, 255)
            } else if !live {
                rgba(150, 150, 156, 255)
            } else {
                lerp_color(rgba(94, 200, 255, 255), rgba(94, 214, 140, 255), talk)
            };
            // The halo swells with the voice; a quiet one breathes.
            let halo = 13.0 + breath + level * 9.0 + talk * 2.0;
            ring(batch, atlas, x, y, 10.0, halo, with_alpha(base, 0.18 + 0.25 * talk.max(level)));
            ring(batch, atlas, x, y, 9.0, 10.0, with_alpha(base, 0.6));
            disc(batch, atlas, x, y, 8.0 + level * 1.5, with_alpha(base, 0.9));
            // The microphone glyph: a capsule and its stand.
            let ink = rgba(16, 16, 20, 255);
            batch.fill(atlas, x - 1.5, y - 5.0, 3.0, 6.0, ink);
            frame(batch, atlas, x - 3.0, y - 2.0, 6.0, 5.0, ink);
            batch.fill(atlas, x - 0.5, y + 3.0, 1.0, 2.0, ink);
            batch.fill(atlas, x - 2.0, y + 4.5, 4.0, 1.0, ink);
            if !vv.enabled {
                // Struck through.
                batch.fill_quad(atlas, [[x - 6.0, y + 5.0], [x - 5.0, y + 6.0], [x + 6.0, y - 5.0], [x + 5.0, y - 6.0]], red());
            }
        }
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
    let anim = &ui.anim;
    // The title settles first, with a line sweeping out under it.
    let start = batch.vertices.len();
    let title = Text::literal(ui.tab.title());
    let tw = atlas.font.width_text(&title);
    let tx = ((w - tw) / 2.0).floor();
    batch.text_styled(atlas, &title, tx, 8.0, WHITE, true);
    let sweep = ease_out(anim.page / (PAGE_IN * 1.5));
    let lw = (tw + 16.0) * sweep;
    batch.fill(atlas, tx + tw / 2.0 - lw / 2.0, 19.0, lw, 1.0, with_alpha(accent(), 0.7 * sweep));
    settle_since(batch, start, anim, 0.0);
    let l = layout(atlas, (w, h), ui, view);
    for p in &l.panels {
        let start = batch.vertices.len();
        batch.fill(atlas, p.x, p.y, p.w, p.h, card());
        frame(batch, atlas, p.x, p.y, p.w, p.h, card_edge());
        settle_since(batch, start, anim, p.delay);
    }
    for line in &l.lines {
        let start = batch.vertices.len();
        batch.text(atlas, &line.text, line.x, line.y, line.color, true);
        settle_since(batch, start, anim, line.delay);
    }
    for m in &l.marks {
        let start = batch.vertices.len();
        draw_mark(batch, atlas, &m.mark, anim, view);
        settle_since(batch, start, anim, m.delay);
    }
    for f in &l.fields {
        let start = batch.vertices.len();
        let pulse = 0.5 + 0.5 * (anim.clock * 4.0).sin();
        let border = if f.focused {
            lerp_color(grey(), WHITE, 0.4 + 0.6 * pulse)
        } else {
            grey()
        };
        batch.fill(atlas, f.x - 1.0, f.y - 1.0, f.w + 2.0, 22.0, border);
        batch.fill(atlas, f.x, f.y, f.w, 20.0, rgba(0, 0, 0, 255));
        let inner = f.w - 8.0;
        if f.value.is_empty() && !f.focused {
            batch.text(atlas, &fit(atlas, f.placeholder, inner), f.x + 4.0, f.y + 6.0, dim(), true);
        } else {
            // The tail of a long value stays in view; a password shows as dots.
            let mut shown = if f.secret {
                "•".repeat(f.value.chars().count())
            } else {
                f.value.clone()
            };
            while HudBatch::text_width(atlas, &shown) > inner && !shown.is_empty() {
                shown.remove(0);
            }
            let adv = batch.text(atlas, &shown, f.x + 4.0, f.y + 6.0, WHITE, true);
            if f.focused && view.blink {
                batch.text(atlas, "_", f.x + 4.0 + adv, f.y + 6.0, WHITE, true);
            }
        }
        settle_since(batch, start, anim, f.delay);
    }
    for s in &l.sliders {
        let start = batch.vertices.len();
        let hover = s.active && ui::hovered(cursor, s.x, s.y, s.w, 20.0);
        ui::slider(batch, atlas, s.x, s.y, s.w, &Text::literal(&s.label), s.value, hover, s.active, false);
        settle_since(batch, start, anim, s.delay);
    }
    for b in &l.buttons {
        let start = batch.vertices.len();
        let hover = b.active && ui::hovered(cursor, b.x, b.y, b.w, 20.0);
        ui::button(batch, atlas, b.x, b.y, b.w, 20.0, &label(&b.label), b.active, hover);
        if hover {
            // A hovered button leans forward.
            let glow = anim.hover.map_or(0.0, |(_, g)| g);
            let s = 1.0 + 0.03 * glow;
            batch.scale_since(start, b.x + b.w / 2.0, b.y + 10.0, s, s);
            frame(batch, atlas, b.x - 1.0, b.y - 1.0, b.w + 2.0, 22.0, with_alpha(WHITE, 0.35 * glow));
        }
        settle_since(batch, start, anim, b.delay);
    }
    // The ripple of the last press.
    if let Some(([x, y, bw, bh], age)) = anim.pressed {
        let t = (age / RIPPLE).clamp(0.0, 1.0);
        let grow = ease_out(t) * 6.0;
        frame(
            batch,
            atlas,
            x - grow,
            y - grow,
            bw + 2.0 * grow,
            bh + 2.0 * grow,
            with_alpha(WHITE, 0.6 * (1.0 - t)),
        );
    }
}

/// The button, slider or field under a click, and its box (for the ripple).
pub fn click(atlas: &HudAtlas, size: (f32, f32), cursor: (f32, f32), ui: &Ui, view: &View) -> Option<(Action, [f32; 4])> {
    let l = layout(atlas, size, ui, view);
    if let Some(b) = l
        .buttons
        .into_iter()
        .find(|b| b.active && ui::hovered(cursor, b.x, b.y, b.w, 20.0))
    {
        return Some((b.action, [b.x, b.y, b.w, 20.0]));
    }
    if let Some([x, y, w, h]) = l.toggle
        && ui::hovered(cursor, x, y, w, h)
    {
        return Some((Action::VoiceEnabled, [x, y, w, h]));
    }
    if let Some(s) = l
        .sliders
        .into_iter()
        .find(|s| s.active && ui::hovered(cursor, s.x, s.y, s.w, 20.0))
    {
        let value = ui::slider_value(s.x, s.w, cursor.0);
        return Some((Action::VoiceSet(s.knob, value), [s.x, s.y, s.w, 20.0]));
    }
    if let Some(f) = l
        .fields
        .into_iter()
        .find(|f| ui::hovered(cursor, f.x, f.y, f.w, 20.0))
    {
        return Some((Action::Focus(f.focus), [f.x, f.y, f.w, 20.0]));
    }
    Some((Action::Focus(Focus::None), [cursor.0, cursor.1, 0.0, 0.0]))
}

/// The button under the cursor (its box), for the hover glow.
pub fn hovered_box(atlas: &HudAtlas, size: (f32, f32), cursor: (f32, f32), ui: &Ui, view: &View) -> Option<[f32; 4]> {
    layout(atlas, size, ui, view)
        .buttons
        .into_iter()
        .find(|b| b.active && ui::hovered(cursor, b.x, b.y, b.w, 20.0))
        .map(|b| [b.x, b.y, b.w, 20.0])
}

/// The rows the open list can scroll by (for the mouse wheel).
pub fn scroll_range(atlas: &HudAtlas, size: (f32, f32), ui: &Ui, view: &View) -> i32 {
    layout(atlas, size, ui, view)
        .overflow
        .map_or(0, |(total, fit_rows)| (total - fit_rows).max(0))
}
/// The pixels a scrolled page (the Host page) can go down by; 0 for lists.
pub fn page_scroll_range(atlas: &HudAtlas, size: (f32, f32), ui: &Ui, view: &View) -> f32 {
    layout(atlas, size, ui, view).page_overflow.unwrap_or(0.0)
}
/// A wheel notch / finger step on a scrolled page, in GUI pixels.
pub const PAGE_STEP: f32 = 24.0;

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
    fn press(atlas: &HudAtlas, size: (f32, f32), at: (f32, f32), ui: &Ui, view: &View) -> Option<Action> {
        click(atlas, size, at, ui, view).map(|(a, _)| a)
    }

    #[test]
    fn pages_fields_and_done_are_clickable_and_nothing_overlaps_the_buttons() {
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
                world: Some("A very long world name indeed".into()),
            }],
            ..View::default()
        };
        let l = layout(&atlas, size, &ui, &view);
        let done = l.buttons.iter().find(|b| b.action == Action::Done).unwrap();
        assert_eq!(press(&atlas, size, (done.x + 1.0, done.y + 1.0), &ui, &view), Some(Action::Done));
        let join = l.buttons.iter().find(|b| matches!(b.action, Action::Join(_))).unwrap();
        assert_eq!(
            press(&atlas, size, (join.x + 1.0, join.y + 1.0), &ui, &view),
            Some(Action::Join("abc.e4mc.link".into()))
        );
        // The status text ends before the Join button starts.
        let status = l.lines.iter().find(|t| t.text.starts_with("Hosting")).unwrap();
        assert!(status.x + HudBatch::text_width(&atlas, &status.text) <= join.x, "status under the button");
        let field = &l.fields[0];
        assert_eq!(
            press(&atlas, size, (field.x + 2.0, field.y + 2.0), &ui, &view),
            Some(Action::Focus(Focus::AddCode))
        );
        assert!(l.lines.iter().all(|t| t.y + 9.0 <= size.1 - 27.0));
        for tab in [Tab::Chat, Tab::Host, Tab::Voice] {
            let voice = l.buttons.iter().find(|b| b.action == Action::Tab(tab)).unwrap();
            assert_eq!(press(&atlas, size, (voice.x + 1.0, voice.y + 1.0), &ui, &view), Some(Action::Tab(tab)));
        }
    }

    #[test]
    fn the_host_page_scrolls_as_a_whole_and_never_shows_half_an_element() {
        let atlas = atlas();
        let size = (427.0, 240.0);
        let mut ui = Ui::default();
        ui.go(Tab::Host);
        let mut view = View::default();
        view.host = HostView {
            in_world: true,
            can_publish: true,
            publishing: true,
            address: Some("abc.e4mc.link".into()),
            guests: 1,
            problem: None,
            copied: false,
            world: "OneBlock".into(),
        };
        let max = page_scroll_range(&atlas, size, &ui, &view);
        assert!(max > 0.0, "the full Host page is taller than a phone screen");
        let top = layout(&atlas, size, &ui, &view);
        // Unscrolled: the relay Save button is below the fold, nothing crosses the bottom.
        assert!(!top.buttons.iter().any(|b| b.action == Action::SaveRelay));
        assert!(top.lines.iter().all(|t| t.y + 9.0 <= size.1 - 27.0));
        assert!(top.buttons.iter().filter(|b| b.action == Action::CycleIcon).all(|b| b.y + 20.0 <= size.1 - 27.0 + 0.5));
        // Scrolled to the end: Save is there and clickable, and nothing pokes above the page.
        ui.page_scroll = max;
        let end = layout(&atlas, size, &ui, &view);
        let save = end.buttons.iter().find(|b| b.action == Action::SaveRelay).expect("Save after scrolling");
        assert_eq!(press(&atlas, size, (save.x + 2.0, save.y + 2.0), &ui, &view), Some(Action::SaveRelay));
        let page_top = end.buttons.iter().find(|b| b.action == Action::Tab(Tab::Friends)).unwrap().y + 26.0;
        assert!(end.lines.iter().all(|t| t.y >= page_top - 0.5), "no line above the page");
        assert!(end.fields.iter().all(|f| f.y >= page_top - 0.5));
        // Over-scrolling clamps to the same layout.
        ui.page_scroll = max + 500.0;
        let over = layout(&atlas, size, &ui, &view);
        assert_eq!(over.lines.len(), end.lines.len());
    }

    #[test]
    fn the_voice_page_creates_joins_and_asks_for_passwords() {
        let atlas = atlas();
        let size = (427.0, 240.0);
        let mut ui = Ui::default();
        ui.go(Tab::Voice);
        ui.group_name = "squad".into();
        let mut view = View::default();
        view.voice = VoiceView {
            enabled: true,
            ptt: true,
            volume: 100,
            mic: 100,
            range: 48,
            available: true,
            in_world: true,
            groups: vec![
                GroupRow {
                    name: "Open".into(),
                    locked: false,
                    members: vec![("Alex".into(), true, false)],
                    mine: false,
                },
                GroupRow {
                    name: "Locked".into(),
                    locked: true,
                    members: vec![("Bob".into(), false, false)],
                    mine: false,
                },
            ],
            players: vec![("Alex".into(), false, true), ("Bob".into(), true, false)],
            ..VoiceView::default()
        };
        let l = layout(&atlas, size, &ui, &view);
        let create = l.buttons.iter().find(|b| b.action == Action::VoiceGroupCreate).unwrap();
        assert!(create.active);
        assert_eq!(press(&atlas, size, (create.x + 1.0, create.y + 1.0), &ui, &view), Some(Action::VoiceGroupCreate));
        assert!(l.buttons.iter().any(|b| b.action == Action::VoiceGroupJoin("Open".into())));
        assert!(l.buttons.iter().any(|b| b.action == Action::VoiceAskPassword("Locked".into())));
        assert!(l.buttons.iter().any(|b| b.action == Action::VoiceMute("Bob".into())));
        let volume = l.sliders.iter().find(|s| s.knob == Knob::Volume).unwrap();
        match press(&atlas, size, (volume.x + volume.w - 4.0, volume.y + 1.0), &ui, &view) {
            Some(Action::VoiceSet(Knob::Volume, v)) => assert!((v - 1.0).abs() < 0.01),
            other => panic!("not the volume slider: {other:?}"),
        }
        // The password prompt replaces the create form.
        ui.join_target = Some("Locked".into());
        ui.password = "pw".into();
        let l = layout(&atlas, size, &ui, &view);
        assert!(l.buttons.iter().any(|b| b.action == Action::VoiceGroupJoin("Locked".into()) && b.active));
        assert!(l.fields.iter().any(|f| f.focus == Focus::Password && f.secret));
        assert!(!l.buttons.iter().any(|b| b.action == Action::VoiceGroupCreate));
        // In a group: members, Invite and Leave; the invite picker lists the others.
        view.voice.groups[0].mine = true;
        view.voice.groups[0].members.insert(0, ("You".into(), false, false));
        view.voice.invitable = vec!["Bob".into()];
        ui.join_target = None;
        let l = layout(&atlas, size, &ui, &view);
        assert!(l.buttons.iter().any(|b| b.action == Action::VoiceGroupLeave));
        assert!(l.buttons.iter().any(|b| b.action == Action::VoiceInviteToggle));
        assert!(l.buttons.iter().any(|b| b.action == Action::VoiceMute("Alex".into())));
        ui.inviting = true;
        let l = layout(&atlas, size, &ui, &view);
        assert!(l.buttons.iter().any(|b| b.action == Action::VoiceInvite("Bob".into())));
    }

    #[test]
    fn pages_settle_in_and_eased_values_chase_their_targets() {
        let mut anim = Anim::default();
        assert_eq!(settle(&anim, 0.0), 0.0);
        anim.page = PAGE_IN;
        assert!((settle(&anim, 0.0) - 1.0).abs() < 1e-6);
        assert!(settle(&anim, STAGGER) < 1.0);
        anim.last = Some(Instant::now() - std::time::Duration::from_millis(50));
        anim.tick(true, false, true, 0.8);
        assert!(anim.enabled > 0.0 && anim.enabled < 1.0);
        assert!(anim.level > 0.0);
        assert_eq!(with_alpha(0xFF00_0000, 0.5), 0x8000_0000);
        assert_eq!(lerp_color(rgba(0, 0, 0, 255), rgba(255, 255, 255, 255), 0.0), rgba(0, 0, 0, 255));
    }
}
