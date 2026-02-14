use std::collections::HashMap;

use crate::Rc;
use crate::analyzer::decoder::FinishType;
use crate::types::{EntityId, GameClock, GameParamId, PlaneId};

use super::controller::{Entity, GameMessage, Player, SharedPlayer};
use super::state::{
    ActiveConsumable, ActivePlane, ActiveShot, ActiveTorpedo, ActiveWard, BuffZoneState,
    CapturePointState, CapturedBuff, DeadShip, KillRecord, MinimapPosition, ShipPosition,
    TeamScore,
};

/// Readonly view into BattleController state.
///
/// This trait hides the `G: ResourceLoader` generic on BattleController,
/// allowing callers to read state without being generic themselves.
pub trait BattleControllerState {
    /// Current replay clock time
    fn clock(&self) -> GameClock;

    /// Latest world-space position per ship entity
    fn ship_positions(&self) -> &HashMap<EntityId, ShipPosition>;

    /// Latest minimap position per entity
    fn minimap_positions(&self) -> &HashMap<EntityId, MinimapPosition>;

    /// Players parsed from arena state (entity_id -> Player)
    fn player_entities(&self) -> &HashMap<EntityId, Rc<Player>>;

    /// Players parsed from replay metadata
    fn metadata_players(&self) -> &[SharedPlayer];

    /// All tracked entities (vehicles, buildings, smoke screens)
    fn entities_by_id(&self) -> &HashMap<EntityId, Entity>;

    /// Current capture point states
    fn capture_points(&self) -> &[CapturePointState];

    /// Current buff zone states (arms race powerup zones)
    fn buff_zones(&self) -> &HashMap<EntityId, BuffZoneState>;

    /// Buffs captured so far (arms race)
    fn captured_buffs(&self) -> &[CapturedBuff];

    /// Current team scores
    fn team_scores(&self) -> &[TeamScore];

    /// Chat messages received so far
    fn game_chat(&self) -> &[GameMessage];

    /// Active consumables per entity
    fn active_consumables(&self) -> &HashMap<EntityId, Vec<ActiveConsumable>>;

    /// Active artillery salvos in flight
    fn active_shots(&self) -> &[ActiveShot];

    /// Active torpedoes in the water
    fn active_torpedoes(&self) -> &[ActiveTorpedo];

    /// Active plane squadrons on the minimap
    fn active_planes(&self) -> &HashMap<PlaneId, ActivePlane>;

    /// Active fighter patrol wards (stationary patrol circles)
    fn active_wards(&self) -> &HashMap<PlaneId, ActiveWard>;

    /// All ship kills that have occurred
    fn kills(&self) -> &[KillRecord];

    /// Dead ships and their last known positions
    fn dead_ships(&self) -> &HashMap<EntityId, DeadShip>;

    /// Clock time when the battle ended, if it has ended
    fn battle_end_clock(&self) -> Option<GameClock>;

    /// Which team won the match (0 or 1), or negative for draw. None if match hasn't ended.
    fn winning_team(&self) -> Option<i8>;

    /// How the battle ended (extermination, score, timeout, etc.). None if not yet decided.
    fn finish_type(&self) -> Option<&FinishType>;

    /// Main battery turret yaws per entity (group 0 only).
    /// Each entry maps entity_id -> vec of turret yaws in radians (relative to ship heading).
    fn turret_yaws(&self) -> &HashMap<EntityId, Vec<f32>>;

    /// World-space gun aim yaw per entity, decoded from `targetLocalPos` EntityProperty.
    /// Updated frequently (~6000 times per match). Values are radians in [-PI, PI].
    fn target_yaws(&self) -> &HashMap<EntityId, f32>;

    /// Currently selected ammo per entity. Maps entity_id -> ammo_param_id.
    /// Only tracked for artillery (weapon_type 0).
    fn selected_ammo(&self) -> &HashMap<EntityId, GameParamId>;
}
