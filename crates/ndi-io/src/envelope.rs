//! Wire format for one NDI frame (video, audio, or metadata) as an opaque
//! `Bytes` blob, so it can travel through `crosspoint-core`'s payload-agnostic
//! broadcast channel exactly like an SRT relay chunk does. This is what lets
//! `crosspoint-core` stay untouched: it never sees a frame, only bytes.
//!
//! Since the move to [`crate::sys`], the round trip is **byte-exact**. The
//! previous `grafton-ndi` implementation rebuilt each video frame from
//! resolution + pixel format via a builder that allocated a default (unpadded)
//! line stride, so a source with a non-default stride would have round-tripped
//! with corrupted rows. The stride is now carried in the envelope and restored
//! verbatim, which removes that whole class of bug — see `round_trips_a_padded_stride`.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use thiserror::Error;

use crate::sys::{AudioFrame, MetadataFrame, VideoFrame};

const KIND_VIDEO: u8 = 0;
const KIND_AUDIO: u8 = 1;
const KIND_METADATA: u8 = 2;

#[derive(Debug, Error)]
pub enum EnvelopeError {
    #[error("envelope truncated")]
    Truncated,
    #[error("unknown frame kind byte {0}")]
    UnknownKind(u8),
    #[error("invalid utf-8 metadata")]
    BadMetadata,
}

pub fn encode_video(frame: &VideoFrame) -> Bytes {
    let mut buf = BytesMut::with_capacity(1 + 40 + frame.data.len());
    buf.put_u8(KIND_VIDEO);
    buf.put_i32(frame.xres);
    buf.put_i32(frame.yres);
    buf.put_u32(frame.four_cc);
    buf.put_i32(frame.frame_rate_n);
    buf.put_i32(frame.frame_rate_d);
    buf.put_f32(frame.picture_aspect_ratio);
    buf.put_i32(frame.frame_format_type);
    // Carried, not recomputed: this is what makes a padded source round-trip.
    buf.put_i32(frame.line_stride_in_bytes);
    buf.put_i64(frame.timecode);
    buf.put_u32(frame.data.len() as u32);
    buf.put_slice(&frame.data);
    buf.freeze()
}

pub fn encode_audio(frame: &AudioFrame) -> Bytes {
    let mut buf = BytesMut::with_capacity(1 + 24 + frame.data.len() * 4);
    buf.put_u8(KIND_AUDIO);
    buf.put_i32(frame.sample_rate);
    buf.put_i32(frame.no_channels);
    buf.put_i32(frame.no_samples);
    buf.put_i64(frame.timecode);
    buf.put_u32(frame.data.len() as u32);
    for sample in &frame.data {
        buf.put_f32(*sample);
    }
    buf.freeze()
}

pub fn encode_metadata(frame: &MetadataFrame) -> Bytes {
    let data = frame.data.as_bytes();
    let mut buf = BytesMut::with_capacity(1 + 12 + data.len());
    buf.put_u8(KIND_METADATA);
    buf.put_i64(frame.timecode);
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
    // 8 fixed fields before the payload: 7x4 bytes + 1x8 + the 4-byte length.
    if bytes.remaining() < 40 {
        return Err(EnvelopeError::Truncated);
    }
    let xres = bytes.get_i32();
    let yres = bytes.get_i32();
    let four_cc = bytes.get_u32();
    let frame_rate_n = bytes.get_i32();
    let frame_rate_d = bytes.get_i32();
    let picture_aspect_ratio = bytes.get_f32();
    let frame_format_type = bytes.get_i32();
    let line_stride_in_bytes = bytes.get_i32();
    let timecode = bytes.get_i64();
    let len = bytes.get_u32() as usize;
    if bytes.remaining() < len {
        return Err(EnvelopeError::Truncated);
    }
    Ok(VideoFrame {
        xres,
        yres,
        four_cc,
        frame_rate_n,
        frame_rate_d,
        picture_aspect_ratio,
        frame_format_type,
        timecode,
        line_stride_in_bytes,
        data: bytes.split_to(len).to_vec(),
    })
}

