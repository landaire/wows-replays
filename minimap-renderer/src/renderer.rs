use std::collections::HashMap;

use wowsunpack::data::ResourceLoader as _;
use wowsunpack::game_params::provider::GameMetadataProvider;
use wowsunpack::game_params::types::{GameParamProvider, PlaneCategory, Species};

use wows_replays::analyzer::battle_controller::listener::BattleControllerState;
use wows_replays::analyzer::decoder::Consumable;
use wows_replays::types::{EntityId, PlaneId, Relation};

use crate::draw_command::{DrawCommand, ShipVisibility};
use crate::map_data::{self, WorldPos};

use crate::MINIMAP_SIZE;

// How long various effects persist in game-seconds
const TRACER_LEN: f32 = 0.12; // fraction of total shot path length
const KILL_FEED_DURATION: f32 = 10.0;

// Visual constants
const SMOKE_COLOR: [u8; 3] = [200, 200, 200];
const SMOKE_ALPHA: f32 = 0.5;
const TRACER_COLOR: [u8; 3] = [255, 255, 255];
const TORPEDO_FRIENDLY_COLOR: [u8; 3] = [76, 232, 170];
const TORPEDO_ENEMY_COLOR: [u8; 3] = [254, 77, 42];
const HP_BAR_FULL_COLOR: [u8; 3] = [0, 255, 0];
const HP_BAR_MID_COLOR: [u8; 3] = [255, 255, 0];
const HP_BAR_LOW_COLOR: [u8; 3] = [255, 0, 0];
const HP_BAR_BG_COLOR: [u8; 3] = [50, 50, 50];
const HP_BAR_BG_ALPHA: f32 = 0.7;
const UNDETECTED_OPACITY: f32 = 0.4;
const TEAM0_COLOR: [u8; 3] = [76, 232, 170]; // Green
const TEAM1_COLOR: [u8; 3] = [254, 77, 42]; // Red

/// Configurable rendering options.
#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub show_hp_bars: bool,
    pub show_tracers: bool,
    pub show_torpedoes: bool,
    pub show_planes: bool,
    pub show_smoke: bool,
    pub show_score: bool,
    pub show_timer: bool,
    pub show_kill_feed: bool,
    pub show_player_names: bool,
    pub show_ship_names: bool,
    pub show_capture_points: bool,
    pub show_buildings: bool,
    pub show_turret_direction: bool,
    pub show_consumables: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            show_hp_bars: true,
            show_tracers: true,
            show_torpedoes: true,
            show_planes: true,
            show_smoke: true,
            show_score: true,
            show_timer: true,
            show_kill_feed: true,
            show_player_names: true,
            show_ship_names: true,
            show_capture_points: true,
            show_buildings: true,
            show_turret_direction: true,
            show_consumables: true,
        }
    }
}

struct SquadronInfo {
    icon_base: String,
    icon_dir: &'static str,
}

/// Streaming minimap renderer.
///
/// Reads live state from `BattleControllerState` at each frame boundary
/// and emits `DrawCommand`s to a `RenderTarget`. No timelines are stored.
pub struct MinimapRenderer<'a> {
    // Config (immutable after construction)
    map_info: Option<map_data::MapInfo>,
    game_params: &'a GameMetadataProvider,
    pub options: RenderOptions,

    // Caches populated lazily from controller state
    squadron_info: HashMap<PlaneId, SquadronInfo>,
    player_species: HashMap<EntityId, String>,
    player_names: HashMap<EntityId, String>,
    ship_display_names: HashMap<EntityId, String>,
    player_relations: HashMap<EntityId, Relation>,
    /// Cache of ship consumable variants: (entity_id, consumable) -> (ability_name, variant_name)
    ship_consumable_variants: HashMap<(EntityId, ConsumableKey), (String, String)>,
    /// Per-ship consumable icon names: (entity_id, Consumable) -> PCY name (e.g. "PCY015_SpeedBoosterPremium")
    ship_ability_icons: HashMap<(EntityId, Consumable), String>,
    /// Track which entities we've already resolved ability icons for
    resolved_entities: std::collections::HashSet<EntityId>,
    players_populated: bool,
}

