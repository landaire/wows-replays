use std::fs::File;
use std::io::BufWriter;

use anyhow::{anyhow, Context};
use bytes::Bytes;
use openh264::encoder::{Encoder, EncoderConfig, FrameRate};
use openh264::formats::{RgbSliceU8, YUVBuffer};
use openh264::OpenH264API;

use wows_replays::analyzer::battle_controller::listener::BattleControllerState;
use wows_replays::types::GameClock;

use crate::draw_command::RenderTarget;
use crate::drawing::ImageTarget;
use crate::renderer::MinimapRenderer;

const TOTAL_FRAMES: usize = 1800;
const FPS: f64 = 30.0;
const MINIMAP_SIZE: u32 = 768;
const CANVAS_HEIGHT: u32 = MINIMAP_SIZE + 32; // 800

/// Mode for dumping a single frame instead of rendering a full video.
#[derive(Clone, Debug)]
pub enum DumpMode {
    Frame(usize),
    Midpoint,
}

/// Video encoder that drives frame-by-frame rendering and H.264/MP4 output.
///
/// Owns the frame timing state and video encoding pipeline. Calls into
/// `MinimapRenderer::draw_frame()` and `ImageTarget` to produce each frame,
/// then encodes to H.264 on the fly.
pub struct VideoEncoder {
    output_path: String,
    dump_mode: Option<DumpMode>,
    game_duration: f32,
    last_rendered_frame: i64,

    // H.264 encoder (created lazily on first video frame)
    encoder: Option<Encoder>,
    // Encoded H.264 Annex B NAL data per frame
    h264_frames: Vec<Vec<u8>>,
}

impl VideoEncoder {
    pub fn new(output_path: &str, dump_mode: Option<DumpMode>, game_duration: f32) -> Self {
        Self {
            output_path: output_path.to_string(),
            dump_mode,
            game_duration,
            last_rendered_frame: -1,
            encoder: None,
            h264_frames: Vec::with_capacity(TOTAL_FRAMES),
        }
    }

    /// Create the H.264 encoder on first use.
    fn ensure_encoder(&mut self) -> anyhow::Result<()> {
        if self.encoder.is_some() {
            return Ok(());
        }
        let config = EncoderConfig::new()
            .max_frame_rate(FrameRate::from_hz(FPS as f32))
            .usage_type(openh264::encoder::UsageType::ScreenContentRealTime)
            .rate_control_mode(openh264::encoder::RateControlMode::Bitrate)
            .bitrate(openh264::encoder::BitRate::from_bps(20_000_000))
            .qp(openh264::encoder::QpRange::new(0, 24))
            .adaptive_quantization(false)
            .background_detection(false);
        self.encoder = Some(
            Encoder::with_api_config(OpenH264API::from_source(), config)
                .context("Failed to create H.264 encoder")?,
        );
        println!(
            "Rendering {} frames ({}x{}, {:.1}s game time at {:.0} fps)...",
            TOTAL_FRAMES, MINIMAP_SIZE, CANVAS_HEIGHT, self.game_duration, FPS
        );
        Ok(())
    }

    /// Encode a rendered frame to H.264 immediately.
    fn encode_frame(&mut self, target: &ImageTarget) -> anyhow::Result<()> {
        let encoder = self
            .encoder
            .as_mut()
            .ok_or_else(|| anyhow!("Encoder not initialized"))?;
        let frame_image = target.frame();
        let rgb_data = frame_image.as_raw();
        let rgb = RgbSliceU8::new(rgb_data, (MINIMAP_SIZE as usize, CANVAS_HEIGHT as usize));
        let yuv = YUVBuffer::from_rgb_source(rgb);
        let bitstream = encoder
            .encode(&yuv)
            .map_err(|e| anyhow!("H.264 encode error: {:?}", e))?;
        self.h264_frames.push(bitstream.to_vec());
        Ok(())
    }

    /// Called after each packet is processed by the controller.
    ///
    /// If the new clock has crossed one or more frame boundaries, renders
    /// frames from the controller's current (up-to-date) state and encodes them.
    pub fn advance_clock(
        &mut self,
        new_clock: GameClock,
        renderer: &mut MinimapRenderer,
        controller: &dyn BattleControllerState,
        target: &mut ImageTarget,
    ) {
        if self.game_duration <= 0.0 {
            return;
        }

        let frame_duration = self.game_duration / TOTAL_FRAMES as f32;
        let target_frame = (new_clock.seconds() / frame_duration) as i64;

        while self.last_rendered_frame < target_frame {
            self.last_rendered_frame += 1;
            if self.last_rendered_frame >= TOTAL_FRAMES as i64 {
                break;
            }

            let commands = renderer.draw_frame(controller);

            if let Some(ref dump_mode) = self.dump_mode {
                let dump_frame = match dump_mode {
                    DumpMode::Frame(n) => *n as i64,
                    DumpMode::Midpoint => TOTAL_FRAMES as i64 / 2,
                };
                if self.last_rendered_frame == dump_frame {
                    target.begin_frame();
                    for cmd in &commands {
                        target.draw(cmd);
                    }
                    target.end_frame();

                    let png_path = self.output_path.replace(".mp4", ".png");
                    let png_path = if png_path == self.output_path {
                        format!("{}.png", self.output_path)
                    } else {
                        png_path
                    };
                    if let Err(e) = target.frame().save(&png_path) {
                        eprintln!("Error saving frame: {}", e);
                    } else {
                        let (w, h) = target.canvas_size();
                        println!("Frame {} saved to {} ({}x{})", dump_frame, png_path, w, h);
                    }
                }
            } else {
                // Full video mode: render, encode to H.264 immediately
                if let Err(e) = self.ensure_encoder() {
                    eprintln!("Encoder error: {}", e);
                    return;
                }

                target.begin_frame();
                for cmd in &commands {
                    target.draw(cmd);
                }
                target.end_frame();

                if let Err(e) = self.encode_frame(target) {
                    eprintln!("Encode error: {}", e);
                    return;
                }

                if self.last_rendered_frame % 100 == 0 {
                    println!("  Frame {}/{}", self.last_rendered_frame, TOTAL_FRAMES);
                }
            }
        }
    }

