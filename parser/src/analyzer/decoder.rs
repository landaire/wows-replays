use crate::IResult;
use crate::packet2::{EntityMethodPacket, Packet, PacketType};
use crate::types::{AccountId, EntityId, GameParamId, NormalizedPos, PlaneId};
use kinded::Kinded;
use nom::number::complete::{le_f32, le_u8, le_u16, le_u64};
use pickled::Value;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::convert::TryInto;
use std::iter::FromIterator;
use wowsunpack::data::Version;
use wowsunpack::game_params::convert::pickle_to_json;
use wowsunpack::rpc::typedefs::ArgValue;
use wowsunpack::unpack_rpc_args;

use super::analyzer::Analyzer;

pub struct DecoderBuilder {
    silent: bool,
    no_meta: bool,
    path: Option<String>,
}

impl DecoderBuilder {
    pub fn new(silent: bool, no_meta: bool, output: Option<&str>) -> Self {
        Self {
            silent,
            no_meta,
            path: output.map(|s| s.to_string()),
        }
    }

    pub fn build(self, meta: &crate::ReplayMeta) -> Box<dyn Analyzer> {
        let version = Version::from_client_exe(&meta.clientVersionFromExe);
        let mut decoder = Decoder {
            silent: self.silent,
            output: self.path.as_ref().map(|path| {
                Box::new(std::fs::File::create(path).unwrap()) as Box<dyn std::io::Write>
            }),
            version,
        };
        if !self.no_meta {
            decoder.write(&serde_json::to_string(&meta).unwrap());
        }
        Box::new(decoder)
    }
}

/// Enumerates voicelines which can be said in the game.
#[derive(Debug, Clone, Copy, Serialize)]
pub enum VoiceLine {
    IntelRequired,
    FairWinds,
    Wilco,
    Negative,
    WellDone,
    Curses,
    UsingRadar,
    UsingHydroSearch,
    DefendTheBase, // TODO: ...except when it's "thank you"?
    SetSmokeScreen,
    FollowMe,
    // TODO: definitely has associated data similar to AttentionToSquare
    /// World x and y coordinates corresponding to the map grid
    /// MapPointQuickCommand in game code
    MapPointAttention(f32, f32),
    UsingSubmarineLocator,
    /// "Provide anti-aircraft support"
    ProvideAntiAircraft,
    /// If a player is called out in the message, their avatar ID will be here.
    RequestingSupport(Option<u32>),
    /// If a player is called out in the message, their avatar ID will be here.
    Retreat(Option<i32>),

    /// The position is (letter,number) and zero-indexed. e.g. F2 is (5,1)
    /// `RectangleAttentionCommand`` in game code
    AttentionToSquare(u32, u32),

    /// Field is the avatar ID of the target
    /// Pair of the target type and target ID
    QuickTactic(u16, u64),
}

/// Enumerates the ribbons which appear in the top-right
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy, Serialize)]
pub enum Ribbon {
    PlaneShotDown,
    Incapacitation,
    SetFire,
    Citadel,
    SecondaryHit,
    OverPenetration,
    Penetration,
    NonPenetration,
    Ricochet,
    TorpedoProtectionHit,
    Captured,
    AssistedInCapture,
    Spotted,
    Destroyed,
    TorpedoHit,
    Defended,
    Flooding,
    DiveBombPenetration,
    RocketPenetration,
    RocketNonPenetration,
    RocketTorpedoProtectionHit,
    DepthChargeHit,
    ShotDownByAircraft,
    BuffSeized,
    SonarOneHit,
    SonarTwoHits,
    SonarNeutralized,
    Unknown(i8),
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy, Serialize)]
pub enum DeathCause {
    Secondaries,
    Artillery,
    Fire,
    Flooding,
    Torpedo,
    DiveBomber,
    AerialRocket,
    AerialTorpedo,
    Detonation,
    Ramming,
    DepthCharge,
    SkipBombs,
    Unknown(u32),
}

/// Contains the information describing a player
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerStateData {
    /// The username of this player
    pub(crate) username: String,
    /// The player's clan
    pub(crate) clan: String,
    /// The player's clan DB id
    pub(crate) clan_id: i64,
    /// The color of the player's clan tag as an RGB integer
    pub(crate) clan_color: i64,
    /// The player's DB ID (unique player ID)
    pub(crate) db_id: AccountId,
    /// The realm this player belongs to
    pub(crate) realm: String,
    /// Their avatar ID in the game
    pub(crate) avatar_id: AccountId,
    /// Their meta ID in the game (account-level identifier)
    pub(crate) meta_ship_id: AccountId,
    /// This player's entity created by a CreateEntity packet
    pub(crate) entity_id: EntityId,
    //playeravatarid: i64,
    /// Which team they're on.
    pub(crate) team_id: i64,
    /// Division ID
    pub(crate) prebattle_id: i64,
    /// Their starting health
    pub(crate) max_health: i64,
    /// ????
    pub(crate) is_abuser: bool,
    /// Has hidden stats
    pub(crate) is_hidden: bool,
    /// Has the client loaded into the game
    pub(crate) is_client_loaded: bool,
    /// Is the client connected into the game
    pub(crate) is_connected: bool,

    /// This is a raw dump (with the values converted to strings) of every key for the player.
    // TODO: Replace String with the actual pickle value (which is cleanly serializable)
    #[serde(skip_deserializing)]
    pub(crate) raw: HashMap<i64, String>,
    #[serde(skip_deserializing)]
    pub(crate) raw_with_names: HashMap<&'static str, serde_json::Value>,
}

impl PlayerStateData {
    // Key string constants for player data fields
    pub(crate) const KEY_ACCOUNT_DBID: &'static str = "accountDBID";
    pub(crate) const KEY_ANTI_ABUSE_ENABLED: &'static str = "antiAbuseEnabled";
    pub(crate) const KEY_AVATAR_ID: &'static str = "avatarId";
    pub(crate) const KEY_CAMOUFLAGE_INFO: &'static str = "camouflageInfo";
    pub(crate) const KEY_CLAN_COLOR: &'static str = "clanColor";
    pub(crate) const KEY_CLAN_ID: &'static str = "clanID";
    pub(crate) const KEY_CLAN_TAG: &'static str = "clanTag";
    pub(crate) const KEY_CREW_PARAMS: &'static str = "crewParams";
    pub(crate) const KEY_DOG_TAG: &'static str = "dogTag";
    pub(crate) const KEY_FRAGS_COUNT: &'static str = "fragsCount";
    pub(crate) const KEY_FRIENDLY_FIRE_ENABLED: &'static str = "friendlyFireEnabled";
    pub(crate) const KEY_ID: &'static str = "id";
    pub(crate) const KEY_INVITATIONS_ENABLED: &'static str = "invitationsEnabled";
    pub(crate) const KEY_IS_ABUSER: &'static str = "isAbuser";
    pub(crate) const KEY_IS_ALIVE: &'static str = "isAlive";
    pub(crate) const KEY_IS_BOT: &'static str = "isBot";
    pub(crate) const KEY_IS_CLIENT_LOADED: &'static str = "isClientLoaded";
    pub(crate) const KEY_IS_CONNECTED: &'static str = "isConnected";
    pub(crate) const KEY_IS_HIDDEN: &'static str = "isHidden";
    pub(crate) const KEY_IS_LEAVER: &'static str = "isLeaver";
    pub(crate) const KEY_IS_PRE_BATTLE_OWNER: &'static str = "isPreBattleOwner";
    pub(crate) const KEY_IS_T_SHOOTER: &'static str = "isTShooter";
    pub(crate) const KEY_KEY_TARGET_MARKERS: &'static str = "keyTargetMarkers";
    pub(crate) const KEY_KILLED_BUILDINGS_COUNT: &'static str = "killedBuildingsCount";
    pub(crate) const KEY_MAX_HEALTH: &'static str = "maxHealth";
    pub(crate) const KEY_NAME: &'static str = "name";
    pub(crate) const KEY_PLAYER_MODE: &'static str = "playerMode";
    pub(crate) const KEY_PRE_BATTLE_ID_ON_START: &'static str = "preBattleIdOnStart";
    pub(crate) const KEY_PRE_BATTLE_SIGN: &'static str = "preBattleSign";
    pub(crate) const KEY_PREBATTLE_ID: &'static str = "prebattleId";
    pub(crate) const KEY_REALM: &'static str = "realm";
    pub(crate) const KEY_SHIP_COMPONENTS: &'static str = "shipComponents";
    pub(crate) const KEY_SHIP_CONFIG_DUMP: &'static str = "shipConfigDump";
    pub(crate) const KEY_SHIP_ID: &'static str = "shipId";
    pub(crate) const KEY_SHIP_PARAMS_ID: &'static str = "shipParamsId";
    pub(crate) const KEY_SKIN_ID: &'static str = "skinId";
    pub(crate) const KEY_TEAM_ID: &'static str = "teamId";
    pub(crate) const KEY_TTK_STATUS: &'static str = "ttkStatus";

