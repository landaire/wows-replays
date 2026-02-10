use std::collections::HashMap;

use serde::Serialize;

use crate::analyzer::decoder::{ArtillerySalvo, Consumable, DeathCause, TorpedoData};
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
    pub last_updated: GameClock,
}

/// Current state of a capture point.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CapturePointState {
    pub index: usize,
    pub team_id: i64,
    pub invader_team: i64,
    /// (fraction captured 0..1, time remaining)
    pub progress: (f64, f64),
    pub has_invaders: bool,
    pub both_inside: bool,
}

/// Current score for a team.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TeamScore {
    pub team_index: usize,
    pub score: i64,
}

/// An active consumable on a ship.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveConsumable {
    pub consumable: Consumable,
    pub activated_at: GameClock,
    pub duration: f32,
}

/// A building/structure entity in the game.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BuildingEntity {
    pub id: EntityId,
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
    pub x: f32,
    pub y: f32,
    pub last_updated: GameClock,
}

/// A ship kill event.
#[derive(Debug, Clone, Serialize)]
pub struct KillRecord {
    pub clock: GameClock,
    pub killer: EntityId,
    pub victim: EntityId,
    pub cause: DeathCause,
}

/// A dead ship's last known position.
#[derive(Debug, Clone, Serialize)]
pub struct DeadShip {
    pub clock: GameClock,
    pub position: WorldPos,
}
