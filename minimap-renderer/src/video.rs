use std::fs::File;
use std::io::BufWriter;

use anyhow::{Context, anyhow};
use bytes::Bytes;

use wows_replays::analyzer::battle_controller::listener::BattleControllerState;
use wows_replays::types::GameClock;

use crate::draw_command::RenderTarget;
use crate::drawing::ImageTarget;
use crate::renderer::MinimapRenderer;
use crate::{CANVAS_HEIGHT, MINIMAP_SIZE};

pub const TOTAL_FRAMES: usize = 1800;
pub const FPS: f64 = 30.0;

#[derive(Clone, Debug)]
pub enum DumpMode {
    Frame(usize),
    Midpoint,
}

// ---------------------------------------------------------------------------
// GPU backend (vk-video + yuvutils-rs)
// ---------------------------------------------------------------------------

#[cfg(feature = "gpu")]
mod gpu {
    use std::num::NonZeroU32;

    use anyhow::anyhow;
    use vk_video::parameters::{RateControl, VideoParameters};
    use vk_video::{BytesEncoder, Frame, RawFrameData, VulkanInstance};
    use yuvutils_rs::{
        BufferStoreMut, YuvBiPlanarImageMut, YuvConversionMode, YuvRange, YuvStandardMatrix,
    };

    use super::FPS;

    pub struct GpuEncoder {
        encoder: BytesEncoder,
        nv12_buf: Vec<u8>,
        frame_count: u64,
    }

    impl GpuEncoder {
        pub fn new(width: u32, height: u32) -> anyhow::Result<Self> {
            let instance =
                VulkanInstance::new().map_err(|e| anyhow!("Vulkan init failed: {:?}", e))?;
            let adapter = instance
                .create_adapter(None)
                .map_err(|e| anyhow!("No Vulkan adapter: {:?}", e))?;

            if !adapter.supports_encoding() {
                return Err(anyhow!(
                    "Vulkan adapter '{}' does not support video encoding",
                    adapter.info().name
                ));
            }

            let device = adapter
                .create_device(
                    wgpu::Features::empty(),
                    wgpu::ExperimentalFeatures::disabled(),
                    wgpu::Limits {
                        max_immediate_size: 128,
                        ..Default::default()
                    },
                )
                .map_err(|e| anyhow!("Vulkan device creation failed: {:?}", e))?;

            let params = device
                .encoder_parameters_high_quality(
                    VideoParameters {
                        width: NonZeroU32::new(width).expect("non-zero width"),
                        height: NonZeroU32::new(height).expect("non-zero height"),
                        target_framerate: (FPS as u32).into(),
                    },
                    RateControl::VariableBitrate {
                        average_bitrate: 20_000_000,
                        max_bitrate: 40_000_000,
                        virtual_buffer_size: std::time::Duration::from_secs(2),
                    },
                )
                .map_err(|e| anyhow!("Encoder params failed: {:?}", e))?;

            let encoder = device
                .create_bytes_encoder(params)
                .map_err(|e| anyhow!("Encoder creation failed: {:?}", e))?;

            let nv12_size = (width as usize) * (height as usize) * 3 / 2;

            Ok(Self {
                encoder,
                nv12_buf: vec![0u8; nv12_size],
                frame_count: 0,
            })
        }

