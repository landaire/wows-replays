use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::BufWriter;

use anyhow::{anyhow, Context};
use bytes::Bytes;
use image::{Rgb, RgbImage, RgbaImage};
use openh264::encoder::{Encoder, EncoderConfig, FrameRate};
use openh264::formats::{RgbSliceU8, YUVBuffer};
use openh264::OpenH264API;
use wowsunpack::data::Version;
use wowsunpack::game_params::provider::GameMetadataProvider;
use wowsunpack::game_params::types::{GameParamProvider, PlaneCategory, Species};

use wows_replays::analyzer::decoder::{DecodedPacket, DecodedPacketPayload};
use wows_replays::analyzer::{AnalyzerMut, AnalyzerMutBuilder};
use wows_replays::types::{PlaneId, Relation};
use wows_replays::ReplayMeta;

use crate::drawing;
use crate::map_data;

const TOTAL_FRAMES: usize = 1800;
const FPS: f64 = 30.0;
// Use 768 (multiple of 16) for H.264 macroblock alignment
const MINIMAP_SIZE: u32 = 768;
// Top margin for HUD elements (score bar, timer, kill feed)
const HUD_HEIGHT: u32 = 32;
// Total canvas height = map + HUD. Round up to next multiple of 16 for H.264.
const CANVAS_HEIGHT: u32 = MINIMAP_SIZE + HUD_HEIGHT; // 800

#[derive(Clone, Debug)]
pub enum DumpMode {
    Frame(usize),
    Midpoint,
}

// How long various effects persist in game-seconds
const SHOT_DURATION: f32 = 3.0;
const TORPEDO_MAX_DURATION: f32 = 180.0; // safety fallback; torpedoes primarily removed by hit events
const KILL_FEED_DURATION: f32 = 10.0;

use crate::map_data::{EntityId, GameClock, MinimapPos, NormalizedPos, WorldPos};

struct ShipSnapshot {
    pos: WorldPos,
}

struct ShotTrail {
    origin: WorldPos,
    target: WorldPos,
    flight_duration: f32,
    clock: GameClock,
}

struct TorpedoSnapshot {
    composite_id: u64,
    owner_id: EntityId,
    origin: WorldPos,
    velocity: WorldPos,
    clock: GameClock,
}

enum SmokeEvent {
    SetRange { start: usize, points: Vec<WorldPos> },
    RemoveRange { start: usize, count: usize },
}

struct SmokeCloud {
    radius: f32,
    created: GameClock,
    destroyed: Option<GameClock>,
    /// Initial point from EntityCreate
    initial_point: WorldPos,
    /// Timeline of points array mutations
    events: Vec<(GameClock, SmokeEvent)>,
}

impl SmokeCloud {
    fn points_at(&self, time: GameClock) -> Vec<WorldPos> {
        let mut points = vec![self.initial_point];
        for (clock, event) in &self.events {
            if *clock > time {
                break;
            }
            match event {
                SmokeEvent::SetRange {
                    start,
                    points: new_pts,
                } => {
                    // Extend if needed
                    while points.len() < start + new_pts.len() {
                        points.push(WorldPos {
                            x: 0.0,
                            y: 0.0,
                            z: 0.0,
                        });
                    }
                    for (i, p) in new_pts.iter().enumerate() {
                        points[start + i] = *p;
                    }
                }
                SmokeEvent::RemoveRange { start, count } => {
                    let end = (start + count).min(points.len());
                    points.drain(*start..end);
                }
            }
        }
        points
    }
}

struct SquadronInfo {
    owner_id: EntityId,
    /// Icon base name derived from aircraft species (e.g. "fighter", "torpedo_regular")
    icon_base: String,
    /// Which icon directory to load from
    icon_dir: &'static str,
}

struct KillEvent {
    clock: GameClock,
    killer_entity: EntityId,
    victim_entity: EntityId,
}

struct MinimapShipUpdate {
    entity_id: EntityId,
    pos: NormalizedPos,
    heading: f32,
    disappearing: bool,
    clock: GameClock,
}

/// Pre-rasterized ship icon (RGBA, white/alpha mask to be tinted at draw time).
pub type ShipIcon = RgbaImage;

pub struct MinimapBuilder {
    output_path: String,
    map_image: Option<RgbImage>,
    map_info: Option<map_data::MapInfo>,
    dump_mode: Option<DumpMode>,
    ship_icons: HashMap<String, ShipIcon>,
    plane_icons: HashMap<String, RgbaImage>,
    game_params: GameMetadataProvider,
}

impl MinimapBuilder {
    pub fn new(
        output_path: &str,
        map_image: Option<RgbImage>,
        map_info: Option<map_data::MapInfo>,
        dump_mode: Option<DumpMode>,
        ship_icons: HashMap<String, ShipIcon>,
        plane_icons: HashMap<String, RgbaImage>,
        game_params: GameMetadataProvider,
    ) -> Self {
        Self {
            output_path: output_path.to_string(),
            map_image,
            map_info,
            dump_mode,
            ship_icons,
            plane_icons,
            game_params,
        }
    }
}