    fn convert_raw_dict(
        values: &HashMap<i64, Value>,
        version: &Version,
    ) -> HashMap<&'static str, Value> {
        let keys: HashMap<&'static str, i64> =
            if version.is_at_least(&Version::from_client_exe("0,12,8,0")) {
                let mut h = HashMap::new();
                h.insert(Self::KEY_ACCOUNT_DBID, 0);
                h.insert(Self::KEY_ANTI_ABUSE_ENABLED, 1);
                h.insert(Self::KEY_AVATAR_ID, 2);
                h.insert(Self::KEY_CAMOUFLAGE_INFO, 3);
                h.insert(Self::KEY_CLAN_COLOR, 4);
                h.insert(Self::KEY_CLAN_ID, 5);
                h.insert(Self::KEY_CLAN_TAG, 6);
                h.insert(Self::KEY_CREW_PARAMS, 7);
                h.insert(Self::KEY_DOG_TAG, 8);
                h.insert(Self::KEY_FRAGS_COUNT, 9);
                h.insert(Self::KEY_FRIENDLY_FIRE_ENABLED, 10);
                h.insert(Self::KEY_ID, 11);
                h.insert(Self::KEY_INVITATIONS_ENABLED, 12);
                h.insert(Self::KEY_IS_ABUSER, 13);
                h.insert(Self::KEY_IS_ALIVE, 14);
                h.insert(Self::KEY_IS_BOT, 15);
                h.insert(Self::KEY_IS_CLIENT_LOADED, 16);
                h.insert(Self::KEY_IS_CONNECTED, 17);
                h.insert(Self::KEY_IS_HIDDEN, 18);
                h.insert(Self::KEY_IS_LEAVER, 19);
                h.insert(Self::KEY_IS_PRE_BATTLE_OWNER, 20);
                h.insert(Self::KEY_IS_T_SHOOTER, 21);
                h.insert(Self::KEY_KEY_TARGET_MARKERS, 22);
                h.insert(Self::KEY_KILLED_BUILDINGS_COUNT, 23);
                h.insert(Self::KEY_MAX_HEALTH, 24);
                h.insert(Self::KEY_NAME, 25);
                h.insert(Self::KEY_PLAYER_MODE, 26);
                h.insert(Self::KEY_PRE_BATTLE_ID_ON_START, 27);
                h.insert(Self::KEY_PRE_BATTLE_SIGN, 28);
                h.insert(Self::KEY_PREBATTLE_ID, 29);
                h.insert(Self::KEY_REALM, 30);
                h.insert(Self::KEY_SHIP_COMPONENTS, 31);
                h.insert(Self::KEY_SHIP_CONFIG_DUMP, 32);
                h.insert(Self::KEY_SHIP_ID, 33);
                h.insert(Self::KEY_SHIP_PARAMS_ID, 34);
                h.insert(Self::KEY_SKIN_ID, 35);
                h.insert(Self::KEY_TEAM_ID, 36);
                h.insert(Self::KEY_TTK_STATUS, 37);
                h
            } else if version.is_at_least(&Version::from_client_exe("0,10,9,0")) {
                // 0.10.9 inserted things at 0x1 and 0x1F
                let mut h = HashMap::new();
                h.insert(Self::KEY_AVATAR_ID, 0x2);
                h.insert(Self::KEY_CLAN_TAG, 0x6);
                h.insert(Self::KEY_MAX_HEALTH, 0x17);
                h.insert(Self::KEY_NAME, 0x18);
                h.insert(Self::KEY_SHIP_ID, 0x20);
                h.insert(Self::KEY_SHIP_PARAMS_ID, 0x21);
                h.insert(Self::KEY_SKIN_ID, 0x22);
                h.insert(Self::KEY_TEAM_ID, 0x23);
                h
            } else if version.is_at_least(&Version::from_client_exe("0,10,7,0")) {
                // 0.10.7
                let mut h = HashMap::new();
                h.insert(Self::KEY_AVATAR_ID, 0x1);
                h.insert(Self::KEY_CLAN_TAG, 0x5);
                h.insert(Self::KEY_MAX_HEALTH, 0x16);
                h.insert(Self::KEY_NAME, 0x17);
                h.insert(Self::KEY_SHIP_ID, 0x1e);
                h.insert(Self::KEY_SHIP_PARAMS_ID, 0x1f);
                h.insert(Self::KEY_SKIN_ID, 0x20);
                h.insert(Self::KEY_TEAM_ID, 0x21);
                h
            } else {
                // 0.10.6 and earlier
                let mut h = HashMap::new();
                h.insert(Self::KEY_AVATAR_ID, 0x1);
                h.insert(Self::KEY_CLAN_TAG, 0x5);
                h.insert(Self::KEY_MAX_HEALTH, 0x15);
                h.insert(Self::KEY_NAME, 0x16);
                h.insert(Self::KEY_SHIP_ID, 0x1d);
                h.insert(Self::KEY_SHIP_PARAMS_ID, 0x1e);
                h.insert(Self::KEY_SKIN_ID, 0x1f);
                h.insert(Self::KEY_TEAM_ID, 0x20);
                h
            };

        let mut raw_with_names = HashMap::new();
        for (k, v) in values.iter() {
            if let Some(name) = keys
                .iter()
                .find_map(|(name, idx)| if *idx == *k { Some(*name) } else { None })
            {
                raw_with_names.insert(name, v.clone());
            }
        }

        raw_with_names
    }

    fn from_pickle(value: &pickled::Value, version: &Version) -> Self {
        let raw_values = convert_flat_dict_to_real_dict(value);

        let mapped_values = Self::convert_raw_dict(&raw_values, version);
        Self::from_values(raw_values, mapped_values, version)
    }

    fn from_values(
        raw_values: HashMap<i64, pickled::Value>,
        mut mapped_values: HashMap<&'static str, pickled::Value>,
        _version: &Version,
    ) -> Self {
        let avatar = *mapped_values
            .get(Self::KEY_AVATAR_ID)
            .unwrap()
            .i64_ref()
            .expect("avatarId is not an i64");

        let username = mapped_values
            .get(Self::KEY_NAME)
            .unwrap()
            .string_ref()
            .expect("name is not a string")
            .inner()
            .clone();

        let clan = mapped_values
            .get(Self::KEY_CLAN_TAG)
            .unwrap()
            .string_ref()
            .expect("clanTag is not a string")
            .inner()
            .clone();

        let clan_id = *mapped_values
            .get(Self::KEY_CLAN_ID)
            .unwrap()
            .i64_ref()
            .expect("clanID is not an i64");

        let shipid = *mapped_values
            .get(Self::KEY_SHIP_ID)
            .unwrap()
            .i64_ref()
            .expect("shipId is not an i64");
        let meta_ship_id = *mapped_values
            .get(Self::KEY_ID)
            .unwrap()
            .i64_ref()
            .expect("shipId is not an i64");
        let _playerid = *mapped_values
            .get(Self::KEY_SHIP_PARAMS_ID)
            .unwrap()
            .i64_ref()
            .expect("shipParamsId is not an i64");
        let _playeravatarid = *mapped_values
            .get(Self::KEY_SKIN_ID)
            .unwrap()
            .i64_ref()
            .expect("skinId is not an i64");
        let team = *mapped_values
            .get(Self::KEY_TEAM_ID)
            .unwrap()
            .i64_ref()
            .expect("teamId is not an i64");
        let health = *mapped_values
            .get(Self::KEY_MAX_HEALTH)
            .unwrap()
            .i64_ref()
            .expect("maxHealth is not an i64");

        let realm = mapped_values
            .get(Self::KEY_REALM)
            .unwrap()
            .string_ref()
            .expect("realm is not a string")
            .inner()
            .clone();

        let db_id = mapped_values
            .get(Self::KEY_ACCOUNT_DBID)
            .unwrap()
            .i64_ref()
            .cloned()
            .expect("accountDBID is not an i64");

        let prebattle_id = mapped_values
            .get(Self::KEY_PREBATTLE_ID)
            .unwrap()
            .i64_ref()
            .cloned()
            .expect("prebattleId is not an i64");

        let _anti_abuse_enabled = mapped_values
            .get(Self::KEY_ANTI_ABUSE_ENABLED)
            .unwrap()
            .bool_ref()
            .cloned()
            .expect("antiAbuseEnabled is not a bool");

        let is_abuser = mapped_values
            .get(Self::KEY_IS_ABUSER)
            .unwrap()
            .bool_ref()
            .cloned()
            .expect("isAbuser is not a bool");

        let is_hidden = mapped_values
            .get(Self::KEY_IS_HIDDEN)
            .unwrap()
            .bool_ref()
            .cloned()
            .expect("isHidden is not a bool");

        let is_connected = mapped_values
            .get(Self::KEY_IS_CONNECTED)
            .unwrap()
            .bool_ref()
            .cloned()
            .expect("isConnected is not a bool");

        let is_client_loaded = mapped_values
            .get(Self::KEY_IS_CLIENT_LOADED)
            .unwrap()
            .bool_ref()
            .cloned()
            .expect("isClientLoaded is not a bool");

        let clan_color = mapped_values
            .get(Self::KEY_CLAN_COLOR)
            .unwrap()
            .i64_ref()
            .cloned()
            .expect("clanColor is not an integer");

        let mut raw = HashMap::new();
        for (k, v) in raw_values.iter() {
            raw.insert(*k, format!("{:?}", v));
        }

        PlayerStateData {
            username,
            clan,
            clan_id,
            clan_color,
            realm,
            db_id: AccountId::from(db_id),
            avatar_id: AccountId::from(avatar),
            meta_ship_id: AccountId::from(meta_ship_id),
            entity_id: EntityId::from(shipid),
            team_id: team,
            max_health: health,
            is_abuser,
            is_hidden,
            raw,
            is_connected,
            is_client_loaded,
            raw_with_names: HashMap::from_iter(
                mapped_values.drain().map(|(k, v)| (k, pickle_to_json(v))),
            ),
            prebattle_id,
        }
    }

    /// Updates the PlayerStateData from a dictionary of values.
    /// Only fields present in the dictionary will be updated.
    pub fn update_from_dict(&mut self, values: &HashMap<&'static str, pickled::Value>) {
        if let Some(v) = values.get(Self::KEY_AVATAR_ID) {
            if let Some(id) = v.i64_ref() {
                self.avatar_id = AccountId::from(*id);
            }
        }
        if let Some(v) = values.get(Self::KEY_NAME) {
            if let Some(s) = v.string_ref() {
                self.username = s.inner().clone();
            }
        }
        if let Some(v) = values.get(Self::KEY_CLAN_TAG) {
            if let Some(s) = v.string_ref() {
                self.clan = s.inner().clone();
            }
        }
        if let Some(v) = values.get(Self::KEY_CLAN_ID) {
            if let Some(id) = v.i64_ref() {
                self.clan_id = *id;
            }
        }
        if let Some(v) = values.get(Self::KEY_CLAN_COLOR) {
            if let Some(id) = v.i64_ref() {
                self.clan_color = *id;
            }
        }
        if let Some(v) = values.get(Self::KEY_SHIP_ID) {
            if let Some(id) = v.i64_ref() {
                self.entity_id = EntityId::from(*id);
            }
        }
        if let Some(v) = values.get(Self::KEY_ID) {
            if let Some(id) = v.i64_ref() {
                self.meta_ship_id = AccountId::from(*id);
            }
        }
        if let Some(v) = values.get(Self::KEY_TEAM_ID) {
            if let Some(id) = v.i64_ref() {
                self.team_id = *id;
            }
        }
        if let Some(v) = values.get(Self::KEY_MAX_HEALTH) {
            if let Some(id) = v.i64_ref() {
                self.max_health = *id;
            }
        }
        if let Some(v) = values.get(Self::KEY_REALM) {
            if let Some(s) = v.string_ref() {
                self.realm = s.inner().clone();
            }
        }
        if let Some(v) = values.get(Self::KEY_ACCOUNT_DBID) {
            if let Some(id) = v.i64_ref() {
                self.db_id = AccountId::from(*id);
            }
        }
        if let Some(v) = values.get(Self::KEY_PREBATTLE_ID) {
            if let Some(id) = v.i64_ref() {
                self.prebattle_id = *id;
            }
        }
        if let Some(v) = values.get(Self::KEY_IS_ABUSER) {
            if let Some(b) = v.bool_ref() {
                self.is_abuser = *b;
            }
        }
        if let Some(v) = values.get(Self::KEY_IS_HIDDEN) {
            if let Some(b) = v.bool_ref() {
                self.is_hidden = *b;
            }
        }
        if let Some(v) = values.get(Self::KEY_IS_CONNECTED) {
            if let Some(b) = v.bool_ref() {
                self.is_connected = *b;
            }
        }
        if let Some(v) = values.get(Self::KEY_IS_CLIENT_LOADED) {
            if let Some(b) = v.bool_ref() {
                self.is_client_loaded = *b;
            }
        }

        // Update raw_with_names with any new values
        for (k, v) in values.iter() {
            self.raw_with_names.insert(k, pickle_to_json(v.clone()));
        }
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn clan(&self) -> &str {
        &self.clan
    }

    pub fn clan_id(&self) -> i64 {
        self.clan_id
    }

    pub fn clan_color(&self) -> i64 {
        self.clan_color
    }

    pub fn db_id(&self) -> AccountId {
        self.db_id
    }

    pub fn realm(&self) -> &str {
        &self.realm
    }

    pub fn avatar_id(&self) -> AccountId {
        self.avatar_id
    }

    pub fn meta_ship_id(&self) -> AccountId {
        self.meta_ship_id
    }

    pub fn entity_id(&self) -> EntityId {
        self.entity_id
    }

    pub fn team_id(&self) -> i64 {
        self.team_id
    }

    pub fn division_id(&self) -> i64 {
        self.prebattle_id
    }

    pub fn max_health(&self) -> i64 {
        self.max_health
    }

    pub fn is_abuser(&self) -> bool {
        self.is_abuser
    }

    pub fn is_hidden(&self) -> bool {
        self.is_hidden
    }

    pub fn is_client_loaded(&self) -> bool {
        self.is_client_loaded
    }

    pub fn is_connected(&self) -> bool {
        self.is_connected
    }

    pub fn raw(&self) -> &HashMap<i64, String> {
        &self.raw
    }

    pub fn raw_with_names(&self) -> &HashMap<&'static str, serde_json::Value> {
        &self.raw_with_names
    }
}

