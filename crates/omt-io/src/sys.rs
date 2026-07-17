//! Hand-transcribed FFI declarations for `libomt.h` (OMT SDK, MIT license,
//! <https://github.com/openmediatransport/libomtnet>). No bindgen: the C
//! API is small and stable enough that transcribing it directly is less
//! risk than adding a bindgen build step for a still-young SDK with no
//! guarantee of stable header paths across platforms.
//!
//! Struct field order/types here must exactly match `OMTMediaFrame` in the
//! header — `#[repr(C)]` then reproduces the same alignment/padding the C
//! compiler would, but only if the fields themselves are declared in the
//! same order. Verified against the real header pulled from the v1.0.0.16
//! release zip.

#![allow(non_camel_case_types, non_snake_case, dead_code)]

use std::ffi::{c_char, c_int, c_void};

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OMTFrameType {
    None = 0,
    Metadata = 1,
    Video = 2,
    Audio = 4,
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OMTCodec {
    VMX1 = 0x3158_4D56,
    FPA1 = 0x3141_5046,
    UYVY = 0x5956_5955,
    YUY2 = 0x3259_5559,
    BGRA = 0x4152_4742,
    NV12 = 0x3231_564E,
    YV12 = 0x3231_5659,
    UYVA = 0x4156_5955,
    P216 = 0x3631_3250,
    PA16 = 0x3631_4150,
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OMTQuality {
    Default = 0,
    Low = 1,
    Medium = 50,
    High = 100,
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OMTPreferredVideoFormat {
    UYVY = 0,
    UYVYorBGRA = 1,
    BGRA = 2,
    UYVYorUYVA = 3,
    UYVYorUYVAorP216orPA16 = 4,
    P216 = 5,
}

/// Bitflags (`OMTFrameType` values are ORed to request multiple types from
/// one receive call) — kept as a raw `i32` newtype rather than the enum
/// above so `Video as i32 | Audio as i32` etc. is straightforward at call
/// sites without a bitflags dependency for three values.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OMTFrameTypeMask(pub i32);
impl OMTFrameTypeMask {
    pub const NONE: Self = Self(0);
    pub const METADATA: Self = Self(1);
    pub const VIDEO: Self = Self(2);
    pub const AUDIO: Self = Self(4);
    pub const ALL: Self = Self(1 | 2 | 4);
}
impl std::ops::BitOr for OMTFrameTypeMask {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OMTReceiveFlags {
    None = 0,
    Preview = 1,
    IncludeCompressed = 2,
    CompressedOnly = 4,
}

/// Mirrors `OMTMediaFrame` field-for-field. See module docs on why layout
/// correctness here matters and how it's ensured.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OMTMediaFrame {
    pub frame_type: i32, // OMTFrameType
    pub timestamp: i64,
    pub codec: i32, // OMTCodec
    pub width: c_int,
    pub height: c_int,
    pub stride: c_int,
    pub flags: i32, // OMTVideoFlags
    pub frame_rate_n: c_int,
    pub frame_rate_d: c_int,
    pub aspect_ratio: f32,
    pub color_space: i32, // OMTColorSpace
    pub sample_rate: c_int,
    pub channels: c_int,
    pub samples_per_channel: c_int,
    pub data: *mut c_void,
    pub data_length: c_int,
    pub compressed_data: *mut c_void,
    pub compressed_length: c_int,
    pub frame_metadata: *mut c_void,
    pub frame_metadata_length: c_int,
}

impl Default for OMTMediaFrame {
    /// The header requires zeroing before use (`OMTMediaFrame frame = {};`)
    /// — this is the Rust equivalent.
    fn default() -> Self {
        // SAFETY: an all-zero bit pattern is a valid OMTMediaFrame per the
        // header's own documented construction requirement, and every
        // field here is a plain integer, float, or nullable pointer (all
        // valid when zeroed).
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
pub struct OMTTally {
    pub preview: c_int,
    pub program: c_int,
}

/// Opaque handles — `omt_receive_t`/`omt_send_t` are never constructed or
/// read from Rust, only passed back to the library that created them.
#[repr(C)]
pub struct omt_receive_t {
    _private: [u8; 0],
}
#[repr(C)]
pub struct omt_send_t {
    _private: [u8; 0],
}

#[link(name = "omt")]
extern "C" {
    pub fn omt_discovery_getaddresses(count: *mut c_int) -> *mut *mut c_char;

    pub fn omt_receive_create(
        address: *const c_char,
        frame_types: OMTFrameTypeMask,
        format: OMTPreferredVideoFormat,
        flags: OMTReceiveFlags,
    ) -> *mut omt_receive_t;
    pub fn omt_receive_destroy(instance: *mut omt_receive_t);
    pub fn omt_receive(
        instance: *mut omt_receive_t,
        frame_types: OMTFrameTypeMask,
        timeout_milliseconds: c_int,
    ) -> *mut OMTMediaFrame;

    pub fn omt_send_create(name: *const c_char, quality: OMTQuality) -> *mut omt_send_t;
    pub fn omt_send_destroy(instance: *mut omt_send_t);
    pub fn omt_send(instance: *mut omt_send_t, frame: *mut OMTMediaFrame) -> c_int;
}
