use serde::Serialize;

use super::controller::ChatChannel;
use crate::analyzer::decoder::{ArtillerySalvo, Consumable, DeathCause, Ribbon, TorpedoData};

pub use crate::packet2::GameClock;

/// A timestamped event in the battle timeline.
#[derive(Debug, Clone, Serialize)]
pub struct TimestampedEvent {
    pub clock: GameClock,
    pub event: TimelineEvent,
}

/// All discrete events that can be recorded in the timeline.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum TimelineEvent {
    /// Ship position from a Position packet (world coordinates)
    ShipPosition {
        entity_id: u32,
        x: f32,
        y: f32,
        z: f32,
        yaw: f32,
        pitch: f32,
        roll: f32,
    },

    /// Minimap vision update (normalized map coordinates)
    MinimapVisionUpdate {
        entity_id: u32,
        /// 0..1 range, left to right
        x: f32,
        /// 0..1 range, bottom to top
        y: f32,
        /// Heading in degrees, 0 = up, positive = clockwise
        heading: f32,
        /// True if the entity is disappearing from the minimap
        disappearing: bool,
    },

    /// A ship was destroyed
    ShipDestroyed {
        killer: u32,
        victim: u32,
        cause: DeathCause,
    },

    /// Damage was dealt from one entity to another
    DamageDealt {
        aggressor_id: u32,
        victim_id: u32,
        damage: f32,
    },

    /// A consumable was activated
    ConsumableActivated {
        entity_id: u32,
        consumable: Consumable,
        duration: f32,
    },

    /// Capture point state changed
    CapturePointUpdate {
        point_index: usize,
        team_id: Option<i64>,
        invader_team: Option<i64>,
        progress: Option<(f64, f64)>,
        has_invaders: Option<bool>,
        both_inside: Option<bool>,
    },

    /// Team score changed
    TeamScoreUpdate { team_index: usize, score: i64 },

    /// A smoke screen was created
    SmokeScreenCreated { entity_id: u32, radius: f32 },

    /// A smoke screen was destroyed
    SmokeScreenDestroyed { entity_id: u32 },

    /// A building's state changed
    BuildingStateChanged {
        entity_id: u32,
        is_alive: bool,
        is_suppressed: bool,
        team_id: i8,
    },

    /// A ribbon was earned
    Ribbon(Ribbon),

    /// A chat message was sent
    ChatMessage {
        entity_id: u32,
        sender_name: String,
        channel: ChatChannel,
        message: String,
    },

    /// The battle ended
    BattleEnd { winning_team: Option<i8> },

    /// Artillery shells were fired
    ArtilleryShots {
        entity_id: u32,
        salvos: Vec<ArtillerySalvo>,
    },

    /// Torpedoes were launched
    TorpedoesLaunched {
        entity_id: u32,
        torpedoes: Vec<TorpedoData>,
    },

    /// A plane/squadron position was updated on the minimap
    PlanePosition {
        entity_id: u32,
        squadron_id: u64,
        x: f32,
        y: f32,
    },
}

/// Append-only timeline of battle events. Events are pushed in packet order,
/// which is monotonically increasing by clock time.
#[derive(Debug, Default, Serialize)]
pub struct GameTimeline {
    events: Vec<TimestampedEvent>,
}

impl GameTimeline {
    pub fn new() -> Self {
        Self {
            events: Vec::with_capacity(50_000),
        }
    }

    pub fn push(&mut self, clock: GameClock, event: TimelineEvent) {
        self.events.push(TimestampedEvent { clock, event });
    }

    pub fn events(&self) -> &[TimestampedEvent] {
        &self.events
    }

    /// Returns events within a time window [start, end) in raw clock seconds.
    pub fn events_in_range(&self, start: GameClock, end: GameClock) -> &[TimestampedEvent] {
        let start_idx = self.events.partition_point(|e| e.clock < start);
        let end_idx = self.events.partition_point(|e| e.clock < end);
        &self.events[start_idx..end_idx]
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}