impl AnalyzerMutBuilder for MinimapBuilder {
    fn build(self, meta: &ReplayMeta) -> Box<dyn AnalyzerMut> {
        let version = Version::from_client_exe(&meta.clientVersionFromExe);
        let map_name = meta.mapName.clone();

        // Build relation map from meta.vehicles
        let mut relations: HashMap<String, Relation> = HashMap::new();
        for v in &meta.vehicles {
            relations.insert(v.name.clone(), Relation::new(v.relation));
        }

        // Resolve species by player name (meta.vehicles has shipId = shipParamsId)
        let mut species_by_name: HashMap<String, String> = HashMap::new();
        for v in &meta.vehicles {
            if let Some(param) = self.game_params.game_param_by_id(v.shipId.raw()) {
                if let Some(species) = param.species() {
                    species_by_name.insert(v.name.clone(), format!("{:?}", species));
                }
            }
        }

        Box::new(MinimapRenderer {
            version,
            map_name,
            output_path: self.output_path,
            dump_mode: self.dump_mode,
            map_image: self.map_image,
            map_info: self.map_info,
            relations_by_name: relations,
            species_by_name,
            ship_icons: self.ship_icons,
            plane_icons: self.plane_icons,
            game_params: self.game_params,
            player_species: HashMap::new(),
            positions: BTreeMap::new(),
            yaw_timeline: BTreeMap::new(),
            minimap_updates: Vec::new(),
            shots: Vec::new(),
            torpedoes: Vec::new(),
            torpedo_hits: HashMap::new(),
            planes: BTreeMap::new(),
            squadron_info: HashMap::new(),
            smoke_screens: BTreeMap::new(),
            kills: Vec::new(),
            scores: Vec::new(),
            player_names: HashMap::new(),
            player_relations: HashMap::new(),
            dead_ships: BTreeMap::new(),
            last_clock: GameClock(0.0),
        })
    }
}

struct MinimapRenderer {
    version: Version,
    map_name: String,
    output_path: String,
    dump_mode: Option<DumpMode>,
    map_image: Option<RgbImage>,
    map_info: Option<map_data::MapInfo>,

    // From replay meta
    relations_by_name: HashMap<String, Relation>,
    species_by_name: HashMap<String, String>,

    // Ship icons + per-entity species (resolved during OnArenaStateReceived)
    ship_icons: HashMap<String, ShipIcon>,
    plane_icons: HashMap<String, RgbaImage>,
    game_params: GameMetadataProvider,
    player_species: HashMap<EntityId, String>,

    // Collected data — BTreeMaps for deterministic draw order
    positions: BTreeMap<EntityId, Vec<(GameClock, ShipSnapshot)>>,
    yaw_timeline: BTreeMap<EntityId, Vec<(GameClock, f32)>>,
    minimap_updates: Vec<MinimapShipUpdate>,
    shots: Vec<ShotTrail>,
    torpedoes: Vec<TorpedoSnapshot>,
    torpedo_hits: HashMap<u64, GameClock>, // composite_id -> clock when hit
    planes: BTreeMap<PlaneId, Vec<(GameClock, WorldPos)>>,
    squadron_info: HashMap<PlaneId, SquadronInfo>,
    smoke_screens: BTreeMap<EntityId, SmokeCloud>,
    kills: Vec<KillEvent>,
    scores: Vec<(GameClock, i32, i32)>,
    player_names: HashMap<EntityId, String>,
    player_relations: HashMap<EntityId, Relation>,
    dead_ships: BTreeMap<EntityId, (GameClock, MinimapPos)>,
    last_clock: GameClock,
}

/// Build the icon base name from species, consumable flag, and ammo type.
///
/// Consumable planes use simple names (e.g. "fighter", "scout") from the
/// consumables icon directory. Controllable (CV) planes combine species with
/// ammo type (e.g. "bomber_he", "torpedo_regular", "fighter_ap").
fn species_to_icon_base(species: Species, is_consumable: bool, ammo_type: &str) -> String {
    use convert_case::{Case, Casing};

    // ammoType values like "depthcharge" have no word boundary for convert_case,
    // so normalize known cases before converting
    let normalized = match ammo_type {
        "depthcharge" => "depth_charge",
        other => other,
    };
    let ammo = normalized.to_case(Case::Snake);
    if is_consumable {
        match species {
            // ASW depth charge planes (consumable airsupport)
            Species::Dive => format!("bomber_{ammo}"),
            _ => {
                let species_name: &str = (&species).into();
                species_name.to_case(Case::Snake)
            }
        }
    } else {
        match species {
            Species::Fighter => format!("fighter_{ammo}"),
            Species::Dive => format!("bomber_{ammo}"),
            Species::Bomber => match ammo.as_str() {
                "torpedo_deepwater" => "torpedo_deepwater".to_string(),
                _ => "torpedo_regular".to_string(),
            },
            Species::Skip => format!("skip_{ammo}"),
            Species::Airship => "auxiliary".to_string(),
            _ => format!("fighter_{ammo}"),
        }
    }
}

impl MinimapRenderer {
    /// Interpolate between position samples, or return the nearest sample.
    /// Returns None only if there are no samples at all or game_time is before the first sample.
    fn interpolate_position(
        positions: &[(GameClock, ShipSnapshot)],
        game_time: GameClock,
    ) -> Option<WorldPos> {
        if positions.is_empty() {
            return None;
        }

        let idx = positions.partition_point(|(t, _)| *t <= game_time);

        let before = idx.checked_sub(1).map(|i| &positions[i]);
        let after = positions.get(idx);

        match (before, after) {
            (Some((t0, s0)), Some((t1, s1))) => {
                let span = *t1 - *t0;
                let frac = if span.abs() > 0.001 {
                    (game_time - *t0) / span
                } else {
                    0.0
                };
                Some(s0.pos.lerp(s1.pos, frac))
            }
            (Some((_t, snap)), None) => Some(snap.pos),
            (None, Some((_t, snap))) => Some(snap.pos),
            _ => None,
        }
    }

