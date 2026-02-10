use crate::map_data::MinimapPos;

/// How a ship should be rendered based on its visibility state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShipVisibility {
    /// Ship is directly visible (Position packets). Solid fill.
    Visible,
    /// Ship is detected on minimap but not directly rendered. Outline only.
    MinimapOnly,
    /// Ship has gone undetected. Gray, semi-transparent at last known position.
    Undetected,
}

/// A high-level draw command emitted by the renderer.
///
/// The renderer reads game state and produces a sequence of these commands.
/// A `RenderTarget` implementation consumes them to produce visual output,
/// whether that's a software-rendered image or GPU draw calls.
#[derive(Debug)]
pub enum DrawCommand {
    /// Artillery tracer line segment
    ShotTracer { from: MinimapPos, to: MinimapPos },
    /// Torpedo dot
    Torpedo { pos: MinimapPos, friendly: bool },
    /// Smoke puff circle (alpha blended)
    Smoke { pos: MinimapPos, radius: i32 },
    /// Ship with icon, rotation, color, visibility
    Ship {
        pos: MinimapPos,
        yaw: f32,
        /// Species name for icon lookup (e.g. "Destroyer")
        species: Option<String>,
        color: [u8; 3],
        visibility: ShipVisibility,
        health_fraction: Option<f32>,
    },
    /// Dead ship X marker
    DeadShip { pos: MinimapPos },
    /// Plane icon
    Plane {
        pos: MinimapPos,
        /// Icon key for lookup (e.g. "controllable/fighter_he_enemy")
        icon_key: String,
        fallback_color: [u8; 3],
    },
    /// Score bar
    ScoreBar { team0: i32, team1: i32 },
    /// Game timer
    Timer { seconds: f32 },
    /// Kill feed entries (killer_name, victim_name)
    KillFeed { entries: Vec<(String, String)> },
}

/// Trait for rendering backends that consume `DrawCommand`s.
///
/// Implementations produce visual output from high-level draw commands.
/// The software image renderer and a future GPU renderer both implement this.
pub trait RenderTarget {
    /// Prepare a fresh frame (clear canvas, draw background map + grid).
    fn begin_frame(&mut self);

    /// Execute a single draw command.
    fn draw(&mut self, cmd: &DrawCommand);

    /// Finalize the current frame. After this call, the frame is ready to read/encode.
    fn end_frame(&mut self);
}
