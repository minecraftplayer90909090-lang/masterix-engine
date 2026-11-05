//! CS Friends (CS Launcher Plus): the engine's own friends system — a friend
//! code per player, friend requests, who is online or hosting, invites to the
//! world you host (its public e4mc address), one-to-one chat — all inside the
//! game. The backend is CS Launcher's Supabase project, reached through a few
//! SECURITY DEFINER RPCs with the anonymous key: no account, no password. A
//! player is a random secret token kept in `cs-friends-identity.json`; the
//! server only ever sees its hash.
//!
//! Network work runs on one worker thread; the game thread sends `Cmd`s and
//! reads `Reply`s once a frame (`Client::poll`), never blocking on the network.
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        OnceLock,
        mpsc::{Receiver, Sender, TryRecvError, channel},
    },
    time::{Duration, Instant},
};

const URL: &str = "https://ygteavpkursyeyfthlxm.supabase.co/rest/v1/rpc/";
const KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6InlndGVhdnBrdXJzeWV5ZnRobHhtIiwicm9sZSI6ImFub24iLCJpYXQiOjE3OTI2ODk5NjksImV4cCI6MjEwODI2NTk2OX0.wqCioElyk_GlIkeaiRHlls7hk-WDABhigwb1EGy8iJU";
const ENGINE: &str = "cs-engine";
/// Heartbeat period: friends show offline 75 s after the last one.
const SYNC_EVERY: Duration = Duration::from_secs(6);
/// With the Friends screen open, and with a conversation open, news lands faster.
const SYNC_SCREEN: Duration = Duration::from_secs(3);
const SYNC_CHAT: Duration = Duration::from_millis(1500);

static DIR: OnceLock<PathBuf> = OnceLock::new();
/// Where the identity file lives (the launcher's home, shared by every profile).
pub fn configure_dir(dir: PathBuf) {
    let _ = DIR.set(dir);
}
/// The directory the identity (and the other CS files shared by every
/// profile: voice settings) live in.
pub fn identity_dir() -> PathBuf {
    DIR.get()
        .cloned()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}
