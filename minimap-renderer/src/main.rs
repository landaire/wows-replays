mod drawing;
mod map_data;
mod renderer;

use anyhow::{anyhow, Context};
use clap::{App, Arg};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::read_dir;
use std::io::Cursor;
use std::path::Path;
use wowsunpack::data::idx::{self, FileNode};
use wowsunpack::data::pkg::PkgFileLoader;
use wowsunpack::data::DataFileWithCallback;
use wowsunpack::game_params::provider::GameMetadataProvider;
use wowsunpack::rpc::entitydefs::{parse_scripts, EntitySpec};

use image::{RgbImage, RgbaImage};
use wows_replays::analyzer::{AnalyzerAdapter, AnalyzerMutBuilder};
use wows_replays::ReplayFile;

use renderer::{DumpMode, MinimapBuilder};

const MINIMAP_SIZE: u32 = 768;

fn load_packed_image(
    path: &str,
    file_tree: &FileNode,
    pkg_loader: &PkgFileLoader,
) -> Option<image::DynamicImage> {
    let file_path = Path::new(path);
    let mut buf = Vec::new();
    if file_tree
        .read_file_at_path(file_path, pkg_loader, &mut buf)
        .is_ok()
    {
        if let Ok(img) = image::load_from_memory(&buf) {
            return Some(img);
        }
    }
    None
}

fn load_map_image(
    map_name: &str,
    file_tree: &FileNode,
    pkg_loader: &PkgFileLoader,
) -> Option<RgbImage> {
    // map_name from meta is e.g. "spaces/28_naval_mission"
    // minimap images live at spaces/<map>/minimap.png in the packed files
    let bare_name = map_name.strip_prefix("spaces/").unwrap_or(map_name);

    let water_path = format!("spaces/{}/minimap_water.png", bare_name);
    let land_path = format!("spaces/{}/minimap.png", bare_name);

    // Load water (background) and land (foreground with alpha) separately,
    // then composite land over water to get the final map image.
    let water = load_packed_image(&water_path, file_tree, pkg_loader);
    let land = load_packed_image(&land_path, file_tree, pkg_loader);

    let result = match (water, land) {
        (Some(water_img), Some(land_img)) => {
            // Composite: start with water, overlay land using alpha
            let mut base = water_img.to_rgba8();
            let overlay = land_img.to_rgba8();
            image::imageops::overlay(&mut base, &overlay, 0, 0);
            println!(
                "Loaded map image: {}x{} (water + land composited)",
                base.width(),
                base.height()
            );
            image::DynamicImage::ImageRgba8(base).to_rgb8()
        }
        (Some(water_img), None) => {
            println!("Loaded map image: water only");
            water_img.to_rgb8()
        }
        (None, Some(land_img)) => {
            println!("Loaded map image: land only (no water background)");
            land_img.to_rgb8()
        }
        (None, None) => {
            println!(
                "Warning: Could not load map image for '{}', using blank background",
                map_name
            );
            return None;
        }
    };

    if result.width() != MINIMAP_SIZE || result.height() != MINIMAP_SIZE {
        let resized = image::imageops::resize(
            &result,
            MINIMAP_SIZE,
            MINIMAP_SIZE,
            image::imageops::FilterType::Lanczos3,
        );
        return Some(resized);
    }
    Some(result)
}

fn load_map_info(
    map_name: &str,
    file_tree: &FileNode,
    pkg_loader: &PkgFileLoader,
) -> Option<map_data::MapInfo> {
    let bare_name = map_name.strip_prefix("spaces/").unwrap_or(map_name);

    // Try multiple path variants — the virtual filesystem layout may differ
    let candidates = [
        format!("spaces/{}/space.settings", bare_name),
        format!("content/gameplay/{}/space.settings", bare_name),
    ];
    let mut buf = Vec::new();
    let mut found = false;
    for candidate in &candidates {
        buf.clear();
        let file_path = Path::new(candidate);
        if file_tree
            .read_file_at_path(file_path, pkg_loader, &mut buf)
            .is_ok()
            && !buf.is_empty()
        {
            println!("Loaded space.settings from: {}", candidate);
            found = true;
            break;
        }
    }
    if !found {
        println!(
            "Warning: Could not load space.settings for '{}' (tried: {:?})",
            bare_name, candidates
        );
        return None;
    }

    let content = String::from_utf8_lossy(&buf);
    let doc = roxmltree::Document::parse(&content).ok()?;

    // Helper: read a value either as an attribute on `node` or as a child element's text
    let read_value = |parent: &roxmltree::Node, name: &str| -> Option<String> {
        // Try attribute first (e.g. <bounds minX="-9" />)
        if let Some(v) = parent.attribute(name) {
            return Some(v.to_string());
        }
        // Then try child element (e.g. <bounds><minX> -9 </minX></bounds>)
        parent
            .children()
            .find(|c| c.has_tag_name(name))
            .and_then(|c| c.text())
            .map(|t| t.trim().to_string())
    };

    let bounds = doc.descendants().find(|n| n.has_tag_name("bounds"))?;
    let min_x: i32 = read_value(&bounds, "minX")?.parse().ok()?;
    let max_x: i32 = read_value(&bounds, "maxX")?.parse().ok()?;
    let min_y: i32 = read_value(&bounds, "minY")?.parse().ok()?;
    let max_y: i32 = read_value(&bounds, "maxY")?.parse().ok()?;

    // chunkSize can be a child element of root or of <terrain>
    let chunk_size: f64 = doc
        .descendants()
        .find(|n| n.has_tag_name("chunkSize"))
        .and_then(|n| n.text().and_then(|t| t.trim().parse().ok()))
        .unwrap_or(100.0);

    // Formula from Python spaces.py:
    // w = len(range(min_x, max_x + 1)) * chunk_size - 4 * chunk_size
    let chunks_x = (max_x - min_x + 1) as f64;
    let chunks_y = (max_y - min_y + 1) as f64;
    let space_w = ((chunks_x - 4.0) * chunk_size).round() as i32;
    let space_h = ((chunks_y - 4.0) * chunk_size).round() as i32;

    // Use the larger dimension as space_size (maps should be square)
    let space_size = space_w.max(space_h);

    println!(
        "Map '{}': bounds ({},{})..({},{}), chunk_size={}, space_size={}",
        bare_name, min_x, min_y, max_x, max_y, chunk_size, space_size
    );

    Some(map_data::MapInfo { space_size })
}

