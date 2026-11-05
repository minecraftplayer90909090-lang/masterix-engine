//! CS Engine HUD Editor: the CS HUD modules (FPS, coordinates, keystrokes) dragged
//! anywhere on the screen and resized, like Essential's HUD editor. A module without a
//! saved place stays in the corner its settings name (`horizon-extras.txt`); a dragged one
//! keeps a fraction of the HUD's width/height and a scale in `cs-hud-layout.json` beside
//! `options.txt` (per profile). The editor opens from the pause menu, over the paused world.
use super::{Game, Screen};
use horizon_client::{
    hud::{HudBatch, rgba},
    text::Text,
    ui,
};
use std::collections::BTreeMap;

pub(super) const FILE: &str = "cs-hud-layout.json";
const MIN_SCALE: f32 = 0.5;
const MAX_SCALE: f32 = 3.0;
const STEP: f32 = 0.25;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum Module {
    Fps,
    Coords,
    Keystrokes,
}
impl Module {
    pub(super) const ALL: [Module; 3] = [Module::Fps, Module::Coords, Module::Keystrokes];
    fn key(self) -> &'static str {
        match self {
            Module::Fps => "fps",
            Module::Coords => "coords",
            Module::Keystrokes => "keystrokes",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Module::Fps => "FPS",
            Module::Coords => "XYZ",
            Module::Keystrokes => "Keys",
        }
    }
}

/// Where a module was dragged: its top-left as a fraction of the HUD, and its scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Placement {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
}

#[derive(Default, Debug)]
pub(super) struct Layout {
    pub placed: BTreeMap<Module, Placement>,
}
impl Layout {
    pub(super) fn load() -> Self {
        let path = std::env::current_dir().unwrap_or_default().join(FILE);
        let mut out = Self::default();
        let Ok(text) = std::fs::read_to_string(path) else {
            return out;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return out;
        };
        for m in Module::ALL {
            let Some(v) = value.get(m.key()) else {
                continue;
            };
            let f = |k: &str| v.get(k).and_then(serde_json::Value::as_f64).map(|x| x as f32);
            if let (Some(x), Some(y)) = (f("x"), f("y")) {
                out.placed.insert(
                    m,
                    Placement {
                        x: x.clamp(0.0, 1.0),
                        y: y.clamp(0.0, 1.0),
                        scale: f("scale").unwrap_or(1.0).clamp(MIN_SCALE, MAX_SCALE),
                    },
                );
            }
        }
        out
    }
    pub(super) fn save(&self) {
        let path = std::env::current_dir().unwrap_or_default().join(FILE);
        if self.placed.is_empty() {
            let _ = std::fs::remove_file(path);
            return;
        }
        let mut value = serde_json::Map::new();
        for (m, p) in &self.placed {
            value.insert(
                m.key().to_owned(),
                serde_json::json!({"x": p.x, "y": p.y, "scale": p.scale}),
            );
        }
        let _ = std::fs::write(path, serde_json::Value::Object(value).to_string());
    }
}

/// The editor while open: the module under the finger and where in it the drag began.
#[derive(Default, Debug)]
pub(super) struct Editor {
    pub selected: Option<Module>,
    /// The dragged module and the cursor's offset from its top-left corner.
    pub dragging: Option<(Module, f32, f32)>,
    /// Whether the pause menu opened the editor (Done returns there) or a host token did.
    pub from_pause: bool,
}

/// What the editor's bottom bar offers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Button {
    Smaller,
    Bigger,
    Toggle(Module),
    Reset,
    Done,
}

