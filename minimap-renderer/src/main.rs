use anyhow::anyhow;
use clap::{App, Arg};
use std::fs::File;
use std::path::Path;
use wowsunpack::data::Version;
use wowsunpack::game_data;
use wowsunpack::game_params::provider::GameMetadataProvider;

use wows_replays::ReplayFile;
use wows_replays::analyzer::Analyzer;
use wows_replays::analyzer::battle_controller::BattleController;
use wows_replays::game_constants::GameConstants;

use wows_minimap_renderer::assets::{
    load_consumable_icons, load_map_image, load_map_info, load_plane_icons, load_ship_icons,
};
use wows_minimap_renderer::drawing::ImageTarget;
use wows_minimap_renderer::renderer::{MinimapRenderer, RenderOptions};
use wows_minimap_renderer::video::{DumpMode, VideoEncoder};

fn main() -> anyhow::Result<()> {
    let matches = App::new("Minimap Renderer")
        .about("Generates a minimap timelapse video from a WoWS replay")
        .arg(
            Arg::with_name("GAME_DIRECTORY")
                .help("Path to the World of Warships game directory")
                .short("g")
                .long("game")
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::with_name("OUTPUT")
                .help("Output MP4 file path")
                .short("o")
                .long("output")
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::with_name("DUMP_FRAME")
                .help("Dump a single frame as PNG instead of rendering video (specify frame number or 'mid' for midpoint)")
                .long("dump-frame")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("NO_PLAYER_NAMES")
                .help("Hide player names above ship icons")
                .long("no-player-names"),
        )
        .arg(
            Arg::with_name("NO_SHIP_NAMES")
                .help("Hide ship names above ship icons")
                .long("no-ship-names"),
        )
        .arg(
            Arg::with_name("NO_CAPTURE_POINTS")
                .help("Hide capture point zones")
                .long("no-capture-points"),
        )
        .arg(
            Arg::with_name("NO_BUILDINGS")
                .help("Hide building markers")
                .long("no-buildings"),
        )
        .arg(
            Arg::with_name("NO_TURRET_DIRECTION")
                .help("Hide turret direction indicators")
                .long("no-turret-direction"),
        )
        .arg(
            Arg::with_name("REPLAY")
                .help("The replay file to process")
                .required(true)
                .index(1),
        )
        .get_matches();

    let game_dir = matches.value_of("GAME_DIRECTORY").unwrap();
    let output = matches.value_of("OUTPUT").unwrap();
    let replay_path = matches.value_of("REPLAY").unwrap();

    let dump_mode = match matches.value_of("DUMP_FRAME") {
        Some("mid") => Some(DumpMode::Midpoint),
        Some(n) => Some(DumpMode::Frame(
            n.parse::<usize>().expect("invalid frame number"),
        )),
        None => None,
    };

    println!("Parsing replay...");
    let replay_file = ReplayFile::from_file(&std::path::PathBuf::from(replay_path))?;
    let replay_version = Version::from_client_exe(&replay_file.meta.clientVersionFromExe);

    println!("Loading game data for build {}...", replay_version.build);
    let wows_dir = Path::new(game_dir);
    let resources =
        game_data::load_game_resources(wows_dir, &replay_version).map_err(|e| anyhow!("{}", e))?;
    let file_tree = &resources.file_tree;
    let pkg_loader = &resources.pkg_loader;
    let specs = &resources.specs;

    println!("Loading game params...");
    let mut game_params = GameMetadataProvider::from_pkg(file_tree, pkg_loader)
        .map_err(|e| anyhow!("Failed to load GameParams: {:?}", e))?;
    let controller_game_params = GameMetadataProvider::from_pkg(file_tree, pkg_loader)
        .map_err(|e| anyhow!("Failed to load GameParams for controller: {:?}", e))?;

    // Load translations for ship name localization
    let mo_path = game_data::translations_path(wows_dir, replay_version.build);
    if mo_path.exists() {
        let catalog = gettext::Catalog::parse(File::open(&mo_path)?)
            .map_err(|e| anyhow!("Failed to parse global.mo: {:?}", e))?;
        game_params.set_translations(catalog);
    } else {
        eprintln!(
            "Warning: translations not found at {:?}, ship names will be unavailable",
            mo_path
        );
    }

    println!("Loading icons...");
    let ship_icons = load_ship_icons(file_tree, pkg_loader);
    let plane_icons = load_plane_icons(file_tree, pkg_loader);
    let consumable_icons = load_consumable_icons(file_tree, pkg_loader);

    // Load game constants from game data (falls back to hardcoded defaults per-field)
    let game_constants = GameConstants::from_pkg(file_tree, pkg_loader);

    if let Some(mode_name) = game_constants.game_mode_name(replay_file.meta.gameMode as i32) {
        println!("Game mode: {} ({})", mode_name, replay_file.meta.gameMode);
    }

    // Load map image and metadata from game files
    let map_name = &replay_file.meta.mapName;
    let map_image = load_map_image(map_name, &file_tree, &pkg_loader);
    let map_info = load_map_info(map_name, &file_tree, &pkg_loader);

    let game_duration = replay_file.meta.duration as f32;

    let mut target = ImageTarget::new(map_image, ship_icons, plane_icons, consumable_icons);

    let mut options = RenderOptions::default();
    options.show_player_names = !matches.is_present("NO_PLAYER_NAMES");
    options.show_ship_names = !matches.is_present("NO_SHIP_NAMES");
    options.show_capture_points = !matches.is_present("NO_CAPTURE_POINTS");
    options.show_buildings = !matches.is_present("NO_BUILDINGS");
    options.show_turret_direction = !matches.is_present("NO_TURRET_DIRECTION");

    let mut renderer = MinimapRenderer::new(map_info.clone(), &game_params, options);
    let mut encoder = VideoEncoder::new(output, dump_mode, game_duration);

    let mut controller = BattleController::new(
        &replay_file.meta,
        &controller_game_params,
        Some(&game_constants),
    );

    let mut parser = wows_replays::packet2::Parser::new(&specs);
    let mut remaining = &replay_file.packet_data[..];
    let mut prev_clock = wows_replays::types::GameClock(0.0);

    while !remaining.is_empty() {
        let (rest, packet) = parser
            .parse_packet(remaining)
            .map_err(|e| anyhow!("Packet parse error: {:?}", e))?;
        remaining = rest;

        // Render when clock changes (all prev_clock packets have been processed)
        if packet.clock != prev_clock && prev_clock.seconds() > 0.0 {
            renderer.populate_players(&controller);
            renderer.update_squadron_info(&controller);
            renderer.update_ship_abilities(&controller);
            encoder.advance_clock(prev_clock, &controller, &mut renderer, &mut target);
            prev_clock = packet.clock;
        } else if prev_clock.seconds() == 0.0 {
            prev_clock = packet.clock;
        }

        // Process the packet to update state
        controller.process(&packet);
    }

    // Render final tick
    if prev_clock.seconds() > 0.0 {
        renderer.populate_players(&controller);
        renderer.update_squadron_info(&controller);
        renderer.update_ship_abilities(&controller);
        encoder.advance_clock(prev_clock, &controller, &mut renderer, &mut target);
    }

    controller.finish();
    encoder.finish(&controller, &mut renderer, &mut target)?;

    println!("Done!");
    Ok(())
}