/// Icon size in pixels for rasterized ship icons.
const ICON_SIZE: u32 = 24;

/// Load and rasterize ship SVG icons from game files.
/// Returns a map from species name to RGBA image.
fn load_ship_icons(file_tree: &FileNode, pkg_loader: &PkgFileLoader) -> HashMap<String, RgbaImage> {
    let species_names = [
        "Destroyer",
        "Cruiser",
        "Battleship",
        "AirCarrier",
        "Submarine",
        "Auxiliary",
    ];
    let mut icons = HashMap::new();
    for name in &species_names {
        let path = format!(
            "gui/fla/minimap/ship_icons/minimap_{}.svg",
            name.to_ascii_lowercase()
        );
        let file_path = Path::new(&path);
        let mut buf = Vec::new();
        if file_tree
            .read_file_at_path(file_path, pkg_loader, &mut buf)
            .is_ok()
            && !buf.is_empty()
        {
            if let Some(img) = rasterize_svg(&buf, ICON_SIZE) {
                println!("Loaded ship icon: {}", name);
                icons.insert(name.to_string(), img);
            }
        }
    }
    if icons.is_empty() {
        println!("Warning: No ship icons loaded, using fallback circles");
    }
    icons
}

/// Load all plane icons from game files into a HashMap keyed by name (e.g. "fighter_ally").
fn load_plane_icons(
    file_tree: &FileNode,
    pkg_loader: &PkgFileLoader,
) -> HashMap<String, RgbaImage> {
    let dirs = [
        "gui/battle_hud/markers_minimap/plane/consumables",
        "gui/battle_hud/markers_minimap/plane/controllable",
        "gui/battle_hud/markers_minimap/plane/airsupport",
    ];
    let suffixes = ["ally", "enemy", "own", "division", "teamkiller"];
    let base_names = [
        // controllable
        "fighter_he",
        "fighter_ap",
        "fighter_he_st2024",
        "bomber_he",
        "bomber_ap",
        "bomber_ap_st2024",
        "skip_he",
        "skip_ap",
        "torpedo_regular",
        "torpedo_regular_st2024",
        "torpedo_deepwater",
        "auxiliary",
        // consumables
        "fighter",
        "fighter_upgrade",
        "scout",
        "smoke",
        // airsupport
        "bomber_depth_charge",
        "bomber_mine",
    ];

    let mut icons = HashMap::new();
    for dir in &dirs {
        // Use the last path component as namespace (e.g. "consumables", "controllable", "airsupport")
        let dir_name = dir.rsplit('/').next().unwrap_or(dir);
        for base in &base_names {
            for suffix in &suffixes {
                let name = format!("{}_{}", base, suffix);
                let path = format!("{}/{}.png", dir, name);
                if let Some(img) = load_packed_image(&path, file_tree, pkg_loader) {
                    let key = format!("{}/{}", dir_name, name);
                    icons.insert(key, img.to_rgba8());
                }
            }
        }
    }
    println!("Loaded {} plane icons", icons.len());
    icons
}

/// Rasterize an SVG byte buffer to an RGBA image at the given size.
fn rasterize_svg(svg_data: &[u8], size: u32) -> Option<RgbaImage> {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(svg_data, &opt).ok()?;

    let tree_size = tree.size();
    let sx = size as f32 / tree_size.width();
    let sy = size as f32 / tree_size.height();
    let scale = sx.min(sy);

    let mut pixmap = tiny_skia::Pixmap::new(size, size)?;

    // Center the icon in the output
    let offset_x = (size as f32 - tree_size.width() * scale) / 2.0;
    let offset_y = (size as f32 - tree_size.height() * scale) / 2.0;
    let transform =
        tiny_skia::Transform::from_translate(offset_x, offset_y).post_scale(scale, scale);

    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let data = pixmap.data().to_vec();
    RgbaImage::from_raw(size, size, data)
}

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

    println!("Loading ship icons...");
    let ship_icons = load_ship_icons(&file_tree, &pkg_loader);
    let plane_icons = load_plane_icons(&file_tree, &pkg_loader);

    println!("Parsing replay...");
    let replay_file = ReplayFile::from_file(&std::path::PathBuf::from(replay_path))?;

    // Load map image and metadata from game files
    let map_name = &replay_file.meta.mapName;
    let map_image = load_map_image(map_name, &file_tree, &pkg_loader);
    let map_info = load_map_info(map_name, &file_tree, &pkg_loader);

    let builder = MinimapBuilder::new(
        output,
        map_image,
        map_info,
        dump_mode,
        ship_icons,
        plane_icons,
        game_params,
    );
    let processor = builder.build(&replay_file.meta);

    let mut p = wows_replays::packet2::Parser::new(&specs);
    let mut analyzer_set = AnalyzerAdapter::new(vec![processor]);
    p.parse_packets_mut::<AnalyzerAdapter>(&replay_file.packet_data, &mut analyzer_set)?;
    analyzer_set.finish();

    println!("Done!");
    Ok(())
}