    fn interpolate_yaw(yaw_samples: &[(GameClock, f32)], game_time: GameClock) -> f32 {
        use std::f32::consts::PI;

        if yaw_samples.is_empty() {
            return 0.0;
        }

        let idx = yaw_samples.partition_point(|(t, _)| *t <= game_time);

        let before = idx.checked_sub(1).map(|i| &yaw_samples[i]);
        let after = yaw_samples.get(idx);

        match (before, after) {
            (Some((t0, y0)), Some((t1, y1))) => {
                let span = *t1 - *t0;
                let frac = if span.abs() > 0.001 {
                    (game_time - *t0) / span
                } else {
                    0.0
                };
                // Shortest-arc interpolation: wrap the delta to [-PI, PI]
                let mut delta = y1 - y0;
                while delta > PI {
                    delta -= 2.0 * PI;
                }
                while delta < -PI {
                    delta += 2.0 * PI;
                }
                y0 + delta * frac
            }
            (Some((_, y)), None) => *y,
            (None, Some((_, y))) => *y,
            _ => 0.0,
        }
    }

    /// Build a full canvas (MINIMAP_SIZE x CANVAS_HEIGHT) with the map placed below the HUD area.
    fn build_canvas(&self, map_image: &RgbImage) -> RgbImage {
        let mut canvas = RgbImage::from_pixel(MINIMAP_SIZE, CANVAS_HEIGHT, Rgb([20, 25, 35]));
        // Paste map image at y=HUD_HEIGHT
        for y in 0..map_image.height().min(MINIMAP_SIZE) {
            for x in 0..map_image.width().min(MINIMAP_SIZE) {
                canvas.put_pixel(x, y + HUD_HEIGHT, *map_image.get_pixel(x, y));
            }
        }
        canvas
    }

    fn render_single_frame(&mut self, frame_idx: usize) -> anyhow::Result<RgbImage> {
        let map_info = self
            .map_info
            .as_ref()
            .ok_or_else(|| anyhow!("No map info for: {}", self.map_name))?;
        let font = drawing::load_font();
        let map_image = self
            .map_image
            .clone()
            .unwrap_or_else(|| RgbImage::from_pixel(MINIMAP_SIZE, MINIMAP_SIZE, Rgb([30, 40, 60])));

        let game_duration = self.last_clock;
        if game_duration.seconds() <= 0.0 {
            return Err(anyhow!("No game data found"));
        }

        let game_time = GameClock(frame_idx as f32 * game_duration.seconds() / TOTAL_FRAMES as f32);
        let mut frame = self.build_canvas(&map_image);
        self.draw_frame(&mut frame, game_time, &map_info, &font);
        Ok(frame)
    }

