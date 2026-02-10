use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;

use anyhow::{anyhow, Context};
use bytes::Bytes;
use openh264::encoder::{Encoder, EncoderConfig, FrameRate};
use openh264::formats::{RgbSliceU8, YUVBuffer};
use openh264::OpenH264API;
use wowsunpack::game_params::provider::GameMetadataProvider;
use wowsunpack::game_params::types::{GameParamProvider, PlaneCategory, Species};

use wows_replays::analyzer::battle_controller::listener::BattleControllerState;
use wows_replays::types::{EntityId, GameClock, PlaneId, Relation};

use crate::draw_command::{DrawCommand, RenderTarget, ShipVisibility};
use crate::drawing::ImageTarget;
use crate::map_data::{self, MinimapPos, WorldPos};

/// Convert a parser NormalizedPos to a minimap pixel position.
fn normalized_to_minimap(pos: &wows_replays::types::NormalizedPos, output_size: u32) -> MinimapPos {
    MinimapPos {
        x: (pos.x * output_size as f32) as i32,
        y: ((1.0 - pos.y) * output_size as f32) as i32,
    }
}

const TOTAL_FRAMES: usize = 1800;
const FPS: f64 = 30.0;
// Use 768 (multiple of 16) for H.264 macroblock alignment
const MINIMAP_SIZE: u32 = 768;
const CANVAS_HEIGHT: u32 = MINIMAP_SIZE + 32; // 800

// How long various effects persist in game-seconds
const TRACER_LEN: f32 = 0.12; // fraction of total shot path length
const KILL_FEED_DURATION: f32 = 10.0;

#[derive(Clone, Debug)]
pub enum DumpMode {
    Frame(usize),
    Midpoint,
}

struct SquadronInfo {
    owner_id: EntityId,
    icon_base: String,
    icon_dir: &'static str,
}

/// Streaming minimap renderer.
///
/// Reads live state from `BattleControllerState` at each frame boundary
/// and emits `DrawCommand`s to a `RenderTarget`. No timelines are stored.
/// Frames are encoded to H.264 on the fly to avoid storing raw RGB data.
pub struct MinimapRenderer {
    // Config (immutable after construction)
    output_path: String,
    dump_mode: Option<DumpMode>,
    map_info: Option<map_data::MapInfo>,
    game_params: GameMetadataProvider,

    // Caches populated lazily from controller state
    squadron_info: HashMap<PlaneId, SquadronInfo>,
    player_species: HashMap<EntityId, String>,
    player_names: HashMap<EntityId, String>,
    player_relations: HashMap<EntityId, Relation>,
    players_populated: bool,

    // Frame tracking
    game_duration: f32,
    last_rendered_frame: i64,

    // H.264 encoder (created lazily on first video frame)
    encoder: Option<Encoder>,
    // Encoded H.264 Annex B NAL data per frame (much smaller than raw RGB)
    h264_frames: Vec<Vec<u8>>,
}

impl MinimapRenderer {
    pub fn new(
        output_path: &str,
        map_info: Option<map_data::MapInfo>,
        dump_mode: Option<DumpMode>,
        game_params: GameMetadataProvider,
        game_duration: f32,
    ) -> Self {
        Self {
            output_path: output_path.to_string(),
            dump_mode,
            map_info,
            game_params,
            squadron_info: HashMap::new(),
            player_species: HashMap::new(),
            player_names: HashMap::new(),
            player_relations: HashMap::new(),
            players_populated: false,
            game_duration,
            last_rendered_frame: -1,
            encoder: None,
            h264_frames: Vec::with_capacity(TOTAL_FRAMES),
        }
    }

    /// Create the H.264 encoder on first use.
    fn ensure_encoder(&mut self) -> anyhow::Result<()> {
        if self.encoder.is_some() {
            return Ok(());
        }
        let config = EncoderConfig::new()
            .max_frame_rate(FrameRate::from_hz(FPS as f32))
            .usage_type(openh264::encoder::UsageType::ScreenContentRealTime)
            .rate_control_mode(openh264::encoder::RateControlMode::Bitrate)
            .bitrate(openh264::encoder::BitRate::from_bps(20_000_000))
            .qp(openh264::encoder::QpRange::new(0, 24))
            .adaptive_quantization(false)
            .background_detection(false);
        self.encoder = Some(
            Encoder::with_api_config(OpenH264API::from_source(), config)
                .context("Failed to create H.264 encoder")?,
        );
        println!(
            "Rendering {} frames ({}x{}, {:.1}s game time at {:.0} fps)...",
            TOTAL_FRAMES, MINIMAP_SIZE, CANVAS_HEIGHT, self.game_duration, FPS
        );
        Ok(())
    }