/// Converts a list of key-value pairs to a real dictionary
fn convert_flat_dict_to_real_dict(value: &Value) -> HashMap<i64, Value> {
    let mut raw_values = HashMap::new();
    if let pickled::value::Value::List(elements) = value {
        for elem in elements.inner().iter() {
            if let pickled::value::Value::Tuple(kv) = elem {
                let key = kv.inner()[0]
                    .i64_ref()
                    .expect("tuple first value was not an integer");

                raw_values.insert(key.clone(), kv.inner()[1].clone());
            }
        }
    }

    raw_values
}

/// Indicates that the given attacker has dealt damage
#[derive(Debug, Clone, Serialize)]
pub struct DamageReceived {
    /// Ship ID of the aggressor
    pub aggressor: EntityId,
    /// Amount of damage dealt
    pub damage: f32,
}

/// Sent to update the minimap display
#[derive(Debug, Clone, Serialize)]
pub struct MinimapUpdate {
    /// The ship ID of the ship to update
    pub entity_id: EntityId,
    /// Set to true if the ship should disappear from the minimap (false otherwise)
    pub disappearing: bool,
    /// The heading of the ship. Unit is degrees, 0 is up, positive is clockwise
    /// (so 90.0 is East)
    pub heading: f32,
    /// Normalized position on the minimap
    pub position: NormalizedPos,
    /// Unknown, but this appears to be something related to the big hunt
    pub unknown: bool,
}

/// A single shell in an artillery salvo
#[derive(Debug, Clone, Serialize)]
pub struct ArtilleryShotData {
    pub origin: (f32, f32, f32),
    pub target: (f32, f32, f32),
    pub shot_id: u32,
    pub speed: f32,
}

/// A salvo of artillery shells from one ship
#[derive(Debug, Clone, Serialize)]
pub struct ArtillerySalvo {
    pub owner_id: EntityId,
    pub params_id: GameParamId,
    pub salvo_id: u32,
    pub shots: Vec<ArtilleryShotData>,
}

/// A single torpedo launch
#[derive(Debug, Clone, Serialize)]
pub struct TorpedoData {
    pub owner_id: EntityId,
    pub params_id: GameParamId,
    pub salvo_id: u32,
    pub shot_id: u32,
    pub origin: (f32, f32, f32),
    pub direction: (f32, f32, f32),
}

/// A single projectile hit (from receiveShotKills)
#[derive(Debug, Clone, Serialize)]
pub struct ShotHit {
    pub owner_id: EntityId,
    pub shot_id: u32,
}

/// Enumerates usable consumables in-game
#[derive(Debug, Clone, Copy, Serialize)]
pub enum Consumable {
    DamageControl,
    SpottingAircraft,
    DefensiveAntiAircraft,
    SpeedBoost,
    RepairParty,
    CatapultFighter,
    MainBatteryReloadBooster,
    TorpedoReloadBooster,
    Smoke,
    Radar,
    HydroacousticSearch,
    Hydrophone,
    EnhancedRudders,
    ReserveBattery,
    Unknown(i8),
}

#[derive(Debug, Clone, Copy, Serialize)]
pub enum CameraMode {
    OverheadMap,
    FollowingShells,
    FollowingPlanes,
    FollowingShip,
    FollowingSubmarine,
    FreeFlying,
    Unknown(u32),
}

/// Enumerates the "cruise states". See <https://github.com/lkolbly/wows-replays/issues/14#issuecomment-976784004>
/// for more information.
#[derive(Debug, Clone, Copy, Serialize)]
pub enum CruiseState {
    /// Possible values for the throttle range from -1 for reverse to 4 for full power ahead.
    Throttle,
    /// Note that not all rudder changes are indicated via cruise states, only ones
    /// set via the Q & E keys. Temporarily setting the rudder will not trigger this
    /// packet.
    ///
    /// Possible associated values are:
    /// - -2: Full rudder to port,
    /// - -1: Half rudder to port,
    /// - 0: Neutral
    /// - 1: Half rudder to starboard,
    /// - 2: Full rudder to starboard.
    Rudder,
    /// Sets the dive depth. Known values are:
    /// - 0: 0m
    /// - 1: -6m (periscope depth)
    /// - 2: -18m
    /// - 3: -30m
    /// - 4: -42m
    /// - 5: -54m
    /// - 6: -66m
    /// - 7: -80m
    DiveDepth,
    /// Indicates an unknown cruise state. Send me your replay!
    Unknown(u32),
}

#[derive(Debug, Serialize)]
pub struct ChatMessageExtra {
    pre_battle_sign: i64,
    pre_battle_id: i64,
    player_clan_tag: String,
    typ: i64,
    player_avatar_id: AccountId,
    player_name: String,
}