    fn draw_frame(
        &self,
        frame: &mut RgbImage,
        game_time: GameClock,
        map_info: &map_data::MapInfo,
        font: &ab_glyph::FontRef,
    ) {
        let y_off = HUD_HEIGHT as i32;

        // 1. Score bar (drawn in HUD area, no offset)
        let (score0, score1) = self.get_scores_at(game_time);
        drawing::draw_score_bar(frame, score0, score1, font);

        // 2. Artillery shot tracers (short segments moving from origin to target)
        const TRACER_LEN: f32 = 0.12; // fraction of total path length
        for shot in &self.shots {
            let elapsed = game_time - shot.clock;
            if elapsed < 0.0 || elapsed > shot.flight_duration {
                continue;
            }
            let frac = elapsed / shot.flight_duration;
            let head = shot.origin.lerp(shot.target, frac);
            let tail = shot.origin.lerp(shot.target, (frac - TRACER_LEN).max(0.0));
            let head_px = map_info.world_to_minimap(head, MINIMAP_SIZE);
            let tail_px = map_info.world_to_minimap(tail, MINIMAP_SIZE);
            drawing::draw_shot_line(
                frame,
                tail_px.x as f32,
                tail_px.y as f32 + y_off as f32,
                head_px.x as f32,
                head_px.y as f32 + y_off as f32,
            );
        }

        // 3. Torpedoes
        // Direction vector magnitude IS the speed (m/s)
        // Torpedoes are removed when hit (via receiveShotKills) or out of bounds
        let half_space = map_info.space_size as f32 / 2.0;
        for torp in &self.torpedoes {
            if game_time < torp.clock || game_time > torp.clock + TORPEDO_MAX_DURATION {
                continue;
            }
            // Skip torpedoes that have been hit (at or after the hit time)
            if let Some(&hit_time) = self.torpedo_hits.get(&torp.composite_id) {
                if game_time >= hit_time {
                    continue;
                }
            }
            let elapsed = game_time - torp.clock;
            let world = torp.origin + torp.velocity * elapsed;
            // Skip torpedoes that have left the map
            if world.x.abs() > half_space || world.z.abs() > half_space {
                continue;
            }
            let px = map_info.world_to_minimap(world, MINIMAP_SIZE);
            let relation = self
                .player_relations
                .get(&torp.owner_id)
                .copied()
                .unwrap_or(Relation::new(2));
            let color = if relation.is_self() || relation.is_ally() {
                drawing::COLOR_TORPEDO_FRIENDLY
            } else {
                drawing::COLOR_TORPEDO
            };
            drawing::draw_torpedo(frame, px.x, px.y + y_off, color);
        }

        // 4. Smoke screens
        if let Some(map_info) = &self.map_info {
            for smoke in self.smoke_screens.values() {
                if game_time < smoke.created {
                    continue;
                }
                if let Some(end) = smoke.destroyed {
                    if game_time >= end {
                        continue;
                    }
                }
                let active_points = smoke.points_at(game_time);
                let px_radius =
                    (smoke.radius / map_info.space_size as f32 * MINIMAP_SIZE as f32) as i32;
                for point in &active_points {
                    let px = map_info.world_to_minimap(*point, MINIMAP_SIZE);
                    drawing::draw_smoke(frame, px.x, px.y + y_off, px_radius.max(3));
                }
            }
        }

        // 5. Ships — unified pass using both world positions and minimap updates
        //
        // Precompute per-entity minimap update index (sorted by time already)
        let mut minimap_by_entity: HashMap<EntityId, Vec<usize>> = HashMap::new();
        for (i, u) in self.minimap_updates.iter().enumerate() {
            minimap_by_entity.entry(u.entity_id).or_default().push(i);
        }

        // Collect all known entity IDs from both sources
        let all_entity_ids: BTreeSet<EntityId> = self
            .positions
            .keys()
            .chain(minimap_by_entity.keys())
            .copied()
            .collect();

        for entity_id in &all_entity_ids {
            // Skip dead ships
            if let Some((death_time, _)) = self.dead_ships.get(entity_id) {
                if game_time >= *death_time {
                    continue;
                }
            }

            // Find the most recent minimap update at or before current game time.
            // This is the authoritative source for whether a ship is detected.
            let latest_minimap = minimap_by_entity.get(entity_id).and_then(|indices| {
                indices.iter().rev().find_map(|&i| {
                    let u = &self.minimap_updates[i];
                    if u.clock <= game_time {
                        Some(u)
                    } else {
                        None
                    }
                })
            });

            // Visibility is determined by the minimap data:
            // - disappearing=false means detected (Visible or MinimapOnly depending on position data)
            // - disappearing=true means undetected (show last known position)
            // - no minimap data yet means the ship hasn't appeared
            let detected = latest_minimap.map(|u| !u.disappearing).unwrap_or(false);

            // Best world position (interpolated or last sample)
            let world_pos = self
                .positions
                .get(entity_id)
                .and_then(|p| Self::interpolate_position(p, game_time));

            let relation = self
                .player_relations
                .get(entity_id)
                .copied()
                .unwrap_or(Relation::new(2));
            let color = drawing::ship_color(relation);

            if detected {
                if let Some(world) = world_pos {
                    // Detected + world position → Visible (solid icon at precise position)
                    let px = map_info.world_to_minimap(world, MINIMAP_SIZE);
                    let yaw = self
                        .yaw_timeline
                        .get(entity_id)
                        .map(|ys| Self::interpolate_yaw(ys, game_time))
                        .unwrap_or(0.0);
                    self.draw_ship_or_icon(
                        frame,
                        *entity_id,
                        px.x,
                        px.y + y_off,
                        yaw,
                        color,
                        drawing::ShipVisibility::Visible,
                    );
                } else if let Some(update) = latest_minimap {
                    // Detected but no world position → MinimapOnly (outline at minimap coords)
                    let px = update.pos.to_minimap(MINIMAP_SIZE);
                    self.draw_ship_or_icon(
                        frame,
                        *entity_id,
                        px.x,
                        px.y + y_off,
                        update.heading,
                        color,
                        drawing::ShipVisibility::MinimapOnly,
                    );
                }
            } else if let Some(disappear_update) = latest_minimap {
                // Undetected — show frozen at the position when the ship went dark.
                // Use the disappearing event's timestamp to look up where the ship was.
                let vanish_time = disappear_update.clock;

                let frozen_world = self
                    .positions
                    .get(entity_id)
                    .and_then(|p| Self::interpolate_position(p, vanish_time));

                if let Some(world) = frozen_world {
                    let px = map_info.world_to_minimap(world, MINIMAP_SIZE);
                    let yaw = self
                        .yaw_timeline
                        .get(entity_id)
                        .map(|ys| Self::interpolate_yaw(ys, vanish_time))
                        .unwrap_or(0.0);
                    self.draw_ship_or_icon(
                        frame,
                        *entity_id,
                        px.x,
                        px.y + y_off,
                        yaw,
                        color,
                        drawing::ShipVisibility::Undetected,
                    );
                } else {
                    // No world position — use the minimap position from the disappearing event
                    let px = disappear_update.pos.to_minimap(MINIMAP_SIZE);
                    self.draw_ship_or_icon(
                        frame,
                        *entity_id,
                        px.x,
                        px.y + y_off,
                        disappear_update.heading,
                        color,
                        drawing::ShipVisibility::Undetected,
                    );
                }
            }
            // If no minimap data exists yet, skip — ship hasn't appeared in the replay
        }

        // 6. Dead ship markers
        for (_, &(death_time, ref pos)) in &self.dead_ships {
            if game_time >= death_time {
                drawing::draw_dead_ship(frame, pos.x, pos.y + y_off);
            }
        }

        // 7. Planes
        // Show at interpolated position between updates. After the last update,
        // the squadron is gone (recalled or destroyed), so don't render.
        for (plane_id, plane_positions) in &self.planes {
            if plane_positions.is_empty() {
                continue;
            }
            let first_time = plane_positions.first().unwrap().0;
            let last_time = plane_positions.last().unwrap().0;
            if game_time < first_time || game_time > last_time {
                continue;
            }
            let idx = plane_positions.partition_point(|(t, _)| *t <= game_time);
            let before = idx.checked_sub(1).map(|i| &plane_positions[i]);
            let after = plane_positions.get(idx);
            let world = match (before, after) {
                (Some((t0, p0)), Some((t1, p1))) => {
                    let span = *t1 - *t0;
                    let frac = if span.abs() > 0.001 {
                        (game_time - *t0) / span
                    } else {
                        0.0
                    };
                    p0.lerp(*p1, frac)
                }
                (Some((_, p)), None) => *p,
                (None, Some((_, p))) => *p,
                _ => continue,
            };
            if let Some(map_info) = &self.map_info {
                let px = map_info.world_to_minimap(world, MINIMAP_SIZE);
                let info = self.squadron_info.get(plane_id);
                let is_enemy = info
                    .and_then(|i| self.player_relations.get(&i.owner_id))
                    .map(|r| r.is_enemy())
                    .unwrap_or(false);
                let icon_base = info.map(|i| i.icon_base.as_str()).unwrap_or("fighter");
                let icon_dir = info.map(|i| i.icon_dir).unwrap_or("consumables");
                let suffix = if is_enemy { "enemy" } else { "ally" };
                let key = format!("{}/{}_{}", icon_dir, icon_base, suffix);
                let icon = self.plane_icons.get(&key).or_else(|| {
                    self.plane_icons
                        .get(&format!("consumables/fighter_{}", suffix))
                });
                if let Some(icon) = icon {
                    drawing::draw_plane_icon(frame, icon, px.x, px.y + y_off);
                } else {
                    let color = if is_enemy {
                        drawing::COLOR_TEAM_RED
                    } else {
                        drawing::COLOR_TEAM_GREEN
                    };
                    drawing::draw_plane_dot(frame, px.x, px.y + y_off, color);
                }
            }
        }

        // 8. Kill feed
        let recent_kills = self.get_recent_kills(game_time);
        if !recent_kills.is_empty() {
            drawing::draw_kill_feed(frame, &recent_kills, font);
        }

        // 9. Timer
        drawing::draw_timer(frame, game_time.seconds(), font);
    }