        pub fn encode_frame(
            &mut self,
            rgb: &[u8],
            width: u32,
            height: u32,
        ) -> anyhow::Result<Vec<u8>> {
            let y_len = (width * height) as usize;
            let uv_len = (width * height / 2) as usize;

            // Split nv12_buf into Y and UV planes
            let (y_plane, uv_plane) = self.nv12_buf[..y_len + uv_len].split_at_mut(y_len);

            let mut nv12_image = YuvBiPlanarImageMut {
                y_plane: BufferStoreMut::Borrowed(y_plane),
                y_stride: width,
                uv_plane: BufferStoreMut::Borrowed(uv_plane),
                uv_stride: width,
                width,
                height,
            };

            yuvutils_rs::rgb_to_yuv_nv12(
                &mut nv12_image,
                rgb,
                width * 3,
                YuvRange::Full,
                YuvStandardMatrix::Bt709,
                YuvConversionMode::Balanced,
            )
            .map_err(|e| anyhow!("RGB→NV12 conversion failed: {:?}", e))?;

            let force_keyframe = self.frame_count == 0;
            let frame = Frame {
                data: RawFrameData {
                    frame: self.nv12_buf.clone(),
                    width,
                    height,
                },
                pts: Some(self.frame_count),
            };

            let output = self
                .encoder
                .encode(&frame, force_keyframe)
                .map_err(|e| anyhow!("GPU encode failed: {:?}", e))?;

            self.frame_count += 1;
            Ok(output.data)
        }
    }
}

// ---------------------------------------------------------------------------
// CPU backend (openh264)
// ---------------------------------------------------------------------------

#[cfg(feature = "cpu")]
mod cpu {
    use anyhow::{Context, anyhow};
    use openh264::OpenH264API;
    use openh264::encoder::{Encoder, EncoderConfig, FrameRate};
    use openh264::formats::{RgbSliceU8, YUVBuffer};

    use super::FPS;

    pub struct CpuEncoder {
        encoder: Encoder,
    }

    impl CpuEncoder {
        pub fn new() -> anyhow::Result<Self> {
            let config = EncoderConfig::new()
                .max_frame_rate(FrameRate::from_hz(FPS as f32))
                .usage_type(openh264::encoder::UsageType::ScreenContentRealTime)
                .rate_control_mode(openh264::encoder::RateControlMode::Off)
                .qp(openh264::encoder::QpRange::new(0, 0))
                .adaptive_quantization(false)
                .background_detection(false);
            let encoder = Encoder::with_api_config(OpenH264API::from_source(), config)
                .context("Failed to create H.264 encoder")?;
            Ok(Self { encoder })
        }

        pub fn encode_frame(
            &mut self,
            rgb: &[u8],
            width: usize,
            height: usize,
        ) -> anyhow::Result<Vec<u8>> {
            let rgb_slice = RgbSliceU8::new(rgb, (width, height));
            let yuv = YUVBuffer::from_rgb_source(rgb_slice);
            let bitstream = self
                .encoder
                .encode(&yuv)
                .map_err(|e| anyhow!("H.264 encode error: {:?}", e))?;
            Ok(bitstream.to_vec())
        }
    }
}

// ---------------------------------------------------------------------------
// Encoder backend dispatch
// ---------------------------------------------------------------------------

enum EncoderBackend {
    #[cfg(feature = "gpu")]
    Gpu(gpu::GpuEncoder),
    #[cfg(feature = "cpu")]
    Cpu(cpu::CpuEncoder),
}

impl EncoderBackend {
    fn create(_width: u32, _height: u32) -> anyhow::Result<Self> {
        // Try GPU first when available
        #[cfg(feature = "gpu")]
        {
            match gpu::GpuEncoder::new(_width, _height) {
                Ok(enc) => {
                    println!("Using GPU (Vulkan Video) encoder");
                    return Ok(Self::Gpu(enc));
                }
                Err(e) => {
                    #[cfg(feature = "cpu")]
                    {
                        eprintln!("GPU encoder unavailable ({}), falling back to CPU", e);
                    }
                    #[cfg(not(feature = "cpu"))]
                    {
                        return Err(e.context(
                            "GPU encoder failed and no CPU fallback (enable 'cpu' feature)",
                        ));
                    }
                }
            }
        }

        #[cfg(feature = "cpu")]
        {
            println!("Using CPU (openh264) encoder");
            return Ok(Self::Cpu(cpu::CpuEncoder::new()?));
        }

        #[cfg(not(any(feature = "gpu", feature = "cpu")))]
        {
            compile_error!("At least one of 'gpu' or 'cpu' features must be enabled");
        }
    }

