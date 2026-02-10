use anyhow::{anyhow, Context};
use clap::{App, Arg};
use std::borrow::Cow;
use std::fs::read_dir;
use std::io::Cursor;
use std::path::Path;
use wowsunpack::data::idx::{self, FileNode};
use wowsunpack::data::pkg::PkgFileLoader;
use wowsunpack::data::DataFileWithCallback;
use wowsunpack::game_params::provider::GameMetadataProvider;
use wowsunpack::rpc::entitydefs::{parse_scripts, EntitySpec};

use wows_replays::analyzer::battle_controller::BattleController;
use wows_replays::analyzer::Analyzer;
use wows_replays::ReplayFile;

use minimap_renderer::assets::{load_map_image, load_map_info, load_plane_icons, load_ship_icons};
use minimap_renderer::drawing::ImageTarget;
use minimap_renderer::renderer::{MinimapRenderer, RenderOptions};
use minimap_renderer::video::{DumpMode, VideoEncoder};

fn load_game_resources(
    game_dir: &str,
) -> anyhow::Result<(Vec<EntitySpec>, FileNode, PkgFileLoader)> {
    let wows_directory = Path::new(game_dir);

    let mut idx_files = Vec::new();
    let mut latest_build: Option<usize> = None;
    for file in read_dir(wows_directory.join("bin"))? {
        let file = file?;
        if file.file_type()?.is_file() {
            continue;
        }
        if let Some(build_num) = file
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<usize>().ok())
        {
            if latest_build.map(|n| n < build_num).unwrap_or(true) {
                latest_build = Some(build_num);
            }
        }
    }

    let latest_build =
        latest_build.ok_or_else(|| anyhow!("Could not determine latest WoWs build"))?;

    for file in read_dir(
        wows_directory
            .join("bin")
            .join(latest_build.to_string())
            .join("idx"),
    )
    .context("failed to read idx directory")?
    {
        let file = file?;
        if file.file_type()?.is_file() {
            let file_data = std::fs::read(file.path())?;
            let mut cursor = Cursor::new(file_data.as_slice());
            idx_files.push(idx::parse(&mut cursor)?);
        }
    }

    let pkgs_path = wows_directory.join("res_packages");
    if !pkgs_path.exists() {
        return Err(anyhow!("Invalid wows directory -- res_packages not found"));
    }

    let pkg_loader = PkgFileLoader::new(pkgs_path);
    let file_tree = idx::build_file_tree(idx_files.as_slice());

    let specs = {
        let loader = DataFileWithCallback::new(|path| {
            let path = Path::new(path);
            let mut file_data = Vec::new();
            file_tree
                .read_file_at_path(path, &pkg_loader, &mut file_data)
                .unwrap();
            Ok(Cow::Owned(file_data))
        });
        parse_scripts(&loader)?
    };

    Ok((specs, file_tree, pkg_loader))
}

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

    println!("Loading game data...");
    let (specs, file_tree, pkg_loader) = load_game_resources(game_dir)?;

    println!("Loading game params...");
    let game_params = GameMetadataProvider::from_pkg(&file_tree, &pkg_loader)
        .map_err(|e| anyhow!("Failed to load GameParams: {:?}", e))?;
    let controller_game_params = GameMetadataProvider::from_pkg(&file_tree, &pkg_loader)
        .map_err(|e| anyhow!("Failed to load GameParams for controller: {:?}", e))?;

    println!("Loading ship icons...");
    let ship_icons = load_ship_icons(&file_tree, &pkg_loader);
    let plane_icons = load_plane_icons(&file_tree, &pkg_loader);

    println!("Parsing replay...");
    let replay_file = ReplayFile::from_file(&std::path::PathBuf::from(replay_path))?;

    // Load map image and metadata from game files
    let map_name = &replay_file.meta.mapName;
    let map_image = load_map_image(map_name, &file_tree, &pkg_loader);
    let map_info = load_map_info(map_name, &file_tree, &pkg_loader);

    let game_duration = replay_file.meta.duration as f32;

    let mut target = ImageTarget::new(map_image, ship_icons, plane_icons);

    let mut renderer =
        MinimapRenderer::new(map_info.clone(), game_params, RenderOptions::default());
    let mut encoder = VideoEncoder::new(output, dump_mode, game_duration);

    let mut controller = BattleController::new(&replay_file.meta, &controller_game_params);

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
        encoder.advance_clock(prev_clock, &controller, &mut renderer, &mut target);
    }

    controller.finish();
    encoder.finish(&controller, &mut renderer, &mut target)?;

    println!("Done!");
    Ok(())
}