    fn render_frames(&mut self) -> anyhow::Result<()> {
        let map_info = self
            .map_info
            .as_ref()
            .ok_or_else(|| anyhow!("No map info for: {}", self.map_name))?
            .clone();

        let font = drawing::load_font();

        let map_image = self
            .map_image
            .take()
            .unwrap_or_else(|| RgbImage::from_pixel(MINIMAP_SIZE, MINIMAP_SIZE, Rgb([30, 40, 60])));

        let game_duration = self.last_clock;
        if game_duration.seconds() <= 0.0 {
            return Err(anyhow!("No game data found"));
        }

        println!(
            "Rendering {} frames ({}x{}, {} game time at {:.0} fps)...",
            TOTAL_FRAMES, MINIMAP_SIZE, CANVAS_HEIGHT, game_duration, FPS
        );

        // Setup H.264 encoder with quality settings
        let config = EncoderConfig::new()
            .max_frame_rate(FrameRate::from_hz(FPS as f32))
            .rate_control_mode(openh264::encoder::RateControlMode::Quality)
            .bitrate(openh264::encoder::BitRate::from_bps(2_000_000)); // 2 Mbps
        let mut encoder = Encoder::with_api_config(OpenH264API::from_source(), config)
            .context("Failed to create H.264 encoder")?;

        // Encode all frames first, collecting Annex B NAL units
        let mut encoded_frames: Vec<Vec<u8>> = Vec::with_capacity(TOTAL_FRAMES);
        for frame_idx in 0..TOTAL_FRAMES {
            let game_time =
                GameClock(frame_idx as f32 * game_duration.seconds() / TOTAL_FRAMES as f32);

            let mut frame = self.build_canvas(&map_image);
            self.draw_frame(&mut frame, game_time, &map_info, &font);

            let rgb_data = frame.into_raw();
            let rgb = RgbSliceU8::new(&rgb_data, (MINIMAP_SIZE as usize, CANVAS_HEIGHT as usize));
            let yuv = YUVBuffer::from_rgb_source(rgb);
            let bitstream = encoder
                .encode(&yuv)
                .map_err(|e| anyhow!("H.264 encode error: {:?}", e))?;

            let encoded = bitstream.to_vec();
            encoded_frames.push(encoded);

            if frame_idx % 100 == 0 {
                println!("  Frame {}/{}", frame_idx, TOTAL_FRAMES);
            }
        }

        // Extract SPS and PPS from the first keyframe, and convert all frames to AVCC format
        let first_frame = &encoded_frames[0];
        let nals = parse_annexb_nals(first_frame);
        let sps = nals
            .iter()
            .find(|n| (n[0] & 0x1f) == 7)
            .ok_or_else(|| anyhow!("No SPS found in first frame"))?;
        let pps = nals
            .iter()
            .find(|n| (n[0] & 0x1f) == 8)
            .ok_or_else(|| anyhow!("No PPS found in first frame"))?;

        // Setup MP4 writer
        let mp4_config = mp4::Mp4Config {
            major_brand: str::parse("isom").unwrap(),
            minor_version: 512,
            compatible_brands: vec![
                str::parse("isom").unwrap(),
                str::parse("iso2").unwrap(),
                str::parse("avc1").unwrap(),
                str::parse("mp41").unwrap(),
            ],
            timescale: 1000,
        };

        let file = File::create(&self.output_path).context("Failed to create output file")?;
        let writer = BufWriter::new(file);
        let mut mp4_writer = mp4::Mp4Writer::write_start(writer, &mp4_config)?;

        let track_config = mp4::TrackConfig {
            track_type: mp4::TrackType::Video,
            timescale: 1000,
            language: "und".to_string(),
            media_conf: mp4::MediaConfig::AvcConfig(mp4::AvcConfig {
                width: MINIMAP_SIZE as u16,
                height: CANVAS_HEIGHT as u16,
                seq_param_set: sps.to_vec(),
                pic_param_set: pps.to_vec(),
            }),
        };
        mp4_writer.add_track(&track_config)?;

        let sample_duration = 1000 / FPS as u32; // ms per frame

        for (frame_idx, annexb_data) in encoded_frames.iter().enumerate() {
            if annexb_data.is_empty() {
                continue;
            }

            let nals = parse_annexb_nals(annexb_data);
            let is_sync = nals.iter().any(|n| (n[0] & 0x1f) == 5);

            // Convert to AVCC format (length-prefixed): skip SPS/PPS NALs
            let mut avcc_data = Vec::new();
            for nal in &nals {
                let nal_type = nal[0] & 0x1f;
                if nal_type == 7 || nal_type == 8 {
                    continue; // SPS/PPS are in the track config, not in samples
                }
                let len = nal.len() as u32;
                avcc_data.extend_from_slice(&len.to_be_bytes());
                avcc_data.extend_from_slice(nal);
            }

            if avcc_data.is_empty() {
                continue;
            }

            let sample = mp4::Mp4Sample {
                start_time: frame_idx as u64 * sample_duration as u64,
                duration: sample_duration,
                rendering_offset: 0,
                is_sync,
                bytes: Bytes::from(avcc_data),
            };
            mp4_writer.write_sample(1, &sample)?;
        }

        mp4_writer.write_end()?;
        println!("Video saved to {}", self.output_path);
        Ok(())
    }

