use std::{
    borrow::Borrow,
    cell::{Cell, Ref, RefCell, UnsafeCell},
    collections::HashMap,
    str::FromStr,
    time::Duration,
};

use nom::{multi::count, number::complete::le_u32, sequence::pair};
use serde::{Deserialize, Serialize};
use strum_macros::EnumString;
use tracing::{Level, debug, span, trace, warn};
use variantly::Variantly;
use wowsunpack::{
    data::{ResourceLoader, Version},
    game_params::types::{CrewSkill, Param, ParamType, Species},
    rpc::typedefs::ArgValue,
};

static TIME_UNTIL_GAME_START: Duration = Duration::from_secs(30);

use crate::{
    IResult, Rc, ReplayMeta,
    analyzer::{
        analyzer::AnalyzerMut,
        decoder::{ChatMessageExtra, DeathCause, DecodedPacket, PlayerStateData},
    },
    nested_property_path::{PropertyNestLevel, UpdateAction},
    packet2::{EntityCreatePacket, Packet, PacketProcessorMut, PacketType},
    types::{AccountId, EntityId, GameClock, GameParamId, NormalizedPos, Relation, WorldPos},
};

use super::state::{
    ActiveConsumable, BuildingEntity, CapturePointState, MinimapPosition, ShipPosition,
    SmokeScreenEntity, TeamScore,
};
use super::timeline::{GameTimeline, TimelineEvent};

#[derive(Debug, Default, Clone, Serialize)]
pub struct ShipConfig {
    abilities: Vec<u32>,
    hull: u32,
    modernization: Vec<u32>,
    units: Vec<u32>,
    signals: Vec<u32>,
}

impl ShipConfig {
    pub fn signals(&self) -> &[u32] {
        self.signals.as_ref()
    }

    pub fn units(&self) -> &[u32] {
        self.units.as_ref()
    }

    pub fn modernization(&self) -> &[u32] {
        self.modernization.as_ref()
    }

    pub fn hull(&self) -> u32 {
        self.hull
    }

    pub fn abilities(&self) -> &[u32] {
        self.abilities.as_ref()
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Skills {
    aircraft_carrier: Vec<u8>,
    battleship: Vec<u8>,
    cruiser: Vec<u8>,
    destroyer: Vec<u8>,
    auxiliary: Vec<u8>,
    submarine: Vec<u8>,
}

impl Skills {
    pub fn submarine(&self) -> &[u8] {
        self.submarine.as_ref()
    }

    pub fn auxiliary(&self) -> &[u8] {
        self.auxiliary.as_ref()
    }

    pub fn destroyer(&self) -> &[u8] {
        self.destroyer.as_ref()
    }

    pub fn cruiser(&self) -> &[u8] {
        self.cruiser.as_ref()
    }

    pub fn battleship(&self) -> &[u8] {
        self.battleship.as_ref()
    }

    pub fn aircraft_carrier(&self) -> &[u8] {
        self.aircraft_carrier.as_ref()
    }
}

#[derive(Debug, Default, Serialize)]
pub struct ShipLoadout {
    config: Option<ShipConfig>,
    skills: Option<Skills>,
}

impl ShipLoadout {
    pub fn skills(&self) -> Option<&Skills> {
        self.skills.as_ref()
    }

    pub fn config(&self) -> Option<&ShipConfig> {
        self.config.as_ref()
    }
}

#[derive(Debug, Serialize, Deserialize, Copy, Clone, PartialEq, Eq)]
pub enum ConnectionChangeKind {
    Connected,
    Disconnected,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConnectionChangeInfo {
    /// Duration from start of arena when the connection change
    /// event occurred
    at_game_duration: Duration,
    event_kind: ConnectionChangeKind,
    /// Whether or not this player had a death event when this connection change
    /// occurred
    had_death_event: bool,
}

impl ConnectionChangeInfo {
    pub fn at_game_duration(&self) -> Duration {
        self.at_game_duration
    }

    pub fn event_kind(&self) -> ConnectionChangeKind {
        self.event_kind
    }

    pub fn had_death_event(&self) -> bool {
        self.had_death_event
    }
}

/// Players that were received from parsing the replay packets
pub struct Player {
    initial_state: PlayerStateData,
    end_state: UnsafeCell<PlayerStateData>,
    connection_change_info: UnsafeCell<Vec<ConnectionChangeInfo>>,
    vehicle: Rc<Param>,
    vehicle_entity: Option<VehicleEntity>,
    /// The relation of this player to the recording player
    relation: Relation,
}

/// SAFETY: `UnsafeCell` fields are never mutated after BattleController
/// builds.
#[cfg(feature = "arc")]
unsafe impl Sync for Player {}

impl std::fmt::Debug for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Player")
            .field("initial_state", &self.initial_state)
            .field("end_state", self.end_state())
            .field("connection_change_info", unsafe {
                &*self.connection_change_info.get()
            })
            .field("vehicle", &self.vehicle)
            .field("vehicle_entity", &self.vehicle_entity)
            .field("relation", &self.relation)
            .finish()
    }
}

impl Clone for Player {
    fn clone(&self) -> Self {
        Self {
            initial_state: self.initial_state.clone(),
            end_state: UnsafeCell::new(unsafe { (*self.end_state.get()).clone() }),
            connection_change_info: UnsafeCell::new(unsafe {
                (*self.connection_change_info.get()).clone()
            }),
            vehicle: self.vehicle.clone(),
            vehicle_entity: self.vehicle_entity.clone(),
            relation: self.relation,
        }
    }
}

impl Serialize for Player {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Player", 5)?;
        state.serialize_field("initial_state", &self.initial_state)?;
        state.serialize_field("end_state", self.end_state())?;
        state.serialize_field("connection_change_info", self.connection_change_info())?;
        state.serialize_field("vehicle", &self.vehicle)?;
        state.serialize_field("relation", &self.relation)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for Player {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct PlayerHelper {
            initial_state: PlayerStateData,
            end_state: PlayerStateData,
            connection_change_info: Vec<ConnectionChangeInfo>,
            vehicle: Rc<Param>,
            relation: Relation,
        }

        let helper = PlayerHelper::deserialize(deserializer)?;
        Ok(Player {
            initial_state: helper.initial_state,
            end_state: UnsafeCell::new(helper.end_state),
            connection_change_info: UnsafeCell::new(helper.connection_change_info),
            vehicle: helper.vehicle,
            vehicle_entity: None,
            relation: helper.relation,
        })
    }
}

impl std::cmp::PartialEq for Player {
    fn eq(&self, other: &Self) -> bool {
        self.initial_state.db_id == other.initial_state.db_id
            && self.initial_state.realm == other.initial_state.realm
    }
}

impl std::cmp::Eq for Player {}

impl std::hash::Hash for Player {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.initial_state.db_id.hash(state);
        self.initial_state.realm.hash(state);
    }
}

impl Player {
    fn from_arena_player<G: ResourceLoader>(
        player: &PlayerStateData,
        metadata_player: &MetadataPlayer,
        resources: &G,
    ) -> Player {
        Player {
            initial_state: player.clone(),
            end_state: UnsafeCell::new(player.clone()),
            vehicle_entity: None,
            connection_change_info: UnsafeCell::new(Vec::new()),
            vehicle: resources
                .game_param_by_id(metadata_player.vehicle().id())
                .expect("could not find player vehicle"),
            relation: metadata_player.relation(),
        }
    }

    /// A list of events for when this player connected or disconnected
    /// from the match.
    pub fn connection_change_info(&self) -> &[ConnectionChangeInfo] {
        unsafe { (&*self.connection_change_info.get()).as_slice() }
    }

    fn connection_change_info_mut(&self) -> &mut Vec<ConnectionChangeInfo> {
        unsafe { &mut *self.connection_change_info.get() }
    }

    pub fn end_state(&self) -> &PlayerStateData {
        // SAFETY: `end_state` is never mutated after the battle
        // controller is constructed
        unsafe { &*self.end_state.get() }
    }

    fn end_state_mut(&self) -> &mut PlayerStateData {
        // SAFETY: `end_state` is never mutated after the battle
        // controller is constructed
        unsafe { &mut *self.end_state.get() }
    }

    pub fn initial_state(&self) -> &PlayerStateData {
        &self.initial_state
    }

    pub fn relation(&self) -> Relation {
        self.relation
    }

    pub fn vehicle_entity(&self) -> Option<&VehicleEntity> {
        self.vehicle_entity.as_ref()
    }

    pub fn vehicle(&self) -> &Param {
        &self.vehicle
    }
}

#[derive(Debug)]
/// Players that were parsed from just the replay metadata
pub struct MetadataPlayer {
    id: AccountId,
    name: String,
    relation: Relation,
    vehicle: Rc<Param>,
}

impl MetadataPlayer {
    pub fn name(&self) -> &str {
        self.name.as_ref()
    }

    pub fn relation(&self) -> Relation {
        self.relation
    }

    pub fn vehicle(&self) -> &Param {
        self.vehicle.as_ref()
    }

    pub fn id(&self) -> AccountId {
        self.id
    }
}

pub type SharedPlayer = Rc<MetadataPlayer>;
type MethodName = String;

pub trait EventHandler {
    fn on_chat_message(&self, message: GameMessage) {}
    fn on_aren_state_received(&self, entity_id: EntityId) {}
}

