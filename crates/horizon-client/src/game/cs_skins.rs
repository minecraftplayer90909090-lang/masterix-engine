//! CS Engine skin sharing: CS Launcher players see each other's offline skins
//! and capes. The player's own skin (the launcher's, or one set in game) is
//! published to the CS Friends backend under their identity; the skins of the
//! other players in the world are looked up by name and drawn where the server
//! gave none. Java Edition players (Mojang's session server) are untouched.
use horizon_client::cs_friends;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// A shared skin as drawn: 64x64 RGBA skin, its model, and the cape in its 64x64 slot.
pub(super) struct CsSkin {
    pub rgba: Option<Vec<u8>>,
    pub slim: bool,
    pub cape: Option<Vec<u8>>,
    /// `skin_updated` from the backend: a new one re-uploads the textures.
    pub stamp: String,
}

enum Entry {
    Pending(Instant),
    Missing(Instant),
    /// A skin and when it was fetched (refreshed after `HAVE_REFRESH`).
    Have(CsSkin, Instant),
}

pub(super) struct CsSkins {
    /// By lower-case name.
    entries: HashMap<String, Entry>,
    tx: Sender<(String, Option<CsSkin>)>,
    rx: Receiver<(String, Option<CsSkin>)>,
    /// The offline-skin generation last published, if any.
    published: Option<u64>,
    publish_tried: Option<Instant>,
    /// Location → stamp of the textures stored in the atlas.
    stored: HashMap<String, String>,
}

impl Default for CsSkins {
    fn default() -> Self {
        let (tx, rx) = channel();
        CsSkins {
            entries: HashMap::new(),
            tx,
            rx,
            published: None,
            publish_tried: None,
            stored: HashMap::new(),
        }
    }
}

const PENDING_TIMEOUT: Duration = Duration::from_secs(40);
const MISSING_RETRY: Duration = Duration::from_secs(120);
const HAVE_REFRESH: Duration = Duration::from_secs(600);
const BATCH: usize = 16;

impl CsSkins {
    /// Results from the lookups, into the table.
    fn poll(&mut self) {
        while let Ok((name, skin)) = self.rx.try_recv() {
            let entry = match skin {
                Some(s) => Entry::Have(s, Instant::now()),
                None => Entry::Missing(Instant::now()),
            };
            self.entries.insert(name, entry);
        }
    }
    /// The shared skin of `name`, asking the backend when it is not known yet
    /// (or is due a refresh); `None` until it answers.
    fn get(&mut self, name: &str, ask: &mut Vec<String>) -> Option<&CsSkin> {
        let key = name.to_lowercase();
        let due = match self.entries.get(&key) {
            None => true,
            Some(Entry::Pending(at)) => at.elapsed() > PENDING_TIMEOUT,
            Some(Entry::Missing(at)) => at.elapsed() > MISSING_RETRY,
            Some(Entry::Have(_, at)) => at.elapsed() > HAVE_REFRESH,
        };
        if due && ask.len() < BATCH {
            ask.push(name.to_owned());
            match self.entries.get_mut(&key) {
                // Keep drawing the skin held while the refresh is on its way.
                Some(Entry::Have(_, at)) => *at = Instant::now(),
                _ => {
                    self.entries.insert(key.clone(), Entry::Pending(Instant::now()));
                }
            }
        }
        match self.entries.get(&key) {
            Some(Entry::Have(s, _)) => Some(s),
            _ => None,
        }
    }
    fn fetch(&self, names: Vec<String>) {
        if names.is_empty() {
            return;
        }
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name("cs-skins".into())
            .spawn(move || {
                let mut found: HashMap<String, CsSkin> = HashMap::new();
                if let Ok(json) = cs_friends::rpc("cs_get_skins", &serde_json::json!({ "p_names": names }))
                    && let Some(rows) = json.as_array()
                {
                    for row in rows {
                        let Some(name) = row["name"].as_str() else {
                            continue;
                        };
                        let rgba = row["skin"].as_str().and_then(decode_skin);
                        let cape = row["cape"].as_str().and_then(decode_cape);
                        if rgba.is_none() && cape.is_none() {
                            continue;
                        }
                        found.insert(
                            name.to_lowercase(),
                            CsSkin {
                                rgba,
                                slim: row["model"].as_str() == Some("slim"),
                                cape,
                                stamp: row["updated"].as_str().unwrap_or_default().to_owned(),
                            },
                        );
                    }
                }
                for name in names {
                    let key = name.to_lowercase();
                    let _ = tx.send((key.clone(), found.remove(&key)));
                }
            })
            .ok();
    }
}

