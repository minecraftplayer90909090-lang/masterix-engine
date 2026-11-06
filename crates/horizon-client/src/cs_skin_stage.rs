//! CS Engine: the title screen's player stage, the Essential mod's way — the
//! player's own skin (and cape) as a 3D model that stands on the main menu,
//! sways slowly and turns its head after the cursor. It is the same player
//! model the inventory preview and the world draw (`mobs::render`), projected
//! orthographically into the HUD batch (`HudBatch::quad_3d` + `depth_since`,
//! as `InventoryScreen.renderEntityInInventory` does), so it looks exactly
//! like the in-game player and needs no extra GPU pass.
use crate::hud::{HudAtlas, HudBatch};
use crate::mobs::data::MobData;
use crate::mobs::{Mob, MobType};
use horizon_core::geometry::Vec3;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// `Lighting.Entry.ENTITY_IN_UI` through the entity shader's
/// `minecraft_mix_light`: the two inventory lights, (0.2, -1, 1) and
/// (-0.2, -1, 0), with the 0.6 light power and 0.4 ambient. `normal` is the
/// face's outward normal in the preview's screen space (x right, y down, z
/// towards the viewer), where a face that looks up has a negative y.
pub fn entity_in_ui_shade(normal: glam::Vec3) -> f32 {
    const LIGHT_0: glam::Vec3 = glam::Vec3::new(0.2, -1.0, 1.0);
    const LIGHT_1: glam::Vec3 = glam::Vec3::new(-0.2, -1.0, 0.0);
    let light = LIGHT_0.normalize().dot(normal).max(0.0) + LIGHT_1.normalize().dot(normal).max(0.0);
    (light * 0.6 + 0.4).min(1.0)
}

/// The model's vertices through the GUI transform: back faces dropped,
/// the inventory lighting applied, sorted far to near (`depth` is the
/// quad's mean z, +z toward the viewer).
pub fn preview_quads(
    verts: &[crate::mesher::Vertex],
    gui: glam::Mat4,
) -> Vec<(f32, [[f32; 3]; 4], [[f32; 2]; 4], u32)> {
    let mut quads: Vec<(f32, [[f32; 3]; 4], [[f32; 2]; 4], u32)> = Vec::new();
    for q in verts.as_chunks::<4>().0 {
        let p: [glam::Vec3; 4] =
            std::array::from_fn(|i| gui.transform_point3(glam::Vec3::from_array(q[i].pos)));
        let n = (p[1] - p[0]).cross(p[2] - p[0]);
        // The winding gives inward normals: faces with n.z >= 0 point away.
        if n.z >= 0.0 {
            continue;
        }
        let shade = entity_in_ui_shade(-n.normalize_or_zero());
        let c = q[0].color;
        let ch = |s: u32| ((((c >> s) & 255) as f32 * shade) as u32) << s;
        let color = (c & 0xFF00_0000) | ch(0) | ch(8) | ch(16);
        let depth = p.iter().map(|v| v.z).sum::<f32>() / 4.0;
        quads.push((
            depth,
            p.map(|v| v.to_array()),
            std::array::from_fn(|i| q[i].uv),
            color,
        ));
    }
    quads.sort_by(|a, b| a.0.total_cmp(&b.0));
    quads
}

/// The stage's model: the player type of the resources root, the keyframe
/// animations the humanoid pose reads, and a player mob standing at the
/// origin. Built once per title screen (`entity-models.json` is parsed once
/// per root anyway, see `mobs::player_type`).
pub struct Stage {
    mob: Mob,
    data: MobData,
}

impl Stage {
    /// `None` when the resources carry no player model (the stage stays empty).
    pub fn new(resources: &Path) -> Option<Self> {
        let kind: Arc<MobType> = crate::mobs::player_type(resources)?;
        let mut mob = Mob::new(0, kind, Vec3::ZERO, 180.0);
        // `extractEntityInInventory` divides the size by the entity's scale:
        // the stage is the same size whatever a scale attribute would say.
        mob.extra.model_scale = 0.0;
        mob.body_yaw = 180.0;
        mob.previous_body_yaw = 180.0;
        mob.head_yaw = 180.0;
        mob.previous_head_yaw = 180.0;
        let data = MobData {
            states: Vec::new(),
            placements: HashMap::new(),
            heightmaps: HashMap::new(),
            animations: MobData::load_animations(resources),
            villager_hats: std::sync::RwLock::new(Default::default()),
            biome_tags: HashMap::new(),
            entity_tags: HashMap::new(),
            pois: std::sync::OnceLock::new(),
        };
        Some(Self { mob, data })
    }