pub enum xEntityType {
    Client = 1,
    Cell = 2,
    Base = 4,
}

#[derive(Debug, Clone, Copy, EnumString)]
pub enum EntityType {
    Building,
    BattleEntity,
    BattleLogic,
    Vehicle,
    InteractiveZone,
    SmokeScreen,
}

#[derive(Copy, Clone, Serialize)]
#[serde(tag = "type")]
pub enum BattleResult {
    /// A win, and which team won (inferred to be the team of the player)
    Win(i8),
    /// A loss, and which other team won
    Loss(i8),
    Draw,
}

#[derive(Serialize)]
pub struct BattleReport {
    arena_id: i64,
    self_player: Rc<Player>,
    version: Version,
    map_name: String,
    game_mode: String,
    game_type: String,
    match_group: String,
    players: Vec<Rc<Player>>,
    game_chat: Vec<GameMessage>,
    battle_results: Option<String>,
    frags: HashMap<Rc<Player>, Vec<DeathInfo>>,
    match_result: Option<BattleResult>,
    timeline: GameTimeline,
    capture_points: Vec<CapturePointState>,
    team_scores: Vec<TeamScore>,
    buildings: Vec<BuildingEntity>,
}

impl BattleReport {
    pub fn self_player(&self) -> &Rc<Player> {
        &self.self_player
    }

    pub fn game_chat(&self) -> &[GameMessage] {
        self.game_chat.as_ref()
    }

    pub fn match_group(&self) -> &str {
        self.match_group.as_ref()
    }

    pub fn map_name(&self) -> &str {
        self.map_name.as_ref()
    }

    pub fn version(&self) -> Version {
        self.version
    }

    pub fn game_mode(&self) -> &str {
        self.game_mode.as_ref()
    }

    pub fn game_type(&self) -> &str {
        self.game_type.as_ref()
    }

    pub fn battle_results(&self) -> Option<&str> {
        self.battle_results.as_deref()
    }

    pub fn players(&self) -> &[Rc<Player>] {
        &self.players
    }

    pub fn arena_id(&self) -> i64 {
        self.arena_id
    }

    /// Returns a map of players and their frags.
    pub fn frags(&self) -> &HashMap<Rc<Player>, Vec<DeathInfo>> {
        &self.frags
    }

    /// The result of the battle. This may be `None` if the player left the match before it finished.
    pub fn battle_result(&self) -> Option<&BattleResult> {
        self.match_result.as_ref()
    }

    pub fn timeline(&self) -> &GameTimeline {
        &self.timeline
    }

    pub fn capture_points(&self) -> &[CapturePointState] {
        &self.capture_points
    }

    pub fn team_scores(&self) -> &[TeamScore] {
        &self.team_scores
    }

    pub fn buildings(&self) -> &[BuildingEntity] {
        &self.buildings
    }
}

struct DamageEvent {
    amount: f32,
    victim: EntityId,
}

pub struct BattleController<'res, 'replay, G> {
    game_meta: &'replay ReplayMeta,
    game_resources: &'res G,
    metadata_players: Vec<SharedPlayer>,
    player_entities: HashMap<EntityId, Rc<Player>>,
    entities_by_id: HashMap<EntityId, Entity>,
    method_callbacks: HashMap<(ParamType, String), fn(&PacketType<'_, '_>)>,
    property_callbacks: HashMap<(ParamType, String), fn(&ArgValue<'_>)>,
    damage_dealt: HashMap<EntityId, Vec<DamageEvent>>,
    frags: HashMap<EntityId, Vec<Death>>,
    event_handler: Option<Rc<dyn EventHandler>>,
    game_chat: Vec<GameMessage>,
    version: Version,
    battle_results: Option<String>,
    match_finished: bool,
    winning_team: Option<i8>,
    arena_id: i64,

    // Timeline and minimap state
    timeline: GameTimeline,
    ship_positions: HashMap<EntityId, ShipPosition>,
    minimap_positions: HashMap<EntityId, MinimapPosition>,
    capture_points: Vec<CapturePointState>,
    team_scores: Vec<TeamScore>,
    active_consumables: HashMap<EntityId, Vec<ActiveConsumable>>,
}

impl<'res, 'replay, G> BattleController<'res, 'replay, G>
where
    G: ResourceLoader,
{
    pub fn new(game_meta: &'replay ReplayMeta, game_resources: &'res G) -> Self {
        let players: Vec<SharedPlayer> = game_meta
            .vehicles
            .iter()
            .map(|vehicle| {
                Rc::new(MetadataPlayer {
                    id: vehicle.id,
                    name: vehicle.name.clone(),
                    relation: Relation::new(vehicle.relation),
                    vehicle: game_resources
                        .game_param_by_id(vehicle.shipId.raw())
                        .expect("could not find vehicle"),
                })
            })
            .collect();

        Self {
            game_meta,
            game_resources,
            metadata_players: players,
            player_entities: HashMap::default(),
            entities_by_id: Default::default(),
            method_callbacks: Default::default(),
            property_callbacks: Default::default(),
            event_handler: None,
            game_chat: Default::default(),
            version: Version::from_client_exe(&game_meta.clientVersionFromExe),
            damage_dealt: Default::default(),
            frags: Default::default(),
            battle_results: Default::default(),
            match_finished: false,
            winning_team: None,
            arena_id: 0,
            timeline: GameTimeline::new(),
            ship_positions: HashMap::default(),
            minimap_positions: HashMap::default(),
            capture_points: Vec::new(),
            team_scores: Vec::new(),
            active_consumables: HashMap::default(),
        }
    }

    pub fn set_event_handler(&mut self, event_handler: Rc<dyn EventHandler>) {
        self.event_handler = Some(event_handler);
    }

    pub fn players(&self) -> &[SharedPlayer] {
        self.metadata_players.as_ref()
    }

    pub fn game_mode(&self) -> String {
        let id = format!("IDS_SCENARIO_{}", self.game_meta.scenario.to_uppercase());
        self.game_resources
            .localized_name_from_id(&id)
            .unwrap_or_else(|| self.game_meta.scenario.clone())
    }

    pub fn map_name(&self) -> String {
        let id = format!("IDS_{}", self.game_meta.mapName.to_uppercase());
        self.game_resources
            .localized_name_from_id(&id)
            .unwrap_or_else(|| self.game_meta.mapName.clone())
    }

    pub fn player_name(&self) -> &str {
        self.game_meta.playerName.as_ref()
    }

    pub fn match_group(&self) -> &str {
        self.game_meta.matchGroup.as_ref()
    }

    pub fn game_version(&self) -> &str {
        self.game_meta.clientVersionFromExe.as_ref()
    }

    pub fn game_type(&self) -> String {
        let id = format!("IDS_{}", self.game_meta.gameType.to_ascii_uppercase());
        self.game_resources
            .localized_name_from_id(&id)
            .unwrap_or_else(|| self.game_meta.gameType.clone())
    }

    fn handle_chat_message<'packet>(
        &mut self,
        entity_id: EntityId,
        sender_id: AccountId,
        audience: &str,
        message: &str,
        extra_data: Option<ChatMessageExtra>,
        clock: GameClock,
    ) {
        // System messages
        if sender_id.raw() == 0 {
            return;
        }

        let channel = match audience {
            "battle_common" => ChatChannel::Global,
            "battle_team" => ChatChannel::Team,
            "battle_prebattle" => ChatChannel::Division,
            other => panic!("unknown channel {}", other),
        };

        let mut sender_team = None;
        let mut sender_name = "Unknown".to_owned();
        let mut player = None;
        for meta_vehicle in &self.game_meta.vehicles {
            if meta_vehicle.id == sender_id {
                sender_name = meta_vehicle.name.clone();
                sender_team = Some(Relation::new(meta_vehicle.relation));
                player = self
                    .player_entities
                    .values()
                    .find(|player| player.initial_state.meta_ship_id() == sender_id)
                    .cloned();
            }
        }

        debug!("chat message from sender {sender_name} in channel {channel:?}: {message}");

        self.timeline.push(
            clock,
            TimelineEvent::ChatMessage {
                entity_id,
                sender_name: sender_name.clone(),
                channel: channel.clone(),
                message: message.to_string(),
            },
        );

        let message = GameMessage {
            clock,
            sender_relation: sender_team,
            sender_name,
            channel,
            message: message.to_string(),
            entity_id,
            player,
        };

        self.game_chat.push(message.clone());
        debug!(
            "{:p} game chat len: {}",
            &self.game_chat,
            self.game_chat.len()
        );

        if let Some(event_handler) = self.event_handler.as_ref() {
            event_handler.on_chat_message(message);
        }
    }

