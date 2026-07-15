//! Wire format for one NDI frame (video, audio, or metadata) as an opaque
//! `Bytes` blob, so it can travel through `crosspoint-core`'s payload-agnostic
//! broadcast channel exactly like an SRT relay chunk does. This is what lets
//! `crosspoint-core` stay untouched: it never sees a `VideoFrame`, only bytes.
//!
//! Known limitation: video frames are re-created via `VideoFrame::builder()`
//! from resolution + pixel format, which allocates a default (unpadded) line
//! stride. A source with a non-default stride (uncommon, but the NDI SDK
//! doesn't rule it out) would round-trip with corrupted rows. Not something
//! that could be exercised without such a source; see the crate-level docs.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use grafton_ndi::{
    AudioFormat, AudioFrame, AudioLayout, MetadataFrame, PixelFormat, ScanType, VideoFrame,
};
use thiserror::Error;

const KIND_VIDEO: u8 = 0;
const KIND_AUDIO: u8 = 1;
const KIND_METADATA: u8 = 2;

#[derive(Debug, Error)]
pub enum EnvelopeError {
    #[error("envelope truncated")]
    Truncated,
    #[error("unknown frame kind byte {0}")]
    UnknownKind(u8),
    #[error("unrecognized pixel format {0}")]
    BadPixelFormat(u32),
    #[error("unrecognized scan type {0}")]
    BadScanType(u32),
    #[error("invalid utf-8 metadata")]
    BadMetadata,
    #[error("grafton-ndi rejected the reconstructed frame: {0}")]
    Ndi(#[from] grafton_ndi::Error),
}

pub fn encode_video(frame: &VideoFrame) -> Bytes {
    let data = frame.data();
    let mut buf = BytesMut::with_capacity(1 + 33 + data.len());
    buf.put_u8(KIND_VIDEO);
    buf.put_i32(frame.width());
    buf.put_i32(frame.height());
    buf.put_u32(frame.pixel_format().into());
    buf.put_i32(frame.frame_rate_n());
    buf.put_i32(frame.frame_rate_d());
    buf.put_f32(frame.picture_aspect_ratio());
    buf.put_u32(frame.scan_type().into());
    buf.put_i64(frame.timecode());
    buf.put_u32(data.len() as u32);
    buf.put_slice(data);
    buf.freeze()
}

pub fn encode_audio(frame: &AudioFrame) -> Bytes {
    let data = frame.data();
    let mut buf = BytesMut::with_capacity(1 + 20 + data.len() * 4);
    buf.put_u8(KIND_AUDIO);
    buf.put_i32(frame.sample_rate());
    buf.put_i32(frame.num_channels());
    buf.put_i32(frame.num_samples());
    buf.put_i64(frame.timecode());
    buf.put_u32(data.len() as u32);
    for sample in data {
        buf.put_f32(*sample);
    }
    buf.freeze()
}

pub fn encode_metadata(frame: &MetadataFrame) -> Bytes {
    let data = frame.data().as_bytes();
    let mut buf = BytesMut::with_capacity(1 + 12 + data.len());
    buf.put_u8(KIND_METADATA);
    buf.put_i64(frame.timecode());
    buf.put_u32(data.len() as u32);
    buf.put_slice(data);
    buf.freeze()
}

pub enum DecodedFrame {
    Video(VideoFrame),
    Audio(AudioFrame),
    Metadata(MetadataFrame),
}

pub fn decode(mut bytes: Bytes) -> Result<DecodedFrame, EnvelopeError> {
    if bytes.is_empty() {
        return Err(EnvelopeError::Truncated);
    }
    match bytes.get_u8() {
        KIND_VIDEO => decode_video(bytes).map(DecodedFrame::Video),
        KIND_AUDIO => decode_audio(bytes).map(DecodedFrame::Audio),
        KIND_METADATA => decode_metadata(bytes).map(DecodedFrame::Metadata),
        other => Err(EnvelopeError::UnknownKind(other)),
    }
}

fn decode_video(mut bytes: Bytes) -> Result<VideoFrame, EnvelopeError> {
    if bytes.remaining() < 33 {
        return Err(EnvelopeError::Truncated);
    }
    let width = bytes.get_i32();
    let height = bytes.get_i32();
    let pixel_format_raw = bytes.get_u32();
    let pixel_format = PixelFormat::try_from(pixel_format_raw)
        .map_err(|_| EnvelopeError::BadPixelFormat(pixel_format_raw))?;
    let frame_rate_n = bytes.get_i32();
    let frame_rate_d = bytes.get_i32();
    let aspect_ratio = bytes.get_f32();
    let scan_type_raw = bytes.get_u32();
    let scan_type =
        ScanType::try_from(scan_type_raw).map_err(|_| EnvelopeError::BadScanType(scan_type_raw))?;
    let timecode = bytes.get_i64();
    let len = bytes.get_u32() as usize;
    if bytes.remaining() < len {
        return Err(EnvelopeError::Truncated);
    }
    let data = bytes.split_to(len).to_vec();

    let mut frame = VideoFrame::builder()
        .resolution(width, height)
        .pixel_format(pixel_format)
        .frame_rate(frame_rate_n, frame_rate_d)
        .aspect_ratio(aspect_ratio)
        .scan_type(scan_type)
        .timecode(timecode)
        .build()?;
    frame.replace_data(data)?;
    Ok(frame)
}

fn decode_audio(mut bytes: Bytes) -> Result<AudioFrame, EnvelopeError> {
    if bytes.remaining() < 20 {
        return Err(EnvelopeError::Truncated);
    }
    let sample_rate = bytes.get_i32();
    let channels = bytes.get_i32();
    let samples = bytes.get_i32();
    let timecode = bytes.get_i64();
    let len = bytes.get_u32() as usize;
    if bytes.remaining() < len * 4 {
        return Err(EnvelopeError::Truncated);
    }
    let mut data = Vec::with_capacity(len);
    for _ in 0..len {
        data.push(bytes.get_f32());
    }

    let frame = AudioFrame::builder()
        .sample_rate(sample_rate)
        .channels(channels)
        .samples(samples)
        .timecode(timecode)
        .format(AudioFormat::FLTP)
        .layout(AudioLayout::Planar)
        .data(data)
        .build()?;
    Ok(frame)
}

fn decode_metadata(mut bytes: Bytes) -> Result<MetadataFrame, EnvelopeError> {
    if bytes.remaining() < 12 {
        return Err(EnvelopeError::Truncated);
    }
    let timecode = bytes.get_i64();
    let len = bytes.get_u32() as usize;
    if bytes.remaining() < len {
        return Err(EnvelopeError::Truncated);
    }
    let data = bytes.split_to(len).to_vec();
    let text = String::from_utf8(data).map_err(|_| EnvelopeError::BadMetadata)?;
    Ok(MetadataFrame::with_data(text, timecode)?)
}