    /// Draw the player into the box `[x0, x1) x [y0, y1)` of the batch.
    ///
    /// * `skin` / `cape`: atlas sprite names (the title's own slots or the
    ///   default skin); `slim` picks the 3-pixel arms.
    /// * `seconds`: the title's clock, for the slow sway and the breathing.
    /// * `cursor`: the pointer in batch pixels; the head turns toward it
    ///   (`InventoryScreen`'s follow-mouse look), capped like a real player.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        atlas: &HudAtlas,
        batch: &mut HudBatch,
        [x0, y0, x1, y1]: [f32; 4],
        skin: &str,
        slim: bool,
        cape: Option<&str>,
        seconds: f32,
        cursor: (f32, f32),
    ) {
        let m = &mut self.mob;
        m.skin = Some(skin.to_owned());
        m.slim = slim;
        m.cape = cape.map(str::to_owned);
        // The pose: a slow sway of the whole body (Essential's menu player
        // turns a little either way rather than spinning), the head looking
        // at the cursor on top of it.
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let sway = (seconds * 0.45).sin() * 22.0;
        let x_angle = ((cx - cursor.0) / 40.0).atan();
        let y_angle = ((cy - cursor.1) / 40.0).atan();
        m.body_yaw = 180.0 + sway;
        m.previous_body_yaw = m.body_yaw;
        m.head_yaw = m.body_yaw + (x_angle * 20.0).clamp(-40.0, 40.0);
        m.previous_head_yaw = m.head_yaw;
        m.head_pitch = (-y_angle * 20.0).clamp(-30.0, 30.0);
        m.previous_head_pitch = m.head_pitch;
        m.position = Vec3::ZERO;
        m.previous = Vec3::ZERO;
        // Keep the arms' idle bob (`AnimationUtils.bobArms` reads the age).
        m.age = (seconds * 20.0).max(0.0) as u32;

        let lookup = |name: &str| atlas.sprites.get(name).map(|r| [r.u0, r.v0, r.u1, r.v1]);
        let light = 240 | 240 << 8;
        let mut verts = Vec::new();
        let camera = glam::DVec3::ZERO;
        crate::mobs::render::render(m, 1.0, camera, &lookup, light, &self.data, &mut verts);
        if let Some(rect) = m.cape.as_deref().and_then(|c| atlas.sprites.get(c)) {
            let base = crate::mobs::render::base_transform(m, 1.0, camera);
            crate::mobs::render::cape(
                m,
                1.0,
                base,
                [rect.u0, rect.v0, rect.u1, rect.v1],
                light,
                &self.data,
                &mut verts,
            );
        }
        // Fit a 1.8-block player into the box with a little air above the
        // head; a breathing bob of a fraction of a pixel keeps it alive.
        let box_h = (y1 - y0).max(1.0);
        let size = (box_h / 2.1).floor().max(8.0);
        let bob = (seconds * 1.6).sin() * size * 0.012;
        let gui = glam::Mat4::from_translation(glam::Vec3::new(cx.round(), cy.round() + bob, 0.0))
            * glam::Mat4::from_scale(glam::Vec3::new(size, size, -size))
            * glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.9, 0.0))
            * glam::Mat4::from_rotation_z(std::f32::consts::PI)
            * glam::Mat4::from_rotation_x((y_angle * 20.0).clamp(-20.0, 20.0).to_radians() * 0.25);
        let quads = preview_quads(&verts, gui);
        let start = batch.vertices.len();
        for (_, p, uv, color) in quads {
            batch.quad_3d(p, uv, color);
        }
        batch.depth_since(start, [x0, y0, x1, y1]);
    }
}