    /// Encode a rendered frame to H.264 immediately.
    fn encode_frame(&mut self, target: &ImageTarget) -> anyhow::Result<()> {
        let encoder = self
            .encoder
            .as_mut()
            .ok_or_else(|| anyhow!("Encoder not initialized"))?;
        let frame_image = target.frame();
        let rgb_data = frame_image.as_raw();
        let rgb = RgbSliceU8::new(rgb_data, (MINIMAP_SIZE as usize, CANVAS_HEIGHT as usize));
        let yuv = YUVBuffer::from_rgb_source(rgb);
        let bitstream = encoder
            .encode(&yuv)
            .map_err(|e| anyhow!("H.264 encode error: {:?}", e))?;
        self.h264_frames.push(bitstream.to_vec());
        Ok(())
    }

    /// Called before each packet is processed by the controller.
    ///
    /// If the new clock has crossed one or more frame boundaries, renders
    /// frames from the controller's current state (which reflects all
    /// packets up to but not including this one).
    pub fn advance_clock(
        &mut self,
        new_clock: GameClock,
        controller: &dyn BattleControllerState,
        target: &mut ImageTarget,
    ) {
        if self.game_duration <= 0.0 {
            return;
        }

        // Populate player info on first opportunity
        self.populate_players(controller);

        let frame_duration = self.game_duration / TOTAL_FRAMES as f32;
        let target_frame = (new_clock.seconds() / frame_duration) as i64;

        while self.last_rendered_frame < target_frame {
            self.last_rendered_frame += 1;
            if self.last_rendered_frame >= TOTAL_FRAMES as i64 {
                break;
            }

            // Update squadron info for any new planes
            self.update_squadron_info(controller);

            let commands = self.draw_frame(controller);

            if let Some(ref dump_mode) = self.dump_mode {
                let dump_frame = match dump_mode {
                    DumpMode::Frame(n) => *n as i64,
                    DumpMode::Midpoint => TOTAL_FRAMES as i64 / 2,
                };
                if self.last_rendered_frame == dump_frame {
                    target.begin_frame();
                    for cmd in &commands {
                        target.draw(cmd);
                    }
                    target.end_frame();

                    let png_path = self.output_path.replace(".mp4", ".png");
                    let png_path = if png_path == self.output_path {
                        format!("{}.png", self.output_path)
                    } else {
                        png_path
                    };
                    if let Err(e) = target.frame().save(&png_path) {
                        eprintln!("Error saving frame: {}", e);
                    } else {
                        let (w, h) = target.canvas_size();
                        println!("Frame {} saved to {} ({}x{})", dump_frame, png_path, w, h);
                    }
                }
            } else {
                // Full video mode: render, encode to H.264 immediately
                if let Err(e) = self.ensure_encoder() {
                    eprintln!("Encoder error: {}", e);
                    return;
                }

                target.begin_frame();
                for cmd in &commands {
                    target.draw(cmd);
                }
                target.end_frame();

                if let Err(e) = self.encode_frame(target) {
                    eprintln!("Encode error: {}", e);
                    return;
                }

                if self.last_rendered_frame % 100 == 0 {
                    println!("  Frame {}/{}", self.last_rendered_frame, TOTAL_FRAMES);
                }
            }
        }
    }

    /// Finalize: flush any remaining frames and write the video file.
    pub fn finish(
        &mut self,
        controller: &dyn BattleControllerState,
        target: &mut ImageTarget,
    ) -> anyhow::Result<()> {
        // Render up to the actual battle end (or last packet), not meta.duration.
        // This avoids duplicating frozen frames when the match ends early.
        let end_clock = controller.battle_end_clock().unwrap_or(controller.clock());
        self.advance_clock(end_clock, controller, target);

        if self.dump_mode.is_some() {
            return Ok(());
        }

        // Mux the already-encoded H.264 frames into MP4
        self.mux_to_mp4()
    }

