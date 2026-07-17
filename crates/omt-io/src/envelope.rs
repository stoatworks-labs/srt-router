//! Wire format for one OMT frame as an opaque `Bytes` blob — the same
//! pattern as `ndi-io`'s envelope, for the same reason: OMT has no single
//! opaque payload the way SRT/MPEG-TS does, so a receiver encodes an
//! `OMTMediaFrame`'s fields + data into this shape and a sender decodes it
//! back into a fresh `OMTMediaFrame` on the other side.

use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::sys::OMTMediaFrame;

const HEADER_LEN: usize = 64;

/// `data` is passed separately (not read from `frame.data`) since the
/// caller already has it as a safe Rust slice copied out of the raw
/// pointer — see `run_receiver` in `lib.rs`.
pub fn encode(frame: &OMTMediaFrame, data: &[u8]) -> Bytes {
    let mut buf = BytesMut::with_capacity(HEADER_LEN + data.len());
    buf.put_i32(frame.frame_type);
    buf.put_i64(frame.timestamp);
    buf.put_i32(frame.codec);
    buf.put_i32(frame.width);
    buf.put_i32(frame.height);
    buf.put_i32(frame.stride);
    buf.put_i32(frame.flags);
    buf.put_i32(frame.frame_rate_n);
    buf.put_i32(frame.frame_rate_d);
    buf.put_f32(frame.aspect_ratio);
    buf.put_i32(frame.color_space);
    buf.put_i32(frame.sample_rate);
    buf.put_i32(frame.channels);
    buf.put_i32(frame.samples_per_channel);
    buf.put_u32(data.len() as u32);
    buf.put_slice(data);
    buf.freeze()
}

pub struct DecodedFrame {
    pub frame_type: i32,
    pub timestamp: i64,
    pub codec: i32,
    pub width: i32,
    pub height: i32,
    pub stride: i32,
    pub flags: i32,
    pub frame_rate_n: i32,
    pub frame_rate_d: i32,
    pub aspect_ratio: f32,
    pub color_space: i32,
    pub sample_rate: i32,
    pub channels: i32,
    pub samples_per_channel: i32,
    pub data: Vec<u8>,
}

/// `None` on truncated/malformed input rather than panicking — this reads
/// data that arrived over a broadcast channel from a receiver, not
/// something we can `?` a parse error up from without a real error type,
/// and the caller (`run_sender`) already treats "couldn't use this frame"
/// as a log-and-skip situation, same as `ndi-io`'s decode errors.
pub fn decode(mut bytes: Bytes) -> Option<DecodedFrame> {
    if bytes.remaining() < HEADER_LEN {
        return None;
    }
    let frame_type = bytes.get_i32();
    let timestamp = bytes.get_i64();
    let codec = bytes.get_i32();
    let width = bytes.get_i32();
    let height = bytes.get_i32();
    let stride = bytes.get_i32();
    let flags = bytes.get_i32();
    let frame_rate_n = bytes.get_i32();
    let frame_rate_d = bytes.get_i32();
    let aspect_ratio = bytes.get_f32();
    let color_space = bytes.get_i32();
    let sample_rate = bytes.get_i32();
    let channels = bytes.get_i32();
    let samples_per_channel = bytes.get_i32();
    let len = bytes.get_u32() as usize;
    if bytes.remaining() < len {
        return None;
    }
    let data = bytes.split_to(len).to_vec();
    Some(DecodedFrame {
        frame_type,
        timestamp,
        codec,
        width,
        height,
        stride,
        flags,
        frame_rate_n,
        frame_rate_d,
        aspect_ratio,
        color_space,
        sample_rate,
        channels,
        samples_per_channel,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_frame() {
        let frame = OMTMediaFrame {
            frame_type: 2,
            timestamp: 12345,
            codec: 0x5956_5955, // UYVY
            width: 640,
            height: 360,
            stride: 1280,
            flags: 0,
            frame_rate_n: 30,
            frame_rate_d: 1,
            aspect_ratio: 16.0 / 9.0,
            color_space: 709,
            sample_rate: 0,
            channels: 0,
            samples_per_channel: 0,
            data: std::ptr::null_mut(),
            data_length: 0,
            compressed_data: std::ptr::null_mut(),
            compressed_length: 0,
            frame_metadata: std::ptr::null_mut(),
            frame_metadata_length: 0,
        };
        let payload = vec![1u8, 2, 3, 4, 5];
        let encoded = encode(&frame, &payload);
        let decoded = decode(encoded).expect("decode");
        assert_eq!(decoded.frame_type, 2);
        assert_eq!(decoded.width, 640);
        assert_eq!(decoded.height, 360);
        assert_eq!(decoded.aspect_ratio, 16.0 / 9.0);
        assert_eq!(decoded.data, payload);
    }

    #[test]
    fn rejects_truncated_input() {
        assert!(decode(Bytes::from_static(b"short")).is_none());
    }
}