/// Key for consumable variant lookup (simplified consumable type for detection consumables)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ConsumableKey {
    Radar,
    Hydro,
    Hydrophone,
    SubSurveillance,
}

impl<'a> MinimapRenderer<'a> {
    pub fn new(
        map_info: Option<map_data::MapInfo>,
        game_params: &'a GameMetadataProvider,
        options: RenderOptions,
    ) -> Self {
        Self {
            map_info,
            game_params,
            options,
            squadron_info: HashMap::new(),
            player_species: HashMap::new(),
            player_names: HashMap::new(),
            ship_display_names: HashMap::new(),
            player_relations: HashMap::new(),
            ship_consumable_variants: HashMap::new(),
            ship_ability_icons: HashMap::new(),
            resolved_entities: std::collections::HashSet::new(),
            players_populated: false,
        }
    }

    /// Reset all cached state, allowing the renderer to be reused after a seek.
    pub fn reset(&mut self) {
        self.squadron_info.clear();
        self.player_species.clear();
        self.player_names.clear();
        self.ship_display_names.clear();
        self.player_relations.clear();
        self.ship_consumable_variants.clear();
        self.ship_ability_icons.clear();
        self.resolved_entities.clear();
        self.players_populated = false;
    }

    /// Populate player info from controller state (once).
    ///
    /// Uses `player_entities` (populated from onArenaStateReceived packet parsing).
    pub fn populate_players(&mut self, controller: &dyn BattleControllerState) {
        if self.players_populated {
            return;
        }

        let players = controller.player_entities();
        if players.is_empty() {
            return;
        }

        for (entity_id, player) in players {
            self.player_relations.insert(*entity_id, player.relation());
            if let Some(species) = player.vehicle().species() {
                self.player_species
                    .insert(*entity_id, format!("{:?}", species));
            }
            self.player_names
                .insert(*entity_id, player.initial_state().username().to_string());
            if let Some(name) = self.game_params.localized_name_from_param(player.vehicle()) {
                self.ship_display_names.insert(*entity_id, name.to_string());
            }

            // Cache consumable variants for detection radius lookup
            let ship_id = player.vehicle().id();
            let ship_param = GameParamProvider::game_param_by_id(self.game_params, ship_id);
            if let Some(vehicle) = ship_param.as_ref().and_then(|p| p.vehicle()) {
                if let Some(abilities) = vehicle.abilities() {
                    for slot in abilities {
                        for (ability_name, variant_name) in slot {
                            if let Some(consumable_key) =
                                ability_name_to_consumable_key(ability_name)
                            {
                                self.ship_consumable_variants.insert(
                                    (*entity_id, consumable_key),
                                    (ability_name.clone(), variant_name.clone()),
                                );
                            }
                        }
                    }
                }
            }
        }
        self.players_populated = true;
    }

    /// Resolve per-ship ability icon names from entity vehicle data.
    ///
    /// For each vehicle entity, reads `ship_config().abilities()` (equipped GameParam IDs),
    /// looks up each ability in GameParams to get its `consumable_type` and `name`,
    /// and maps `(EntityId, Consumable)` → PCY name for icon lookup.
    pub fn update_ship_abilities(&mut self, controller: &dyn BattleControllerState) {
        for (entity_id, entity) in controller.entities_by_id() {
            if self.resolved_entities.contains(entity_id) {
                continue;
            }
            let vehicle = match entity.vehicle_ref() {
                Some(v) => v,
                None => continue,
            };
            let vehicle = vehicle.borrow();
            let abilities = vehicle.props().ship_config().abilities();
            if abilities.is_empty() {
                continue;
            }
            self.resolved_entities.insert(*entity_id);
            for &ability_id in abilities {
                let param = match GameParamProvider::game_param_by_id(self.game_params, ability_id)
                {
                    Some(p) => p,
                    None => continue,
                };
                let ability = match param.ability() {
                    Some(a) => a,
                    None => continue,
                };
                // Get consumable_type from the first category
                let consumable_type = match ability.categories().values().next() {
                    Some(cat) => cat.consumable_type().to_string(),
                    None => continue,
                };
                if let Some(consumable) = consumable_type_to_enum(&consumable_type) {
                    self.ship_ability_icons
                        .insert((*entity_id, consumable), param.name().to_string());
                }
            }
        }
    }

