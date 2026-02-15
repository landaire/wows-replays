use serde::Serialize;
use wowsunpack::game_params::types::BigWorldDistance;

use crate::analyzer::decoder::{ArtillerySalvo, Consumable, DeathCause, Recognized, TorpedoData};
use crate::types::{EntityId, GameClock, GameParamId, NormalizedPos, PlaneId, WorldPos};

/// Last known world-space position of a ship entity.
#[derive(Debug, Clone, Serialize)]
pub struct ShipPosition {
    pub entity_id: EntityId,
    pub position: WorldPos,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub last_updated: GameClock,
}

/// Last known minimap position of an entity (normalized coordinates).
#[derive(Debug, Clone, Serialize)]
pub struct MinimapPosition {
    pub entity_id: EntityId,
    /// Normalized minimap position
    pub position: NormalizedPos,
    /// Heading in degrees
    pub heading: f32,
    pub visible: bool,
    /// Bitmask of detection reasons (radar, hydro, etc.). Non-zero means the
    /// ship is detected through special means. Sourced from the Vehicle entity's
    /// `visibilityFlags` property.
    pub visibility_flags: u32,
    /// True when the ship is invisible (e.g. submarine submerged). Sourced from
    /// the Vehicle entity's `isInvisible` property.
    pub is_invisible: bool,
    pub last_updated: GameClock,
}

/// Current state of a capture point.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CapturePointState {
    pub index: usize,
    /// World position of the zone center (from InteractiveZone entity)
    pub position: Option<WorldPos>,
    /// Zone radius in world units (from InteractiveZone entity)
    pub radius: f32,
    /// Control point type: 5=flag, others=lettered (A, B, C...)
    pub control_point_type: i32,
    pub team_id: i64,
    pub invader_team: i64,
    /// (fraction captured 0..1, time remaining)
    pub progress: (f64, f64),
    pub has_invaders: bool,
    pub both_inside: bool,
    /// Whether this capture point is enabled (arms race: starts disabled, enabled mid-game)
    pub is_enabled: bool,
}

/// State of a buff zone (arms race powerup drop).
///
/// InteractiveZone entities with `controlPoint: null` in `componentsState`.
/// These appear in waves during arms race, can be captured by either team,
/// and disappear (EntityLeave) once consumed.
#[derive(Debug, Clone, Serialize)]
pub struct BuffZoneState {
    pub entity_id: EntityId,
    /// World position of the zone center
    pub position: WorldPos,
    /// Zone radius in world units
    pub radius: f32,
    pub team_id: i64,
    /// Whether this zone is currently active and visible
    pub is_active: bool,
    /// GameParam ID of the associated Drop (powerup type)
    pub drop_params_id: Option<GameParamId>,
}

/// A buff that has been captured by a team.
#[derive(Debug, Clone, Serialize)]
pub struct CapturedBuff {
    /// GameParam ID of the Drop
    pub params_id: GameParamId,
    /// Team that captured it (entity_id of owner → team_id)
    pub team_id: i64,
    /// Game clock when captured
    pub clock: GameClock,
}

/// Current score for a team.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TeamScore {
    pub team_index: usize,
    pub score: i64,
}

/// Scoring rules extracted from BattleLogic state.missions.
#[derive(Debug, Clone, Serialize)]
pub struct ScoringRules {
    /// Score required to win (typically 1000)
    pub team_win_score: i64,
    /// Points awarded per owned cap per tick
    pub hold_reward: i64,
    /// Seconds between cap tick scoring
    pub hold_period: f32,
    /// Which capture point indices participate in hold scoring
    pub hold_cp_indices: Vec<usize>,
}

/// An active consumable on a ship.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveConsumable {
    pub consumable: Recognized<Consumable>,
    pub activated_at: GameClock,
    pub duration: f32,
}

/// A building/structure entity in the game.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BuildingEntity {
    pub id: EntityId,
    pub position: WorldPos,
    pub is_alive: bool,
    pub is_hidden: bool,
    pub is_suppressed: bool,
    pub team_id: i8,
    pub params_id: GameParamId,
}

/// A smoke screen entity in the game.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SmokeScreenEntity {
    pub id: EntityId,
    pub radius: f32,
    /// World position where the smoke was created
    pub position: WorldPos,
    /// Current active smoke puff positions (mutated via SetRange/RemoveRange)
    pub points: Vec<WorldPos>,
}

/// An active artillery salvo in flight.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveShot {
    pub entity_id: EntityId,
    pub salvo: ArtillerySalvo,
    pub fired_at: GameClock,
}

/// An active torpedo in the water.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveTorpedo {
    pub entity_id: EntityId,
    pub torpedo: TorpedoData,
    pub launched_at: GameClock,
}

/// An active plane squadron on the minimap.
#[derive(Debug, Clone, Serialize)]
pub struct ActivePlane {
    pub plane_id: PlaneId,
    pub owner_id: EntityId,
    pub team_id: u32,
    pub params_id: GameParamId,
    /// Current position (world coordinates), updated by minimap updates.
    pub position: WorldPos,
    pub last_updated: GameClock,
}

/// A fighter patrol ward — a stationary circle where fighters patrol.
/// Created by `receive_wardAdded`, removed by `receive_wardRemoved`.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveWard {
    pub plane_id: PlaneId,
    /// Patrol center position (world coordinates)
    pub position: WorldPos,
    /// Patrol radius in BigWorld units
    pub radius: BigWorldDistance,
    /// Owner ship entity ID
    pub owner_id: EntityId,
}

/// A ship kill event.
#[derive(Debug, Clone, Serialize)]
pub struct KillRecord {
    pub clock: GameClock,
    pub killer: EntityId,
    pub victim: EntityId,
    pub cause: Recognized<DeathCause>,
}

/// A dead ship's last known position.
#[derive(Debug, Clone, Serialize)]
pub struct DeadShip {
    pub clock: GameClock,
    pub position: WorldPos,
}