/// A base64 PNG → the 64x64 skin layout.
fn decode_skin(b64: &str) -> Option<Vec<u8>> {
    let bytes = horizon_client::skins::base64_decode(b64)?;
    let (w, h, rgba) = horizon_client::hud::decode_bytes(&bytes).ok()?;
    horizon_client::skins::process_legacy_skin(w, h, &rgba)
}
/// A base64 PNG → the cape in its 64x64 slot (a 64x64 upload already is one).
fn decode_cape(b64: &str) -> Option<Vec<u8>> {
    let bytes = horizon_client::skins::base64_decode(b64)?;
    let (w, h, rgba) = horizon_client::hud::decode_bytes(&bytes).ok()?;
    if w == 64 && h == 64 && rgba.len() == 64 * 64 * 4 {
        return Some(rgba);
    }
    horizon_client::skins::process_cape(w, h, &rgba)
}
/// 64x64 RGBA → PNG bytes.
fn encode_png(rgba: &[u8]) -> Option<Vec<u8>> {
    if rgba.len() != 64 * 64 * 4 {
        return None;
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 64, 64);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(rgba).ok()?;
    }
    Some(out)
}

impl super::Game<'_> {
    /// Publish the player's own offline skin and cape to the CS Friends backend
    /// once per change (and not for an account whose skin Mojang serves).
    pub(super) fn cs_skins_publish(&mut self) {
        if self.cs_friends.is_none() {
            return;
        }
        let generation = horizon_client::skins::offline_generation();
        if self.cs_skins.published == Some(generation) {
            return;
        }
        if self
            .cs_skins
            .publish_tried
            .is_some_and(|t| t.elapsed() < Duration::from_secs(30))
        {
            return;
        }
        self.cs_skins.publish_tried = Some(Instant::now());
        self.cs_skins.published = Some(generation);
        let skin = horizon_client::skins::offline_skin();
        let cape = horizon_client::skins::offline_cape();
        let skin_png = skin.as_ref().and_then(|s| encode_png(&s.0));
        let slim = skin.as_ref().is_some_and(|s| s.1);
        let cape_png = cape.as_ref().and_then(|c| encode_png(c));
        std::thread::Builder::new()
            .name("cs-skins-publish".into())
            .spawn(move || {
                let token = cs_friends::identity_token();
                let body = serde_json::json!({
                    "p_token": token,
                    "p_skin": skin_png.map(|p| horizon_client::skins::base64_encode(&p)),
                    "p_model": if slim { "slim" } else { "classic" },
                    "p_cape": cape_png.map(|p| horizon_client::skins::base64_encode(&p)),
                });
                if let Err(e) = cs_friends::rpc("cs_set_skin", &body) {
                    log::warn!("Masterix skins: publish failed: {e}");
                }
            })
            .ok();
    }
    /// The shared skins of the other players in the world, stored in the atlas:
    /// `(uuid, skin location, slim, cape location)` for each one known.
    pub(super) fn cs_skins_for(
        &mut self,
        players: &[(u128, String)],
    ) -> Vec<(u128, Option<String>, bool, Option<String>)> {
        self.cs_skins.poll();
        let mut ask = Vec::new();
        let mut out = Vec::new();
        // (key, stamp, has skin, slim, has cape)
        let mut known: Vec<(u128, String, String, bool, bool, bool)> = Vec::new();
        for (uuid, name) in players {
            if let Some(skin) = self.cs_skins.get(name, &mut ask) {
                known.push((
                    *uuid,
                    name.to_lowercase(),
                    skin.stamp.clone(),
                    skin.rgba.is_some(),
                    skin.slim,
                    skin.cape.is_some(),
                ));
            }
        }
        self.cs_skins.fetch(ask);
        for (uuid, key, stamp, has_skin, slim, has_cape) in known {
            let skin_loc = has_skin.then(|| format!("cs/skins/{key}"));
            let cape_loc = has_cape.then(|| format!("capes/cs/{key}"));
            for (loc, cape) in [(&skin_loc, false), (&cape_loc, true)] {
                let Some(loc) = loc else {
                    continue;
                };
                let fresh = self.cs_skins.stored.get(loc) == Some(&stamp)
                    && self.skin_slots.contains(loc)
                    && self.gui.atlas.sprites.contains_key(loc);
                if fresh {
                    continue;
                }
                let rgba = match self.cs_skins.entries.get(&key) {
                    Some(Entry::Have(s, _)) => {
                        if cape {
                            s.cape.clone()
                        } else {
                            s.rgba.clone()
                        }
                    }
                    _ => None,
                };
                if let Some(rgba) = rgba {
                    self.store_skin(loc.clone(), rgba);
                    self.cs_skins.stored.insert(loc.clone(), stamp.clone());
                }
            }
            out.push((uuid, skin_loc, slim, cape_loc));
        }
        out
    }
    /// Once a tick: publish the player's own skin when it changed.
    pub(super) fn cs_skins_tick(&mut self) {
        self.cs_skins_publish();
    }
}