    /// Get the icon key for a consumable on a specific ship.
    ///
    /// Uses the per-ship ability mapping if available, falling back to the
    /// hardcoded base PCY name.
    fn consumable_icon_key(&self, entity_id: EntityId, consumable: Consumable) -> Option<String> {
        if let Some(name) = self.ship_ability_icons.get(&(entity_id, consumable)) {
            return Some(name.clone());
        }
        consumable_to_base_icon_key(consumable)
    }

    /// Look up detection radius for a consumable on a specific ship from GameParams.
    ///
    /// Returns radius in world units (meters), or None if not a detection consumable
    /// or if the lookup fails.
    fn get_consumable_radius(&self, entity_id: EntityId, consumable: Consumable) -> Option<f32> {
        // Try ship-specific lookup first (uses cached ability variants from populate_players)
        let consumable_key = match consumable {
            Consumable::Radar => Some(ConsumableKey::Radar),
            Consumable::HydroacousticSearch => Some(ConsumableKey::Hydro),
            Consumable::Hydrophone => Some(ConsumableKey::Hydrophone),
            Consumable::SubmarineSurveillance => Some(ConsumableKey::SubSurveillance),
            _ => None,
        };

        if let Some(key) = consumable_key {
            if let Some((ability_name, variant_name)) =
                self.ship_consumable_variants.get(&(entity_id, key))
            {
                if let Some(param) =
                    GameParamProvider::game_param_by_name(self.game_params, ability_name)
                {
                    if let Some(ability) = param.ability() {
                        if let Some(cat) = ability.get_category(variant_name) {
                            if let Some(radius) = cat.detection_radius() {
                                return Some(radius);
                            }
                        }
                    }
                }
            }
        }

        // Fallback: look up the default ability by well-known name
        // Try both the Modifier variant (from ship abilities) and the base name
        let ability_names: &[&str] = match consumable {
            Consumable::Radar => &["PCY019_RLSSearchModifier", "PCY019_RLSSearch"],
            Consumable::HydroacousticSearch => {
                &["PCY008_SonarSearchModifier", "PCY008_SonarSearch"]
            }
            Consumable::Hydrophone => &["PCY045_HydrophoneModifier", "PCY045_Hydrophone"],
            Consumable::SubmarineSurveillance => {
                &["PCY048_SubmarineLocatorModifier", "PCY048_SubmarineLocator"]
            }
            _ => return None,
        };

        for ability_name in ability_names {
            if let Some(param) =
                GameParamProvider::game_param_by_name(self.game_params, ability_name)
            {
                if let Some(ability) = param.ability() {
                    if let Some(radius) = ability
                        .categories()
                        .iter()
                        .find_map(|(_, cat)| cat.detection_radius())
                    {
                        return Some(radius);
                    }
                }
            }
        }

        None
    }