fn decode_audio(mut bytes: Bytes) -> Result<AudioFrame, EnvelopeError> {
    if bytes.remaining() < 24 {
        return Err(EnvelopeError::Truncated);
    }
    let sample_rate = bytes.get_i32();
    let no_channels = bytes.get_i32();
    let no_samples = bytes.get_i32();
    let timecode = bytes.get_i64();
    let len = bytes.get_u32() as usize;
    if bytes.remaining() < len * 4 {
        return Err(EnvelopeError::Truncated);
    }
    let mut data = Vec::with_capacity(len);
    for _ in 0..len {
        data.push(bytes.get_f32());
    }
    Ok(AudioFrame {
        sample_rate,
        no_channels,
        no_samples,
        timecode,
        data,
    })
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
    Ok(MetadataFrame {
        timecode,
        data: text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(stride: i32, height: i32) -> VideoFrame {
        VideoFrame {
            xres: 4,
            yres: height,
            four_cc: u32::from_le_bytes(*b"UYVY"),
            frame_rate_n: 30000,
            frame_rate_d: 1001,
            picture_aspect_ratio: 1.7778,
            frame_format_type: 1,
            timecode: 1234,
            line_stride_in_bytes: stride,
            data: (0..(stride * height) as usize).map(|i| i as u8).collect(),
        }
    }

    #[test]
    fn round_trips_video() {
        let original = video(8, 3);
        let Ok(DecodedFrame::Video(back)) = decode(encode_video(&original)) else {
            panic!("expected a video frame")
        };
        assert_eq!(back.xres, original.xres);
        assert_eq!(back.yres, original.yres);
        assert_eq!(back.four_cc, original.four_cc);
        assert_eq!(back.frame_rate_n, original.frame_rate_n);
        assert_eq!(back.frame_rate_d, original.frame_rate_d);
        assert_eq!(back.frame_format_type, original.frame_format_type);
        assert_eq!(back.timecode, original.timecode);
        assert_eq!(back.data, original.data);
    }

    /// The bug the old `grafton-ndi` envelope could not avoid: it rebuilt the
    /// frame from resolution alone, so a stride wider than the pixels (padding
    /// at the end of each row) came back with the rows shifted. Carrying the
    /// stride is what fixes it, so it gets its own test.
    #[test]
    fn round_trips_a_padded_stride() {
        // 4 pixels of UYVY is 8 bytes, but this source pads each row to 12.
        let original = video(12, 3);
        let Ok(DecodedFrame::Video(back)) = decode(encode_video(&original)) else {
            panic!("expected a video frame")
        };
        assert_eq!(back.line_stride_in_bytes, 12);
        assert_eq!(back.data, original.data);
        assert_eq!(back.data.len(), 36);
    }

    #[test]
    fn round_trips_audio() {
        let original = AudioFrame {
            sample_rate: 48_000,
            no_channels: 2,
            no_samples: 4,
            timecode: -1,
            data: vec![0.0, 0.5, -0.5, 1.0, 0.25, -0.25, 0.75, -0.75],
        };
        let Ok(DecodedFrame::Audio(back)) = decode(encode_audio(&original)) else {
            panic!("expected an audio frame")
        };
        assert_eq!(back.sample_rate, original.sample_rate);
        assert_eq!(back.no_channels, original.no_channels);
        assert_eq!(back.no_samples, original.no_samples);
        assert_eq!(back.timecode, original.timecode);
        assert_eq!(back.data, original.data);
    }

    #[test]
    fn round_trips_metadata() {
        let original = MetadataFrame {
            timecode: 99,
            data: "<ndi_capabilities video_quality=\"good\"/>".into(),
        };
        let Ok(DecodedFrame::Metadata(back)) = decode(encode_metadata(&original)) else {
            panic!("expected a metadata frame")
        };
        assert_eq!(back.timecode, original.timecode);
        assert_eq!(back.data, original.data);
    }

    #[test]
    fn rejects_an_empty_envelope() {
        assert!(matches!(
            decode(Bytes::new()),
            Err(EnvelopeError::Truncated)
        ));
    }

    #[test]
    fn rejects_an_unknown_kind() {
        assert!(matches!(
            decode(Bytes::from_static(&[9, 0, 0])),
            Err(EnvelopeError::UnknownKind(9))
        ));
    }

    #[test]
    fn rejects_a_truncated_payload() {
        let mut encoded = encode_video(&video(8, 3)).to_vec();
        encoded.truncate(encoded.len() - 4);
        assert!(matches!(
            decode(Bytes::from(encoded)),
            Err(EnvelopeError::Truncated)
        ));
    }
}