    /// Finalize: flush any remaining frames and write the video file.
    pub fn finish(
        &mut self,
        renderer: &mut MinimapRenderer,
        controller: &dyn BattleControllerState,
        target: &mut ImageTarget,
    ) -> anyhow::Result<()> {
        let end_clock = controller.battle_end_clock().unwrap_or(controller.clock());
        self.advance_clock(end_clock, renderer, controller, target);

        if self.dump_mode.is_some() {
            return Ok(());
        }

        self.mux_to_mp4()
    }

    /// Mux pre-encoded H.264 Annex B frames into an MP4 file.
    fn mux_to_mp4(&self) -> anyhow::Result<()> {
        if self.h264_frames.is_empty() {
            return Err(anyhow!("No frames to mux"));
        }

        let first_frame = &self.h264_frames[0];
        let nals = parse_annexb_nals(first_frame);
        let sps = nals
            .iter()
            .find(|n| (n[0] & 0x1f) == 7)
            .ok_or_else(|| anyhow!("No SPS found in first frame"))?;
        let pps = nals
            .iter()
            .find(|n| (n[0] & 0x1f) == 8)
            .ok_or_else(|| anyhow!("No PPS found in first frame"))?;

        let mp4_config = mp4::Mp4Config {
            major_brand: str::parse("isom").unwrap(),
            minor_version: 512,
            compatible_brands: vec![
                str::parse("isom").unwrap(),
                str::parse("iso2").unwrap(),
                str::parse("avc1").unwrap(),
                str::parse("mp41").unwrap(),
            ],
            timescale: 1000,
        };

        let file = File::create(&self.output_path).context("Failed to create output file")?;
        let writer = BufWriter::new(file);
        let mut mp4_writer = mp4::Mp4Writer::write_start(writer, &mp4_config)?;

        let track_config = mp4::TrackConfig {
            track_type: mp4::TrackType::Video,
            timescale: 1000,
            language: "und".to_string(),
            media_conf: mp4::MediaConfig::AvcConfig(mp4::AvcConfig {
                width: MINIMAP_SIZE as u16,
                height: CANVAS_HEIGHT as u16,
                seq_param_set: sps.to_vec(),
                pic_param_set: pps.to_vec(),
            }),
        };
        mp4_writer.add_track(&track_config)?;

        let sample_duration = 1000 / FPS as u32;

        for (frame_idx, annexb_data) in self.h264_frames.iter().enumerate() {
            if annexb_data.is_empty() {
                continue;
            }
            let nals = parse_annexb_nals(annexb_data);
            let is_sync = nals.iter().any(|n| (n[0] & 0x1f) == 5);

            let mut avcc_data = Vec::new();
            for nal in &nals {
                let nal_type = nal[0] & 0x1f;
                if nal_type == 7 || nal_type == 8 {
                    continue;
                }
                let len = nal.len() as u32;
                avcc_data.extend_from_slice(&len.to_be_bytes());
                avcc_data.extend_from_slice(nal);
            }

            if avcc_data.is_empty() {
                continue;
            }

            let sample = mp4::Mp4Sample {
                start_time: frame_idx as u64 * sample_duration as u64,
                duration: sample_duration,
                rendering_offset: 0,
                is_sync,
                bytes: Bytes::from(avcc_data),
            };
            mp4_writer.write_sample(1, &sample)?;
        }

        mp4_writer.write_end()?;
        println!("Video saved to {}", self.output_path);
        Ok(())
    }
}

/// Parse Annex B byte stream into individual NAL units (without start codes).
fn parse_annexb_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut i = 0;
    while i < data.len() {
        if i + 2 < data.len() && data[i] == 0 && data[i + 1] == 0 {
            let (start, _) = if i + 3 < data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                (i + 4, 4)
            } else if data[i + 2] == 1 {
                (i + 3, 3)
            } else {
                i += 1;
                continue;
            };
            let mut end = start;
            while end < data.len() {
                if end + 2 < data.len()
                    && data[end] == 0
                    && data[end + 1] == 0
                    && (data[end + 2] == 1
                        || (end + 3 < data.len() && data[end + 2] == 0 && data[end + 3] == 1))
                {
                    break;
                }
                end += 1;
            }
            if end > start {
                nals.push(&data[start..end]);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    nals
}