fn identity_path() -> PathBuf {
    DIR.get()
        .cloned()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
        .join("cs-friends-identity.json")
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Friend {
    pub id: String,
    pub name: String,
    pub code: String,
    /// `online`, `in_game`, `hosting` or `offline`.
    pub status: String,
    /// The world's public address while the friend hosts one.
    pub address: Option<String>,
    pub world: Option<String>,
}
impl Friend {
    pub fn online(&self) -> bool {
        self.status != "offline"
    }
    pub fn hosting(&self) -> bool {
        self.status == "hosting" && self.address.is_some()
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub id: String,
    pub name: String,
    pub code: String,
    /// They asked us (Accept / Decline); else we asked them (waiting).
    pub incoming: bool,
}
#[derive(Clone, Debug)]
pub struct Invite {
    pub id: i64,
    pub from: String,
    pub from_id: String,
    pub address: String,
    pub world: String,
    pub received: Instant,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub id: i64,
    /// The friend this conversation is with.
    pub with: String,
    pub body: String,
    pub mine: bool,
}

/// What the worker fetched.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub me_id: String,
    pub me_code: String,
    pub me_name: String,
    pub friends: Vec<Friend>,
    pub requests: Vec<Request>,
    pub invites: Vec<Invite>,
    pub messages: Vec<Message>,
}

/// Something to tell the player (chat line, toast, system notification).
#[derive(Clone, Debug)]
pub struct Notice {
    pub title: String,
    pub text: String,
    /// Shown as an Android notification too (the player may have switched apps).
    pub system: bool,
}

enum Cmd {
    Presence {
        status: String,
        address: Option<String>,
        world: Option<String>,
    },
    Name(String),
    Add(String),
    Respond(String, bool),
    Remove(String),
    Invite {
        to: Option<String>,
        address: String,
        world: String,
    },
    Send {
        to: String,
        text: String,
    },
    /// 0 = in the background, 1 = the Friends screen is open, 2 = a conversation is open.
    Attention(u8),
    SyncNow,
}
enum Reply {
    Snapshot(Snapshot),
    Error(String),
    Info(String),
    Invited(usize),
}

/// What the screens read: the last snapshot plus the conversations.
#[derive(Default)]
pub struct State {
    pub me_id: String,
    pub me_code: String,
    pub me_name: String,
    pub friends: Vec<Friend>,
    pub requests: Vec<Request>,
    /// Invites not yet answered, newest last.
    pub invites: Vec<Invite>,
    /// Every conversation, by friend id, oldest first.
    pub chats: HashMap<String, Vec<Message>>,
    /// Messages not yet seen, by friend id.
    pub unread: HashMap<String, usize>,
    /// The last problem (a line under the title), and when.
    pub error: Option<(String, Instant)>,
    /// A short confirmation ("Request sent to Alex"), and when.
    pub info: Option<(String, Instant)>,
    /// The server answered at least once.
    pub connected: bool,
    /// Highest message id seen (the next sync asks from here).
    last_message: i64,
}
impl State {
    pub fn friend(&self, id: &str) -> Option<&Friend> {
        self.friends.iter().find(|f| f.id == id)
    }
    pub fn unread_total(&self) -> usize {
        self.unread.values().sum()
    }
    pub fn incoming_requests(&self) -> usize {
        self.requests.iter().filter(|r| r.incoming).count()
    }
    fn note_error(&mut self, e: String) {
        log::warn!("Masterix Friends: {e}");
        self.error = Some((e, Instant::now()));
    }
}

pub struct Client {
    tx: Sender<Cmd>,
    rx: Receiver<Reply>,
    pub state: State,
    /// Notices produced since the last `take_notices`.
    notices: Vec<Notice>,
    /// Friends known at the previous snapshot: status changes become notices.
    known: HashMap<String, String>,
    first_snapshot: bool,
    presence: (String, Option<String>, Option<String>),
    attention: u8,
}

impl Client {
    /// Start the worker for the player named `name` (the account's name).
    pub fn start(name: &str) -> Client {
        let (tx, cmd_rx) = channel::<Cmd>();
        let (reply_tx, rx) = channel::<Reply>();
        let name = name.to_owned();
        std::thread::Builder::new()
            .name("cs-friends".into())
            .spawn(move || worker(&name, &cmd_rx, &reply_tx))
            .ok();
        Client {
            tx,
            rx,
            state: State::default(),
            notices: Vec::new(),
            known: HashMap::new(),
            first_snapshot: true,
            presence: ("online".to_owned(), None, None),
            attention: 0,
        }
    }

    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
    /// What friends see about us: `online`, `in_game`, or `hosting` with the
    /// world's public address.
    pub fn set_presence(&mut self, status: &str, address: Option<&str>, world: Option<&str>) {
        let next = (
            status.to_owned(),
            address.map(str::to_owned),
            world.map(str::to_owned),
        );
        if next == self.presence {
            return;
        }
        self.presence = next.clone();
        self.send(Cmd::Presence {
            status: next.0,
            address: next.1,
            world: next.2,
        });
    }
    pub fn presence_address(&self) -> Option<&str> {
        self.presence.1.as_deref()
    }
    pub fn set_name(&self, name: &str) {
        self.send(Cmd::Name(name.to_owned()));
    }
    pub fn add_friend(&self, code: &str) {
        let code: String = code.trim().to_uppercase();
        let code = code.replacen("MX-", "CS-", 1);
        if code.is_empty() {
            return;
        }
        self.send(Cmd::Add(code));
    }
    pub fn respond(&self, id: &str, accept: bool) {
        self.send(Cmd::Respond(id.to_owned(), accept));
    }
    pub fn remove_friend(&mut self, id: &str) {
        self.state.friends.retain(|f| f.id != id);
        self.send(Cmd::Remove(id.to_owned()));
    }
    /// Invite one friend (or every online friend, `None`) to `address`.
    pub fn invite(&self, to: Option<&str>, address: &str, world: &str) {
        self.send(Cmd::Invite {
            to: to.map(str::to_owned),
            address: address.to_owned(),
            world: world.to_owned(),
        });
    }
    /// Send a chat line; it shows at once (the server's copy replaces it).
    pub fn send_message(&mut self, to: &str, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.state
            .chats
            .entry(to.to_owned())
            .or_default()
            .push(Message {
                id: 0,
                with: to.to_owned(),
                body: text.to_owned(),
                mine: true,
            });
        self.send(Cmd::Send {
            to: to.to_owned(),
            text: text.to_owned(),
        });
    }
    /// How closely the player watches: 0 background, 1 screen open, 2 conversation open.
    pub fn attention(&mut self, level: u8) {
        if self.attention == level {
            return;
        }
        self.attention = level;
        self.send(Cmd::Attention(level));
    }
    pub fn sync_now(&self) {
        self.send(Cmd::SyncNow);
    }
    /// The conversation with `id` was read.
    pub fn mark_read(&mut self, id: &str) {
        self.state.unread.remove(id);
    }
    /// An invite was answered (joined or dismissed).
    pub fn dismiss_invite(&mut self, id: i64) {
        self.state.invites.retain(|i| i.id != id);
    }
    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    /// Once a frame: what the worker sent since the last poll.
    pub fn poll(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(Reply::Snapshot(s)) => self.apply(s),
                Ok(Reply::Error(e)) => self.state.note_error(e),
                Ok(Reply::Info(i)) => self.state.info = Some((i, Instant::now())),
                Ok(Reply::Invited(n)) => {
                    self.state.info = Some((
                        if n == 0 {
                            "No friend is online to invite right now.".to_owned()
                        } else {
                            format!("Invite sent to {n} friend{}.", if n == 1 { "" } else { "s" })
                        },
                        Instant::now(),
                    ));
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    fn apply(&mut self, s: Snapshot) {
        let st = &mut self.state;
        st.connected = true;
        st.me_id = s.me_id;
        st.me_code = s.me_code;
        st.me_name = s.me_name;
        // Status changes, after the first picture.
        if !self.first_snapshot {
            for f in &s.friends {
                let was = self.known.get(&f.id).cloned().unwrap_or_default();
                if was != f.status {
                    if f.status == "hosting" {
                        self.notices.push(Notice {
                            title: "Masterix Friends".to_owned(),
                            text: format!(
                                "{} is hosting {}.",
                                f.name,
                                f.world.as_deref().unwrap_or("a world")
                            ),
                            system: false,
                        });
                    } else if was == "offline" && f.status != "offline" {
                        self.notices.push(Notice {
                            title: "Masterix Friends".to_owned(),
                            text: format!("{} is online.", f.name),
                            system: false,
                        });
                    }
                }
            }
            for r in &s.requests {
                if r.incoming && !st.requests.iter().any(|k| k.id == r.id) {
                    self.notices.push(Notice {
                        title: "Friend request".to_owned(),
                        text: format!("{} ({}) wants to be your friend.", r.name, r.code),
                        system: true,
                    });
                }
            }
        }
        self.known = s.friends.iter().map(|f| (f.id.clone(), f.status.clone())).collect();
        st.friends = s.friends;
        st.requests = s.requests;
        for i in s.invites {
            if st.invites.iter().any(|k| k.id == i.id) {
                continue;
            }
            self.notices.push(Notice {
                title: format!("{} invited you", i.from),
                text: format!("Join {} — open Friends in the Game Menu.", i.world),
                system: true,
            });
            st.invites.push(i);
        }
        st.invites
            .retain(|i| i.received.elapsed() < Duration::from_secs(10 * 60));
        for m in s.messages {
            if m.id > st.last_message {
                st.last_message = m.id;
            }
            let chat = st.chats.entry(m.with.clone()).or_default();
            if chat.iter().any(|k| k.id == m.id) {
                continue;
            }
            if m.mine {
                // The server's copy of a line we showed at once.
                if let Some(local) = chat.iter_mut().find(|k| k.id == 0 && k.body == m.body) {
                    local.id = m.id;
                    continue;
                }
            } else if !self.first_snapshot {
                *st.unread.entry(m.with.clone()).or_default() += 1;
                let who = st
                    .friends
                    .iter()
                    .find(|f| f.id == m.with)
                    .map_or("A friend", |f| f.name.as_str())
                    .to_owned();
                self.notices.push(Notice {
                    title: who,
                    text: m.body.clone(),
                    system: true,
                });
            }
            chat.push(m);
            if chat.len() > 300 {
                let extra = chat.len() - 300;
                chat.drain(..extra);
            }
        }
        self.first_snapshot = false;
    }
}

// ---- the worker ----

struct Identity {
    token: String,
}
/// The player's identity token (created on first use), for the other CS backends.
pub fn identity_token() -> String {
    load_or_create_identity().token
}
fn load_or_create_identity() -> Identity {
    let path = identity_path();
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
        && let Some(token) = json["token"].as_str()
        && token.len() >= 24
    {
        return Identity {
            token: token.to_owned(),
        };
    }
    let token = fresh_token();
    let json = serde_json::json!({ "token": token, "created": chrono::Utc::now().to_rfc3339() });
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, json.to_string()) {
        log::warn!("Masterix Friends: identity not saved to {}: {e}", path.display());
    }
    Identity { token }
}
/// 48 hex characters of a SHA-256 over time, process and addresses: unique
/// per install, never shown.
fn fresh_token() -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    h.update(now.to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    let stack = 0u8;
    h.update((std::ptr::addr_of!(stack) as usize).to_le_bytes());
    let heap = Box::new(0u8);
    h.update((&*heap as *const u8 as usize).to_le_bytes());
    h.update(Instant::now().elapsed().as_nanos().to_le_bytes());
    if let Ok(name) = std::env::var("ANDROID_SERIAL") {
        h.update(name.as_bytes());
    }
    let digest = h.finalize();
    let mut out = String::with_capacity(48);
    for b in digest.iter().take(24) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// One backend call (also used by CS skin sharing).
pub fn rpc(name: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(12))
        .timeout_read(Duration::from_secs(20))
        .build();
    let response = agent
        .post(&format!("{URL}{name}"))
        .set("apikey", KEY)
        .set("Authorization", &format!("Bearer {KEY}"))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string());
    let text = match response {
        Ok(r) => r.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(code, r)) => {
            let text = r.into_string().unwrap_or_default();
            let message = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|j| j["message"].as_str().map(str::to_owned))
                .unwrap_or(text);
            return Err(format!("{name}: {code} {message}"));
        }
        Err(e) => return Err(format!("{name}: {e}")),
    };
    serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))
}

