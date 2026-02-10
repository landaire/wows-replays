use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

/// Per-replay-session entity identifier for game objects (ships, buildings, smoke screens).
/// The wire format is u32 but some packet types use i32 or i64.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct EntityId(pub u32);

impl EntityId {
    pub fn raw(self) -> u32 {
        self.0
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u32> for EntityId {
    fn from(v: u32) -> Self {
        EntityId(v)
    }
}

impl From<i32> for EntityId {
    fn from(v: i32) -> Self {
        EntityId(v as u32)
    }
}

impl From<i64> for EntityId {
    fn from(v: i64) -> Self {
        EntityId(v as u32)
    }
}

/// A persistent player account identifier (db_id, avatar_id).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountId(pub u64);

impl AccountId {
    pub fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u32> for AccountId {
    fn from(v: u32) -> Self {
        AccountId(v as u64)
    }
}

impl From<i32> for AccountId {
    fn from(v: i32) -> Self {
        AccountId(v as u64)
    }
}

impl From<i64> for AccountId {
    fn from(v: i64) -> Self {
        AccountId(v as u64)
    }
}

/// A game parameter type identifier from GameParams (ships, equipment, etc.).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GameParamId(pub u32);

impl GameParamId {
    pub fn raw(self) -> u32 {
        self.0
    }
}

impl fmt::Display for GameParamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u32> for GameParamId {
    fn from(v: u32) -> Self {
        GameParamId(v)
    }
}

impl From<u64> for GameParamId {
    fn from(v: u64) -> Self {
        GameParamId(v as u32)
    }
}

impl From<i64> for GameParamId {
    fn from(v: i64) -> Self {
        GameParamId(v as u32)
    }
}

/// Represents the relation of a player/entity to the recording player.
/// - 0 = self (the player who recorded the replay)
/// - 1 = teammate (ally)
/// - 2+ = enemy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Relation(u32);

impl Relation {
    /// Creates a new Relation from a raw value.
    pub fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns true if this is the recording player (relation == 0).
    pub fn is_self(&self) -> bool {
        self.0 == 0
    }

    /// Returns true if this player is a teammate (relation == 1).
    pub fn is_ally(&self) -> bool {
        self.0 == 1
    }

    /// Returns true if this player is an enemy (relation >= 2).
    pub fn is_enemy(&self) -> bool {
        self.0 >= 2
    }

    /// Returns the raw relation value.
    pub fn value(&self) -> u32 {
        self.0
    }
}

impl From<u32> for Relation {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

/// Packed minimap squadron identifier.
/// Encodes `(avatar_id: u32, index: u3, purpose: u3, departures: u1)` in the low 39 bits.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct PlaneId(pub u64);

impl PlaneId {
    /// Extracts the owner's entity ID (avatar_id) from the low 32 bits.
    pub fn owner_id(self) -> EntityId {
        EntityId((self.0 & 0xFFFF_FFFF) as u32)
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for PlaneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for PlaneId {
    fn from(v: u64) -> Self {
        PlaneId(v)
    }
}

impl From<i64> for PlaneId {
    fn from(v: i64) -> Self {
        PlaneId(v as u64)
    }
}

/// World-space position in BigWorld coordinates.
/// X = east/west, Y = up/down (altitude), Z = north/south. Origin at map center.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct WorldPos {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl WorldPos {
    pub fn lerp(self, other: WorldPos, t: f32) -> WorldPos {
        self + (other - self) * t
    }
}

impl std::ops::Add for WorldPos {
    type Output = WorldPos;
    fn add(self, rhs: WorldPos) -> WorldPos {
        WorldPos {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
        }
    }
}

impl std::ops::Sub for WorldPos {
    type Output = WorldPos;
    fn sub(self, rhs: WorldPos) -> WorldPos {
        WorldPos {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
            z: self.z - rhs.z,
        }
    }
}

impl std::ops::Mul<f32> for WorldPos {
    type Output = WorldPos;
    fn mul(self, rhs: f32) -> WorldPos {
        WorldPos {
            x: self.x * rhs,
            y: self.y * rhs,
            z: self.z * rhs,
        }
    }
}

/// Normalized minimap position from MinimapUpdate packets.
/// Values roughly in [-0.5, 1.5] range (centered around [0,1]).
/// X: 0 = left edge, 1 = right edge.
/// Y: 0 = bottom edge, 1 = top edge.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct NormalizedPos {
    pub x: f32,
    pub y: f32,
}

/// A game clock value in seconds since the replay started recording.
/// Note: there is typically a ~30s pre-game countdown, so game_time = clock - 30.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct GameClock(pub f32);

impl GameClock {
    pub fn seconds(self) -> f32 {
        self.0
    }

    pub fn to_duration(self) -> Duration {
        Duration::from_secs_f32(self.0)
    }

    /// Returns the game time (after countdown), clamped to 0.
    pub fn game_time(self) -> f32 {
        (self.0 - 30.0).max(0.0)
    }
}

impl fmt::Display for GameClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.1}s", self.0)
    }
}

impl std::ops::Add<f32> for GameClock {
    type Output = GameClock;
    fn add(self, rhs: f32) -> GameClock {
        GameClock(self.0 + rhs)
    }
}

impl std::ops::Add<Duration> for GameClock {
    type Output = GameClock;
    fn add(self, rhs: Duration) -> GameClock {
        GameClock(self.0 + rhs.as_secs_f32())
    }
}

impl std::ops::Sub for GameClock {
    type Output = f32;
    fn sub(self, rhs: GameClock) -> f32 {
        self.0 - rhs.0
    }
}

impl std::ops::Sub<Duration> for GameClock {
    type Output = GameClock;
    fn sub(self, rhs: Duration) -> GameClock {
        GameClock(self.0 - rhs.as_secs_f32())
    }
}