    fn get_scores_at(&self, game_time: GameClock) -> (i32, i32) {
        let mut best = (0, 0);
        for &(t, s0, s1) in &self.scores {
            if t <= game_time {
                best = (s0, s1);
            } else {
                break;
            }
        }
        best
    }

    fn get_recent_kills(&self, game_time: GameClock) -> Vec<(String, String)> {
        let mut result = Vec::new();
        for kill in self.kills.iter().rev() {
            if game_time >= kill.clock && game_time <= kill.clock + KILL_FEED_DURATION {
                let killer_name = self
                    .player_names
                    .get(&kill.killer_entity)
                    .cloned()
                    .unwrap_or_else(|| format!("#{}", kill.killer_entity));
                let victim_name = self
                    .player_names
                    .get(&kill.victim_entity)
                    .cloned()
                    .unwrap_or_else(|| format!("#{}", kill.victim_entity));
                result.push((killer_name, victim_name));
                if result.len() >= 5 {
                    break;
                }
            }
        }
        result.reverse();
        result
    }
}

impl AnalyzerMut for MinimapRenderer {
    fn process_mut(&mut self, packet: &wows_replays::packet2::Packet<'_, '_>) {
        let decoded = DecodedPacket::from(&self.version, false, packet);
        if decoded.clock > self.last_clock {
            self.last_clock = decoded.clock;
        }

        match decoded.payload {
            DecodedPacketPayload::Position(pos) => {
                let eid = EntityId::from(pos.pid);
                self.positions.entry(eid).or_default().push((
                    decoded.clock,
                    ShipSnapshot {
                        pos: WorldPos {
                            x: pos.position.x,
                            y: pos.position.y,
                            z: pos.position.z,
                        },
                    },
                ));
            }

            DecodedPacketPayload::PlayerOrientation(ref orient) => {
                // When parent_id is 0, this is the ship's absolute world position
                if orient.parent_id == EntityId::from(0u32) {
                    let eid = EntityId::from(orient.pid);
                    self.positions.entry(eid).or_default().push((
                        decoded.clock,
                        ShipSnapshot {
                            pos: WorldPos {
                                x: orient.position.x,
                                y: orient.position.y,
                                z: orient.position.z,
                            },
                        },
                    ));
                }
            }

            DecodedPacketPayload::OnArenaStateReceived { player_states, .. } => {
                for player in &player_states {
                    let entity_id = EntityId::from(player.entity_id());
                    let name = player.username().to_string();
                    self.player_names.insert(entity_id, name.clone());

                    if let Some(relation) = self.relations_by_name.get(&name) {
                        self.player_relations.insert(entity_id, *relation);
                    }
                    if let Some(species) = self.species_by_name.get(&name) {
                        self.player_species.insert(entity_id, species.clone());
                    }
                }
            }

            DecodedPacketPayload::MinimapUpdate { ref updates, .. } => {
                for u in updates {
                    let eid = EntityId::from(u.entity_id);
                    // Minimap heading: 0=north, 90=east (degrees, CW).
                    // Convert to math convention: 0=east, PI/2=north (radians, CCW).
                    let yaw = std::f32::consts::FRAC_PI_2 - u.heading.to_radians();
                    self.minimap_updates.push(MinimapShipUpdate {
                        entity_id: eid,
                        pos: NormalizedPos {
                            x: u.position.x,
                            y: u.position.y,
                        },
                        heading: yaw,
                        disappearing: u.disappearing,
                        clock: decoded.clock,
                    });
                    if !u.disappearing {
                        self.yaw_timeline
                            .entry(eid)
                            .or_default()
                            .push((decoded.clock, yaw));
                    }
                }
            }

            DecodedPacketPayload::ArtilleryShots {
                entity_id: _,
                ref salvos,
            } => {
                for salvo in salvos {
                    for shot in &salvo.shots {
                        let origin = WorldPos {
                            x: shot.origin.0,
                            y: shot.origin.1,
                            z: shot.origin.2,
                        };
                        let target = WorldPos {
                            x: shot.target.0,
                            y: shot.target.1,
                            z: shot.target.2,
                        };
                        let dx = target.x - origin.x;
                        let dz = target.z - origin.z;
                        let distance = (dx * dx + dz * dz).sqrt();
                        let flight_duration = if shot.speed > 0.0 {
                            distance / shot.speed
                        } else {
                            SHOT_DURATION
                        };
                        self.shots.push(ShotTrail {
                            origin,
                            target,
                            flight_duration,
                            clock: decoded.clock,
                        });
                    }
                }
            }

            DecodedPacketPayload::TorpedoesReceived {
                entity_id: _,
                ref torpedoes,
            } => {
                for torp in torpedoes {
                    let composite_id = format!("{}{}", torp.owner_id, torp.shot_id)
                        .parse::<u64>()
                        .unwrap_or(0);
                    self.torpedoes.push(TorpedoSnapshot {
                        composite_id,
                        owner_id: EntityId::from(torp.owner_id),
                        origin: WorldPos {
                            x: torp.origin.0,
                            y: torp.origin.1,
                            z: torp.origin.2,
                        },
                        velocity: WorldPos {
                            x: torp.direction.0,
                            y: torp.direction.1,
                            z: torp.direction.2,
                        },
                        clock: decoded.clock,
                    });
                }
            }

            DecodedPacketPayload::ShotKills {
                entity_id: _,
                ref hits,
            } => {
                for hit in hits {
                    let composite_id = format!("{}{}", hit.owner_id, hit.shot_id)
                        .parse::<u64>()
                        .unwrap_or(0);
                    self.torpedo_hits
                        .entry(composite_id)
                        .or_insert(decoded.clock);
                }
            }

            DecodedPacketPayload::PlanePosition { plane_id, x, y, .. } => {
                self.planes
                    .entry(plane_id)
                    .or_default()
                    .push((decoded.clock, WorldPos { x, y: 0.0, z: y }));
            }

            DecodedPacketPayload::PlaneAdded {
                plane_id,
                params_id,
                x,
                y,
                ..
            } => {
                let owner_id = plane_id.owner_id();
                let param = self.game_params.game_param_by_id(params_id.raw());
                let aircraft = param.as_ref().and_then(|p| p.aircraft());
                let category = aircraft
                    .map(|a| a.category())
                    .unwrap_or(&PlaneCategory::Controllable);
                let is_consumable = matches!(
                    category,
                    PlaneCategory::Consumable | PlaneCategory::Airsupport
                );
                let ammo_type = aircraft.map(|a| a.ammo_type()).unwrap_or("");
                let icon_base = param
                    .as_ref()
                    .and_then(|p| p.species())
                    .map(|s| species_to_icon_base(s, is_consumable, ammo_type))
                    .unwrap_or_else(|| "fighter".to_string());
                let icon_dir = match category {
                    PlaneCategory::Consumable => "consumables",
                    PlaneCategory::Airsupport => "airsupport",
                    PlaneCategory::Controllable => "controllable",
                };
                self.squadron_info.insert(
                    plane_id,
                    SquadronInfo {
                        owner_id,
                        icon_base,
                        icon_dir,
                    },
                );
                // Record initial position
                self.planes
                    .entry(plane_id)
                    .or_default()
                    .push((decoded.clock, WorldPos { x, y: 0.0, z: y }));
            }

            DecodedPacketPayload::PlaneRemoved { plane_id, .. } => {
                self.planes.remove(&plane_id);
            }

            DecodedPacketPayload::ShipDestroyed { killer, victim, .. } => {
                let killer_id = EntityId::from(killer);
                let victim_id = EntityId::from(victim);
                self.kills.push(KillEvent {
                    clock: decoded.clock,
                    killer_entity: killer_id,
                    victim_entity: victim_id,
                });

                if let Some(ref info) = self.map_info {
                    if let Some(positions) = self.positions.get(&victim_id) {
                        if let Some(last) = positions.last() {
                            let pos = info.world_to_minimap(last.1.pos, MINIMAP_SIZE);
                            self.dead_ships.insert(victim_id, (decoded.clock, pos));
                        }
                    } else {
                        for u in self.minimap_updates.iter().rev() {
                            if u.entity_id == victim_id && !u.disappearing {
                                let pos = u.pos.to_minimap(MINIMAP_SIZE);
                                self.dead_ships.insert(victim_id, (decoded.clock, pos));
                                break;
                            }
                        }
                    }
                }
            }

            DecodedPacketPayload::PropertyUpdate(prop_update) => {
                if prop_update.property == "state" {
                    self.handle_property_update(prop_update, decoded.clock);
                }
                if prop_update.property == "points"
                    && self.smoke_screens.contains_key(&prop_update.entity_id)
                {
                    use wows_replays::nested_property_path::UpdateAction;
                    let event = match &prop_update.update_cmd.action {
                        UpdateAction::SetRange { start, values, .. } => {
                            let points: Vec<WorldPos> = values
                                .iter()
                                .filter_map(|v| match v {
                                    wowsunpack::rpc::typedefs::ArgValue::Vector3((x, y, z)) => {
                                        Some(WorldPos {
                                            x: *x,
                                            y: *y,
                                            z: *z,
                                        })
                                    }
                                    _ => None,
                                })
                                .collect();
                            Some(SmokeEvent::SetRange {
                                start: *start,
                                points,
                            })
                        }
                        UpdateAction::RemoveRange { start, stop } => {
                            Some(SmokeEvent::RemoveRange {
                                start: *start,
                                count: stop - start,
                            })
                        }
                        _ => None,
                    };
                    if let Some(event) = event {
                        if let Some(smoke) = self.smoke_screens.get_mut(&prop_update.entity_id) {
                            smoke.events.push((decoded.clock, event));
                        }
                    }
                }
            }

            DecodedPacketPayload::EntityCreate(create) => {
                if create.entity_type == "BattleLogic" {
                    self.extract_initial_scores(&create.props, decoded.clock);
                } else if create.entity_type == "SmokeScreen" {
                    let radius = create
                        .props
                        .get("radius")
                        .and_then(|v| v.float_32_ref().copied())
                        .unwrap_or(0.0);
                    self.smoke_screens.insert(
                        create.entity_id,
                        SmokeCloud {
                            radius,
                            created: decoded.clock,
                            destroyed: None,
                            initial_point: WorldPos {
                                x: create.position.x,
                                y: create.position.y,
                                z: create.position.z,
                            },
                            events: Vec::new(),
                        },
                    );
                }
            }

            DecodedPacketPayload::EntityLeave(leave) => {
                if let Some(smoke) = self.smoke_screens.get_mut(&leave.entity_id) {
                    smoke.destroyed = Some(decoded.clock);
                }
            }

            _ => {}
        }
    }