#[derive(Debug, Serialize, Kinded)]
#[kinded(derive(Serialize))]
pub enum DecodedPacketPayload<'replay, 'argtype, 'rawpacket> {
    /// Represents a chat message. Note that this only includes text chats, voicelines
    /// are represented by the VoiceLine variant.
    Chat {
        entity_id: EntityId,
        /// Avatar ID of the sender
        sender_id: AccountId,
        /// Represents the audience for the chat: Division, team, or all.
        audience: &'replay str,
        /// The actual chat message.
        message: &'replay str,
        /// Extra data that may be present if sender_id is 0
        extra_data: Option<ChatMessageExtra>,
    },
    /// Sent when a voice line is played (for example, "Wilco!")
    VoiceLine {
        /// Avatar ID of the player sending the voiceline
        sender_id: AccountId,
        /// True if the voiceline is visible in all chat, false if only in team chat
        is_global: bool,
        /// Which voiceline it is.
        message: VoiceLine,
    },
    /// Sent when the player earns a ribbon
    Ribbon(Ribbon),
    /// Indicates the position of the given object.
    Position(crate::packet2::PositionPacket),
    /// Indicates the position of the player's object or camera.
    PlayerOrientation(crate::packet2::PlayerOrientationPacket),
    /// Indicates updating a damage statistic. The first tuple, `(i64,i64)`, is a two-part
    /// label indicating what type of damage this refers to. The second tuple, `(i64,f64)`,
    /// indicates the actual damage counter increment.
    ///
    /// Some known keys include:
    /// - (1, 0) key is (# AP hits that dealt damage, total AP damage dealt)
    /// - (1, 3) is (# artillery fired, total possible damage) ?
    /// - (2, 0) is (# HE penetrations, total HE damage)
    /// - (17, 0) is (# fire tick marks, total fire damage)
    DamageStat(Vec<((i64, i64), (i64, f64))>),
    /// Sent when a ship is destroyed.
    ShipDestroyed {
        /// The ship ID (note: Not the avatar ID) of the killer
        killer: EntityId,
        /// The ship ID (note: Not the avatar ID) of the victim
        victim: EntityId,
        /// Cause of death
        cause: DeathCause,
    },
    EntityMethod(&'rawpacket EntityMethodPacket<'argtype>),
    EntityProperty(&'rawpacket crate::packet2::EntityPropertyPacket<'argtype>),
    BasePlayerCreate(&'rawpacket crate::packet2::BasePlayerCreatePacket<'argtype>),
    CellPlayerCreate(&'rawpacket crate::packet2::CellPlayerCreatePacket<'argtype>),
    EntityEnter(&'rawpacket crate::packet2::EntityEnterPacket),
    EntityLeave(&'rawpacket crate::packet2::EntityLeavePacket),
    EntityCreate(&'rawpacket crate::packet2::EntityCreatePacket<'argtype>),
    /// Contains all of the info required to setup the arena state and show the initial loading screen.
    OnArenaStateReceived {
        /// Unknown
        arena_id: i64,
        /// Unknown
        team_build_type_id: i8,
        /// Unknown
        pre_battles_info: HashMap<i64, Vec<Option<HashMap<String, String>>>>,
        /// A list of the players in this game
        player_states: Vec<PlayerStateData>,
    },
    /// Contains info when the arena state changes
    OnGameRoomStateChanged {
        /// Updated player states
        player_states: Vec<HashMap<&'static str, pickled::Value>>,
    },
    CheckPing(u64),
    /// Indicates that the given victim has received damage from one or more attackers.
    DamageReceived {
        /// Ship ID of the ship being damaged
        victim: EntityId,
        /// List of damages happening to this ship
        aggressors: Vec<DamageReceived>,
    },
    /// Contains data for a minimap update
    MinimapUpdate {
        /// A list of the updates to make to the minimap
        updates: Vec<MinimapUpdate>,
        /// Unknown
        arg1: &'rawpacket Vec<ArgValue<'argtype>>,
    },
    /// Indicates a property update. Note that many properties contain a hierarchy of properties,
    /// for example the "state" property on the battle manager contains nested dictionaries and
    /// arrays. The top-level entity and property are specified by the `entity_id` and `property`
    /// fields. The nesting structure and how to modify the leaves are indicated by the
    /// `update_cmd` field.
    ///
    /// Within the `update_cmd` field is two fields, `levels` and `action`. `levels` indicates how
    /// to traverse to the leaf property, for example by following a dictionary key or array index.
    /// `action` indicates what action to perform once there, such as setting a subproperty to
    /// a specific value.
    ///
    /// For example, to set the `state[controlPoints][0][hasInvaders]` property, you will see a
    /// packet payload that looks like:
    /// ```ignore
    /// {
    ///     "entity_id": 576258,
    ///     "property": "state",
    ///     "update_cmd": {
    ///         "levels": [
    ///             {"DictKey": "controlPoints"},
    ///             {"ArrayIndex": 0}
    ///         ],
    ///         "action": {
    ///             "SetKey":{"key":"hasInvaders","value":1}
    ///         }
    ///     }
    /// }
    /// ```
    /// This says to take the "state" property on entity 576258, navigate to `state["controlPoints"][0]`,
    /// and set the sub-key `hasInvaders` there to 1.
    ///
    /// The following properties and values are known:
    /// - `state["controlPoints"][N]["invaderTeam"]`: Indicates the team ID of the team currently
    ///   contesting the control point. -1 if nobody is invading point.
    /// - `state["controlPoints"][N]["hasInvaders"]`: 1 if the point is being contested, 0 otherwise.
    /// - `state["controlPoints"][N]["progress"]`: A tuple of two elements. The first is the fraction
    ///   captured, ranging from 0 to 1 as the point is captured, and the second is the amount of
    ///   time remaining until the point is captured.
    /// - `state["controlPoints"][N]["bothInside"]`: 1 if both teams are currently in point, 0 otherwise.
    /// - `state["missions"]["teamsScore"][N]["score"]`: The value of team N's score.
    PropertyUpdate(&'rawpacket crate::packet2::PropertyUpdatePacket<'argtype>),
    /// Indicates that the battle has ended
    BattleEnd {
        /// The team ID of the winning team (corresponds to the teamid in [OnArenaStateReceivedPlayer])
        winning_team: Option<i8>,
        /// Unknown
        // TODO: Probably how the game was won? (time expired, score, or ships destroyed)
        state: Option<u8>,
    },
    /// Sent when a consumable is activated
    Consumable {
        /// The ship ID of the ship using the consumable
        entity: EntityId,
        /// The consumable
        consumable: Consumable,
        /// How long the consumable will be active for
        duration: f32,
    },
    /// Indicates a change to the "cruise state," which is the fixed settings for various controls
    /// such as steering (using the Q & E keys), throttle, and dive planes.
    CruiseState {
        /// Which cruise state is being affected
        state: CruiseState,
        /// See [CruiseState] for what the values mean.
        value: i32,
    },
    Map(&'rawpacket crate::packet2::MapPacket<'replay>),
    /// A string representation of the game version this replay is from.
    Version(String),
    Camera(&'rawpacket crate::packet2::CameraPacket),
    /// Indicates a change in the current camera mode
    CameraMode(CameraMode),
    /// If true, indicates that the player has enabled the "free look" camera (by holding right click)
    CameraFreeLook(bool),
    /// Artillery shells fired
    ArtilleryShots {
        entity_id: EntityId,
        salvos: Vec<ArtillerySalvo>,
    },
    /// Torpedoes launched
    TorpedoesReceived {
        entity_id: EntityId,
        torpedoes: Vec<TorpedoData>,
    },
    /// Projectile hits (shells or torpedoes hitting targets)
    ShotKills {
        entity_id: EntityId,
        hits: Vec<ShotHit>,
    },
    /// Turret rotation sync for a ship
    GunSync {
        entity_id: EntityId,
        /// Gun group (0 = main battery)
        group: u32,
        /// Turret index within the group
        turret: u32,
        /// Turret yaw in radians relative to ship heading (0 = forward, PI = aft)
        yaw: f32,
        /// Barrel elevation in radians
        pitch: f32,
    },
    /// A new squadron appears on the minimap
    PlaneAdded {
        entity_id: EntityId,
        plane_id: PlaneId,
        /// Team index: 0 = recording player's team, 1 = enemy team
        team_id: u32,
        params_id: GameParamId,
        x: f32,
        y: f32,
    },
    /// A squadron is removed from the minimap
    PlaneRemoved {
        entity_id: EntityId,
        plane_id: PlaneId,
    },
    /// Plane/squadron position update on the minimap
    PlanePosition {
        entity_id: EntityId,
        plane_id: PlaneId,
        x: f32,
        y: f32,
    },
    /// This is a packet of unknown type
    Unknown(&'replay [u8]),
    /// This is a packet of known type, but which we were unable to parse
    Invalid(&'rawpacket crate::packet2::InvalidPacket<'replay>),
    /// If parsing with audits enabled, this indicates a packet that may be of special interest
    /// for whoever is reading the audits.
    Audit(String),
    /// End of battle results (free xp, damage details, etc.)
    BattleResults(&'replay str),
    /*
    ArtilleryHit(ArtilleryHitPacket<'a>),
    */
}

fn try_convert_hashable_pickle_to_string(
    value: pickled::value::HashableValue,
) -> pickled::value::HashableValue {
    match value {
        pickled::value::HashableValue::Bytes(b) => {
            if let Ok(s) = std::str::from_utf8(&b.inner()) {
                pickled::value::HashableValue::String(s.to_owned().into())
            } else {
                pickled::value::HashableValue::Bytes(b)
            }
        }
        pickled::value::HashableValue::Tuple(t) => pickled::value::HashableValue::Tuple(
            t.inner()
                .iter()
                .cloned()
                .map(try_convert_hashable_pickle_to_string)
                .collect::<Vec<_>>()
                .into(),
        ),
        pickled::value::HashableValue::FrozenSet(s) => pickled::value::HashableValue::FrozenSet(
            s.inner()
                .iter()
                .cloned()
                .map(try_convert_hashable_pickle_to_string)
                .collect::<BTreeSet<_>>()
                .into(),
        ),
        value => value,
    }
}

/// Helper function to recursively convert byte values to strings where possible.
fn try_convert_pickle_to_string(value: pickled::value::Value) -> pickled::value::Value {
    match value {
        pickled::value::Value::Bytes(b) => {
            if let Ok(s) = std::str::from_utf8(&b.inner()) {
                pickled::value::Value::String(s.to_owned().into())
            } else {
                pickled::value::Value::Bytes(b)
            }
        }
        pickled::value::Value::List(l) => pickled::value::Value::List(
            l.inner()
                .iter()
                .cloned()
                .map(try_convert_pickle_to_string)
                .collect::<Vec<_>>()
                .into(),
        ),
        pickled::value::Value::Tuple(t) => pickled::value::Value::Tuple(
            t.inner()
                .iter()
                .cloned()
                .map(try_convert_pickle_to_string)
                .collect::<Vec<_>>()
                .into(),
        ),
        pickled::value::Value::Set(s) => pickled::value::Value::Set(
            s.inner()
                .iter()
                .cloned()
                .map(try_convert_hashable_pickle_to_string)
                .collect::<BTreeSet<_>>()
                .into(),
        ),
        pickled::value::Value::FrozenSet(s) => pickled::value::Value::FrozenSet(
            s.inner()
                .iter()
                .cloned()
                .map(try_convert_hashable_pickle_to_string)
                .collect::<BTreeSet<_>>()
                .into(),
        ),
        pickled::value::Value::Dict(d) => pickled::value::Value::Dict(
            d.inner()
                .iter()
                .map(|(k, v)| {
                    (
                        try_convert_hashable_pickle_to_string(k.clone()),
                        try_convert_pickle_to_string(v.clone()),
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>()
                .into(),
        ),
        value => value,
    }
}

fn parse_receive_common_cmd_blob(blob: &[u8]) -> IResult<&[u8], (VoiceLine, bool)> {
    let i = blob;
    let (i, line) = le_u16(i)?;
    let (i, audience) = le_u8(i)?;

    // if !matches!(line, 2 | 13 | 16 | 15 | 19) {
    //     panic!("{:#X?}", blob);
    // }

    let is_global = match audience {
        0 => false,
        1 => true,
        _ => {
            panic!("Got unknown audience {}", audience);
        }
    };
    let (i, message) = match line {
        1 => {
            let (i, x) = le_u16(i)?;
            let (i, y) = le_u16(i)?;
            (i, VoiceLine::AttentionToSquare(x as u32, y as u32))
        }
        2 => {
            let (i, target_type) = le_u16(i)?;
            let (i, target_id) = le_u64(i)?;
            (i, VoiceLine::QuickTactic(target_type, target_id))
        }
        3 => (i, VoiceLine::RequestingSupport(None)),
        // 4 is "QUICK_SOS"
        // 5 is AYE_AYE
        5 => (i, VoiceLine::Wilco),
        // 6 is NO_WAY
        6 => (i, VoiceLine::Negative),
        // GOOD_GAME
        7 => (i, VoiceLine::WellDone), // TODO: Find the corresponding field
        // GOOD_LUCK
        8 => (i, VoiceLine::FairWinds),
        // CARAMBA
        9 => (i, VoiceLine::Curses),
        // 10 -> THANK_YOU
        10 => (i, VoiceLine::DefendTheBase),
        // 11 -> NEED_AIR_DEFENSE
        11 => (i, VoiceLine::ProvideAntiAircraft),
        // BACK
        12 => {
            let (i, _target_type) = le_u16(i)?;
            let (i, target_id) = le_u64(i)?;
            (
                i,
                VoiceLine::Retreat(if target_id != 0 {
                    Some(target_id as i32)
                } else {
                    None
                }),
            )
        }
        // NEED_VISION
        13 => (i, VoiceLine::IntelRequired),
        // NEED_SMOKE
        14 => (i, VoiceLine::SetSmokeScreen),
        // RLS
        15 => (i, VoiceLine::UsingRadar),
        // SONAR
        16 => (i, VoiceLine::UsingHydroSearch),
        // FOLLOW_ME
        17 => (i, VoiceLine::FollowMe),
        // MAP_POINT_ATTENTION
        18 => {
            let (i, x) = le_f32(i)?;
            let (i, y) = le_f32(i)?;
            (i, VoiceLine::MapPointAttention(x, y))
        }
        //  SUBMARINE_LOCATOR
        19 => (i, VoiceLine::UsingSubmarineLocator),
        line => {
            panic!("Unknown voice line {}, {:#X?}", line, i);
        }
    };

    Ok((i, (message, is_global)))
}

impl<'replay, 'argtype, 'rawpacket> DecodedPacketPayload<'replay, 'argtype, 'rawpacket>
where
    'rawpacket: 'replay,
    'rawpacket: 'argtype,
{
    fn from(
        version: &Version,
        audit: bool,
        payload: &'rawpacket crate::packet2::PacketType<'replay, 'argtype>,
        packet_type: u32,
    ) -> Self {
        match payload {
            PacketType::EntityMethod(em) => {
                DecodedPacketPayload::from_entity_method(version, audit, em)
            }
            PacketType::Camera(camera) => DecodedPacketPayload::Camera(camera),
            PacketType::CameraMode(mode) => match mode {
                3 => DecodedPacketPayload::CameraMode(CameraMode::OverheadMap),
                5 => DecodedPacketPayload::CameraMode(CameraMode::FollowingShells),
                6 => DecodedPacketPayload::CameraMode(CameraMode::FollowingPlanes),
                8 => DecodedPacketPayload::CameraMode(CameraMode::FollowingShip),
                9 => DecodedPacketPayload::CameraMode(CameraMode::FreeFlying),
                11 => DecodedPacketPayload::CameraMode(CameraMode::FollowingSubmarine),
                _ => {
                    if audit {
                        DecodedPacketPayload::Audit(format!("CameraMode({})", mode))
                    } else {
                        DecodedPacketPayload::CameraMode(CameraMode::Unknown(*mode))
                    }
                }
            },
            PacketType::CameraFreeLook(freelook) => match freelook {
                0 => DecodedPacketPayload::CameraFreeLook(false),
                1 => DecodedPacketPayload::CameraFreeLook(true),
                _ => {
                    if audit {
                        DecodedPacketPayload::Audit(format!("CameraFreeLook({})", freelook))
                    } else {
                        DecodedPacketPayload::CameraFreeLook(true)
                    }
                }
            },
            PacketType::CruiseState(cs) => match cs.key {
                0 => DecodedPacketPayload::CruiseState {
                    state: CruiseState::Throttle,
                    value: cs.value,
                },
                1 => DecodedPacketPayload::CruiseState {
                    state: CruiseState::Rudder,
                    value: cs.value,
                },
                2 => DecodedPacketPayload::CruiseState {
                    state: CruiseState::DiveDepth,
                    value: cs.value,
                },
                _ => {
                    if audit {
                        DecodedPacketPayload::Audit(format!(
                            "CruiseState(unknown={}, {})",
                            cs.key, cs.value
                        ))
                    } else {
                        DecodedPacketPayload::CruiseState {
                            state: CruiseState::Unknown(cs.key),
                            value: cs.value,
                        }
                    }
                }
            },
            PacketType::Map(map) => {
                if audit && map.unknown != 0 && map.unknown != 1 {
                    DecodedPacketPayload::Audit(format!(
                        "Map: Unknown bool is not a bool (is {})",
                        map.unknown
                    ))
                } else if audit
                    && map.matrix
                        != [
                            0, 0, 128, 63, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                            128, 63, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 63,
                            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 63,
                        ]
                {
                    DecodedPacketPayload::Audit(format!(
                        "Map: Unit matrix is not a unit matrix (is {:?})",
                        map.matrix
                    ))
                } else {
                    DecodedPacketPayload::Map(map)
                }
            }
            PacketType::EntityProperty(p) => DecodedPacketPayload::EntityProperty(p),
            PacketType::Position(pos) => DecodedPacketPayload::Position((*pos).clone()),
            PacketType::PlayerOrientation(pos) => {
                DecodedPacketPayload::PlayerOrientation((*pos).clone())
            }
            PacketType::BasePlayerCreate(b) => DecodedPacketPayload::BasePlayerCreate(b),
            PacketType::CellPlayerCreate(c) => DecodedPacketPayload::CellPlayerCreate(c),
            PacketType::EntityEnter(e) => DecodedPacketPayload::EntityEnter(e),
            PacketType::EntityLeave(e) => DecodedPacketPayload::EntityLeave(e),
            PacketType::EntityCreate(e) => DecodedPacketPayload::EntityCreate(e),
            PacketType::PropertyUpdate(update) => DecodedPacketPayload::PropertyUpdate(update),
            PacketType::Version(version) => DecodedPacketPayload::Version(version.clone()),
            PacketType::Unknown(u) => {
                if packet_type == 0x18 {
                    if audit
                        && u != &[
                            00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00,
                            00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00,
                            00, 00, 00, 00, 00, 00, 0x80, 0xbf, 00, 00, 0x80, 0xbf, 00, 00, 0x80,
                            0xbf,
                        ]
                    {
                        DecodedPacketPayload::Audit("Camera18 unexpected value!".to_string())
                    } else {
                        DecodedPacketPayload::Unknown(u)
                    }
                } else {
                    DecodedPacketPayload::Unknown(u)
                }
            }
            PacketType::Invalid(u) => DecodedPacketPayload::Invalid(u),
            PacketType::BattleResults(results) => DecodedPacketPayload::BattleResults(results),
        }
    }

    fn extract_vec3(val: Option<&ArgValue>) -> (f32, f32, f32) {
        match val {
            Some(ArgValue::Vector3((x, y, z))) => (*x, *y, *z),
            Some(ArgValue::Array(a)) if a.len() >= 3 => {
                let x: f32 = (&a[0]).try_into().unwrap_or(0.0);
                let y: f32 = (&a[1]).try_into().unwrap_or(0.0);
                let z: f32 = (&a[2]).try_into().unwrap_or(0.0);
                (x, y, z)
            }
            _ => (0.0, 0.0, 0.0),
        }
    }

    fn from_entity_method(
        version: &Version,
        audit: bool,
        packet: &'rawpacket EntityMethodPacket<'argtype>,
    ) -> Self {
        let entity_id = &packet.entity_id;
        let method = &packet.method;
        let args = &packet.args;
        if *method == "onChatMessage" {
            let target = match &args[1] {
                ArgValue::String(s) => s,
                _ => panic!("foo"),
            };
            let message = match &args[2] {
                ArgValue::String(s) => s,
                _ => panic!("foo"),
            };
            let sender_id = match &args[0] {
                ArgValue::Int32(i) => i,
                _ => panic!("foo"),
            };
            let mut extra_data = None;
            if *sender_id == 0 && args.len() >= 4 {
                let extra = pickled::de::value_from_slice(
                    args[3].string_ref().expect("failed"),
                    pickled::de::DeOptions::new(),
                )
                .expect("value is not pickled");
                let mut extra_dict: HashMap<String, Value> = HashMap::from_iter(
                    extra
                        .dict()
                        .expect("value is not a dictionary")
                        .inner()
                        .iter()
                        .map(|(key, value)| {
                            let key = match key {
                                pickled::HashableValue::Bytes(bytes) => {
                                    String::from_utf8(bytes.inner().clone())
                                        .expect("key is not a valid utf-8 sequence")
                                }
                                pickled::HashableValue::String(string) => string.inner().clone(),
                                other => {
                                    panic!("unexpected key type {:?}", other)
                                }
                            };

                            let value = match value {
                                Value::Bytes(bytes) => {
                                    if let Ok(result) = String::from_utf8(bytes.inner().clone()) {
                                        Value::String(result.into())
                                    } else {
                                        Value::Bytes(bytes.clone())
                                    }
                                }
                                other => other.clone(),
                            };

                            (key, value)
                        }),
                );

                let extra = ChatMessageExtra {
                    pre_battle_sign: extra_dict
                        .remove("preBattleSign")
                        .unwrap()
                        .i64()
                        .expect("preBattleSign is not an i64"),
                    pre_battle_id: extra_dict
                        .remove("prebattleId")
                        .unwrap()
                        .i64()
                        .expect("preBattleId is not an i64"),
                    player_clan_tag: extra_dict
                        .remove("playerClanTag")
                        .unwrap()
                        .string()
                        .expect("playerClanTag is not a string")
                        .inner()
                        .clone(),
                    typ: extra_dict
                        .remove("type")
                        .unwrap()
                        .i64()
                        .expect("type is not an i64"),
                    player_avatar_id: AccountId::from(
                        extra_dict
                            .remove("playerAvatarId")
                            .unwrap()
                            .i64()
                            .expect("playerAvatarId is not an i64"),
                    ),
                    player_name: extra_dict
                        .remove("playerName")
                        .unwrap()
                        .string()
                        .expect("playerName is not a string")
                        .inner()
                        .clone(),
                };

                assert!(extra_dict.is_empty());

                extra_data = Some(extra);
            }
            DecodedPacketPayload::Chat {
                entity_id: *entity_id,
                sender_id: AccountId::from(*sender_id),
                audience: std::str::from_utf8(target).unwrap(),
                message: std::str::from_utf8(message).unwrap(),
                extra_data,
            }
        } else if *method == "receive_CommonCMD" {
            let (sender_id, message, is_global) =
                if version.is_at_least(&Version::from_client_exe("0,12,8,0")) {
                    let sender = *args[0]
                        .int_32_ref()
                        .expect("receive_CommonCMD: sender is not an i32");

                    let blob = args[1]
                        .blob_ref()
                        .expect("receive_CommonCMD: second argument is not a blob");

                    let (_reminader, (message_type, is_global)) =
                        parse_receive_common_cmd_blob(blob.as_ref())
                            .expect("receive_CommonCMD: failed to parse blob");

                    (sender, message_type, is_global)
                } else {
                    let (audience, sender_id, line, a, b) =
                        unpack_rpc_args!(args, u8, i32, u8, u32, u64);
                    let is_global = match audience {
                        0 => false,
                        1 => true,
                        _ => {
                            panic!(
                                "Got unknown audience {} sender=0x{:x} line={} a={:x} b={:x}",
                                audience, sender_id, line, a, b
                            );
                        }
                    };
                    let message = match line {
                        1 => VoiceLine::AttentionToSquare(a, b as u32),
                        2 => VoiceLine::QuickTactic(a as u16, b),
                        3 => VoiceLine::RequestingSupport(None),
                        5 => VoiceLine::Wilco,
                        6 => VoiceLine::Negative,
                        7 => VoiceLine::WellDone, // TODO: Find the corresponding field
                        8 => VoiceLine::FairWinds,
                        9 => VoiceLine::Curses,
                        10 => VoiceLine::DefendTheBase,
                        11 => VoiceLine::ProvideAntiAircraft,
                        12 => VoiceLine::Retreat(if b != 0 { Some(b as i32) } else { None }),
                        13 => VoiceLine::IntelRequired,
                        14 => VoiceLine::SetSmokeScreen,
                        15 => VoiceLine::UsingRadar,
                        16 => VoiceLine::UsingHydroSearch,
                        17 => VoiceLine::FollowMe,
                        18 => VoiceLine::MapPointAttention(a as f32, b as f32),
                        19 => VoiceLine::UsingSubmarineLocator,
                        _ => {
                            panic!("Unknown voice line {} a={:x} b={:x}!", line, a, b);
                        }
                    };

                    (sender_id, message, is_global)
                };

            // let (audience, sender_id, line, a, b) = unpack_rpc_args!(args, u8, i32, u8, u32, u64);

            DecodedPacketPayload::VoiceLine {
                sender_id: AccountId::from(sender_id),
                is_global,
                message,
            }
        } else if *method == "onGameRoomStateChanged" {
            let player_states = pickled::de::value_from_slice(
                &args[0].blob_ref().expect("player_states arg is not a blob"),
                pickled::de::DeOptions::new(),
            )
            .expect("failed to deserialize player_states");

            let player_states = try_convert_pickle_to_string(player_states);

            let mut players_out = vec![];
            if let pickled::value::Value::List(players) = &player_states {
                for player in players.inner().iter() {
                    let raw_values = convert_flat_dict_to_real_dict(player);

                    let mapped_values = PlayerStateData::convert_raw_dict(&raw_values, version);
                    players_out.push(mapped_values);
                }
            }
            DecodedPacketPayload::OnGameRoomStateChanged {
                player_states: players_out,
            }
        } else if *method == "onArenaStateReceived" {
            let (arg0, arg1) = unpack_rpc_args!(args, i64, i8);

            let value = pickled::de::value_from_slice(
                match &args[2] {
                    ArgValue::Blob(x) => x,
                    _ => panic!("foo"),
                },
                pickled::de::DeOptions::new(),
            )
            .unwrap();

            let value = match value {
                pickled::value::Value::Dict(d) => d,
                _ => panic!(),
            };
            let mut arg2 = HashMap::new();
            for (k, v) in value.inner().iter() {
                let k = match k {
                    pickled::value::HashableValue::I64(i) => *i,
                    _ => panic!(),
                };
                let v = match v {
                    pickled::value::Value::List(l) => l,
                    _ => panic!(),
                };
                let v: Vec<_> = v
                    .inner()
                    .iter()
                    .map(|elem| match elem {
                        pickled::value::Value::Dict(d) => Some(
                            d.inner()
                                .iter()
                                .map(|(k, v)| {
                                    let k = match k {
                                        pickled::value::HashableValue::Bytes(b) => {
                                            std::str::from_utf8(&b.inner()).unwrap().to_string()
                                        }
                                        _ => panic!(),
                                    };
                                    let v = format!("{:?}", v);
                                    (k, v)
                                })
                                .collect(),
                        ),
                        pickled::value::Value::None => None,
                        _ => panic!(),
                    })
                    .collect();
                arg2.insert(k, v);
            }

            let value = pickled::de::value_from_slice(
                match &args[3] {
                    ArgValue::Blob(x) => x,
                    _ => panic!("foo"),
                },
                pickled::de::DeOptions::new(),
            )
            .unwrap();
            let value = try_convert_pickle_to_string(value);

            let mut players_out = vec![];
            if let pickled::value::Value::List(players) = &value {
                for player in players.inner().iter() {
                    players_out.push(PlayerStateData::from_pickle(player, version));
                }
            }
            DecodedPacketPayload::OnArenaStateReceived {
                arena_id: arg0,
                team_build_type_id: arg1,
                pre_battles_info: arg2,
                player_states: players_out,
            }
        } else if *method == "receiveDamageStat" {
            let value = pickled::de::value_from_slice(
                match &args[0] {
                    ArgValue::Blob(x) => x,
                    _ => panic!("foo"),
                },
                pickled::de::DeOptions::new(),
            )
            .unwrap();

            let mut stats = vec![];
            match value {
                pickled::value::Value::Dict(d) => {
                    for (k, v) in d.inner().iter() {
                        let k = match k {
                            pickled::value::HashableValue::Tuple(t) => {
                                let t = t.inner();
                                assert!(t.len() == 2);
                                (
                                    match &t[0] {
                                        pickled::value::HashableValue::I64(i) => *i,
                                        _ => panic!("foo"),
                                    },
                                    match &t[1] {
                                        pickled::value::HashableValue::I64(i) => *i,
                                        _ => panic!("foo"),
                                    },
                                )
                            }
                            _ => panic!("foo"),
                        };
                        let v = match v {
                            pickled::value::Value::List(t) => {
                                let t = t.inner();
                                assert!(t.len() == 2);
                                (
                                    match &t[0] {
                                        pickled::value::Value::I64(i) => *i,
                                        _ => panic!("foo"),
                                    },
                                    match &t[1] {
                                        pickled::value::Value::F64(i) => *i,
                                        // TODO: This appears in the (17,2) key,
                                        // it is unknown what it means
                                        pickled::value::Value::I64(i) => *i as f64,
                                        _ => panic!("foo"),
                                    },
                                )
                            }
                            _ => panic!("foo"),
                        };
                        //println!("{:?}: {:?}", k, v);

                        stats.push((k, v));
                    }
                }
                _ => panic!("foo"),
            }
            DecodedPacketPayload::DamageStat(stats)
        } else if *method == "receiveVehicleDeath" {
            let (victim, killer, cause) = unpack_rpc_args!(args, i32, i32, u32);
            let cause = match cause {
                2 => DeathCause::Secondaries,
                3 => DeathCause::Torpedo,
                4 => DeathCause::DiveBomber,
                5 => DeathCause::AerialTorpedo,
                6 => DeathCause::Fire,
                7 => DeathCause::Ramming,
                9 => DeathCause::Flooding,
                13 => DeathCause::DepthCharge,
                14 => DeathCause::AerialRocket,
                15 => DeathCause::Detonation,
                17 => DeathCause::Artillery,
                18 => DeathCause::Artillery,
                19 => DeathCause::Artillery,
                22 => DeathCause::SkipBombs,
                28 => DeathCause::DepthCharge, // TODO: Why is this different from the above depth charge?
                cause => {
                    if audit {
                        return DecodedPacketPayload::Audit(format!(
                            "receiveVehicleDeath(victim={}, killer={}, unknown cause {})",
                            victim, killer, cause
                        ));
                    } else {
                        DeathCause::Unknown(cause)
                    }
                }
            };
            DecodedPacketPayload::ShipDestroyed {
                victim: EntityId::from(victim),
                killer: EntityId::from(killer),
                cause,
            }
        } else if *method == "onRibbon" {
            let (ribbon,) = unpack_rpc_args!(args, i8);
            let ribbon = match ribbon {
                1 => Ribbon::TorpedoHit,
                3 => Ribbon::PlaneShotDown,
                4 => Ribbon::Incapacitation,
                5 => Ribbon::Destroyed,
                6 => Ribbon::SetFire,
                7 => Ribbon::Flooding,
                8 => Ribbon::Citadel,
                9 => Ribbon::Defended,
                10 => Ribbon::Captured,
                11 => Ribbon::AssistedInCapture,
                13 => Ribbon::SecondaryHit,
                14 => Ribbon::OverPenetration,
                15 => Ribbon::Penetration,
                16 => Ribbon::NonPenetration,
                17 => Ribbon::Ricochet,
                19 => Ribbon::Spotted,
                21 => Ribbon::DiveBombPenetration,
                25 => Ribbon::RocketPenetration,
                26 => Ribbon::RocketNonPenetration,
                27 => Ribbon::ShotDownByAircraft,
                28 => Ribbon::TorpedoProtectionHit,
                30 => Ribbon::RocketTorpedoProtectionHit,
                31 => Ribbon::DepthChargeHit,
                33 => Ribbon::BuffSeized,
                39 => Ribbon::SonarOneHit,
                40 => Ribbon::SonarTwoHits,
                41 => Ribbon::SonarNeutralized,
                ribbon => {
                    if audit {
                        return DecodedPacketPayload::Audit(format!(
                            "onRibbon(unknown ribbon {})",
                            ribbon
                        ));
                    } else {
                        Ribbon::Unknown(ribbon)
                    }
                }
            };
            DecodedPacketPayload::Ribbon(ribbon)
        } else if *method == "receiveDamagesOnShip" {
            let mut v = vec![];
            for elem in match &args[0] {
                ArgValue::Array(a) => a,
                _ => panic!(),
            } {
                let map = match elem {
                    ArgValue::FixedDict(m) => m,
                    _ => panic!(),
                };
                let aggressor_raw: i32 = map.get("vehicleID").unwrap().try_into().unwrap();
                v.push(DamageReceived {
                    aggressor: EntityId::from(aggressor_raw),
                    damage: map.get("damage").unwrap().try_into().unwrap(),
                });
            }
            DecodedPacketPayload::DamageReceived {
                victim: *entity_id,
                aggressors: v,
            }
        } else if *method == "onCheckGamePing" {
            let (ping,) = unpack_rpc_args!(args, u64);
            DecodedPacketPayload::CheckPing(ping)
        } else if *method == "updateMinimapVisionInfo" {
            let v = match &args[0] {
                ArgValue::Array(a) => a,
                _ => panic!(),
            };
            let mut updates = vec![];
            for minimap_update in v.iter() {
                let minimap_update = match minimap_update {
                    ArgValue::FixedDict(m) => m,
                    _ => panic!(),
                };
                let vehicle_id = minimap_update.get("vehicleID").unwrap();

                let packed_data: u32 = minimap_update
                    .get("packedData")
                    .unwrap()
                    .try_into()
                    .unwrap();
                let update = RawMinimapUpdate::from_bytes(packed_data.to_le_bytes());
                let heading = update.heading() as f32 / 256. * 360. - 180.;

                let x = update.x() as f32 / 512. - 1.5;
                let y = update.y() as f32 / 512. - 1.5;

                updates.push(MinimapUpdate {
                    entity_id: match vehicle_id {
                        ArgValue::Uint32(u) => EntityId::from(*u),
                        _ => panic!(),
                    },
                    position: NormalizedPos { x, y },
                    heading,
                    disappearing: update.is_disappearing(),
                    unknown: update.unknown(),
                })
            }

            let args1 = match &args[1] {
                ArgValue::Array(a) => a,
                _ => panic!(),
            };

            DecodedPacketPayload::MinimapUpdate {
                updates,
                arg1: args1,
            }
        } else if *method == "onBattleEnd" {
            let (winning_team, state) =
                if version.is_at_least(&Version::from_client_exe("0,12,8,0")) {
                    (None, None)
                } else {
                    let (winning_team, unknown) = unpack_rpc_args!(args, i8, u8);
                    (Some(winning_team), Some(unknown))
                };
            DecodedPacketPayload::BattleEnd {
                winning_team,
                state,
            }
        } else if *method == "consumableUsed" {
            let (consumable, duration) = unpack_rpc_args!(args, i8, f32);
            let raw_consumable = consumable;
            let consumable = match consumable {
                0 => Consumable::DamageControl,
                1 => Consumable::SpottingAircraft,
                2 => Consumable::DefensiveAntiAircraft,
                3 => Consumable::SpeedBoost,
                5 => Consumable::MainBatteryReloadBooster,
                7 => Consumable::Smoke,
                9 => Consumable::RepairParty,
                10 => Consumable::CatapultFighter,
                11 => Consumable::HydroacousticSearch,
                12 => Consumable::TorpedoReloadBooster,
                13 => Consumable::Radar,
                35 => Consumable::Hydrophone,
                36 => Consumable::EnhancedRudders,
                37 => Consumable::ReserveBattery,
                _ => {
                    if audit {
                        return DecodedPacketPayload::Audit(format!(
                            "consumableUsed({},{},{})",
                            entity_id, raw_consumable, duration
                        ));
                    } else {
                        Consumable::Unknown(consumable)
                    }
                }
            };
            DecodedPacketPayload::Consumable {
                entity: *entity_id,
                consumable,
                duration,
            }
        } else if *method == "receiveArtilleryShots" {
            let salvos_array = match &args[0] {
                ArgValue::Array(a) => a,
                _ => return DecodedPacketPayload::EntityMethod(packet),
            };
            let mut salvos = Vec::new();
            for salvo_val in salvos_array.iter() {
                let salvo_dict = match salvo_val {
                    ArgValue::FixedDict(m) => m,
                    _ => continue,
                };
                let owner_id: i32 = salvo_dict
                    .get("ownerID")
                    .and_then(|v| v.try_into().ok())
                    .unwrap_or(0);
                let params_id: u32 = salvo_dict
                    .get("paramsID")
                    .and_then(|v| v.try_into().ok())
                    .unwrap_or(0);
                let salvo_id: u32 = salvo_dict
                    .get("salvoID")
                    .and_then(|v| v.try_into().ok())
                    .unwrap_or(0);
                let shots_array = match salvo_dict.get("shots") {
                    Some(ArgValue::Array(a)) => a,
                    _ => continue,
                };
                let mut shots = Vec::new();
                for shot_val in shots_array.iter() {
                    let shot_dict = match shot_val {
                        ArgValue::FixedDict(m) => m,
                        _ => continue,
                    };
                    let pos = Self::extract_vec3(shot_dict.get("pos"));
                    let tar_pos = Self::extract_vec3(shot_dict.get("tarPos"));
                    let shot_id: u32 = shot_dict
                        .get("shotID")
                        .and_then(|v| v.try_into().ok())
                        .unwrap_or(0);
                    let speed: f32 = shot_dict
                        .get("speed")
                        .and_then(|v| v.try_into().ok())
                        .unwrap_or(0.0);
                    shots.push(ArtilleryShotData {
                        origin: pos,
                        target: tar_pos,
                        shot_id,
                        speed,
                    });
                }
                salvos.push(ArtillerySalvo {
                    owner_id: EntityId::from(owner_id),
                    params_id: GameParamId::from(params_id),
                    salvo_id,
                    shots,
                });
            }
            DecodedPacketPayload::ArtilleryShots {
                entity_id: *entity_id,
                salvos,
            }
        } else if *method == "receiveTorpedoes" {
            let salvos_array = match &args[0] {
                ArgValue::Array(a) => a,
                _ => return DecodedPacketPayload::EntityMethod(packet),
            };
            let mut torpedoes = Vec::new();
            for salvo_val in salvos_array.iter() {
                let salvo_dict = match salvo_val {
                    ArgValue::FixedDict(m) => m,
                    _ => continue,
                };
                let owner_id: i32 = salvo_dict
                    .get("ownerID")
                    .and_then(|v| v.try_into().ok())
                    .unwrap_or(0);
                let params_id: u32 = salvo_dict
                    .get("paramsID")
                    .and_then(|v| v.try_into().ok())
                    .unwrap_or(0);
                let salvo_id: u32 = salvo_dict
                    .get("salvoID")
                    .and_then(|v| v.try_into().ok())
                    .unwrap_or(0);
                let torps_array = match salvo_dict.get("torpedoes") {
                    Some(ArgValue::Array(a)) => a,
                    _ => continue,
                };
                for torp_val in torps_array.iter() {
                    let torp_dict = match torp_val {
                        ArgValue::FixedDict(m) => m,
                        _ => continue,
                    };
                    let pos = Self::extract_vec3(torp_dict.get("pos"));
                    let dir = Self::extract_vec3(torp_dict.get("dir"));
                    let shot_id: u32 = torp_dict
                        .get("shotID")
                        .and_then(|v| v.try_into().ok())
                        .unwrap_or(0);
                    torpedoes.push(TorpedoData {
                        owner_id: EntityId::from(owner_id),
                        params_id: GameParamId::from(params_id),
                        salvo_id,
                        shot_id,
                        origin: pos,
                        direction: dir,
                    });
                }
            }
            DecodedPacketPayload::TorpedoesReceived {
                entity_id: *entity_id,
                torpedoes,
            }
        } else if *method == "receiveShotKills" {
            // SHOTKILLS_PACK: Array of { ownerID: PLAYER_ID, hitType: UINT8, kills: Array<SHOTKILL> }
            // SHOTKILL: { pos: VECTOR3, shotID: SHOT_ID }
            let packs = match &args[0] {
                ArgValue::Array(a) => a,
                _ => return DecodedPacketPayload::EntityMethod(packet),
            };
            let mut hits = Vec::new();
            for pack in packs {
                let pack_dict = match pack {
                    ArgValue::FixedDict(d) => d,
                    _ => continue,
                };
                let owner_id: i32 = pack_dict
                    .get("ownerID")
                    .and_then(|v| v.try_into().ok())
                    .unwrap_or(0);
                let kills_array = match pack_dict.get("kills") {
                    Some(ArgValue::Array(a)) => a,
                    _ => continue,
                };
                for kill in kills_array {
                    let kill_dict = match kill {
                        ArgValue::FixedDict(d) => d,
                        _ => continue,
                    };
                    let shot_id: u32 = kill_dict
                        .get("shotID")
                        .and_then(|v| v.try_into().ok())
                        .unwrap_or(0);
                    hits.push(ShotHit {
                        owner_id: EntityId::from(owner_id),
                        shot_id,
                    });
                }
            }
            DecodedPacketPayload::ShotKills {
                entity_id: *entity_id,
                hits,
            }
        } else if *method == "receive_addMinimapSquadron" {
            // args: [plane_id, team_id, params_id, position, unknown]
            let plane_id: PlaneId = match &args[0] {
                ArgValue::Uint64(v) => PlaneId::from(*v),
                ArgValue::Int64(v) => PlaneId::from(*v),
                ArgValue::Uint32(v) => PlaneId::from(*v as u64),
                ArgValue::Int32(v) => PlaneId::from(*v as i64),
                _ => return DecodedPacketPayload::EntityMethod(packet),
            };
            let team_id: u32 = match &args[1] {
                ArgValue::Uint32(v) => *v,
                ArgValue::Int32(v) => *v as u32,
                ArgValue::Uint64(v) => *v as u32,
                ArgValue::Int64(v) => *v as u32,
                ArgValue::Uint8(v) => *v as u32,
                ArgValue::Int8(v) => *v as u32,
                _ => return DecodedPacketPayload::EntityMethod(packet),
            };
            let params_id: u64 = match &args[2] {
                ArgValue::Uint64(v) => *v,
                ArgValue::Int64(v) => *v as u64,
                ArgValue::Uint32(v) => *v as u64,
                ArgValue::Int32(v) => *v as u64,
                _ => return DecodedPacketPayload::EntityMethod(packet),
            };
            let position = match &args[3] {
                ArgValue::Array(a) if a.len() >= 2 => {
                    let x: f32 = (&a[0]).try_into().unwrap_or(0.0);
                    let y: f32 = (&a[1]).try_into().unwrap_or(0.0);
                    (x, y)
                }
                ArgValue::Vector2((x, y)) => (*x, *y),
                _ => return DecodedPacketPayload::EntityMethod(packet),
            };
            DecodedPacketPayload::PlaneAdded {
                entity_id: *entity_id,
                plane_id,
                team_id,
                params_id: GameParamId::from(params_id),
                x: position.0,
                y: position.1,
            }
        } else if *method == "receive_removeMinimapSquadron" {
            let plane_id: PlaneId = match &args[0] {
                ArgValue::Uint64(v) => PlaneId::from(*v),
                ArgValue::Int64(v) => PlaneId::from(*v),
                ArgValue::Uint32(v) => PlaneId::from(*v as u64),
                ArgValue::Int32(v) => PlaneId::from(*v as i64),
                _ => return DecodedPacketPayload::EntityMethod(packet),
            };
            DecodedPacketPayload::PlaneRemoved {
                entity_id: *entity_id,
                plane_id,
            }
        } else if *method == "receive_updateMinimapSquadron" {
            let plane_id: PlaneId = match &args[0] {
                ArgValue::Uint64(v) => PlaneId::from(*v),
                ArgValue::Int64(v) => PlaneId::from(*v),
                ArgValue::Uint32(v) => PlaneId::from(*v as u64),
                ArgValue::Int32(v) => PlaneId::from(*v as i64),
                _ => return DecodedPacketPayload::EntityMethod(packet),
            };
            let position = match &args[1] {
                ArgValue::Array(a) if a.len() >= 2 => {
                    let x: f32 = (&a[0]).try_into().unwrap_or(0.0);
                    let y: f32 = (&a[1]).try_into().unwrap_or(0.0);
                    (x, y)
                }
                ArgValue::Vector2((x, y)) => (*x, *y),
                _ => return DecodedPacketPayload::EntityMethod(packet),
            };
            DecodedPacketPayload::PlanePosition {
                entity_id: *entity_id,
                plane_id,
                x: position.0,
                y: position.1,
            }
        } else if *method == "syncGun" {
            // args: [group: int, turret: int, yaw: f32, pitch: f32, state: int, f32, array]
            let group = match &args[0] {
                ArgValue::Uint8(v) => *v as u32,
                ArgValue::Int8(v) => *v as u32,
                _ => return DecodedPacketPayload::EntityMethod(packet),
            };
            let turret = match &args[1] {
                ArgValue::Uint8(v) => *v as u32,
                ArgValue::Int8(v) => *v as u32,
                _ => return DecodedPacketPayload::EntityMethod(packet),
            };
            let yaw = match &args[2] {
                ArgValue::Float32(v) => *v,
                _ => return DecodedPacketPayload::EntityMethod(packet),
            };
            let pitch = match &args[3] {
                ArgValue::Float32(v) => *v,
                _ => return DecodedPacketPayload::EntityMethod(packet),
            };
            DecodedPacketPayload::GunSync {
                entity_id: *entity_id,
                group,
                turret,
                yaw,
                pitch,
            }
        } else {
            DecodedPacketPayload::EntityMethod(packet)
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DecodedPacket<'replay, 'argtype, 'rawpacket> {
    pub packet_type: u32,
    pub clock: crate::types::GameClock,
    pub payload: DecodedPacketPayload<'replay, 'argtype, 'rawpacket>,
}

impl<'replay, 'argtype, 'rawpacket> DecodedPacket<'replay, 'argtype, 'rawpacket>
where
    'rawpacket: 'replay,
    'rawpacket: 'argtype,
{
    pub fn from(version: &Version, audit: bool, packet: &'rawpacket Packet<'_, '_>) -> Self {
        Self {
            clock: packet.clock,
            packet_type: packet.packet_type,
            payload: DecodedPacketPayload::from(
                version,
                audit,
                &packet.payload,
                packet.packet_type,
            ),
        }
    }
}

struct Decoder {
    silent: bool,
    output: Option<Box<dyn std::io::Write>>,
    version: Version,
}

impl Decoder {
    fn write(&mut self, line: &str) {
        if !self.silent {
            match self.output.as_mut() {
                Some(f) => {
                    writeln!(f, "{}", line).unwrap();
                }
                None => {
                    println!("{}", line);
                }
            }
        }
    }
}

#[allow(dead_code)]
mod minimap_update {
    use modular_bitfield::prelude::*;

    #[bitfield]
    pub(super) struct RawMinimapUpdate {
        pub x: B11,
        pub y: B11,
        pub heading: B8,
        pub unknown: bool,
        pub is_disappearing: bool,
    }
}
use minimap_update::RawMinimapUpdate;

impl Analyzer for Decoder {
    fn finish(&mut self) {}

    fn process(&mut self, packet: &Packet<'_, '_>) {
        let decoded = DecodedPacket::from(&self.version, false, packet);
        //println!("{:#?}", decoded);
        //println!("{}", serde_json::to_string_pretty(&decoded).unwrap());
        let encoded = serde_json::to_string(&decoded).unwrap();
        self.write(&encoded);
    }
}