fn worker(name: &str, cmds: &Receiver<Cmd>, out: &Sender<Reply>) {
    let identity = load_or_create_identity();
    let token = identity.token.as_str();
    let mut me_id = String::new();
    // Register (idempotent: the same token keeps its code; the name follows the account).
    let mut registered = false;
    let mut since: i64 = 0;
    let mut presence = ("online".to_owned(), None::<String>, None::<String>);
    let mut attention = 0u8;
    let mut next_sync = Instant::now();
    let mut name = name.to_owned();
    loop {
        // Commands until the next sync is due.
        let wait = next_sync.saturating_duration_since(Instant::now());
        let cmd = match cmds.recv_timeout(wait) {
            Ok(c) => Some(c),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        };
        if !registered {
            match rpc("cs_register", &serde_json::json!({"p_token": token, "p_name": name})) {
                Ok(j) => {
                    registered = true;
                    me_id = j["id"].as_str().unwrap_or_default().to_owned();
                }
                Err(e) => {
                    let _ = out.send(Reply::Error(format!("Can't reach Masterix Friends ({e})")));
                    next_sync = Instant::now() + Duration::from_secs(15);
                    continue;
                }
            }
        }
        let mut sync = cmd.is_none();
        if let Some(cmd) = cmd {
            match cmd {
                Cmd::Presence {
                    status,
                    address,
                    world,
                } => {
                    presence = (status, address, world);
                    sync = true;
                }
                Cmd::Name(n) => {
                    if n != name {
                        name = n;
                        registered = false;
                        continue;
                    }
                }
                Cmd::Add(code) => {
                    match rpc(
                        "cs_add_friend",
                        &serde_json::json!({"p_token": token, "p_code": code}),
                    ) {
                        Ok(j) => {
                            let who = j["name"].as_str().unwrap_or("them");
                            let line = match j["state"].as_str() {
                                Some("accepted") => format!("You and {who} are friends now."),
                                Some("already") => format!("{who} is already your friend."),
                                Some(_) => format!("Request sent to {who}."),
                                None => match j["error"].as_str() {
                                    Some("self") => "That's your own code.".to_owned(),
                                    Some("no_such_code") => {
                                        format!("No player has the code {code}.")
                                    }
                                    Some(e) => format!("Couldn't add: {e}"),
                                    None => "Couldn't add that code.".to_owned(),
                                },
                            };
                            let _ = out.send(Reply::Info(line));
                            sync = true;
                        }
                        Err(e) => {
                            let _ = out.send(Reply::Error(e));
                        }
                    }
                }
                Cmd::Respond(other, accept) => {
                    match rpc(
                        "cs_respond",
                        &serde_json::json!({"p_token": token, "p_other": other, "p_accept": accept}),
                    ) {
                        Ok(_) => sync = true,
                        Err(e) => {
                            let _ = out.send(Reply::Error(e));
                        }
                    }
                }
                Cmd::Remove(other) => {
                    match rpc(
                        "cs_remove_friend",
                        &serde_json::json!({"p_token": token, "p_other": other}),
                    ) {
                        Ok(_) => sync = true,
                        Err(e) => {
                            let _ = out.send(Reply::Error(e));
                        }
                    }
                }
                Cmd::Invite { to, address, world } => {
                    match rpc(
                        "cs_invite",
                        &serde_json::json!({"p_token": token, "p_to": to, "p_address": address, "p_world": world, "p_engine": ENGINE}),
                    ) {
                        Ok(j) => {
                            let n = j["sent"].as_u64().unwrap_or(0) as usize;
                            let _ = out.send(Reply::Invited(n));
                        }
                        Err(e) => {
                            let _ = out.send(Reply::Error(e));
                        }
                    }
                }
                Cmd::Send { to, text } => {
                    match rpc(
                        "cs_send",
                        &serde_json::json!({"p_token": token, "p_to": to, "p_text": text}),
                    ) {
                        Ok(j) => {
                            if let Some(e) = j["error"].as_str() {
                                let _ = out.send(Reply::Error(match e {
                                    "not_friends" => "You're not friends any more.".to_owned(),
                                    "slow_down" => "Slow down a little.".to_owned(),
                                    other => other.to_owned(),
                                }));
                            }
                            sync = true;
                        }
                        Err(e) => {
                            let _ = out.send(Reply::Error(e));
                        }
                    }
                }
                Cmd::Attention(level) => {
                    sync = level > attention;
                    attention = level;
                }
                Cmd::SyncNow => sync = true,
            }
        }
        if !sync {
            continue;
        }
        let body = serde_json::json!({
            "p_token": token,
            "p_status": presence.0,
            "p_address": presence.1,
            "p_world": presence.2,
            "p_engine": ENGINE,
            "p_since": since,
        });
        match rpc("cs_sync", &body) {
            Ok(j) => {
                let snapshot = parse_snapshot(&j, &me_id);
                for m in &snapshot.messages {
                    since = since.max(m.id);
                }
                let _ = out.send(Reply::Snapshot(snapshot));
            }
            Err(e) => {
                let _ = out.send(Reply::Error(format!("Masterix Friends sync failed ({e})")));
            }
        }
        next_sync = Instant::now()
            + match attention {
                0 => SYNC_EVERY,
                1 => SYNC_SCREEN,
                _ => SYNC_CHAT,
            };
    }
}