    /// Update squadron info for any new planes in the controller.
    pub fn update_squadron_info(&mut self, controller: &dyn BattleControllerState) {
        for (plane_id, plane) in controller.active_planes() {
            if self.squadron_info.contains_key(plane_id) {
                continue;
            }
            let param =
                GameParamProvider::game_param_by_id(self.game_params, plane.params_id.raw());
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
                    icon_base,
                    icon_dir,
                },
            );
        }
    }

    /// Produce draw commands for the current frame from controller state.
    pub fn draw_frame(&self, controller: &dyn BattleControllerState) -> Vec<DrawCommand> {
        let map_info = match self.map_info.as_ref() {
            Some(info) => info,
            None => return Vec::new(),
        };

        let clock = controller.clock();
        let mut commands = Vec::new();

        // 1. Score bar
        if self.options.show_score {
            let scores = controller.team_scores();
            if scores.len() >= 2 {
                commands.push(DrawCommand::ScoreBar {
                    team0: scores[0].score as i32,
                    team1: scores[1].score as i32,
                    team0_color: TEAM0_COLOR,
                    team1_color: TEAM1_COLOR,
                });
            }
        }

        // 2. Capture points (drawn early so they're behind everything)
        if self.options.show_capture_points {
            for cp in controller.capture_points() {
                let pos = match cp.position {
                    Some(p) => p,
                    None => continue,
                };
                let px = map_info.world_to_minimap(pos, MINIMAP_SIZE);
                let px_radius =
                    (cp.radius / map_info.space_size as f32 * MINIMAP_SIZE as f32) as i32;
                let color = cap_point_color(cp.team_id);
                let label = if cp.control_point_type == 5 {
                    "\u{2691}".to_string() // flag character
                } else {
                    let letter = (b'A' + cp.index as u8) as char;
                    letter.to_string()
                };
                let progress = cp.progress.0 as f32;
                let invader_color = if cp.has_invaders && cp.invader_team >= 0 {
                    Some(cap_point_color(cp.invader_team))
                } else {
                    None
                };
                commands.push(DrawCommand::CapturePoint {
                    pos: px,
                    radius: px_radius.max(5),
                    color,
                    alpha: 0.15,
                    label,
                    progress,
                    invader_color,
                });
            }
        }

        // 3. Artillery shot tracers
        if self.options.show_tracers {
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
                        color: TRACER_COLOR,
                    });
                }
            }
        }

        // 3. Torpedoes
        if self.options.show_torpedoes {
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
                let color = if relation.is_self() || relation.is_ally() {
                    TORPEDO_FRIENDLY_COLOR
                } else {
                    TORPEDO_ENEMY_COLOR
                };
                commands.push(DrawCommand::Torpedo {
                    pos: map_info.world_to_minimap(world, MINIMAP_SIZE),
                    color,
                });
            }
        }

        // 4. Smoke screens
        if self.options.show_smoke {
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
                            color: SMOKE_COLOR,
                            alpha: SMOKE_ALPHA,
                        });
                    }
                }
            }
        }

        // 5. Buildings
        if self.options.show_buildings {
            for entity in controller.entities_by_id().values() {
                if let Some(building_ref) = entity.building_ref() {
                    let building = building_ref.borrow();
                    if building.is_hidden {
                        continue;
                    }
                    let px = map_info.world_to_minimap(building.position, MINIMAP_SIZE);
                    let color = if building.is_alive {
                        cap_point_color(building.team_id as i64)
                    } else {
                        [40, 40, 40]
                    };
                    commands.push(DrawCommand::Building {
                        pos: px,
                        color,
                        is_alive: building.is_alive,
                    });
                }
            }
        }

        // 6. Ships
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
            let player_name = if self.options.show_player_names {
                self.player_names.get(entity_id).cloned()
            } else {
                None
            };
            let ship_name = if self.options.show_ship_names {
                self.ship_display_names.get(entity_id).cloned()
            } else {
                None
            };

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

            // Compute yaw: prefer minimap heading (more accurate for icon rotation)
            let minimap_yaw =
                minimap.map(|mm| std::f32::consts::FRAC_PI_2 - mm.heading.to_radians());
            let world_yaw = world.map(|sp| sp.yaw);

            // A ship is "spotted" when its visibility_flags are non-zero (game mechanic)
            let is_spotted = controller
                .entities_by_id()
                .get(entity_id)
                .and_then(|e| e.vehicle_ref())
                .map(|v| v.borrow().props().visibility_flags() != 0)
                .unwrap_or(false);

            // Detected teammate = spotted ally (not self)
            let is_detected_teammate = is_spotted && relation.is_ally() && !relation.is_self();

            if detected {
                let yaw = minimap_yaw.or(world_yaw).unwrap_or(0.0);
                if let Some(ship_pos) = world {
                    // Have world position — use it (higher precision than minimap)
                    let px = map_info.world_to_minimap(ship_pos.position, MINIMAP_SIZE);
                    commands.push(DrawCommand::Ship {
                        pos: px,
                        yaw,
                        species: species.clone(),
                        color: Some(color),
                        visibility: ShipVisibility::Visible,
                        opacity: 1.0,
                        is_self: relation.is_self(),
                        player_name: player_name.clone(),
                        ship_name: ship_name.clone(),
                        is_detected_teammate,
                    });
                    if self.options.show_hp_bars {
                        if let Some(frac) = health_fraction {
                            let fill_color = hp_bar_color(frac);
                            commands.push(DrawCommand::HealthBar {
                                pos: px,
                                fraction: frac,
                                fill_color,
                                background_color: HP_BAR_BG_COLOR,
                                background_alpha: HP_BAR_BG_ALPHA,
                            });
                        }
                    }
                } else if let Some(mm) = minimap {
                    // Minimap-only position
                    let px = map_info.normalized_to_minimap(&mm.position, MINIMAP_SIZE);
                    commands.push(DrawCommand::Ship {
                        pos: px,
                        yaw,
                        species: species.clone(),
                        color: None,
                        visibility: ShipVisibility::MinimapOnly,
                        opacity: 1.0,
                        is_self: relation.is_self(),
                        player_name: player_name.clone(),
                        ship_name: ship_name.clone(),
                        is_detected_teammate,
                    });
                    if self.options.show_hp_bars {
                        if let Some(frac) = health_fraction {
                            let fill_color = hp_bar_color(frac);
                            commands.push(DrawCommand::HealthBar {
                                pos: px,
                                fraction: frac,
                                fill_color,
                                background_color: HP_BAR_BG_COLOR,
                                background_alpha: HP_BAR_BG_ALPHA,
                            });
                        }
                    }
                }
            } else {
                // Undetected — prefer world position, fall back to minimap
                let yaw = minimap_yaw.or(world_yaw).unwrap_or(0.0);
                let px = if let Some(ship_pos) = world {
                    map_info.world_to_minimap(ship_pos.position, MINIMAP_SIZE)
                } else if let Some(mm) = minimap {
                    map_info.normalized_to_minimap(&mm.position, MINIMAP_SIZE)
                } else {
                    continue;
                };
                commands.push(DrawCommand::Ship {
                    pos: px,
                    yaw,
                    species: species.clone(),
                    color: None,
                    visibility: ShipVisibility::Undetected,
                    opacity: UNDETECTED_OPACITY,
                    is_self: relation.is_self(),
                    player_name: None,
                    ship_name: None,
                    is_detected_teammate: false, // Undetected ships can't be detected teammates
                });
            }
        }

        // 6. Turret direction indicators (from targetLocalPos EntityProperty)
        if self.options.show_turret_direction {
            let target_yaws = controller.target_yaws();
            for (entity_id, &world_yaw) in target_yaws {
                // Skip undetected ships — aim data is stale
                let detected = minimap_positions
                    .get(entity_id)
                    .map(|m| m.visible)
                    .unwrap_or(false);
                if !detected {
                    continue;
                }
                // Need a position for this ship
                let px = if let Some(sp) = ship_positions.get(entity_id) {
                    map_info.world_to_minimap(sp.position, MINIMAP_SIZE)
                } else if let Some(mm) = minimap_positions.get(entity_id) {
                    map_info.normalized_to_minimap(&mm.position, MINIMAP_SIZE)
                } else {
                    continue;
                };
                // targetLocalPos yaw is compass bearing (0=north, CW positive).
                // Convert to screen math coords: screen_yaw = PI/2 - compass_yaw
                let screen_yaw = std::f32::consts::FRAC_PI_2 - world_yaw;
                let relation = self
                    .player_relations
                    .get(entity_id)
                    .copied()
                    .unwrap_or(Relation::new(2));
                let color = ship_color_rgb(relation);
                commands.push(DrawCommand::TurretDirection {
                    pos: px,
                    yaw: screen_yaw,
                    color,
                    length: 18,
                });
            }
        }

        // 7. Dead ship markers
        for (entity_id, dead) in dead_ships {
            if clock >= dead.clock {
                let px = map_info.world_to_minimap(dead.position, MINIMAP_SIZE);
                let species = self.player_species.get(entity_id).cloned();
                // Use last known heading from minimap positions
                let yaw = minimap_positions
                    .get(entity_id)
                    .map(|mm| std::f32::consts::FRAC_PI_2 - mm.heading.to_radians())
                    .or_else(|| ship_positions.get(entity_id).map(|sp| sp.yaw))
                    .unwrap_or(0.0);
                let relation = self
                    .player_relations
                    .get(entity_id)
                    .copied()
                    .unwrap_or(Relation::new(2));
                let player_name = if self.options.show_player_names {
                    self.player_names.get(entity_id).cloned()
                } else {
                    None
                };
                let ship_name = if self.options.show_ship_names {
                    self.ship_display_names.get(entity_id).cloned()
                } else {
                    None
                };
                commands.push(DrawCommand::DeadShip {
                    pos: px,
                    yaw,
                    species,
                    color: None,
                    is_self: relation.is_self(),
                    player_name,
                    ship_name,
                });
            }
        }

        // 7. Planes
        if self.options.show_planes {
            for (plane_id, plane) in controller.active_planes() {
                let world = WorldPos {
                    x: plane.x,
                    y: 0.0,
                    z: plane.y,
                };
                let px = map_info.world_to_minimap(world, MINIMAP_SIZE);

                let info = self.squadron_info.get(plane_id);
                // team_id: 0 = recording player's team, 1 = enemy team
                let is_enemy = plane.team_id == 1;

                let icon_base = info.map(|i| i.icon_base.as_str()).unwrap_or("fighter");
                let icon_dir = info.map(|i| i.icon_dir).unwrap_or("consumables");
                let suffix = if is_enemy { "enemy" } else { "ally" };
                let icon_key = format!("{}/{}_{}", icon_dir, icon_base, suffix);

                commands.push(DrawCommand::Plane { pos: px, icon_key });
            }
        }

        // 8. Active consumables
        if self.options.show_consumables {
            let all_consumables = controller.active_consumables();
            for (entity_id, consumables) in all_consumables {
                // Get ship position (prefer world position, fall back to minimap)
                let pos = if let Some(sp) = ship_positions.get(entity_id) {
                    Some(map_info.world_to_minimap(sp.position, MINIMAP_SIZE))
                } else if let Some(mm) = minimap_positions.get(entity_id) {
                    Some(map_info.normalized_to_minimap(&mm.position, MINIMAP_SIZE))
                } else {
                    None
                };
                let Some(pos) = pos else { continue };

                let relation = self
                    .player_relations
                    .get(entity_id)
                    .copied()
                    .unwrap_or(Relation::new(2));
                let is_friendly = relation.is_self() || relation.is_ally();

                // Check if this entity has an HP bar rendered
                let has_hp_bar = self.options.show_hp_bars
                    && controller
                        .entities_by_id()
                        .get(entity_id)
                        .and_then(|e| e.vehicle_ref())
                        .map(|v| {
                            let v = v.borrow();
                            v.props().max_health() > 0.0
                        })
                        .unwrap_or(false);

                let mut icon_keys = Vec::new();
                for active in consumables {
                    let still_active =
                        clock.seconds() < active.activated_at.seconds() + active.duration;
                    let past_start = clock.seconds() >= active.activated_at.seconds();
                    if still_active && past_start {
                        // Collect icon key
                        if let Some(icon_key) =
                            self.consumable_icon_key(*entity_id, active.consumable)
                        {
                            icon_keys.push(icon_key);
                        }

                        // Emit radius for detection consumables (radar, hydro, hydrophone)
                        if let Some(radius) =
                            self.get_consumable_radius(*entity_id, active.consumable)
                        {
                            // distShip from GameParams is a diameter, divide by 2 for radius
                            let px_radius = (radius / 2.0 / map_info.space_size as f32
                                * MINIMAP_SIZE as f32)
                                as i32;
                            let color = if is_friendly {
                                TEAM0_COLOR
                            } else {
                                TEAM1_COLOR
                            };
                            commands.push(DrawCommand::ConsumableRadius {
                                pos,
                                radius_px: px_radius,
                                color,
                                alpha: 0.15,
                            });
                        }
                    }
                }

                if !icon_keys.is_empty() {
                    commands.push(DrawCommand::ConsumableIcons {
                        pos,
                        icon_keys,
                        is_friendly,
                        has_hp_bar,
                    });
                }
            }
        }

        // 9. Kill feed
        if self.options.show_kill_feed {
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
        }

        // 10. Timer
        if self.options.show_timer {
            commands.push(DrawCommand::Timer {
                seconds: clock.seconds(),
            });
        }

        commands
    }
}