    fn handle_entity_create<'packet>(
        &mut self,
        clock: GameClock,
        packet: &EntityCreatePacket<'packet>,
    ) {
        self.handle_entity_create_with_clock(clock, packet);
    }

    pub fn game_chat(&self) -> &[GameMessage] {
        self.game_chat.as_slice()
    }

    pub fn build_report(mut self) -> BattleReport {
        // Update vehicle damage from damage events
        for (aggressor, damage_events) in &self.damage_dealt {
            if let Some(aggressor_entity) = self.entities_by_id.get_mut(aggressor) {
                let vehicle = aggressor_entity
                    .vehicle_ref()
                    .expect("aggressor has no vehicle?");

                let mut vehicle = vehicle.borrow_mut();
                vehicle.damage += damage_events.iter().fold(0.0, |mut accum, event| {
                    accum += event.amount;
                    accum
                });
            }
        }

        // Update vehicle death info
        self.entities_by_id.values().for_each(|entity| {
            if let Some(vehicle) = entity.vehicle_ref() {
                let mut vehicle = vehicle.borrow_mut();

                if let Some(death) = self
                    .frags
                    .values()
                    .find_map(|deaths| deaths.iter().find(|death| death.victim == vehicle.id))
                {
                    vehicle.death_info = Some(death.into());
                }
            }
        });

        let parsed_battle_results = self
            .battle_results
            .as_ref()
            .and_then(|results| serde_json::Value::from_str(results.as_str()).ok());

        // Build final Player objects with owned VehicleEntity
        let players: Vec<Rc<Player>> = self
            .player_entities
            .values()
            .filter_map(|player| {
                let entity = self.entities_by_id.get(&player.initial_state.entity_id())?;
                let vehicle_rc = entity.vehicle_ref()?;
                let mut vehicle: VehicleEntity = RefCell::borrow(vehicle_rc).clone();

                // Add battle results info to vehicle
                if let Some(battle_results) = parsed_battle_results
                    .as_ref()
                    .and_then(|results| results.as_object())
                {
                    vehicle.results_info =
                        battle_results.get("playersPublicInfo").and_then(|infos| {
                            infos.as_object().and_then(|infos| {
                                infos
                                    .get(player.initial_state.db_id.to_string().as_str())
                                    .cloned()
                            })
                        });

                    if let Some(frags) = self.frags.get(&player.initial_state.entity_id()) {
                        vehicle.frags = frags.iter().map(DeathInfo::from).collect();
                    }
                }

                // Clone player and attach the finalized vehicle entity
                let mut final_player = player.as_ref().clone();
                final_player.vehicle_entity = Some(vehicle);
                Some(Rc::new(final_player))
            })
            .collect();

        let frags: HashMap<Rc<Player>, Vec<DeathInfo>> =
            HashMap::from_iter(self.frags.drain().filter_map(|(entity_id, kills)| {
                let player = players
                    .iter()
                    .find(|p| p.initial_state.entity_id() == entity_id)?;
                let kills: Vec<DeathInfo> = kills.iter().map(DeathInfo::from).collect();
                Some((Rc::clone(player), kills))
            }));

        let self_player = players
            .iter()
            .find(|player| player.relation.is_self())
            .cloned()
            .expect("could not find self_player");

        // Collect building entities
        let buildings: Vec<BuildingEntity> = self
            .entities_by_id
            .values()
            .filter_map(|e| e.building_ref().map(|b| RefCell::borrow(b).clone()))
            .collect();

        BattleReport {
            arena_id: self.arena_id,
            match_result: if self.match_finished {
                self.winning_team.map(|team| {
                    if team == self_player.initial_state.team_id as i8 {
                        BattleResult::Win(team)
                    } else if team >= 0 {
                        BattleResult::Loss(1)
                    } else {
                        BattleResult::Draw
                    }
                })
            } else {
                None
            },
            self_player,
            version: Version::from_client_exe(self.game_version()),
            match_group: self.match_group().to_owned(),
            map_name: self.map_name(),
            game_mode: self.game_mode(),
            game_type: self.game_type(),
            players,
            game_chat: self.game_chat,
            battle_results: self.battle_results,
            frags,
            timeline: self.timeline,
            capture_points: self.capture_points,
            team_scores: self.team_scores,
            buildings,
        }
    }

    pub fn battle_results(&self) -> Option<&String> {
        self.battle_results.as_ref()
    }

    pub fn timeline(&self) -> &GameTimeline {
        &self.timeline
    }

    pub fn ship_positions(&self) -> &HashMap<EntityId, ShipPosition> {
        &self.ship_positions
    }

    pub fn minimap_positions(&self) -> &HashMap<EntityId, MinimapPosition> {
        &self.minimap_positions
    }

    pub fn capture_points(&self) -> &[CapturePointState] {
        &self.capture_points
    }

    pub fn team_scores(&self) -> &[TeamScore] {
        &self.team_scores
    }

    fn handle_property_update(
        &mut self,
        clock: GameClock,
        update: &crate::packet2::PropertyUpdatePacket<'_>,
    ) {
        if update.property != "state" {
            return;
        }

        let levels = &update.update_cmd.levels;
        let action = &update.update_cmd.action;

        // Match: state -> controlPoints -> [N] -> SetKey{...}
        if levels.len() == 2 {
            if let PropertyNestLevel::DictKey("controlPoints") = &levels[0] {
                if let PropertyNestLevel::ArrayIndex(point_idx) = &levels[1] {
                    // Ensure capture_points vec is large enough
                    while self.capture_points.len() <= *point_idx {
                        self.capture_points.push(CapturePointState {
                            index: self.capture_points.len(),
                            ..Default::default()
                        });
                    }

                    let mut evt_team_id = None;
                    let mut evt_invader_team = None;
                    let mut evt_progress = None;
                    let mut evt_has_invaders = None;
                    let mut evt_both_inside = None;

                    if let UpdateAction::SetKey { key, value } = action {
                        match *key {
                            "hasInvaders" => {
                                if let Some(v) = value.try_into().ok().map(|v: i32| v != 0) {
                                    self.capture_points[*point_idx].has_invaders = v;
                                    evt_has_invaders = Some(v);
                                }
                            }
                            "invaderTeam" => {
                                if let Some(v) = TryInto::<i32>::try_into(value).ok() {
                                    self.capture_points[*point_idx].invader_team = v as i64;
                                    evt_invader_team = Some(v as i64);
                                }
                            }
                            "progress" => {
                                // progress is a tuple: (fraction, time_remaining)
                                if let ArgValue::Array(arr) = value {
                                    if arr.len() >= 2 {
                                        let fraction: f64 = (&arr[0]).try_into().unwrap_or(0.0);
                                        let time_remaining: f64 =
                                            (&arr[1]).try_into().unwrap_or(0.0);
                                        self.capture_points[*point_idx].progress =
                                            (fraction, time_remaining);
                                        evt_progress = Some((fraction, time_remaining));
                                    }
                                }
                            }
                            "bothInside" => {
                                if let Some(v) = value.try_into().ok().map(|v: i32| v != 0) {
                                    self.capture_points[*point_idx].both_inside = v;
                                    evt_both_inside = Some(v);
                                }
                            }
                            "teamId" => {
                                if let Some(v) = TryInto::<i32>::try_into(value).ok() {
                                    self.capture_points[*point_idx].team_id = v as i64;
                                    evt_team_id = Some(v as i64);
                                }
                            }
                            _ => {}
                        }
                    }

                    self.timeline.push(
                        clock,
                        TimelineEvent::CapturePointUpdate {
                            point_index: *point_idx,
                            team_id: evt_team_id,
                            invader_team: evt_invader_team,
                            progress: evt_progress,
                            has_invaders: evt_has_invaders,
                            both_inside: evt_both_inside,
                        },
                    );
                }
            }
        }

        // Match: state -> missions -> teamsScore -> [N] -> SetKey{score}
        if levels.len() == 3 {
            if let PropertyNestLevel::DictKey("missions") = &levels[0] {
                if let PropertyNestLevel::DictKey("teamsScore") = &levels[1] {
                    if let PropertyNestLevel::ArrayIndex(team_idx) = &levels[2] {
                        if let UpdateAction::SetKey {
                            key: "score",
                            value,
                        } = action
                        {
                            if let Some(score) = TryInto::<i32>::try_into(value).ok() {
                                while self.team_scores.len() <= *team_idx {
                                    self.team_scores.push(TeamScore {
                                        team_index: self.team_scores.len(),
                                        ..Default::default()
                                    });
                                }
                                self.team_scores[*team_idx].score = score as i64;
                                self.timeline.push(
                                    clock,
                                    TimelineEvent::TeamScoreUpdate {
                                        team_index: *team_idx,
                                        score: score as i64,
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    fn handle_entity_create_with_clock(
        &mut self,
        clock: GameClock,
        packet: &EntityCreatePacket<'_>,
    ) {
        let entity_type = EntityType::from_str(packet.entity_type).unwrap_or_else(|_| {
            panic!(
                "failed to convert entity type {} to a string",
                packet.entity_type
            );
        });

        match entity_type {
            EntityType::Vehicle => {
                let mut props = VehicleProps::default();
                props.update_from_args(&packet.props, self.version);

                let captain_id = props.crew_modifiers_compact_params.params_id;
                let captain = if captain_id != 0 {
                    Some(
                        self.game_resources
                            .game_param_by_id(captain_id)
                            .expect("failed to get captain"),
                    )
                } else {
                    None
                };

                let vehicle = Rc::new(RefCell::new(VehicleEntity {
                    id: packet.entity_id,
                    props,
                    visibility_changed_at: 0.0,
                    captain,
                    damage: 0.0,
                    death_info: None,
                    results_info: None,
                    frags: Vec::default(),
                }));

                self.entities_by_id
                    .insert(packet.entity_id, Entity::Vehicle(vehicle.clone()));
            }
            EntityType::Building => {
                let mut is_alive = true;
                let mut is_hidden = false;
                let mut is_suppressed = false;
                let mut team_id: i8 = 0;
                let mut params_id: u32 = 0;

                if let Some(v) = packet.props.get("isAlive") {
                    is_alive = v.uint_8_ref().map(|v| *v != 0).unwrap_or(true);
                }
                if let Some(v) = packet.props.get("isHidden") {
                    is_hidden = v.uint_8_ref().map(|v| *v != 0).unwrap_or(false);
                }
                if let Some(v) = packet.props.get("isSuppressed") {
                    is_suppressed = v.uint_8_ref().map(|v| *v != 0).unwrap_or(false);
                }
                if let Some(v) = packet.props.get("teamId") {
                    team_id = v.int_8_ref().copied().unwrap_or(0);
                }
                if let Some(v) = packet.props.get("paramsId") {
                    params_id = v.uint_32_ref().copied().unwrap_or(0);
                }

                let building = BuildingEntity {
                    id: packet.entity_id,
                    is_alive,
                    is_hidden,
                    is_suppressed,
                    team_id,
                    params_id: GameParamId::from(params_id),
                };

                self.entities_by_id.insert(
                    packet.entity_id,
                    Entity::Building(Rc::new(RefCell::new(building.clone()))),
                );

                self.timeline.push(
                    clock,
                    TimelineEvent::BuildingStateChanged {
                        entity_id: packet.entity_id,
                        is_alive,
                        is_suppressed,
                        team_id,
                    },
                );
            }
            EntityType::SmokeScreen => {
                let mut radius: f32 = 0.0;
                if let Some(v) = packet.props.get("radius") {
                    radius = v.float_32_ref().copied().unwrap_or(0.0);
                }

                let smoke = SmokeScreenEntity {
                    id: packet.entity_id,
                    radius,
                };

                self.entities_by_id.insert(
                    packet.entity_id,
                    Entity::SmokeScreen(Rc::new(RefCell::new(smoke))),
                );

                self.timeline.push(
                    clock,
                    TimelineEvent::SmokeScreenCreated {
                        entity_id: packet.entity_id,
                        position: WorldPos {
                            x: packet.position.x,
                            y: packet.position.y,
                            z: packet.position.z,
                        },
                        radius,
                    },
                );
            }
            EntityType::BattleLogic => debug!("BattleLogic create"),
            EntityType::InteractiveZone => debug!("InteractiveZone create"),
            EntityType::BattleEntity => debug!("BattleEntity create"),
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum ChatChannel {
    Division,
    Global,
    Team,
}

fn parse_ship_config(blob: &[u8], version: Version) -> IResult<&[u8], ShipConfig> {
    let i = blob;
    let (i, _unk) = le_u32(i)?;

    let (i, ship_params_id) = le_u32(i)?;
    let (i, _unk2) = le_u32(i)?;

    let (i, unit_count) = le_u32(i)?;
    let (i, units) = count(le_u32, unit_count as usize)(i)?;

    let i = if version.is_at_least(&Version {
        major: 13,
        minor: 2,
        patch: 0,
        build: 0,
    }) {
        let (i, _unk) = le_u32(i)?;
        i
    } else {
        i
    };

    let (i, modernization_count) = le_u32(i)?;
    let (i, modernization) = count(le_u32, modernization_count as usize)(i)?;

    let (i, signal_count) = le_u32(i)?;
    let (i, signals) = count(le_u32, signal_count as usize)(i)?;

    let (i, _supply_state) = le_u32(i)?;

    let (i, camo_info_count) = le_u32(i)?;
    // First item in pair is camo_info, second is camo_scheme
    let (i, _camo) = count(pair(le_u32, le_u32), camo_info_count as usize)(i)?;

    let (i, abilities_count) = le_u32(i)?;
    let (i, abilities) = count(le_u32, abilities_count as usize)(i)?;

    Ok((
        i,
        ShipConfig {
            abilities,
            hull: units[0],
            modernization,
            units,
            signals,
        },
    ))
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GameMessage {
    pub clock: GameClock,
    pub sender_relation: Option<Relation>,
    pub sender_name: String,
    pub channel: ChatChannel,
    pub message: String,
    pub entity_id: EntityId,
    pub player: Option<Rc<Player>>,
}

#[derive(Debug, Default, Serialize, Clone)]
pub struct AAAura {
    id: u32,
    enabled: bool,
}

#[derive(Debug, Default, Serialize, Clone)]
pub struct VehicleState {
    /// TODO
    buffs: Option<()>,
    vehicle_visual_state: u8,
    /// TODO
    battery: Option<()>,
}

#[derive(Debug, Default, Serialize, Clone)]
pub struct CrewModifiersCompactParams {
    params_id: u32,
    is_in_adaption: bool,
    learned_skills: Skills,
}

trait UpdateFromReplayArgs {
    fn update_by_name(&mut self, name: &str, value: &ArgValue<'_>, version: Version) {
        // This is far from optimal, but is an easy solution for now
        let mut dict = HashMap::with_capacity(1);
        dict.insert(name, value.clone());
        self.update_from_args(&dict, version);
    }

    fn update_from_args(&mut self, args: &HashMap<&str, ArgValue<'_>>, version: Version);
}

macro_rules! set_arg_value {
    ($set_var:expr, $args:ident, $key:expr, String) => {
        $set_var = (*value
            .string_ref()
            .unwrap_or_else(|| panic!("{} is not a string", $key)))
        .clone()
    };
    ($set_var:expr, $args:ident, $key:expr, i8) => {
        set_arg_value!($set_var, $args, $key, int_8_ref, i8)
    };
    ($set_var:expr, $args:ident, $key:expr, i16) => {
        set_arg_value!($set_var, $args, $key, int_16_ref, i16)
    };
    ($set_var:expr, $args:ident, $key:expr, i32) => {
        set_arg_value!($set_var, $args, $key, int_32_ref, i32)
    };
    ($set_var:expr, $args:ident, $key:expr, u8) => {
        set_arg_value!($set_var, $args, $key, uint_8_ref, u8)
    };
    ($set_var:expr, $args:ident, $key:expr, u16) => {
        set_arg_value!($set_var, $args, $key, uint_16_ref, u16)
    };
    ($set_var:expr, $args:ident, $key:expr, u32) => {
        set_arg_value!($set_var, $args, $key, uint_32_ref, u32)
    };
    ($set_var:expr, $args:ident, $key:expr, f32) => {
        set_arg_value!($set_var, $args, $key, float_32_ref, f32)
    };
    ($set_var:expr, $args:ident, $key:expr, bool) => {
        if let Some(value) = $args.get($key) {
            $set_var = (*value
                .uint_8_ref()
                .unwrap_or_else(|| panic!("{} is not a u8", $key)))
                != 0
        }
    };
    ($set_var:expr, $args:ident, $key:expr, Vec<u8>) => {
        if let Some(value) = $args.get($key) {
            $set_var = value
                .blob_ref()
                .unwrap_or_else(|| panic!("{} is not a u8", $key))
                .clone()
        }
    };
    ($set_var:expr, $args:ident, $key:expr, &[()]) => {
        set_arg_value!($set_var, $args, $key, array_ref, &[()])
    };
    ($set_var:expr, $args:ident, $key:expr, $conversion_func:ident, $ty:ty) => {
        if let Some(value) = $args.get($key) {
            $set_var = value
                .$conversion_func()
                .unwrap_or_else(|| panic!("{} is not a {}", $key, stringify!($ty)))
                .clone()
        }
    };
}

macro_rules! arg_value_to_type {
    ($args:ident, $key:expr, String) => {
        arg_value_to_type!($args, $key, string_ref, String).clone()
    };
    ($args:ident, $key:expr, i8) => {
        *arg_value_to_type!($args, $key, int_8_ref, i8)
    };
    ($args:ident, $key:expr, i16) => {
        *arg_value_to_type!($args, $key, int_16_ref, i16)
    };
    ($args:ident, $key:expr, i32) => {
        *arg_value_to_type!($args, $key, int_32_ref, i32)
    };
    ($args:ident, $key:expr, u8) => {
        *arg_value_to_type!($args, $key, uint_8_ref, u8)
    };
    ($args:ident, $key:expr, u16) => {
        *arg_value_to_type!($args, $key, uint_16_ref, u16)
    };
    ($args:ident, $key:expr, u32) => {
        *arg_value_to_type!($args, $key, uint_32_ref, u32)
    };
    ($args:ident, $key:expr, bool) => {
        (*arg_value_to_type!($args, $key, uint_8_ref, u8)) != 0
    };
    ($args:ident, $key:expr, &[()]) => {
        arg_value_to_type!($args, $key, array_ref, &[()])
    };
    ($args:ident, $key:expr, &[u8]) => {
        arg_value_to_type!($args, $key, blob_ref, &[()]).as_ref()
    };
    ($args:ident, $key:expr, HashMap<(), ()>) => {
        arg_value_to_type!($args, $key, fixed_dict_ref, HashMap<(), ()>)
    };
    ($args:ident, $key:expr, $conversion_func:ident, $ty:ty) => {
        $args
            .get($key)
            .unwrap_or_else(|| panic!("could not get {}", $key))
            .$conversion_func()
            .unwrap_or_else(|| panic!("{} is not a {}", $key, stringify!($ty)))
    };
}

impl UpdateFromReplayArgs for CrewModifiersCompactParams {
    fn update_from_args(&mut self, args: &HashMap<&str, ArgValue<'_>>, version: Version) {
        const PARAMS_ID_KEY: &str = "paramsId";
        const IS_IN_ADAPTION_KEY: &str = "isInAdaption";
        const LEARNED_SKILLS_KEY: &str = "learnedSkills";

        if args.contains_key(PARAMS_ID_KEY) {
            self.params_id = arg_value_to_type!(args, PARAMS_ID_KEY, u32);
        }
        if args.contains_key(IS_IN_ADAPTION_KEY) {
            self.is_in_adaption = arg_value_to_type!(args, IS_IN_ADAPTION_KEY, bool);
        }

        if args.contains_key(LEARNED_SKILLS_KEY) {
            let learned_skills = arg_value_to_type!(args, LEARNED_SKILLS_KEY, &[()]);
            let skills_from_idx = |idx: usize| -> Vec<u8> {
                learned_skills[idx]
                    .array_ref()
                    .unwrap()
                    .iter()
                    .map(|idx| *(*idx).uint_8_ref().unwrap())
                    .collect()
            };

            let skills = Skills {
                aircraft_carrier: skills_from_idx(0),
                battleship: skills_from_idx(1),
                cruiser: skills_from_idx(2),
                destroyer: skills_from_idx(3),
                auxiliary: skills_from_idx(4),
                submarine: skills_from_idx(5),
            };

            self.learned_skills = skills;
        }
    }
}

#[derive(Debug, Default, Serialize, Clone)]
pub struct VehicleProps {
    ignore_map_borders: bool,
    air_defense_dispersion_radius: f32,
    death_settings: Vec<u8>,
    owner: u32,
    atba_targets: Vec<u32>,
    effects: Vec<String>,
    crew_modifiers_compact_params: CrewModifiersCompactParams,
    laser_target_local_pos: u16,
    anti_air_auras: Vec<AAAura>,
    selected_weapon: u32,
    regeneration_health: f32,
    is_on_forsage: bool,
    is_in_rage_mode: bool,
    has_air_targets_in_range: bool,
    torpedo_local_pos: u16,
    /// TODO
    air_defense_target_ids: Vec<()>,
    buoyancy: f32,
    max_health: f32,
    rudders_angle: f32,
    draught: f32,
    target_local_pos: u16,
    triggered_skills_data: Vec<u8>,
    regenerated_health: f32,
    blocked_controls: u8,
    is_invisible: bool,
    is_fog_horn_on: bool,
    server_speed_raw: u16,
    regen_crew_hp_limit: f32,
    /// TODO
    miscs_presets_status: Vec<()>,
    buoyancy_current_waterline: f32,
    is_alive: bool,
    is_bot: bool,
    visibility_flags: u32,
    heat_infos: Vec<()>,
    buoyancy_rudder_index: u8,
    is_anti_air_mode: bool,
    speed_sign_dir: i8,
    oil_leak_state: u8,
    /// TODO
    sounds: Vec<()>,
    ship_config: ShipConfig,
    wave_local_pos: u16,
    has_active_main_squadron: bool,
    weapon_lock_flags: u16,
    deep_rudders_angle: f32,
    /// TODO
    debug_text: Vec<()>,
    health: f32,
    engine_dir: i8,
    state: VehicleState,
    team_id: i8,
    buoyancy_current_state: u8,
    ui_enabled: bool,
    respawn_time: u16,
    engine_power: u8,
    max_server_speed_raw: u32,
    burning_flags: u16,
}

impl VehicleProps {
    pub fn ignore_map_borders(&self) -> bool {
        self.ignore_map_borders
    }

    pub fn air_defense_dispersion_radius(&self) -> f32 {
        self.air_defense_dispersion_radius
    }

    pub fn death_settings(&self) -> &[u8] {
        self.death_settings.as_ref()
    }

    pub fn owner(&self) -> u32 {
        self.owner
    }

    pub fn atba_targets(&self) -> &[u32] {
        self.atba_targets.as_ref()
    }

    pub fn effects(&self) -> &[String] {
        self.effects.as_ref()
    }

    pub fn crew_modifiers_compact_params(&self) -> &CrewModifiersCompactParams {
        &self.crew_modifiers_compact_params
    }

    pub fn laser_target_local_pos(&self) -> u16 {
        self.laser_target_local_pos
    }

    pub fn anti_air_auras(&self) -> &[AAAura] {
        self.anti_air_auras.as_ref()
    }

    pub fn selected_weapon(&self) -> u32 {
        self.selected_weapon
    }

    pub fn regeneration_health(&self) -> f32 {
        self.regeneration_health
    }

    pub fn is_on_forsage(&self) -> bool {
        self.is_on_forsage
    }

    pub fn is_in_rage_mode(&self) -> bool {
        self.is_in_rage_mode
    }

    pub fn has_air_targets_in_range(&self) -> bool {
        self.has_air_targets_in_range
    }

    pub fn torpedo_local_pos(&self) -> u16 {
        self.torpedo_local_pos
    }

    pub fn air_defense_target_ids(&self) -> &[()] {
        self.air_defense_target_ids.as_ref()
    }

    pub fn buoyancy(&self) -> f32 {
        self.buoyancy
    }

    pub fn max_health(&self) -> f32 {
        self.max_health
    }

    pub fn rudders_angle(&self) -> f32 {
        self.rudders_angle
    }

    pub fn draught(&self) -> f32 {
        self.draught
    }

    pub fn target_local_pos(&self) -> u16 {
        self.target_local_pos
    }

    pub fn triggered_skills_data(&self) -> &[u8] {
        self.triggered_skills_data.as_ref()
    }

    pub fn regenerated_health(&self) -> f32 {
        self.regenerated_health
    }

    pub fn blocked_controls(&self) -> u8 {
        self.blocked_controls
    }

    pub fn is_invisible(&self) -> bool {
        self.is_invisible
    }

    pub fn is_fog_horn_on(&self) -> bool {
        self.is_fog_horn_on
    }

    pub fn server_speed_raw(&self) -> u16 {
        self.server_speed_raw
    }

    pub fn regen_crew_hp_limit(&self) -> f32 {
        self.regen_crew_hp_limit
    }

    pub fn miscs_presets_status(&self) -> &[()] {
        self.miscs_presets_status.as_ref()
    }

    pub fn buoyancy_current_waterline(&self) -> f32 {
        self.buoyancy_current_waterline
    }

    pub fn is_alive(&self) -> bool {
        self.is_alive
    }

    pub fn is_bot(&self) -> bool {
        self.is_bot
    }

    pub fn visibility_flags(&self) -> u32 {
        self.visibility_flags
    }

    pub fn heat_infos(&self) -> &[()] {
        self.heat_infos.as_ref()
    }

    pub fn buoyancy_rudder_index(&self) -> u8 {
        self.buoyancy_rudder_index
    }

    pub fn is_anti_air_mode(&self) -> bool {
        self.is_anti_air_mode
    }

    pub fn speed_sign_dir(&self) -> i8 {
        self.speed_sign_dir
    }

    pub fn oil_leak_state(&self) -> u8 {
        self.oil_leak_state
    }

    pub fn sounds(&self) -> &[()] {
        self.sounds.as_ref()
    }

    pub fn ship_config(&self) -> &ShipConfig {
        &self.ship_config
    }

    pub fn wave_local_pos(&self) -> u16 {
        self.wave_local_pos
    }

    pub fn has_active_main_squadron(&self) -> bool {
        self.has_active_main_squadron
    }

    pub fn weapon_lock_flags(&self) -> u16 {
        self.weapon_lock_flags
    }

    pub fn deep_rudders_angle(&self) -> f32 {
        self.deep_rudders_angle
    }

    pub fn debug_text(&self) -> &[()] {
        self.debug_text.as_ref()
    }

    pub fn health(&self) -> f32 {
        self.health
    }

    pub fn engine_dir(&self) -> i8 {
        self.engine_dir
    }

    pub fn state(&self) -> &VehicleState {
        &self.state
    }

    pub fn team_id(&self) -> i8 {
        self.team_id
    }

    pub fn buoyancy_current_state(&self) -> u8 {
        self.buoyancy_current_state
    }

    pub fn ui_enabled(&self) -> bool {
        self.ui_enabled
    }

    pub fn respawn_time(&self) -> u16 {
        self.respawn_time
    }

    pub fn engine_power(&self) -> u8 {
        self.engine_power
    }

    pub fn max_server_speed_raw(&self) -> u32 {
        self.max_server_speed_raw
    }

    pub fn burning_flags(&self) -> u16 {
        self.burning_flags
    }
}

impl UpdateFromReplayArgs for VehicleProps {
    fn update_by_name(&mut self, name: &str, value: &ArgValue<'_>, version: Version) {
        // This is far from optimal, but is an easy solution for now
        let mut dict = HashMap::with_capacity(1);
        dict.insert(name, value.clone());
        self.update_from_args(&dict, version);
    }

    fn update_from_args(&mut self, args: &HashMap<&str, ArgValue<'_>>, version: Version) {
        const IGNORE_MAP_BORDERS_KEY: &str = "ignoreMapBorders";
        const AIR_DEFENSE_DISPERSION_RADIUS_KEY: &str = "airDefenseDispRadius";
        const DEATH_SETTINGS_KEY: &str = "deathSettings";
        const OWNER_KEY: &str = "owner";
        const ATBA_TARGETS_KEY: &str = "atbaTargets";
        const EFFECTS_KEY: &str = "effects";
        const CREW_MODIFIERS_COMPACT_PARAMS_KEY: &str = "crewModifiersCompactParams";
        const LASER_TARGET_LOCAL_POS_KEY: &str = "laserTargetLocalPos";
        const ANTI_AIR_AUROS_KEY: &str = "antiAirAuras";
        const SELECTED_WEAPON_KEY: &str = "selectedWeapon";
        const REGENERATION_HEALTH_KEY: &str = "regenerationHealth";
        const IS_ON_FORSAGE_KEY: &str = "isOnForsage";
        const IS_IN_RAGE_MODE_KEY: &str = "isInRageMode";
        const HAS_AIR_TARGETS_IN_RANGE_KEY: &str = "hasAirTargetsInRange";
        const TORPEDO_LOCAL_POS_KEY: &str = "torpedoLocalPos";
        const AIR_DEFENSE_TARGET_IDS_KEY: &str = "airDefenseTargetIds";
        const BUOYANCY_KEY: &str = "buoyancy";
        const MAX_HEALTH_KEY: &str = "maxHealth";
        const DRAUGHT_KEY: &str = "draught";
        const RUDDERS_ANGLE_KEY: &str = "ruddersAngle";
        const TARGET_LOCAL_POSITION_KEY: &str = "targetLocalPos";
        const TRIGGERED_SKILLS_DATA_KEY: &str = "triggeredSkillsData";
        const REGENERATED_HEALTH_KEY: &str = "regeneratedHealth";
        const BLOCKED_CONTROLS_KEY: &str = "blockedControls";
        const IS_INVISIBLE_KEY: &str = "isInvisible";
        const IS_FOG_HORN_ON_KEY: &str = "isFogHornOn";
        const SERVER_SPEED_RAW_KEY: &str = "serverSpeedRaw";
        const REGEN_CREW_HP_LIMIT_KEY: &str = "regenCrewHpLimit";
        const MISCS_PRESETS_STATUS_KEY: &str = "miscsPresetsStatus";
        const BUOYANCY_CURRENT_WATERLINE_KEY: &str = "buoyancyCurrentWaterline";
        const IS_ALIVE_KEY: &str = "isAlive";
        const IS_BOT_KEY: &str = "isBot";
        const VISIBILITY_FLAGS_KEY: &str = "visibilityFlags";
        const HEAT_INFOS_KEY: &str = "heatInfos";
        const BUOYANCY_RUDDER_INDEX_KEY: &str = "buoyancyRudderIndex";
        const IS_ANTI_AIR_MODE_KEY: &str = "isAntiAirMode";
        const SPEED_SIGN_DIR_KEY: &str = "speedSignDir";
        const OIL_LEAK_STATE_KEY: &str = "oilLeakState";
        const SOUNDS_KEY: &str = "sounds";
        const SHIP_CONFIG_KEY: &str = "shipConfig";
        const WAVE_LOCAL_POS_KEY: &str = "waveLocalPos";
        const HAS_ACTIVE_MAIN_SQUADRON_KEY: &str = "hasActiveMainSquadron";
        const WEAPON_LOCK_FLAGS_KEY: &str = "weaponLockFlags";
        const DEEP_RUDDERS_ANGLE_KEY: &str = "deepRuddersAngle";
        const DEBUG_TEXT_KEY: &str = "debugText";
        const HEALTH_KEY: &str = "health";
        const ENGINE_DIR_KEY: &str = "engineDir";
        const STATE_KEY: &str = "state";
        const TEAM_ID_KEY: &str = "teamId";
        const BUOYANCY_CURRENT_STATE_KEY: &str = "buoyancyCurrentState";
        const UI_ENABLED_KEY: &str = "uiEnabled";
        const RESPAWN_TIME_KEY: &str = "respawnTime";
        const ENGINE_POWER_KEY: &str = "enginePower";
        const MAX_SERVER_SPEED_RAW_KEY: &str = "maxServerSpeedRaw";
        const BURNING_FLAGS_KEY: &str = "burningFlags";

        set_arg_value!(self.ignore_map_borders, args, IGNORE_MAP_BORDERS_KEY, bool);
        set_arg_value!(
            self.air_defense_dispersion_radius,
            args,
            AIR_DEFENSE_DISPERSION_RADIUS_KEY,
            f32
        );

        set_arg_value!(self.death_settings, args, DEATH_SETTINGS_KEY, Vec<u8>);
        if args.contains_key(OWNER_KEY) {
            let value: u32 = arg_value_to_type!(args, OWNER_KEY, i32) as u32;
            self.owner = value;
        }

        if args.contains_key(ATBA_TARGETS_KEY) {
            let value: Vec<u32> = arg_value_to_type!(args, ATBA_TARGETS_KEY, &[()])
                .iter()
                .map(|elem| *elem.uint_32_ref().expect("atbaTargets elem is not a u32"))
                .collect();
            self.atba_targets = value;
        }

        if args.contains_key(EFFECTS_KEY) {
            let value: Vec<String> = arg_value_to_type!(args, EFFECTS_KEY, &[()])
                .iter()
                .map(|elem| {
                    String::from_utf8(
                        elem.string_ref()
                            .expect("effects elem is not a string")
                            .clone(),
                    )
                    .expect("could not convert effects elem to string")
                })
                .collect();
            self.effects = value;
        }

        if args.contains_key(CREW_MODIFIERS_COMPACT_PARAMS_KEY) {
            self.crew_modifiers_compact_params.update_from_args(
                arg_value_to_type!(args, CREW_MODIFIERS_COMPACT_PARAMS_KEY, HashMap<(), ()>),
                version,
            );
        }

        set_arg_value!(
            self.laser_target_local_pos,
            args,
            LASER_TARGET_LOCAL_POS_KEY,
            u16
        );

        // TODO: AntiAirAuras
        set_arg_value!(self.selected_weapon, args, SELECTED_WEAPON_KEY, u32);

        set_arg_value!(self.is_on_forsage, args, IS_ON_FORSAGE_KEY, bool);

        set_arg_value!(self.is_in_rage_mode, args, IS_IN_RAGE_MODE_KEY, bool);

        set_arg_value!(
            self.has_air_targets_in_range,
            args,
            HAS_AIR_TARGETS_IN_RANGE_KEY,
            bool
        );

        set_arg_value!(self.torpedo_local_pos, args, TORPEDO_LOCAL_POS_KEY, u16);

        // TODO: airDefenseTargetIds

        set_arg_value!(self.buoyancy, args, BUOYANCY_KEY, f32);

        set_arg_value!(self.max_health, args, MAX_HEALTH_KEY, f32);

        set_arg_value!(self.draught, args, DRAUGHT_KEY, f32);

        set_arg_value!(self.rudders_angle, args, RUDDERS_ANGLE_KEY, f32);

        set_arg_value!(self.target_local_pos, args, TARGET_LOCAL_POSITION_KEY, u16);

        set_arg_value!(
            self.triggered_skills_data,
            args,
            TRIGGERED_SKILLS_DATA_KEY,
            Vec<u8>
        );

        set_arg_value!(self.regenerated_health, args, REGENERATED_HEALTH_KEY, f32);

        set_arg_value!(self.blocked_controls, args, BLOCKED_CONTROLS_KEY, u8);

        set_arg_value!(self.is_invisible, args, IS_INVISIBLE_KEY, bool);

        set_arg_value!(self.is_fog_horn_on, args, IS_FOG_HORN_ON_KEY, bool);

        set_arg_value!(self.server_speed_raw, args, SERVER_SPEED_RAW_KEY, u16);

        set_arg_value!(self.regen_crew_hp_limit, args, REGEN_CREW_HP_LIMIT_KEY, f32);

        // TODO: miscs_presets_status

        set_arg_value!(
            self.buoyancy_current_waterline,
            args,
            BUOYANCY_CURRENT_WATERLINE_KEY,
            f32
        );
        set_arg_value!(self.is_alive, args, IS_ALIVE_KEY, bool);
        set_arg_value!(self.is_bot, args, IS_BOT_KEY, bool);
        set_arg_value!(self.visibility_flags, args, VISIBILITY_FLAGS_KEY, u32);

        // TODO: heatInfos

        set_arg_value!(
            self.buoyancy_rudder_index,
            args,
            BUOYANCY_RUDDER_INDEX_KEY,
            u8
        );
        set_arg_value!(self.is_anti_air_mode, args, IS_ANTI_AIR_MODE_KEY, bool);
        set_arg_value!(self.speed_sign_dir, args, SPEED_SIGN_DIR_KEY, i8);
        set_arg_value!(self.oil_leak_state, args, OIL_LEAK_STATE_KEY, u8);

        // TODO: sounds

        if args.contains_key(SHIP_CONFIG_KEY) {
            let (_remainder, ship_config) =
                parse_ship_config(arg_value_to_type!(args, SHIP_CONFIG_KEY, &[u8]), version)
                    .expect("failed to parse ship config");

            self.ship_config = ship_config;
        }

        set_arg_value!(self.wave_local_pos, args, WAVE_LOCAL_POS_KEY, u16);
        set_arg_value!(
            self.has_active_main_squadron,
            args,
            HAS_ACTIVE_MAIN_SQUADRON_KEY,
            bool
        );
        set_arg_value!(self.weapon_lock_flags, args, WEAPON_LOCK_FLAGS_KEY, u16);
        set_arg_value!(self.deep_rudders_angle, args, DEEP_RUDDERS_ANGLE_KEY, f32);

        // TODO: debugText

        set_arg_value!(self.health, args, HEALTH_KEY, f32);
        set_arg_value!(self.engine_dir, args, ENGINE_DIR_KEY, i8);

        // TODO: state

        set_arg_value!(self.team_id, args, TEAM_ID_KEY, i8);
        set_arg_value!(
            self.buoyancy_current_state,
            args,
            BUOYANCY_CURRENT_STATE_KEY,
            u8
        );
        set_arg_value!(self.ui_enabled, args, UI_ENABLED_KEY, bool);
        set_arg_value!(self.respawn_time, args, RESPAWN_TIME_KEY, u16);
        set_arg_value!(self.engine_power, args, ENGINE_POWER_KEY, u8);
        set_arg_value!(
            self.max_server_speed_raw,
            args,
            MAX_SERVER_SPEED_RAW_KEY,
            u32
        );
        set_arg_value!(self.burning_flags, args, BURNING_FLAGS_KEY, u16);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DeathInfo {
    /// Time lived in the game. This may not be accurate if a game rejoin occurs
    /// as there's no known way to detect this event.
    time_lived: Duration,
    killer: EntityId,
    cause: DeathCause,
}

impl DeathInfo {
    pub fn time_lived(&self) -> Duration {
        self.time_lived
    }

    pub fn killer(&self) -> EntityId {
        self.killer
    }

    pub fn cause(&self) -> DeathCause {
        self.cause
    }
}

impl From<&Death> for DeathInfo {
    fn from(death: &Death) -> Self {
        // Can occur if the player rejoins a game
        let time_lived = if death.timestamp > TIME_UNTIL_GAME_START {
            death.timestamp - TIME_UNTIL_GAME_START
        } else {
            Duration::from_secs(0)
        };

        DeathInfo {
            time_lived,
            killer: death.killer,
            cause: death.cause,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct VehicleEntity {
    id: EntityId,
    visibility_changed_at: f32,
    props: VehicleProps,
    captain: Option<Rc<Param>>,
    damage: f32,
    death_info: Option<DeathInfo>,
    results_info: Option<serde_json::Value>,
    frags: Vec<DeathInfo>,
}

impl VehicleEntity {
    pub fn id(&self) -> EntityId {
        self.id
    }

    pub fn props(&self) -> &VehicleProps {
        &self.props
    }

    pub fn commander_id(&self) -> u32 {
        self.props.crew_modifiers_compact_params.params_id
    }

    pub fn commander_skills(&self, vehicle_species: Species) -> Option<Vec<&CrewSkill>> {
        let skills = &self.props.crew_modifiers_compact_params.learned_skills;
        let skills_for_species = match vehicle_species {
            Species::AirCarrier => skills.aircraft_carrier.as_slice(),
            Species::Battleship => skills.battleship.as_slice(),
            Species::Cruiser => skills.cruiser.as_slice(),
            Species::Destroyer => skills.destroyer.as_slice(),
            Species::Submarine => skills.submarine.as_slice(),
            other => {
                panic!("Unexpected vehicle species: {:?}", other);
            }
        };

        let captain = self
            .captain()?
            .data()
            .crew_ref()
            .expect("captain is not a crew?");

        let skills = skills_for_species
            .iter()
            .map(|skill_type| {
                captain
                    .skill_by_type(*skill_type as u32)
                    .expect("could not get skill type")
            })
            .collect();

        Some(skills)
    }

    pub fn commander_skills_raw(&self, vehicle_species: Species) -> &[u8] {
        let skills = &self.props.crew_modifiers_compact_params.learned_skills;
        match vehicle_species {
            Species::AirCarrier => skills.aircraft_carrier.as_slice(),
            Species::Battleship => skills.battleship.as_slice(),
            Species::Cruiser => skills.cruiser.as_slice(),
            Species::Destroyer => skills.destroyer.as_slice(),
            Species::Submarine => skills.submarine.as_slice(),
            other => {
                panic!("Unexpected vehicle species: {:?}", other);
            }
        }
    }

    pub fn captain(&self) -> Option<&Param> {
        self.captain.as_ref().map(|rc| rc.as_ref())
    }

    pub fn damage(&self) -> f32 {
        self.damage
    }

    pub fn death_info(&self) -> Option<&DeathInfo> {
        self.death_info.as_ref()
    }

    pub fn results_info(&self) -> Option<&serde_json::Value> {
        self.results_info.as_ref()
    }

    pub fn frags(&self) -> &[DeathInfo] {
        &self.frags
    }
}

#[derive(Debug, Variantly)]
pub enum Entity {
    Vehicle(Rc<RefCell<VehicleEntity>>),
    Building(Rc<RefCell<BuildingEntity>>),
    SmokeScreen(Rc<RefCell<SmokeScreenEntity>>),
}

#[derive(Debug)]
struct Death {
    timestamp: Duration,
    killer: EntityId,
    victim: EntityId,
    cause: DeathCause,
}

impl<'res, 'replay, G> AnalyzerMut for BattleController<'res, 'replay, G>
where
    G: ResourceLoader,
{
    fn process_mut(&mut self, packet: &Packet<'_, '_>) {
        let span = span!(Level::TRACE, "packet processing");
        let _enter = span.enter();

        // trace!("packet: {packet:#?}");
        //

        let decoded = DecodedPacket::from(&self.version, false, packet);
        let payload_kind = decoded.payload.kind();
        match decoded.payload {
            crate::analyzer::decoder::DecodedPacketPayload::Chat {
                entity_id,
                sender_id,
                audience,
                message,
                extra_data,
            } => {
                self.handle_chat_message(
                    entity_id,
                    sender_id,
                    audience,
                    message,
                    extra_data,
                    packet.clock,
                );
            }
            crate::analyzer::decoder::DecodedPacketPayload::VoiceLine {
                sender_id,
                is_global,
                message,
            } => {
                trace!("HANDLE VOICE LINE");
            }
            crate::analyzer::decoder::DecodedPacketPayload::Ribbon(ribbon) => {
                self.timeline
                    .push(packet.clock, TimelineEvent::Ribbon(ribbon));
            }
            crate::analyzer::decoder::DecodedPacketPayload::Position(pos) => {
                let world_pos = WorldPos {
                    x: pos.position.x,
                    y: pos.position.y,
                    z: pos.position.z,
                };
                let ship_pos = ShipPosition {
                    entity_id: pos.pid,
                    position: world_pos,
                    yaw: pos.rotation.yaw,
                    pitch: pos.rotation.pitch,
                    roll: pos.rotation.roll,
                    last_updated: packet.clock,
                };
                self.ship_positions.insert(pos.pid, ship_pos);
                self.timeline.push(
                    packet.clock,
                    TimelineEvent::ShipPosition {
                        entity_id: pos.pid,
                        position: world_pos,
                        yaw: pos.rotation.yaw,
                        pitch: pos.rotation.pitch,
                        roll: pos.rotation.roll,
                    },
                );
            }
            crate::analyzer::decoder::DecodedPacketPayload::PlayerOrientation(_orientation) => {
                trace!("PLAYER ORIENTATION")
            }
            crate::analyzer::decoder::DecodedPacketPayload::DamageStat(_damage) => {
                trace!("DAMAGE STAT")
            }
            crate::analyzer::decoder::DecodedPacketPayload::ShipDestroyed {
                killer,
                victim,
                cause,
            } => {
                self.frags.entry(killer).or_default().push(Death {
                    timestamp: packet.clock.to_duration(),
                    killer,
                    victim,
                    cause,
                });
                self.timeline.push(
                    packet.clock,
                    TimelineEvent::ShipDestroyed {
                        killer,
                        victim,
                        cause,
                    },
                );
            }
            crate::analyzer::decoder::DecodedPacketPayload::EntityMethod(method) => {
                debug!("ENTITY METHOD, {:#?}", method)
            }
            crate::analyzer::decoder::DecodedPacketPayload::EntityProperty(prop) => {
                if let Some(entity) = self.entities_by_id.get(&prop.entity_id) {
                    if let Some(vehicle) = entity.vehicle_ref() {
                        let mut vehicle = RefCell::borrow_mut(vehicle);
                        vehicle
                            .props
                            .update_by_name(prop.property, &prop.value, self.version);
                    }
                }
            }
            crate::analyzer::decoder::DecodedPacketPayload::BasePlayerCreate(_base) => {
                trace!("BASE PLAYER CREATE");
            }
            crate::analyzer::decoder::DecodedPacketPayload::CellPlayerCreate(_cell) => {
                trace!("CELL PLAYER CREATE");
            }
            crate::analyzer::decoder::DecodedPacketPayload::EntityEnter(_e) => {
                trace!("ENTITY ENTER")
            }
            crate::analyzer::decoder::DecodedPacketPayload::EntityLeave(leave) => {
                if self
                    .entities_by_id
                    .get(&leave.entity_id)
                    .and_then(|e| e.smoke_screen_ref())
                    .is_some()
                {
                    self.entities_by_id.remove(&leave.entity_id);
                    self.timeline.push(
                        packet.clock,
                        TimelineEvent::SmokeScreenDestroyed {
                            entity_id: leave.entity_id,
                        },
                    );
                }
            }
            crate::analyzer::decoder::DecodedPacketPayload::EntityCreate(entity_create) => {
                self.handle_entity_create(packet.clock, entity_create);
            }
            crate::analyzer::decoder::DecodedPacketPayload::OnArenaStateReceived {
                arena_id: arg0,
                team_build_type_id: arg1,
                pre_battles_info: arg2,
                player_states: players,
            } => {
                debug!("OnArenaStateReceived");
                self.arena_id = arg0;
                for player in &players {
                    let metadata_player = self
                        .metadata_players
                        .iter()
                        .find(|meta_player| meta_player.id == player.meta_ship_id())
                        .expect("could not map arena player to metadata player");

                    let battle_player = Player::from_arena_player(
                        player,
                        metadata_player.as_ref(),
                        self.game_resources,
                    );

                    let player_has_died = self
                        .entities_by_id
                        .get(&player.entity_id())
                        .map(|vehicle| {
                            let Some(vehicle) = vehicle.vehicle_ref() else {
                                return false;
                            };
                            let vehicle = RefCell::borrow(vehicle);

                            self.frags.values().any(|deaths| {
                                deaths.iter().any(|death| death.victim == vehicle.id())
                            })
                        })
                        .unwrap_or_default();

                    if player.is_connected {
                        battle_player
                            .connection_change_info_mut()
                            .push(ConnectionChangeInfo {
                                at_game_duration: packet.clock.to_duration(),
                                event_kind: ConnectionChangeKind::Connected,
                                had_death_event: player_has_died,
                            });
                    }

                    let battle_player = Rc::new(battle_player);

                    self.player_entities
                        .insert(battle_player.initial_state.entity_id(), battle_player);
                }
            }
            crate::analyzer::decoder::DecodedPacketPayload::CheckPing(_) => trace!("CHECK PING"),
            crate::analyzer::decoder::DecodedPacketPayload::DamageReceived {
                victim,
                aggressors,
            } => {
                for damage in &aggressors {
                    self.damage_dealt
                        .entry(damage.aggressor)
                        .or_default()
                        .push(DamageEvent {
                            amount: damage.damage,
                            victim,
                        });
                    self.timeline.push(
                        packet.clock,
                        TimelineEvent::DamageDealt {
                            aggressor_id: damage.aggressor,
                            victim_id: victim,
                            damage: damage.damage,
                        },
                    );
                }
            }
            crate::analyzer::decoder::DecodedPacketPayload::MinimapUpdate { updates, arg1: _ } => {
                for update in &updates {
                    let visible = !update.disappearing;
                    self.minimap_positions.insert(
                        update.entity_id,
                        MinimapPosition {
                            entity_id: update.entity_id,
                            position: update.position,
                            heading: update.heading,
                            visible,
                            last_updated: packet.clock,
                        },
                    );
                    self.timeline.push(
                        packet.clock,
                        TimelineEvent::MinimapVisionUpdate {
                            entity_id: update.entity_id,
                            position: update.position,
                            heading: update.heading,
                            disappearing: update.disappearing,
                        },
                    );
                }
            }
            crate::analyzer::decoder::DecodedPacketPayload::PropertyUpdate(update) => {
                if let Some(entity) = self.entities_by_id.get(&update.entity_id) {
                    debug!("PROPERTY UPDATE: {:#?}", update);
                }
                self.handle_property_update(packet.clock, update);
            }
            crate::analyzer::decoder::DecodedPacketPayload::BattleEnd {
                winning_team,
                state,
            } => {
                self.match_finished = true;
                self.winning_team = winning_team;
                self.timeline
                    .push(packet.clock, TimelineEvent::BattleEnd { winning_team });
            }
            crate::analyzer::decoder::DecodedPacketPayload::Consumable {
                entity,
                consumable,
                duration,
            } => {
                self.active_consumables
                    .entry(entity)
                    .or_default()
                    .push(ActiveConsumable {
                        consumable,
                        activated_at: packet.clock,
                        duration,
                    });
                self.timeline.push(
                    packet.clock,
                    TimelineEvent::ConsumableActivated {
                        entity_id: entity,
                        consumable,
                        duration,
                    },
                );
            }
            crate::analyzer::decoder::DecodedPacketPayload::ArtilleryShots {
                entity_id,
                salvos,
            } => {
                self.timeline.push(
                    packet.clock,
                    TimelineEvent::ArtilleryShots { entity_id, salvos },
                );
            }
            crate::analyzer::decoder::DecodedPacketPayload::TorpedoesReceived {
                entity_id,
                torpedoes,
            } => {
                self.timeline.push(
                    packet.clock,
                    TimelineEvent::TorpedoesLaunched {
                        entity_id,
                        torpedoes,
                    },
                );
            }
            crate::analyzer::decoder::DecodedPacketPayload::PlanePosition {
                entity_id,
                plane_id,
                x,
                y,
            } => {
                self.timeline.push(
                    packet.clock,
                    TimelineEvent::PlanePosition {
                        entity_id,
                        plane_id,
                        x,
                        y,
                    },
                );
            }
            crate::analyzer::decoder::DecodedPacketPayload::PlaneAdded { .. } => {}
            crate::analyzer::decoder::DecodedPacketPayload::PlaneRemoved { .. } => {}
            crate::analyzer::decoder::DecodedPacketPayload::CruiseState { state, value } => {
                trace!("CRUISE STATE")
            }
            crate::analyzer::decoder::DecodedPacketPayload::Map(_) => trace!("MAP"),
            crate::analyzer::decoder::DecodedPacketPayload::Version(_) => trace!("VERSION"),
            crate::analyzer::decoder::DecodedPacketPayload::Camera(_) => trace!("CAMERA"),
            crate::analyzer::decoder::DecodedPacketPayload::CameraMode(_) => {
                trace!("CAMERA MODE")
            }
            crate::analyzer::decoder::DecodedPacketPayload::CameraFreeLook(_) => {
                trace!("CAMERA FREE LOOK")
            }
            crate::analyzer::decoder::DecodedPacketPayload::ShotKills { .. } => {}
            crate::analyzer::decoder::DecodedPacketPayload::Unknown(_) => trace!("UNKNOWN"),
            crate::analyzer::decoder::DecodedPacketPayload::Invalid(_) => trace!("INVALID"),
            crate::analyzer::decoder::DecodedPacketPayload::Audit(_) => trace!("AUDIT"),
            crate::analyzer::decoder::DecodedPacketPayload::BattleResults(json) => {
                self.battle_results = Some(json.to_owned());
            }
            crate::analyzer::decoder::DecodedPacketPayload::OnGameRoomStateChanged {
                player_states,
            } => {
                for player_state in &player_states {
                    let Some(meta_ship_id) = player_state.get(PlayerStateData::KEY_ID) else {
                        continue;
                    };

                    let meta_ship_id = *meta_ship_id.i64_ref().expect("player_id is not an i64");

                    let Some(player) = self.player_entities.values().find(|player| {
                        player.initial_state().meta_ship_id() == AccountId::from(meta_ship_id)
                    }) else {
                        warn!("Failed to find player with meta ship ID {meta_ship_id:?}");
                        continue;
                    };

                    {
                        player.end_state_mut().update_from_dict(&player_state);
                    }

                    let player_has_died = self
                        .entities_by_id
                        .get(&player.initial_state().entity_id())
                        .map(|vehicle| {
                            let Some(vehicle) = vehicle.vehicle_ref() else {
                                return false;
                            };
                            let vehicle = RefCell::borrow(vehicle);

                            self.frags.values().any(|deaths| {
                                deaths.iter().any(|death| death.victim == vehicle.id())
                            })
                        })
                        .unwrap_or_default();

                    let connection_event_kind = if player.end_state().is_connected {
                        ConnectionChangeKind::Connected
                    } else {
                        ConnectionChangeKind::Disconnected
                    };

                    if (player.connection_change_info().is_empty()
                        && connection_event_kind != ConnectionChangeKind::Disconnected)
                        || player
                            .connection_change_info()
                            .last()
                            .map(|info| info.event_kind != connection_event_kind)
                            .unwrap_or_default()
                    {
                        player
                            .connection_change_info_mut()
                            .push(ConnectionChangeInfo {
                                at_game_duration: packet.clock.to_duration(),
                                event_kind: connection_event_kind,
                                had_death_event: player_has_died,
                            });
                    }
                }
            }
        }
    }

    fn finish(&mut self) {}
}

impl<'res, 'replay, G> PacketProcessorMut for BattleController<'res, 'replay, G>
where
    G: ResourceLoader,
{
    fn process_mut(&mut self, packet: Packet<'_, '_>) {
        AnalyzerMut::process_mut(self, &packet);
    }
}