    fn finish(&mut self) {
        println!(
            "Data collected: {} entities with positions, {} minimap updates, {} shots, {} torpedoes ({} hits), {} kills, {} smoke screens, {} plane squadrons, last_clock={}",
            self.positions.len(),
            self.minimap_updates.len(),
            self.shots.len(),
            self.torpedoes.len(),
            self.torpedo_hits.len(),
            self.kills.len(),
            self.smoke_screens.len(),
            self.planes.len(),
            self.last_clock,
        );

        // Summarize squadron types
        let mut icon_counts: HashMap<String, usize> = HashMap::new();
        for info in self.squadron_info.values() {
            *icon_counts.entry(info.icon_base.clone()).or_default() += 1;
        }
        println!("Squadron icon types (active at end): {:?}", icon_counts);
        // Count all PlaneAdded events by looking at planes map
        println!("Total unique squadron IDs: {}", self.planes.len());

        if let Some(dump_mode) = self.dump_mode.clone() {
            let frame_idx = match dump_mode {
                DumpMode::Frame(n) => n,
                DumpMode::Midpoint => TOTAL_FRAMES / 2,
            };
            match self.render_single_frame(frame_idx) {
                Ok(frame) => {
                    let png_path = self.output_path.replace(".mp4", ".png");
                    let png_path = if png_path == self.output_path {
                        format!("{}.png", self.output_path)
                    } else {
                        png_path
                    };
                    if let Err(e) = frame.save(&png_path) {
                        eprintln!("Error saving frame: {}", e);
                    } else {
                        println!(
                            "Frame {} saved to {} ({}x{})",
                            frame_idx,
                            png_path,
                            frame.width(),
                            frame.height()
                        );
                    }
                }
                Err(e) => eprintln!("Error rendering frame: {}", e),
            }
        } else {
            if let Err(e) = self.render_frames() {
                eprintln!("Error rendering video: {}", e);
            }
        }
    }
}