impl Game<'_> {
    pub(super) fn cs_hud_layout(&mut self) -> &mut Layout {
        if self.cs_hud_layout.is_none() {
            self.cs_hud_layout = Some(Layout::load());
        }
        self.cs_hud_layout.as_mut().unwrap()
    }

    /// Pause menu › HUD Editor (or the host's `cs:hud_edit` token): the world stays paused
    /// without the menu's buttons, the cursor free, and the modules editable.
    pub(super) fn open_cs_hud_edit(&mut self, from_pause: bool) {
        if self.title.is_some() || self.player.is_none() {
            return;
        }
        self.cs_hud_layout();
        self.cs_hud_edit = Some(Editor {
            from_pause,
            ..Default::default()
        });
        self.screen = Screen::Paused;
        // Paused without the menu (F3+Esc's state): the world pauses, nothing blurs.
        self.pause_menu = false;
        self.capture(false);
    }
    /// Done / Escape: the layout is saved, the pause menu (or the game) is back.
    pub(super) fn close_cs_hud_edit(&mut self) {
        let Some(editor) = self.cs_hud_edit.take() else {
            return;
        };
        self.cs_hud_layout().save();
        if editor.from_pause {
            self.pause_menu = true;
        } else {
            self.screen = Screen::Playing;
            self.pause_menu = true;
            self.capture(true);
        }
    }

    fn cs_hud_edit_buttons(&self, w: f32, h: f32) -> Vec<(Button, f32, f32, f32, String)> {
        let y = h - 26.0;
        let mut out = Vec::new();
        let mut x = ((w - 4.0 * 44.0 - 3.0 * 40.0 - 2.0 * 54.0 - 6.0 * 4.0) / 2.0).floor();
        for m in Module::ALL {
            let on = match m {
                Module::Fps => self.settings.extras.fps,
                Module::Coords => self.settings.extras.coords,
                Module::Keystrokes => self.settings.extras.keystrokes,
            };
            out.push((
                Button::Toggle(m),
                x,
                y,
                44.0,
                format!("{}: {}", m.label(), if on { "ON" } else { "OFF" }),
            ));
            x += 48.0;
        }
        out.push((Button::Smaller, x, y, 40.0, "-".to_owned()));
        x += 44.0;
        out.push((Button::Bigger, x, y, 40.0, "+".to_owned()));
        x += 44.0;
        out.push((Button::Reset, x, y, 54.0, "Reset".to_owned()));
        x += 58.0;
        out.push((Button::Done, x, y, 54.0, "Done".to_owned()));
        out
    }

    /// Drawn over the HUD each frame while the editor is open. A drag in progress follows the
    /// cursor here, so the host's virtual mouse needs no extra motion events.
    pub(super) fn draw_cs_hud_edit(&mut self, batch: &mut HudBatch) {
        if self.cs_hud_edit.is_none() {
            return;
        }
        let (w, h) = (batch.width, batch.height);
        let cursor = self.cursor;
        // The drag: the module's top-left follows the cursor, kept inside the HUD.
        if let Some(editor) = &self.cs_hud_edit
            && let Some((m, dx, dy)) = editor.dragging
        {
            let (rw, rh) = self
                .cs_hud_rects
                .iter()
                .find(|(module, _)| *module == m)
                .map_or((20.0, 10.0), |(_, r)| (r[2], r[3]));
            let (iw, ih, ix, iy) = self.cs_hud_inset;
            let x = (cursor.0 - dx - ix).clamp(0.0, (iw - rw).max(0.0));
            let y = (cursor.1 - dy - iy).clamp(0.0, (ih - rh).max(0.0));
            let scale = self
                .cs_hud_layout()
                .placed
                .get(&m)
                .map_or(1.0, |p| p.scale);
            self.cs_hud_layout().placed.insert(
                m,
                Placement {
                    x: x / iw.max(1.0),
                    y: y / ih.max(1.0),
                    scale,
                },
            );
        }
        let atlas = &self.gui.atlas;
        batch.fill(atlas, 0.0, 0.0, w, h, rgba(0, 0, 0, 90));
        let title = "HUD Editor";
        let tw = HudBatch::text_width(atlas, title);
        batch.text(atlas, title, ((w - tw) / 2.0).floor(), 8.0, rgba(255, 255, 255, 255), true);
        let hint = "Drag a module to move it. Select one and use - / + to resize.";
        let hw = HudBatch::text_width(atlas, hint);
        batch.text(atlas, hint, ((w - hw) / 2.0).floor(), 20.0, rgba(170, 170, 170, 255), true);
        let selected = self.cs_hud_edit.as_ref().and_then(|e| e.selected);
        let rects = self.cs_hud_rects.clone();
        for (m, [x, y, rw, rh]) in &rects {
            let hover = ui::hovered(cursor, *x, *y, *rw, *rh);
            let color = if Some(*m) == selected {
                rgba(255, 215, 0, 255)
            } else if hover {
                rgba(255, 255, 255, 220)
            } else {
                rgba(255, 255, 255, 120)
            };
            // A one-pixel frame.
            batch.fill(atlas, x - 1.0, y - 1.0, rw + 2.0, 1.0, color);
            batch.fill(atlas, x - 1.0, y + rh, rw + 2.0, 1.0, color);
            batch.fill(atlas, x - 1.0, *y, 1.0, *rh, color);
            batch.fill(atlas, x + rw, *y, 1.0, *rh, color);
            if Some(*m) == selected {
                let scale = self
                    .cs_hud_layout
                    .as_ref()
                    .and_then(|l| l.placed.get(m))
                    .map_or(1.0, |p| p.scale);
                let label = format!("{} x{scale:.2}", m.label());
                let ly = if *y > 12.0 { y - 11.0 } else { y + rh + 2.0 };
                batch.text(atlas, &label, *x, ly, rgba(255, 215, 0, 255), true);
            }
        }
        let has_selection = selected.is_some();
        for (button, x, y, bw, label) in self.cs_hud_edit_buttons(w, h) {
            let active = match button {
                Button::Smaller | Button::Bigger => has_selection,
                _ => true,
            };
            let hover = active && ui::hovered(cursor, x, y, bw, 20.0);
            ui::button(batch, atlas, x, y, bw, 20.0, &Text::literal(label), active, hover);
        }
    }

    /// A press while the editor is open: a bar button, else a module to select and drag.
    pub(super) fn cs_hud_edit_press(&mut self) {
        let (w, h) = self.gui_size();
        let cursor = self.cursor;
        let hit = self
            .cs_hud_edit_buttons(w, h)
            .into_iter()
            .find(|(_, x, y, bw, _)| ui::hovered(cursor, *x, *y, *bw, 20.0))
            .map(|(b, ..)| b);
        if let Some(button) = hit {
            self.audio.play_button_click();
            let selected = self.cs_hud_edit.as_ref().and_then(|e| e.selected);
            match button {
                Button::Done => self.close_cs_hud_edit(),
                Button::Reset => {
                    self.cs_hud_layout().placed.clear();
                    self.cs_hud_layout().save();
                }
                Button::Toggle(m) => {
                    let e = &mut self.settings.extras;
                    match m {
                        Module::Fps => e.fps = !e.fps,
                        Module::Coords => e.coords = !e.coords,
                        Module::Keystrokes => e.keystrokes = !e.keystrokes,
                    }
                    self.settings.save(&Self::options_path());
                }
                Button::Smaller | Button::Bigger => {
                    if let Some(m) = selected {
                        let delta = if button == Button::Bigger { STEP } else { -STEP };
                        let (iw, ih, _, _) = self.cs_hud_inset;
                        let current = self
                            .cs_hud_rects
                            .iter()
                            .find(|(module, _)| *module == m)
                            .map(|(_, r)| *r);
                        let layout = self.cs_hud_layout();
                        let entry = layout.placed.entry(m).or_insert_with(|| {
                            // A corner module gains a place where it is, so resizing keeps it put.
                            let [x, y, ..] = current.unwrap_or([0.0, 0.0, 0.0, 0.0]);
                            Placement {
                                x: (x / iw.max(1.0)).clamp(0.0, 1.0),
                                y: (y / ih.max(1.0)).clamp(0.0, 1.0),
                                scale: 1.0,
                            }
                        });
                        entry.scale = (entry.scale + delta).clamp(MIN_SCALE, MAX_SCALE);
                        layout.save();
                    }
                }
            }
            return;
        }
        let module = self
            .cs_hud_rects
            .iter()
            .find(|(_, [x, y, rw, rh])| ui::hovered(cursor, *x, *y, *rw, *rh))
            .map(|(m, r)| (*m, *r));
        let Some(editor) = self.cs_hud_edit.as_mut() else {
            return;
        };
        match module {
            Some((m, [x, y, ..])) => {
                editor.selected = Some(m);
                editor.dragging = Some((m, cursor.0 - x, cursor.1 - y));
            }
            None => editor.selected = None,
        }
    }
    pub(super) fn cs_hud_edit_release(&mut self) {
        let dragged = self
            .cs_hud_edit
            .as_mut()
            .and_then(|e| e.dragging.take())
            .is_some();
        if dragged {
            self.cs_hud_layout().save();
        }
    }

    /// Host tokens from CS Launcher Plus (`nativeHostKey("cs:…")`): `cs:blur` while the
    /// launcher's control editor is open (the world pauses behind a dimmed screen),
    /// `cs:exit` (save and leave to the launcher) and `cs:hud_edit`.
    pub(super) fn cs_host_token(&mut self, token: &str, down: bool) -> bool {
        match token {
            "cs:blur" => {
                if down {
                    if self.title.is_none()
                        && self.player.is_some()
                        && self.screen == Screen::Playing
                        && self.cs_hud_edit.is_none()
                    {
                        self.screen = Screen::Paused;
                        self.pause_menu = true;
                        self.capture(false);
                        self.cs_host_paused = true;
                    }
                    self.cs_host_blur = true;
                } else {
                    self.cs_host_blur = false;
                    if self.cs_host_paused {
                        self.cs_host_paused = false;
                        if self.screen == Screen::Paused && self.title.is_none() {
                            self.screen = Screen::Playing;
                            self.capture(true);
                        }
                    }
                }
                true
            }
            "cs:exit" => {
                if down {
                    self.cs_hud_edit = None;
                    if let Err(e) = self.begin_closing_save(true) {
                        log::warn!("Masterix Engine: exit: {e}");
                    }
                }
                true
            }
            "cs:hud_edit" => {
                if down {
                    if self.cs_hud_edit.is_some() {
                        self.close_cs_hud_edit();
                    } else {
                        self.open_cs_hud_edit(false);
                    }
                }
                true
            }
            _ => false,
        }
    }
}
