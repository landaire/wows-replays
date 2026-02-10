use std::collections::HashMap;
use std::path::Path;

use image::{RgbImage, RgbaImage};
use wowsunpack::data::idx::FileNode;
use wowsunpack::data::pkg::PkgFileLoader;

use crate::map_data;

/// Default minimap render size in pixels (multiple of 16 for H.264 macroblock alignment).
pub const MINIMAP_SIZE: u32 = 768;

/// Icon size in pixels for rasterized ship icons.
pub const ICON_SIZE: u32 = 24;

/// Load an image from the packed game files.
pub fn load_packed_image(
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

/// Load and composite the minimap image (water + land layers) from packed game files.
pub fn load_map_image(
    map_name: &str,
    file_tree: &FileNode,
    pkg_loader: &PkgFileLoader,
) -> Option<RgbImage> {
    let bare_name = map_name.strip_prefix("spaces/").unwrap_or(map_name);

    let water_path = format!("spaces/{}/minimap_water.png", bare_name);
    let land_path = format!("spaces/{}/minimap.png", bare_name);

    let water = load_packed_image(&water_path, file_tree, pkg_loader);
    let land = load_packed_image(&land_path, file_tree, pkg_loader);

    let result = match (water, land) {
        (Some(water_img), Some(land_img)) => {
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

/// Load map metadata (space dimensions) from packed game files.
pub fn load_map_info(
    map_name: &str,
    file_tree: &FileNode,
    pkg_loader: &PkgFileLoader,
) -> Option<map_data::MapInfo> {
    let bare_name = map_name.strip_prefix("spaces/").unwrap_or(map_name);

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

    let read_value = |parent: &roxmltree::Node, name: &str| -> Option<String> {
        if let Some(v) = parent.attribute(name) {
            return Some(v.to_string());
        }
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

    let chunk_size: f64 = doc
        .descendants()
        .find(|n| n.has_tag_name("chunkSize"))
        .and_then(|n| n.text().and_then(|t| t.trim().parse().ok()))
        .unwrap_or(100.0);

    let chunks_x = (max_x - min_x + 1) as f64;
    let chunks_y = (max_y - min_y + 1) as f64;
    let space_w = ((chunks_x - 4.0) * chunk_size).round() as i32;
    let space_h = ((chunks_y - 4.0) * chunk_size).round() as i32;

    let space_size = space_w.max(space_h);

    println!(
        "Map '{}': bounds ({},{})..({},{}), chunk_size={}, space_size={}",
        bare_name, min_x, min_y, max_x, max_y, chunk_size, space_size
    );

    Some(map_data::MapInfo { space_size })
}

/// Load and rasterize ship SVG icons from game files.
pub fn load_ship_icons(
    file_tree: &FileNode,
    pkg_loader: &PkgFileLoader,
) -> HashMap<String, RgbaImage> {
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

/// Load plane icons (pre-colored PNGs) from game files.
pub fn load_plane_icons(
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
        "fighter",
        "fighter_upgrade",
        "scout",
        "smoke",
        "bomber_depth_charge",
        "bomber_mine",
    ];

    let mut icons = HashMap::new();
    for dir in &dirs {
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
pub fn rasterize_svg(svg_data: &[u8], size: u32) -> Option<RgbaImage> {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(svg_data, &opt).ok()?;

    let tree_size = tree.size();
    let sx = size as f32 / tree_size.width();
    let sy = size as f32 / tree_size.height();
    let scale = sx.min(sy);

    let mut pixmap = tiny_skia::Pixmap::new(size, size)?;

    let offset_x = (size as f32 - tree_size.width() * scale) / 2.0;
    let offset_y = (size as f32 - tree_size.height() * scale) / 2.0;
    let transform =
        tiny_skia::Transform::from_translate(offset_x, offset_y).post_scale(scale, scale);

    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let data = pixmap.data().to_vec();
    RgbaImage::from_raw(size, size, data)
}