    fn encode_frame(&mut self, rgb: &[u8], width: u32, height: u32) -> anyhow::Result<Vec<u8>> {
        match self {
            #[cfg(feature = "gpu")]
            Self::Gpu(enc) => enc.encode_frame(rgb, width, height),
            #[cfg(feature = "cpu")]
            Self::Cpu(enc) => enc.encode_frame(rgb, width as usize, height as usize),
        }
    }
}

// ---------------------------------------------------------------------------
// VideoEncoder (public API — unchanged from caller's perspective)
// ---------------------------------------------------------------------------

/// Handles H.264 encoding and MP4 muxing for the minimap renderer.
///
/// Encodes frames on-the-fly to avoid storing raw RGB data in memory.
/// Stores encoded H.264 Annex B NAL data per frame, then muxes to MP4 at the end.
///
/// Uses GPU (vk-video) by default, falls back to CPU (openh264) if the `cpu`
/// feature is enabled and GPU is unavailable.
pub struct VideoEncoder {
    output_path: String,
    dump_mode: Option<DumpMode>,
    game_duration: f32,
    last_rendered_frame: i64,
    backend: Option<EncoderBackend>,
    h264_frames: Vec<Vec<u8>>,
}

impl VideoEncoder {
    pub fn new(output_path: &str, dump_mode: Option<DumpMode>, game_duration: f32) -> Self {
        Self {
            output_path: output_path.to_string(),
            dump_mode,
            game_duration,
            last_rendered_frame: -1,
            backend: None,
            h264_frames: Vec::with_capacity(TOTAL_FRAMES),
        }
    }

    /// Create the encoder backend on first use.
    fn ensure_encoder(&mut self) -> anyhow::Result<()> {
        if self.backend.is_some() {
            return Ok(());
        }
        self.backend = Some(EncoderBackend::create(MINIMAP_SIZE, CANVAS_HEIGHT)?);
        println!(
            "Rendering {} frames ({}x{}, {:.1}s game time at {:.0} fps)...",
            TOTAL_FRAMES, MINIMAP_SIZE, CANVAS_HEIGHT, self.game_duration, FPS
        );
        Ok(())
    }

    /// Encode a rendered frame to H.264 immediately.
    fn encode_frame(&mut self, target: &ImageTarget) -> anyhow::Result<()> {
        let backend = self
            .backend
            .as_mut()
            .ok_or_else(|| anyhow!("Encoder not initialized"))?;
        let frame_image = target.frame();
        let rgb_data = frame_image.as_raw();
        let encoded = backend.encode_frame(rgb_data, MINIMAP_SIZE, CANVAS_HEIGHT)?;
        self.h264_frames.push(encoded);
        Ok(())
    }

    /// Called before each packet is processed by the controller.
    ///
    /// If the new clock has crossed one or more frame boundaries, renders
    /// frames from the controller's current state (which reflects all
    /// packets up to but not including this one).
    pub fn advance_clock(
        &mut self,
        new_clock: GameClock,
        controller: &dyn BattleControllerState,
        renderer: &mut MinimapRenderer,
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

            // Update squadron info for any new planes
            renderer.update_squadron_info(controller);

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
        controller: &dyn BattleControllerState,
        renderer: &mut MinimapRenderer,
        target: &mut ImageTarget,
    ) -> anyhow::Result<()> {
        // Render up to the actual battle end (or last packet), not meta.duration.
        // This avoids duplicating frozen frames when the match ends early.
        let end_clock = controller.battle_end_clock().unwrap_or(controller.clock());
        self.advance_clock(end_clock, controller, renderer, target);

        if self.dump_mode.is_some() {
            return Ok(());
        }

        // Mux the already-encoded H.264 frames into MP4
        self.mux_to_mp4()
    }

    /// Mux pre-encoded H.264 Annex B frames into an MP4 file.
    fn mux_to_mp4(&self) -> anyhow::Result<()> {
        if self.h264_frames.is_empty() {
            return Err(anyhow!("No frames to mux"));
        }

        // Extract SPS and PPS from the first keyframe
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

        // Setup MP4 writer
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
