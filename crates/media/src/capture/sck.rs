//! ScreenCaptureKit backend. SCK scales and converts to NV12 on the GPU, so the CPU only
//! copies planes out of the pixel buffer.

use livekit::webrtc::video_frame::NV12Buffer;
use screencapturekit::cm::CMSampleBufferExt;
use screencapturekit::cv::CVPixelBufferLockFlags;
use screencapturekit::prelude::*;
use screencapturekit::stream::configuration::color_matrix;

use super::{wall_clock_us, CaptureConfig, CaptureError, Frame, FrameBuffer};

/// Capture is stopped when the stream is dropped.
pub struct SckCapture {
    stream: SCStream,
}

// SCStream wraps an Objective-C object that SCK allows to be used from any thread.
unsafe impl Send for SckCapture {}

impl Drop for SckCapture {
    fn drop(&mut self) {
        let _ = self.stream.stop_capture();
    }
}

fn backend_err(e: impl std::fmt::Display) -> CaptureError {
    CaptureError::Backend(e.to_string())
}

pub fn start(
    config: CaptureConfig,
    sink: impl Fn(Frame) + Send + Sync + 'static,
) -> Result<SckCapture, CaptureError> {
    let content = SCShareableContent::get().map_err(|_| CaptureError::NoDisplay)?;
    let display = content
        .displays()
        .into_iter()
        .next()
        .ok_or(CaptureError::NoDisplay)?;
    let filter = SCContentFilter::create()
        .with_display(&display)
        .with_excluding_windows(&[])
        .build()
        .map_err(backend_err)?;

    let stream_config = SCStreamConfiguration::new()
        .with_width(config.width)
        .with_height(config.height)
        // Video-range NV12 matches what WebRTC's I420 pipeline expects; BT.601 matches
        // libyuv's default conversion on the viewer.
        .with_pixel_format(PixelFormat::YCbCr_420v)
        .with_color_matrix(color_matrix::ITU_R_601_4)
        .map_err(backend_err)?
        .with_minimum_frame_interval(&CMTime::new(1, config.max_fps as i32))
        .with_shows_cursor(config.show_cursor)
        .with_queue_depth(3);

    let mut stream = SCStream::new(&filter, &stream_config).map_err(backend_err)?;
    stream
        .add_output_handler(
            move |sample: CMSampleBuffer, _: SCStreamOutputType| {
                if let Some(frame) = to_frame(&sample) {
                    sink(frame);
                }
            },
            SCStreamOutputType::Screen,
        )
        .map_err(backend_err)?;
    stream.start_capture().map_err(backend_err)?;
    Ok(SckCapture { stream })
}

/// Copies an NV12 sample into a LiveKit buffer. Returns `None` for idle samples, which
/// SCK sends without pixels when the screen hasn't changed.
fn to_frame(sample: &CMSampleBuffer) -> Option<Frame> {
    let captured_at_us = capture_time_us(sample);
    let pixels = sample.pixel_buffer()?;
    let guard = pixels.lock(CVPixelBufferLockFlags::READ_ONLY).ok()?;
    if guard.plane_count() != 2 {
        return None;
    }
    let (width, height) = (guard.width(), guard.height());
    let mut buffer = NV12Buffer::new(width as u32, height as u32);
    let (dst_stride_y, dst_stride_uv) = buffer.strides();
    let (dst_y, dst_uv) = buffer.data_mut();
    for (plane, dst, dst_stride, rows) in [
        (0, dst_y, dst_stride_y as usize, height),
        (1, dst_uv, dst_stride_uv as usize, height / 2),
    ] {
        // SAFETY: the buffer is locked for reading until `guard` is dropped.
        let src = unsafe { guard.plane_data(plane) }?;
        let src_stride = guard.bytes_per_row_of_plane(plane);
        let row_len = width.min(dst_stride).min(src_stride);
        for row in 0..rows {
            dst[row * dst_stride..row * dst_stride + row_len]
                .copy_from_slice(&src[row * src_stride..row * src_stride + row_len]);
        }
    }
    Some(Frame {
        buffer: FrameBuffer::Nv12(buffer),
        captured_at_us,
    })
}

extern "C" {
    // Darwin libc; not exposed by the `libc` crate.
    fn clock_gettime_nsec_np(clock_id: libc::clockid_t) -> u64;
}

/// Wall-clock time at which SCK captured the sample, so the measured latency includes
/// SCK's own delay. SCK timestamps samples on the host clock (`CLOCK_UPTIME_RAW`).
fn capture_time_us(sample: &CMSampleBuffer) -> u64 {
    let now_wall = wall_clock_us();
    let pts = sample.presentation_timestamp();
    if pts.timescale <= 0 {
        return now_wall;
    }
    let pts_ns = pts.value as i128 * 1_000_000_000 / pts.timescale as i128;
    // SAFETY: plain libc call with a valid clock id.
    let now_host_ns = unsafe { clock_gettime_nsec_np(libc::CLOCK_UPTIME_RAW) } as i128;
    let age_us = (now_host_ns - pts_ns) / 1000;
    // Only trust a plausible age; otherwise fall back to the arrival time.
    if (0..1_000_000).contains(&age_us) {
        now_wall - age_us as u64
    } else {
        now_wall
    }
}