fn parse_snapshot(j: &serde_json::Value, me_id: &str) -> Snapshot {
    let s = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_owned();
    let opt = |v: &serde_json::Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    let me = &j["me"];
    let me_id = if me["id"].is_string() { s(&me["id"]) } else { me_id.to_owned() };
    let friends = j["friends"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|f| Friend {
                    id: s(&f["id"]),
                    name: s(&f["name"]),
                    code: s(&f["code"]).replacen("CS-", "MX-", 1),
                    status: opt(&f["status"]).unwrap_or_else(|| "offline".to_owned()),
                    address: opt(&f["address"]),
                    world: opt(&f["world"]),
                })
                .collect()
        })
        .unwrap_or_default();
    let requests = j["requests"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| Request {
                    id: s(&r["id"]),
                    name: s(&r["name"]),
                    code: s(&r["code"]).replacen("CS-", "MX-", 1),
                    incoming: r["incoming"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
    let invites = j["invites"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| {
                    Some(Invite {
                        id: i["id"].as_i64()?,
                        from: s(&i["from"]),
                        from_id: s(&i["from_id"]),
                        address: opt(&i["address"])?,
                        world: opt(&i["world"]).unwrap_or_else(|| "their world".to_owned()),
                        received: Instant::now(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let messages = j["messages"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    let from = s(&m["from"]);
                    let to = s(&m["to"]);
                    let mine = from == me_id;
                    Some(Message {
                        id: m["id"].as_i64()?,
                        with: if mine { to } else { from },
                        body: s(&m["body"]),
                        mine,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Snapshot {
        me_id,
        me_code: s(&me["code"]).replacen("CS-", "MX-", 1),
        me_name: s(&me["name"]),
        friends,
        requests,
        invites,
        messages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_parses_and_tokens_differ() {
        let j: serde_json::Value = serde_json::from_str(
            r#"{"me":{"id":"a","code":"CS-1","name":"A"},"friends":[{"id":"b","name":"B","code":"CS-2","status":"hosting","address":"x.e4mc.link","world":"W"}],
            "requests":[{"id":"c","name":"C","code":"CS-3","incoming":true}],
            "invites":[{"id":5,"from":"B","from_id":"b","address":"x.e4mc.link","world":"W"}],
            "messages":[{"id":9,"from":"b","to":"a","body":"hi"},{"id":10,"from":"a","to":"b","body":"yo"}]}"#,
        )
        .unwrap();
        let s = parse_snapshot(&j, "");
        assert_eq!(s.me_code, "MX-1");
        assert!(s.friends[0].hosting());
        assert!(s.requests[0].incoming);
        assert_eq!(s.invites[0].id, 5);
        assert_eq!(s.messages[0].with, "b");
        assert!(!s.messages[0].mine && s.messages[1].mine);
        assert_ne!(fresh_token(), fresh_token());
        assert_eq!(fresh_token().len(), 48);
    }
}