impl MinimapRenderer {
    /// Draw a ship using its species icon if available, otherwise fall back to a circle.
    fn draw_ship_or_icon(
        &self,
        frame: &mut RgbImage,
        entity_id: EntityId,
        x: i32,
        y: i32,
        yaw: f32,
        color: Rgb<u8>,
        visibility: drawing::ShipVisibility,
    ) {
        if let Some(species) = self.player_species.get(&entity_id) {
            if let Some(icon) = self.ship_icons.get(species) {
                drawing::draw_ship_icon(frame, icon, x, y, yaw, color, visibility);
                return;
            }
        }
        // Fallback: circle-based rendering
        match visibility {
            drawing::ShipVisibility::Visible => {
                drawing::draw_ship(frame, x, y, yaw, color, 5);
            }
            drawing::ShipVisibility::MinimapOnly => {
                drawing::draw_ship_outline(frame, x, y, yaw, color, 5);
            }
            drawing::ShipVisibility::Undetected => {
                drawing::draw_ship_undetected(frame, x, y, yaw, 5);
            }
        }
    }

    fn handle_property_update(
        &mut self,
        prop_update: &wows_replays::packet2::PropertyUpdatePacket<'_>,
        clock: GameClock,
    ) {
        use wows_replays::nested_property_path::{PropertyNestLevel, UpdateAction};

        let levels = &prop_update.update_cmd.levels;
        let action = &prop_update.update_cmd.action;

        // Team scores: state -> missions -> teamsScore -> [N] -> SetKey{score}
        if levels.len() >= 3 {
            if let (
                PropertyNestLevel::DictKey("missions"),
                PropertyNestLevel::DictKey("teamsScore"),
                PropertyNestLevel::ArrayIndex(team_idx),
            ) = (&levels[0], &levels[1], &levels[2])
            {
                if let UpdateAction::SetKey {
                    key: "score",
                    value,
                } = action
                {
                    if let Ok(score) = TryInto::<i32>::try_into(value) {
                        let (mut s0, mut s1) = self
                            .scores
                            .last()
                            .map(|&(_, a, b)| (a, b))
                            .unwrap_or((0, 0));
                        match team_idx {
                            0 => s0 = score,
                            1 => s1 = score,
                            _ => {}
                        }
                        self.scores.push((clock, s0, s1));
                    }
                }
            }
        }
    }

    fn extract_initial_scores(
        &mut self,
        props: &HashMap<&str, wowsunpack::rpc::typedefs::ArgValue<'_>>,
        clock: GameClock,
    ) {
        use wowsunpack::rpc::typedefs::ArgValue;

        let state = match props.get("state") {
            Some(ArgValue::FixedDict(d)) => d,
            _ => return,
        };
        let missions = match state.get("missions") {
            Some(ArgValue::FixedDict(d)) => d,
            _ => return,
        };
        let teams_score = match missions.get("teamsScore") {
            Some(ArgValue::Array(a)) => a,
            _ => return,
        };

        let mut s0 = 0i32;
        let mut s1 = 0i32;
        for (i, entry) in teams_score.iter().enumerate() {
            if let ArgValue::FixedDict(d) = entry {
                if let Some(score_val) = d.get("score") {
                    if let Ok(score) = TryInto::<i32>::try_into(score_val) {
                        match i {
                            0 => s0 = score,
                            1 => s1 = score,
                            _ => {}
                        }
                    }
                }
            }
        }
        if s0 != 0 || s1 != 0 {
            self.scores.push((clock, s0, s1));
        }
    }
}

/// Parse Annex B byte stream into individual NAL units (without start codes).
fn parse_annexb_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut i = 0;
    while i < data.len() {
        // Find start code: 0x000001 or 0x00000001
        if i + 2 < data.len() && data[i] == 0 && data[i + 1] == 0 {
            let (start, _) = if i + 3 < data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                (i + 4, 4)
            } else if data[i + 2] == 1 {
                (i + 3, 3)
            } else {
                i += 1;
                continue;
            };
            // Find the end: next start code or end of data
            let mut end = start;
            while end < data.len() {
                if end + 2 < data.len()
                    && data[end] == 0
                    && data[end + 1] == 0
                    && (data[end + 2] == 1
                        || (end + 3 < data.len() && data[end + 2] == 0 && data[end + 3] == 1))
                {
                    break;
                }
                end += 1;
            }
            if end > start {
                nals.push(&data[start..end]);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    nals
}
