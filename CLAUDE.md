# wows-replays

World of Warships replay parser written in Rust. Parses binary `.wowsreplay` files into structured data including player info, damage, frags, chat, positions, and minimap state.

## Project Structure

Cargo workspace with three active crates:

- **`parser/`** — Core library. Decrypts, decompresses, and parses replay packets. Contains analyzers that process packet streams.
- **`analysis/`** — Higher-level analysis tools (damage trails, ship trails). Requires `graphics` feature.
- **`replayshark/`** — CLI application for replay analysis. Commands: `dump`, `survey`, `chat`, `summary`, `investigate`, `search`, `spec`.

## Data Flow Pipeline

```
ReplayFile::from_file()          [parser/src/wowsreplay.rs]
  -> Blowfish decrypt + Zlib decompress
  -> raw packet bytes

Parser::parse_packets_mut()      [parser/src/packet2.rs]
  -> parse_packet() for each packet
  -> Packet { packet_type, clock, payload: PacketType }

DecodedPacket::from()            [parser/src/analyzer/decoder.rs]
  -> PacketType -> DecodedPacketPayload enum
  -> Semantic events: Chat, Position, ShipDestroyed, MinimapUpdate, etc.

BattleController::process_mut()  [parser/src/analyzer/battle_controller/controller.rs]
  -> Accumulates state: players, entities, damage, frags, timeline
  -> build_report() -> BattleReport
```

## Key Types and Locations

### Packet Layer (`parser/src/packet2.rs`)
- `Packet` — Raw parsed packet with `clock: f32`, `packet_type: u32`, `payload: PacketType`
- `PacketType` — Enum: Position, EntityMethod, EntityCreate, EntityProperty, PropertyUpdate, etc.
- `PositionPacket` — `{ pid, position: Vec3, rotation: Rot3 }`
- `EntityMethodPacket` — `{ entity_id, method: &str, args: Vec<ArgValue> }`
- `PropertyUpdatePacket` — `{ entity_id, property: &str, update_cmd: PropertyNesting }`

### Decoder Layer (`parser/src/analyzer/decoder.rs`)
- `DecodedPacketPayload` — High-level enum with ~25 variants
- Key variants: `Chat`, `VoiceLine`, `Ribbon`, `Position`, `ShipDestroyed`, `DamageReceived`, `MinimapUpdate`, `PropertyUpdate`, `Consumable`, `OnArenaStateReceived`, `OnGameRoomStateChanged`, `BattleEnd`, `BattleResults`
- `PlayerStateData` — Player info parsed from pickled Python data
- `MinimapUpdate` — Packed binary position data (11-bit x/y, 8-bit heading)
- `DeathCause`, `Ribbon`, `Consumable`, `VoiceLine` — Game event enums

### Battle Controller (`parser/src/analyzer/battle_controller/`)
- `controller.rs` — Main `BattleController` struct. Implements `AnalyzerMut` trait.
- `timeline.rs` — `GameTimeline` (append-only event log), `TimelineEvent` enum, `GameClock`
- `state.rs` — Snapshot types: `ShipPosition`, `MinimapPosition`, `CapturePointState`, `TeamScore`, `ActiveConsumable`, `BuildingEntity`, `SmokeScreenEntity`
- `observer.rs` — `BattleObserver` trait (extension point for per-tick consumers)

### Key Structs
- `BattleController<'res, 'replay, G: ResourceLoader>` — Stateful analyzer that tracks the full game
- `BattleReport` — Final output: players, frags, damage, chat, timeline, capture points, scores, buildings
- `Player` — Initial + end state, vehicle entity, relation, connection changes
- `VehicleEntity` — Ship entity with `VehicleProps` (50+ properties), captain, damage, death info
- `Entity` — Enum: `Vehicle`, `Building`, `SmokeScreen`

### Property Updates (`parser/src/nested_property_path.rs`)
- `PropertyNestLevel` — `ArrayIndex(usize)` or `DictKey(&str)`
- `UpdateAction` — `SetKey`, `SetRange`, `SetElement`, `RemoveRange`
- Used for capture point state and team scores: `state -> controlPoints -> [N] -> SetKey{hasInvaders|progress|...}`

## Analyzer Pattern

Pluggable analyzers process the same packet stream independently:

```rust
trait AnalyzerMut {
    fn process_mut(&mut self, packet: &Packet<'_, '_>);
    fn finish(&mut self);
}
```

`AnalyzerAdapter` wraps multiple analyzers. Available: `BattleController`, `Decoder`, `Summary`, `ChatLogger`.

## Game Data Access

Uses `wowsunpack` crate with `ResourceLoader` trait for virtual filesystem access to game files (no data dumps needed):
- `game_param_by_id(id)` — Ship/crew parameters
- `localized_name_from_id(id)` — Localized strings

Game data loaded from `<game_dir>/bin/<build>/idx/` index files + `<game_dir>/res_packages/` packages.

## Entity Method Names (confirmed via replays)

| Method | Description |
|--------|-------------|
| `onChatMessage` | Chat messages |
| `receive_CommonCMD` | Voice lines |
| `onRibbon` | Ribbon earned |
| `receiveVehicleDeath` | Ship destroyed |
| `receiveDamagesOnShip` | Damage received |
| `receiveDamageStat` | Damage statistics |
| `updateMinimapVisionInfo` | Minimap position updates |
| `consumableUsed` | Consumable activated |
| `onArenaStateReceived` | Initial player list |
| `onGameRoomStateChanged` | Player state updates |
| `onBattleEnd` | Battle end |
| `receiveArtilleryShots` | Shell trajectories (origin, dest, params) |
| `receiveTorpedoes` | Torpedo launches (pos, dir, params, owner) |
| `receive_updateMinimapSquadron` | Plane positions on minimap |
| `receive_updateSquadron` | Squadron state updates |

## Building and Testing

```bash
cargo build                    # Build all crates
cargo run -p replayshark -- -g <game_dir> dump <replay.wowsreplay>        # Dump decoded packets
cargo run -p replayshark -- -g <game_dir> survey <replay.wowsreplay>      # Parse validation
cargo run -p replayshark -- -g <game_dir> investigate --filter-method <name> <replay>  # Inspect methods
cargo run -p replayshark -- -g <game_dir> investigate --filter-packet 0x8 <replay>     # Inspect packet type
```

Game directory example: `E:\WoWs\World_of_Warships\`

## Feature: `arc`

When enabled, uses `Arc<T>` instead of `Rc<T>` for thread-safe reference counting. Controlled via `parser/Cargo.toml`.

## Future Work (Minimap Renderer Parity)

Features tracked in timeline but not yet decoded:
- **Artillery shots** — `receiveArtilleryShots` needs decoder variant
- **Torpedoes** — `receiveTorpedoes` needs decoder variant (has pos, dir, ownerID, paramsID, shotID)
- **Planes** — `receive_updateMinimapSquadron` needs decoder variant (has squadron ID + position)
- **Acoustic torpedoes** — `receiveTorpedoDirection` for homing torpedo updates