    /// Mux pre-encoded H.264 Annex B frames into an MP4 file.
    fn mux_to_mp4(&self) -> anyhow::Result<()> {
        if self.h264_frames.is_empty() {
            return Err(anyhow!("No frames to mux"));
        }

        // Extract SPS and PPS from the first keyframe
        let first_frame = &self.h264_frames[0];
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

        let sample_duration = 1000 / FPS as u32;

        for (frame_idx, annexb_data) in self.h264_frames.iter().enumerate() {
            if annexb_data.is_empty() {
                continue;
            }
            let nals = parse_annexb_nals(annexb_data);
            let is_sync = nals.iter().any(|n| (n[0] & 0x1f) == 5);

            let mut avcc_data = Vec::new();
            for nal in &nals {
                let nal_type = nal[0] & 0x1f;
                if nal_type == 7 || nal_type == 8 {
                    continue;
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

    /// Produce draw commands for the current frame from controller state.
    fn draw_frame(&self, controller: &dyn BattleControllerState) -> Vec<DrawCommand> {
        let map_info = match self.map_info.as_ref() {
            Some(info) => info,
            None => return Vec::new(),
        };

        let clock = controller.clock();
        let mut commands = Vec::new();

        // 1. Score bar
        let scores = controller.team_scores();
        if scores.len() >= 2 {
            commands.push(DrawCommand::ScoreBar {
                team0: scores[0].score as i32,
                team1: scores[1].score as i32,
            });
        }

        // 2. Artillery shot tracers
        for shot in controller.active_shots() {
            for shot_data in &shot.salvo.shots {
                let origin = WorldPos {
                    x: shot_data.origin.0,
                    y: shot_data.origin.1,
                    z: shot_data.origin.2,
                };
                let target = WorldPos {
                    x: shot_data.target.0,
                    y: shot_data.target.1,
                    z: shot_data.target.2,
                };
                let dx = target.x - origin.x;
                let dz = target.z - origin.z;
                let distance = (dx * dx + dz * dz).sqrt();
                let flight_duration = if shot_data.speed > 0.0 {
                    distance / shot_data.speed
                } else {
                    3.0
                };

                let elapsed = clock - shot.fired_at;
                if elapsed < 0.0 || elapsed > flight_duration {
                    continue;
                }
                let frac = elapsed / flight_duration;
                let head = origin.lerp(target, frac);
                let tail = origin.lerp(target, (frac - TRACER_LEN).max(0.0));
                commands.push(DrawCommand::ShotTracer {
                    from: map_info.world_to_minimap(tail, MINIMAP_SIZE),
                    to: map_info.world_to_minimap(head, MINIMAP_SIZE),
                });
            }
        }

        // 3. Torpedoes
        let half_space = map_info.space_size as f32 / 2.0;
        for torp in controller.active_torpedoes() {
            let elapsed = clock - torp.launched_at;
            if elapsed < 0.0 {
                continue;
            }
            let world = WorldPos {
                x: torp.torpedo.origin.0 + torp.torpedo.direction.0 * elapsed,
                y: 0.0,
                z: torp.torpedo.origin.2 + torp.torpedo.direction.2 * elapsed,
            };
            if world.x.abs() > half_space || world.z.abs() > half_space {
                continue;
            }
            let relation = self
                .player_relations
                .get(&torp.torpedo.owner_id)
                .copied()
                .unwrap_or(Relation::new(2));
            let friendly = relation.is_self() || relation.is_ally();
            commands.push(DrawCommand::Torpedo {
                pos: map_info.world_to_minimap(world, MINIMAP_SIZE),
                friendly,
            });
        }

        // 4. Smoke screens
        for entity in controller.entities_by_id().values() {
            if let Some(smoke_ref) = entity.smoke_screen_ref() {
                let smoke = smoke_ref.borrow();
                let px_radius =
                    (smoke.radius / map_info.space_size as f32 * MINIMAP_SIZE as f32) as i32;
                for point in &smoke.points {
                    let px = map_info.world_to_minimap(*point, MINIMAP_SIZE);
                    commands.push(DrawCommand::Smoke {
                        pos: px,
                        radius: px_radius.max(3),
                    });
                }
            }
        }

        // 5. Ships
        let ship_positions = controller.ship_positions();
        let minimap_positions = controller.minimap_positions();

        // Collect all entity IDs that have either world or minimap positions
        let mut all_ship_ids: Vec<EntityId> = ship_positions
            .keys()
            .chain(minimap_positions.keys())
            .copied()
            .collect();
        all_ship_ids.sort();
        all_ship_ids.dedup();

        let dead_ships = controller.dead_ships();

        for entity_id in &all_ship_ids {
            // Skip dead ships (they get an X marker below)
            if let Some(dead) = dead_ships.get(entity_id) {
                if clock >= dead.clock {
                    continue;
                }
            }

            let relation = self
                .player_relations
                .get(entity_id)
                .copied()
                .unwrap_or(Relation::new(2));
            let color = ship_color_rgb(relation);
            let species = self.player_species.get(entity_id).cloned();

            let minimap = minimap_positions.get(entity_id);
            let world = ship_positions.get(entity_id);
            let detected = minimap.map(|m| m.visible).unwrap_or(false);

            // Get health fraction from entity
            let health_fraction = controller
                .entities_by_id()
                .get(entity_id)
                .and_then(|e| e.vehicle_ref())
                .map(|v| {
                    let v = v.borrow();
                    let max = v.props().max_health();
                    if max > 0.0 {
                        Some((v.props().health() / max).clamp(0.0, 1.0))
                    } else {
                        None
                    }
                })
                .flatten();

            // Compute yaw based on visibility:
            // - Detected: use minimap heading (most accurate for icon rotation)
            // - Undetected: use world position yaw (minimap heading may be stale/wrong)
            let minimap_yaw =
                minimap.map(|mm| std::f32::consts::FRAC_PI_2 - mm.heading.to_radians());
            let world_yaw = world.map(|sp| sp.yaw);

            if detected {
                let yaw = minimap_yaw.or(world_yaw).unwrap_or(0.0);
                if let Some(ship_pos) = world {
                    // Have world position — use it for location, minimap heading for yaw
                    let px = map_info.world_to_minimap(ship_pos.position, MINIMAP_SIZE);
                    commands.push(DrawCommand::Ship {
                        pos: px,
                        yaw,
                        species,
                        color,
                        visibility: ShipVisibility::Visible,
                        health_fraction,
                    });
                } else if let Some(mm) = minimap {
                    // Minimap-only position
                    let px = normalized_to_minimap(&mm.position, MINIMAP_SIZE);
                    commands.push(DrawCommand::Ship {
                        pos: px,
                        yaw,
                        species,
                        color,
                        visibility: ShipVisibility::MinimapOnly,
                        health_fraction,
                    });
                }
            } else {
                // Undetected — prefer minimap heading (more reliable than stale world_yaw=0.0)
                let yaw = minimap_yaw.or(world_yaw).unwrap_or(0.0);
                if let Some(ship_pos) = world {
                    let px = map_info.world_to_minimap(ship_pos.position, MINIMAP_SIZE);
                    commands.push(DrawCommand::Ship {
                        pos: px,
                        yaw,
                        species,
                        color,
                        visibility: ShipVisibility::Undetected,
                        health_fraction,
                    });
                } else if let Some(mm) = minimap {
                    let px = normalized_to_minimap(&mm.position, MINIMAP_SIZE);
                    commands.push(DrawCommand::Ship {
                        pos: px,
                        yaw,
                        species,
                        color,
                        visibility: ShipVisibility::Undetected,
                        health_fraction,
                    });
                }
            }
        }

        // 6. Dead ship markers
        for (_, dead) in dead_ships {
            if clock >= dead.clock {
                let px = map_info.world_to_minimap(dead.position, MINIMAP_SIZE);
                commands.push(DrawCommand::DeadShip { pos: px });
            }
        }

        // 7. Planes
        for (plane_id, plane) in controller.active_planes() {
            let world = WorldPos {
                x: plane.x,
                y: 0.0,
                z: plane.y,
            };
            let px = map_info.world_to_minimap(world, MINIMAP_SIZE);

            let info = self.squadron_info.get(plane_id);
            let is_enemy = info
                .and_then(|i| self.player_relations.get(&i.owner_id))
                .map(|r| r.is_enemy())
                .unwrap_or(false);

            let icon_base = info.map(|i| i.icon_base.as_str()).unwrap_or("fighter");
            let icon_dir = info.map(|i| i.icon_dir).unwrap_or("consumables");
            let suffix = if is_enemy { "enemy" } else { "ally" };
            let icon_key = format!("{}/{}_{}", icon_dir, icon_base, suffix);

            let fallback_color = if is_enemy {
                [254, 77, 42]
            } else {
                [76, 232, 170]
            };

            commands.push(DrawCommand::Plane {
                pos: px,
                icon_key,
                fallback_color,
            });
        }

        // 8. Kill feed
        let kills = controller.kills();
        let mut recent_kills = Vec::new();
        for kill in kills.iter().rev() {
            if clock >= kill.clock && clock <= kill.clock + KILL_FEED_DURATION {
                let killer_name = self
                    .player_names
                    .get(&kill.killer)
                    .cloned()
                    .unwrap_or_else(|| format!("#{}", kill.killer));
                let victim_name = self
                    .player_names
                    .get(&kill.victim)
                    .cloned()
                    .unwrap_or_else(|| format!("#{}", kill.victim));
                recent_kills.push((killer_name, victim_name));
                if recent_kills.len() >= 5 {
                    break;
                }
            }
        }
        if !recent_kills.is_empty() {
            recent_kills.reverse();
            commands.push(DrawCommand::KillFeed {
                entries: recent_kills,
            });
        }

        // 9. Timer
        commands.push(DrawCommand::Timer {
            seconds: clock.seconds(),
        });

        commands
    }

    /// Populate player info from controller state (once).
    fn populate_players(&mut self, controller: &dyn BattleControllerState) {
        if self.players_populated {
            return;
        }
        let players = controller.player_entities();
        if players.is_empty() {
            return;
        }
        for (entity_id, player) in players {
            self.player_names
                .insert(*entity_id, player.initial_state().username().to_string());
            self.player_relations.insert(*entity_id, player.relation());
            if let Some(species) = player.vehicle().species() {
                let species_name = format!("{:?}", species);
                self.player_species.insert(*entity_id, species_name);
            }
        }
        self.players_populated = true;
    }

    /// Update squadron info for any new planes in the controller.
    fn update_squadron_info(&mut self, controller: &dyn BattleControllerState) {
        for (plane_id, plane) in controller.active_planes() {
            if self.squadron_info.contains_key(plane_id) {
                continue;
            }
            let param = self.game_params.game_param_by_id(plane.params_id.raw());
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
                .map(|sp| species_to_icon_base(sp, is_consumable, ammo_type))
                .unwrap_or_else(|| "fighter".to_string());
            let icon_dir = match category {
                PlaneCategory::Consumable => "consumables",
                PlaneCategory::Airsupport => "airsupport",
                PlaneCategory::Controllable => "controllable",
            };
            self.squadron_info.insert(
                *plane_id,
                SquadronInfo {
                    owner_id: plane.owner_id,
                    icon_base,
                    icon_dir,
                },
            );
        }
    }
}

/// Get the ship color as an RGB array based on relation.
fn ship_color_rgb(relation: Relation) -> [u8; 3] {
    if relation.is_self() {
        [255, 255, 255]
    } else if relation.is_ally() {
        [76, 232, 170]
    } else {
        [254, 77, 42]
    }
}

/// Build the icon base name from species, consumable flag, and ammo type.
fn species_to_icon_base(species: Species, is_consumable: bool, ammo_type: &str) -> String {
    use convert_case::{Case, Casing};

    let normalized = match ammo_type {
        "depthcharge" => "depth_charge",
        other => other,
    };
    let ammo = normalized.to_case(Case::Snake);
    if is_consumable {
        match species {
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

/// Parse Annex B byte stream into individual NAL units (without start codes).
fn parse_annexb_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut i = 0;
    while i < data.len() {
        if i + 2 < data.len() && data[i] == 0 && data[i + 1] == 0 {
            let (start, _) = if i + 3 < data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                (i + 4, 4)
            } else if data[i + 2] == 1 {
                (i + 3, 3)
            } else {
                i += 1;
                continue;
            };
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