/// Get the capture point / building color based on team_id.
///
/// Team 0 = recording player's team (green), team 1 = enemy (red), -1 = neutral (white).
fn cap_point_color(team_id: i64) -> [u8; 3] {
    match team_id {
        0 => TEAM0_COLOR,
        1 => TEAM1_COLOR,
        _ => [255, 255, 255], // neutral
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

/// Get the health bar fill color based on health fraction.
fn hp_bar_color(fraction: f32) -> [u8; 3] {
    if fraction > 0.66 {
        HP_BAR_FULL_COLOR
    } else if fraction > 0.33 {
        HP_BAR_MID_COLOR
    } else {
        HP_BAR_LOW_COLOR
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

/// Map a Consumable enum to its base (default) PCY icon name.
///
/// Used as fallback when per-ship ability data is not available.
/// Returns None for consumables that don't have a meaningful icon display.
fn consumable_to_base_icon_key(c: Consumable) -> Option<String> {
    let key = match c {
        Consumable::DamageControl => "PCY001_CrashCrew",
        Consumable::RepairParty => "PCY002_RegenCrew",
        Consumable::DefensiveAntiAircraft => "PCY003_AirDefenseDisp",
        Consumable::CatapultFighter => "PCY004_Fighter",
        Consumable::SpottingAircraft => "PCY005_Spotter",
        Consumable::Smoke => "PCY006_SmokeGenerator",
        Consumable::SpeedBoost => "PCY007_SpeedBooster",
        Consumable::HydroacousticSearch => "PCY008_SonarSearch",
        Consumable::TorpedoReloadBooster => "PCY017_TorpedoReloader",
        Consumable::Radar => "PCY019_RLSSearch",
        Consumable::MainBatteryReloadBooster => "PCY021_ArtilleryBooster",
        Consumable::Hydrophone => "PCY045_Hydrophone",
        Consumable::EnhancedRudders => "PCY046_FastDeepRudders",
        Consumable::SubmarineSurveillance => "PCY048_SubmarineLocator",
        Consumable::ReserveBattery => return None,
        Consumable::Unknown(_) => return None,
    };
    Some(key.to_string())
}

/// Map a consumableType string from GameParams to our Consumable enum.
fn consumable_type_to_enum(consumable_type: &str) -> Option<Consumable> {
    match consumable_type {
        "crashCrew" => Some(Consumable::DamageControl),
        "scout" => Some(Consumable::SpottingAircraft),
        "airDefenseDisp" => Some(Consumable::DefensiveAntiAircraft),
        "speedBoosters" => Some(Consumable::SpeedBoost),
        "regenCrew" | "regenerateHealth" => Some(Consumable::RepairParty),
        "fighter" | "callFighters" => Some(Consumable::CatapultFighter),
        "artilleryBoosters" => Some(Consumable::MainBatteryReloadBooster),
        "torpedoReloader" => Some(Consumable::TorpedoReloadBooster),
        "smokeGenerator" => Some(Consumable::Smoke),
        "rls" => Some(Consumable::Radar),
        "sonar" => Some(Consumable::HydroacousticSearch),
        "hydrophone" => Some(Consumable::Hydrophone),
        "fastRudders" => Some(Consumable::EnhancedRudders),
        "subsEnergyFreeze" => Some(Consumable::ReserveBattery),
        "submarineLocator" => Some(Consumable::SubmarineSurveillance),
        _ => None,
    }
}

/// Map ability name from ship data to ConsumableKey for detection consumables.
///
/// Only maps abilities that have detection radii (radar, hydro, hydrophone).
fn ability_name_to_consumable_key(ability_name: &str) -> Option<ConsumableKey> {
    match ability_name {
        "PCY019_RLSSearchModifier" | "PCY046_RLSSearchDelayed" => Some(ConsumableKey::Radar),
        "PCY008_SonarSearchModifier" => Some(ConsumableKey::Hydro),
        "PCY045_HydrophoneModifier" => Some(ConsumableKey::Hydrophone),
        "PCY048_SubmarineLocatorModifier" => Some(ConsumableKey::SubSurveillance),
        _ => None,
    }
}
