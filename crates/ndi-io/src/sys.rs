//! Minimal dynamic binding to the NDI runtime — no SDK needed at build time.
//!
//! # Why this exists rather than a crate
//!
//! `grafton-ndi` (which this replaced) runs `bindgen` against the installed SDK
//! headers and links `libndi`. That made this crate unbuildable on any machine
//! without the proprietary SDK, so it sat behind an `ndi` Cargo feature that no
//! cross-compiled release target could turn on — meaning **released binaries
//! shipped with no NDI transport at all**. The same defect was removed from
//! `openstage` first; this is the port of that fix.
//!
//! Loading the library at run time removes the build-time dependency entirely.
//! One binary works whether the operator has the full SDK, the redistributable
//! runtime, or neither — with neither, an NDI endpoint reports a clear error
//! naming the download and the rest of the router (SRT, OMT, media) is
//! untouched.
//!
//! # Why not vendor the library
//!
//! NDI's licence *does* permit redistribution — it is royalty-free — but only
//! if the receiving licence forbids modifying, reverse-engineering and
//! decompiling the SDK. MIT grants exactly those rights, so the binaries cannot
//! live in this tree; a signed installer with its own EULA can carry them, and
//! `scripts/release-lib.sh`'s `rl_ndi_bundle` is how. Only the flat C ABI is
//! bound here, which is interface, not SDK code.
//!
//! # Why flat symbols
//!
//! `NDIlib_recv_create_v3` and friends are exported by every NDI 5 and 6
//! runtime. `NDIlib_v6_load()` returns a versioned struct whose layout changes
//! between SDK generations, so binding it would refuse a v5 runtime for nothing.
//!
//! # Ownership, which is the part that bites
//!
//! Frames handed back by `NDIlib_recv_capture_v3` are **SDK-owned** and must be
//! returned with the matching `NDIlib_recv_free_*`. Rather than expose that
//! lifetime, [`Receiver::capture`] copies each frame into an owned Rust value
//! and frees the SDK's copy before returning. That costs one memcpy per frame,
//! which this crate paid anyway — every captured frame is immediately
//! serialised into an envelope (see [`crate::envelope`]).
//!
//! # Struct layouts
//!
//! The `#[repr(C)]` types below mirror `Processing.NDI.structs.h`, `.Send.h`,
//! `.Recv.h` and `.Find.h` field for field, in declaration order. The tests at
//! the bottom check them against the sizes the headers imply and exercise a
//! real runtime — they are the only thing standing between a layout typo and
//! silent memory corruption. Do not change a struct without running them.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

/// Where to send an operator with no runtime. The SDK publishes this as
/// `NDILIB_REDIST_URL`; it is platform-specific, and empty on Linux because no
/// one-click redistributable exists there, so that case points at the SDK page.
pub const REDIST_URL: &str = if cfg!(target_os = "macos") {
    "http://ndi.link/NDIRedistV6Apple"
} else if cfg!(target_os = "windows") {
    "http://ndi.link/NDIRedistV6"
} else {
    "https://ndi.video/for-developers/ndi-sdk/"
};

/// Set by the redistributable installer, and the documented way to find a
/// runtime that is not on the default loader path.
const REDIST_FOLDER_VAR: &str = "NDI_RUNTIME_DIR_V6";
/// Checked after V6, so a machine with both prefers the newer runtime.
const REDIST_FOLDER_VAR_V5: &str = "NDI_RUNTIME_DIR_V5";

#[cfg(target_os = "macos")]
const LIBRARY_NAMES: &[&str] = &["libndi.dylib"];
#[cfg(target_os = "windows")]
const LIBRARY_NAMES: &[&str] = &["Processing.NDI.Lib.x64.dll"];
#[cfg(all(unix, not(target_os = "macos")))]
const LIBRARY_NAMES: &[&str] = &["libndi.so.6", "libndi.so.5", "libndi.so"];

#[cfg(target_os = "macos")]
const EXTRA_DIRS: &[&str] = &[
    "/Library/NDI SDK for Apple/lib/macOS",
    "/Library/NDI SDK for macOS/lib/macOS",
    "/usr/local/lib",
    "/opt/homebrew/lib",
];
#[cfg(target_os = "windows")]
const EXTRA_DIRS: &[&str] = &[];
#[cfg(all(unix, not(target_os = "macos")))]
const EXTRA_DIRS: &[&str] = &["/usr/local/lib", "/usr/lib"];

#[derive(Debug, Error)]
pub enum SysError {
    #[error("NDI runtime not found (tried {tried}). Install it from {REDIST_URL}, or set {REDIST_FOLDER_VAR} to the directory containing it.")]
    NotFound { tried: String },
    #[error(
        "NDI runtime at {path} is missing {symbol} — it is too old (NDI 5 or newer is needed)"
    )]
    TooOld { path: String, symbol: String },
    #[error("the NDI runtime at {path} refused to initialise — this CPU is not supported by it")]
    Init { path: String },
    #[error("the NDI runtime at {path} would not create {what}")]
    CreateFailed { path: String, what: String },
    #[error("{0} contains an interior NUL byte")]
    BadString(&'static str),
}

// ------------------------------------------------------------ enum values --

pub const FOURCC_FLTP: u32 = u32::from_le_bytes(*b"FLTp");

pub const FRAME_TYPE_NONE: c_int = 0;
pub const FRAME_TYPE_VIDEO: c_int = 1;
pub const FRAME_TYPE_AUDIO: c_int = 2;
pub const FRAME_TYPE_METADATA: c_int = 3;
pub const FRAME_TYPE_ERROR: c_int = 4;

/// `NDIlib_recv_color_format_BGRX_BGRA` — no alpha arrives as BGRX, alpha as
/// BGRA.
///
/// This is what `grafton-ndi` defaulted to, and changing it would change what
/// every downstream receiver of this router gets, so the port keeps it.
///
/// Worth revisiting separately: most NDI sources are UYVY or SpeedHQ on the
/// wire, so this asks the SDK to convert every frame and doubles the payload
/// (4 bytes per pixel against 2). `NDIlib_recv_color_format_fastest` (100)
/// would relay closer to what arrived. That is a behaviour change, not a
/// port, so it is not made here.
pub const RECV_COLOR_FORMAT_BGRX_BGRA: c_int = 0;
/// `NDIlib_recv_bandwidth_highest`.
pub const RECV_BANDWIDTH_HIGHEST: c_int = 100;
/// `NDIlib_send_timecode_synthesize`.
pub const SEND_TIMECODE_SYNTHESIZE: i64 = i64::MAX;

// --------------------------------------------------------------- C structs --

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SourceRaw {
    pub p_ndi_name: *const c_char,
    /// Union in C (`p_url_address` / the deprecated `p_ip_address`).
    pub p_url_address: *const c_char,
}

#[repr(C)]
pub struct FindCreate {
    pub show_local_sources: bool,
    pub p_groups: *const c_char,
    pub p_extra_ips: *const c_char,
}

#[repr(C)]
pub struct RecvCreateV3 {
    pub source_to_connect_to: SourceRaw,
    pub color_format: c_int,
    pub bandwidth: c_int,
    pub allow_video_fields: bool,
    pub p_ndi_recv_name: *const c_char,
}

#[repr(C)]
pub struct SendCreate {
    pub p_ndi_name: *const c_char,
    pub p_groups: *const c_char,
    pub clock_video: bool,
    pub clock_audio: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VideoFrameRaw {
    pub xres: c_int,
    pub yres: c_int,
    pub four_cc: u32,
    pub frame_rate_n: c_int,
    pub frame_rate_d: c_int,
    pub picture_aspect_ratio: f32,
    pub frame_format_type: c_int,
    pub timecode: i64,
    pub p_data: *mut u8,
    /// Union in C (`line_stride_in_bytes` / `data_size_in_bytes`); both `int`.
    pub line_stride_in_bytes: c_int,
    pub p_metadata: *const c_char,
    pub timestamp: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AudioFrameRaw {
    pub sample_rate: c_int,
    pub no_channels: c_int,
    pub no_samples: c_int,
    pub timecode: i64,
    pub four_cc: u32,
    pub p_data: *mut u8,
    /// Union in C (`channel_stride_in_bytes` / `data_size_in_bytes`).
    pub channel_stride_in_bytes: c_int,
    pub p_metadata: *const c_char,
    pub timestamp: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MetadataFrameRaw {
    pub length: c_int,
    pub timecode: i64,
    pub p_data: *mut c_char,
}

// ------------------------------------------------------------ owned frames --
//
// What callers actually see. Each owns its payload, so nothing borrows from the
// SDK and no free is outstanding once `Receiver::capture` returns.

/// One video frame, payload included. `data` is `yres` rows of
/// `line_stride_in_bytes`, in whatever `four_cc` says — this crate relays the
/// SDK's own packing rather than converting it.
#[derive(Clone)]
pub struct VideoFrame {
    pub xres: i32,
    pub yres: i32,
    pub four_cc: u32,
    pub frame_rate_n: i32,
    pub frame_rate_d: i32,
    pub picture_aspect_ratio: f32,
    pub frame_format_type: i32,
    pub timecode: i64,
    pub line_stride_in_bytes: i32,
    pub data: Vec<u8>,
}

/// One audio frame. `data` is planar 32-bit float (`FLTp`), `no_channels`
/// planes of `no_samples`.
#[derive(Clone)]
pub struct AudioFrame {
    pub sample_rate: i32,
    pub no_channels: i32,
    pub no_samples: i32,
    pub timecode: i64,
    pub data: Vec<f32>,
}

#[derive(Clone)]
pub struct MetadataFrame {
    pub timecode: i64,
    pub data: String,
}

pub enum Frame {
    Video(VideoFrame),
    Audio(AudioFrame),
    Metadata(MetadataFrame),
}

// ------------------------------------------------------------ the bindings --

type FnInitialize = unsafe extern "C" fn() -> bool;
type FnVersion = unsafe extern "C" fn() -> *const c_char;

type FnFindCreateV2 = unsafe extern "C" fn(*const FindCreate) -> *mut c_void;
type FnFindDestroy = unsafe extern "C" fn(*mut c_void);
type FnFindGetCurrentSources = unsafe extern "C" fn(*mut c_void, *mut u32) -> *const SourceRaw;
type FnFindWaitForSources = unsafe extern "C" fn(*mut c_void, u32) -> bool;

type FnRecvCreateV3 = unsafe extern "C" fn(*const RecvCreateV3) -> *mut c_void;
type FnRecvDestroy = unsafe extern "C" fn(*mut c_void);
type FnRecvCaptureV3 = unsafe extern "C" fn(
    *mut c_void,
    *mut VideoFrameRaw,
    *mut AudioFrameRaw,
    *mut MetadataFrameRaw,
    u32,
) -> c_int;
type FnRecvFreeVideoV2 = unsafe extern "C" fn(*mut c_void, *const VideoFrameRaw);
type FnRecvFreeAudioV3 = unsafe extern "C" fn(*mut c_void, *const AudioFrameRaw);
type FnRecvFreeMetadata = unsafe extern "C" fn(*mut c_void, *const MetadataFrameRaw);

type FnSendCreate = unsafe extern "C" fn(*const SendCreate) -> *mut c_void;
type FnSendDestroy = unsafe extern "C" fn(*mut c_void);
type FnSendVideoV2 = unsafe extern "C" fn(*mut c_void, *const VideoFrameRaw);
type FnSendAudioV3 = unsafe extern "C" fn(*mut c_void, *const AudioFrameRaw);
type FnSendMetadata = unsafe extern "C" fn(*mut c_void, *const MetadataFrameRaw);

/// The entry points this crate needs, and no more.
pub struct Api {
    // Kept so the library outlives every symbol resolved out of it.
    _lib: libloading::Library,
    pub path: PathBuf,
    pub version: String,

    find_create_v2: FnFindCreateV2,
    find_destroy: FnFindDestroy,
    find_get_current_sources: FnFindGetCurrentSources,
    find_wait_for_sources: FnFindWaitForSources,

    recv_create_v3: FnRecvCreateV3,
    recv_destroy: FnRecvDestroy,
    recv_capture_v3: FnRecvCaptureV3,
    recv_free_video_v2: FnRecvFreeVideoV2,
    recv_free_audio_v3: FnRecvFreeAudioV3,
    recv_free_metadata: FnRecvFreeMetadata,

    send_create: FnSendCreate,
    send_destroy: FnSendDestroy,
    send_video_v2: FnSendVideoV2,
    send_audio_v3: FnSendAudioV3,
    send_metadata: FnSendMetadata,
}

// SAFETY: every entry point bound here is documented thread-safe by the SDK,
// and the instance handles below are only ever touched through `&mut self`.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

/// Loads the runtime once per process. Repeated calls hand back the same
/// [`Api`]; a failure is cached too, so a missing runtime costs one search and
/// not one per endpoint (this crate creates one per input and per output).
pub fn load() -> Result<Arc<Api>, SysError> {
    static ONCE: std::sync::OnceLock<Result<Arc<Api>, SysError>> = std::sync::OnceLock::new();
    match ONCE.get_or_init(load_uncached) {
        Ok(api) => Ok(api.clone()),
        // SysError is not Clone (it carries owned strings), and the cached
        // value cannot be moved out of a OnceLock, so rebuild the equivalent.
        Err(e) => Err(match e {
            SysError::NotFound { tried } => SysError::NotFound {
                tried: tried.clone(),
            },
            SysError::TooOld { path, symbol } => SysError::TooOld {
                path: path.clone(),
                symbol: symbol.clone(),
            },
            SysError::Init { path } => SysError::Init { path: path.clone() },
            SysError::CreateFailed { path, what } => SysError::CreateFailed {
                path: path.clone(),
                what: what.clone(),
            },
            SysError::BadString(s) => SysError::BadString(s),
        }),
    }
}

fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for var in [REDIST_FOLDER_VAR, REDIST_FOLDER_VAR_V5] {
        if let Some(dir) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            for name in LIBRARY_NAMES {
                out.push(PathBuf::from(&dir).join(name));
            }
        }
    }
    // Beside the executable: this is what lets a signed installer ship the
    // library in the app's own folder with no code change.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in LIBRARY_NAMES {
                out.push(dir.join(name));
            }
        }
    }
    // Bare name: the platform's own loader search path.
    for name in LIBRARY_NAMES {
        out.push(PathBuf::from(name));
    }
    for dir in EXTRA_DIRS {
        for name in LIBRARY_NAMES {
            out.push(PathBuf::from(dir).join(name));
        }
    }
    out
}

fn load_uncached() -> Result<Arc<Api>, SysError> {
    let mut found = None;
    for path in candidates() {
        // SAFETY: loading a shared library runs its initialisers. This is the
        // vendor's own runtime, at a path whose shape we control.
        if let Ok(handle) = unsafe { libloading::Library::new(&path) } {
            found = Some((handle, path));
            break;
        }
    }
    let (lib, path) = found.ok_or_else(|| SysError::NotFound {
        tried: LIBRARY_NAMES.join(", "),
    })?;
    let shown = path.display().to_string();

    // SAFETY: each name is bound to the signature declared in the SDK headers
    // (see this module's header comment). `Library::get` is the typed lookup,
    // so no transmute is involved and the compiler checks every later use.
    unsafe {
        macro_rules! required {
            ($ty:ty, $name:literal) => {
                *lib.get::<$ty>(concat!($name, "\0").as_bytes())
                    .map_err(|_| SysError::TooOld {
                        path: shown.clone(),
                        symbol: $name.to_string(),
                    })?
            };
        }

        let initialize = required!(FnInitialize, "NDIlib_initialize");
        if !initialize() {
            return Err(SysError::Init { path: shown });
        }

        let version = lib
            .get::<FnVersion>(b"NDIlib_version\0")
            .ok()
            .map(|f| CStr::from_ptr(f()).to_string_lossy().into_owned())
            .unwrap_or_default();

        Ok(Arc::new(Api {
            find_create_v2: required!(FnFindCreateV2, "NDIlib_find_create_v2"),
            find_destroy: required!(FnFindDestroy, "NDIlib_find_destroy"),
            find_get_current_sources: required!(
                FnFindGetCurrentSources,
                "NDIlib_find_get_current_sources"
            ),
            find_wait_for_sources: required!(FnFindWaitForSources, "NDIlib_find_wait_for_sources"),

            recv_create_v3: required!(FnRecvCreateV3, "NDIlib_recv_create_v3"),
            recv_destroy: required!(FnRecvDestroy, "NDIlib_recv_destroy"),
            recv_capture_v3: required!(FnRecvCaptureV3, "NDIlib_recv_capture_v3"),
            recv_free_video_v2: required!(FnRecvFreeVideoV2, "NDIlib_recv_free_video_v2"),
            recv_free_audio_v3: required!(FnRecvFreeAudioV3, "NDIlib_recv_free_audio_v3"),
            recv_free_metadata: required!(FnRecvFreeMetadata, "NDIlib_recv_free_metadata"),

            send_create: required!(FnSendCreate, "NDIlib_send_create"),
            send_destroy: required!(FnSendDestroy, "NDIlib_send_destroy"),
            send_video_v2: required!(FnSendVideoV2, "NDIlib_send_send_video_v2"),
            send_audio_v3: required!(FnSendAudioV3, "NDIlib_send_send_audio_v3"),
            send_metadata: required!(FnSendMetadata, "NDIlib_send_send_metadata"),

            path,
            version,
            _lib: lib,
        }))
    }
}

// `NDIlib_destroy` is deliberately never called: at process teardown it races
// the SDK's own worker threads, and the process is exiting anyway.

// ------------------------------------------------------------------ Finder --

/// A discovered source. Owns its strings, so it outlives the finder that
/// produced it — the SDK's own array is only valid until the next call.
#[derive(Clone, Debug)]
pub struct Source {
    pub name: String,
    pub url: String,
}

pub struct Finder {
    api: Arc<Api>,
    instance: *mut c_void,
}

// SAFETY: every method takes `&mut self`, so no two calls overlap on one
// instance. The router runs each endpoint on its own blocking thread.
unsafe impl Send for Finder {}

impl Finder {
    pub fn new(api: Arc<Api>, show_local_sources: bool) -> Result<Self, SysError> {
        let settings = FindCreate {
            show_local_sources,
            p_groups: std::ptr::null(),
            p_extra_ips: std::ptr::null(),
        };
        // SAFETY: `settings` outlives the call, which copies what it needs.
        let instance = unsafe { (api.find_create_v2)(&settings) };
        if instance.is_null() {
            return Err(SysError::CreateFailed {
                path: api.path.display().to_string(),
                what: "a finder".into(),
            });
        }
        Ok(Self { api, instance })
    }

    /// `true` if the source list changed before the timeout expired.
    pub fn wait_for_sources(&mut self, timeout: Duration) -> bool {
        // SAFETY: `instance` is non-null for this type's whole lifetime.
        unsafe { (self.api.find_wait_for_sources)(self.instance, timeout.as_millis() as u32) }
    }

    pub fn current_sources(&mut self) -> Vec<Source> {
        let mut count: u32 = 0;
        // SAFETY: the SDK fills `count` and returns an array of that length,
        // owned by the finder and valid only until the next call on it — hence
        // the copy into owned Strings before returning.
        unsafe {
            let ptr = (self.api.find_get_current_sources)(self.instance, &mut count);
            if ptr.is_null() {
                return Vec::new();
            }
            (0..count as usize)
                .map(|i| {
                    let raw = &*ptr.add(i);
                    Source {
                        name: cstr_to_string(raw.p_ndi_name),
                        url: cstr_to_string(raw.p_url_address),
                    }
                })
                .collect()
        }
    }
}

impl Drop for Finder {
    fn drop(&mut self) {
        // SAFETY: called once, on a handle this type exclusively owns.
        unsafe { (self.api.find_destroy)(self.instance) }
    }
}

// ---------------------------------------------------------------- Receiver --

pub struct Receiver {
    api: Arc<Api>,
    instance: *mut c_void,
    // Keeps the strings `RecvCreateV3` pointed at alive past construction. The
    // SDK copies them, but holding them costs nothing and removes the question.
    _name: CString,
    _url: CString,
}

// SAFETY: as `Finder`.
unsafe impl Send for Receiver {}

impl Receiver {
    pub fn connect(api: Arc<Api>, source: &Source, recv_name: &str) -> Result<Self, SysError> {
        let name =
            CString::new(source.name.as_str()).map_err(|_| SysError::BadString("source name"))?;
        let url =
            CString::new(source.url.as_str()).map_err(|_| SysError::BadString("source URL"))?;
        let recv = CString::new(recv_name).map_err(|_| SysError::BadString("receiver name"))?;

        let settings = RecvCreateV3 {
            source_to_connect_to: SourceRaw {
                p_ndi_name: name.as_ptr(),
                p_url_address: url.as_ptr(),
            },
            color_format: RECV_COLOR_FORMAT_BGRX_BGRA,
            bandwidth: RECV_BANDWIDTH_HIGHEST,
            allow_video_fields: true,
            p_ndi_recv_name: recv.as_ptr(),
        };
        // SAFETY: `settings` and every string it points at outlive the call.
        let instance = unsafe { (api.recv_create_v3)(&settings) };
        if instance.is_null() {
            return Err(SysError::CreateFailed {
                path: api.path.display().to_string(),
                what: format!("a receiver for {:?}", source.name),
            });
        }
        Ok(Self {
            api,
            instance,
            _name: name,
            _url: url,
        })
    }

    /// Waits up to `timeout` for one frame of any kind.
    ///
    /// `Ok(None)` means the timeout expired with nothing to report — the normal
    /// idle case, and also what a status/source change reports, since neither
    /// carries a payload this crate relays.
    ///
    /// The SDK's frame is freed before this returns; the value handed back owns
    /// its payload.
    pub fn capture(&mut self, timeout: Duration) -> Option<Frame> {
        let mut video = unsafe { std::mem::zeroed::<VideoFrameRaw>() };
        let mut audio = unsafe { std::mem::zeroed::<AudioFrameRaw>() };
        let mut meta = unsafe { std::mem::zeroed::<MetadataFrameRaw>() };

        // SAFETY: all three out-params are valid for the call; the SDK writes
        // whichever matches the returned frame type and leaves the rest alone.
        // Each is freed through the matching recv_free_* below, exactly once.
        unsafe {
            let kind = (self.api.recv_capture_v3)(
                self.instance,
                &mut video,
                &mut audio,
                &mut meta,
                timeout.as_millis() as u32,
            );
            match kind {
                FRAME_TYPE_VIDEO => {
                    let owned = copy_video(&video);
                    (self.api.recv_free_video_v2)(self.instance, &video);
                    Some(Frame::Video(owned))
                }
                FRAME_TYPE_AUDIO => {
                    let owned = copy_audio(&audio);
                    (self.api.recv_free_audio_v3)(self.instance, &audio);
                    Some(Frame::Audio(owned))
                }
                FRAME_TYPE_METADATA => {
                    let owned = MetadataFrame {
                        timecode: meta.timecode,
                        data: cstr_to_string(meta.p_data),
                    };
                    (self.api.recv_free_metadata)(self.instance, &meta);
                    Some(Frame::Metadata(owned))
                }
                // none / error / status_change / source_change: nothing to
                // relay, and nothing was allocated to free.
                _ => None,
            }
        }
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        // SAFETY: called once, on a handle this type exclusively owns.
        unsafe { (self.api.recv_destroy)(self.instance) }
    }
}

/// SAFETY: `raw` must be a frame the SDK just filled and not yet freed.
unsafe fn copy_video(raw: &VideoFrameRaw) -> VideoFrame {
    // Compressed FourCCs reuse this field as `data_size_in_bytes`, so a
    // negative or absurd stride would be a lie either way; clamp at 0 and let
    // an empty payload surface downstream rather than computing a huge length.
    let len = (raw.line_stride_in_bytes.max(0) as usize) * (raw.yres.max(0) as usize);
    let data = if raw.p_data.is_null() || len == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(raw.p_data, len).to_vec()
    };
    VideoFrame {
        xres: raw.xres,
        yres: raw.yres,
        four_cc: raw.four_cc,
        frame_rate_n: raw.frame_rate_n,
        frame_rate_d: raw.frame_rate_d,
        picture_aspect_ratio: raw.picture_aspect_ratio,
        frame_format_type: raw.frame_format_type,
        timecode: raw.timecode,
        line_stride_in_bytes: raw.line_stride_in_bytes,
        data,
    }
}

/// SAFETY: as `copy_video`.
unsafe fn copy_audio(raw: &AudioFrameRaw) -> AudioFrame {
    // FLTp is planar: `no_channels` planes, each `channel_stride_in_bytes`
    // apart, of `no_samples` f32s. The stride can exceed the sample count
    // (padding between planes), so walk plane by plane rather than reading one
    // flat run — that is what makes this correct for a padded frame.
    let channels = raw.no_channels.max(0) as usize;
    let samples = raw.no_samples.max(0) as usize;
    let stride = raw.channel_stride_in_bytes.max(0) as usize;
    let mut data = Vec::with_capacity(channels * samples);
    if !raw.p_data.is_null() {
        for ch in 0..channels {
            let plane = raw.p_data.add(ch * stride) as *const f32;
            data.extend_from_slice(std::slice::from_raw_parts(plane, samples));
        }
    }
    AudioFrame {
        sample_rate: raw.sample_rate,
        no_channels: raw.no_channels,
        no_samples: raw.no_samples,
        timecode: raw.timecode,
        data,
    }
}

// ------------------------------------------------------------------ Sender --

pub struct Sender {
    api: Arc<Api>,
    instance: *mut c_void,
}

// SAFETY: as `Finder`.
unsafe impl Send for Sender {}

impl Sender {
    pub fn new(api: Arc<Api>, name: &str) -> Result<Self, SysError> {
        let c_name = CString::new(name).map_err(|_| SysError::BadString("sender name"))?;
        let settings = SendCreate {
            p_ndi_name: c_name.as_ptr(),
            p_groups: std::ptr::null(),
            // The router paces frames from whatever it is relaying; letting the
            // SDK clock them as well would fight that.
            clock_video: false,
            clock_audio: false,
        };
        // SAFETY: `settings` and the string it points at outlive the call.
        let instance = unsafe { (api.send_create)(&settings) };
        if instance.is_null() {
            return Err(SysError::CreateFailed {
                path: api.path.display().to_string(),
                what: format!("a sender named {name:?}"),
            });
        }
        Ok(Self { api, instance })
    }

    pub fn send_video(&mut self, frame: &VideoFrame) {
        let raw = VideoFrameRaw {
            xres: frame.xres,
            yres: frame.yres,
            four_cc: frame.four_cc,
            frame_rate_n: frame.frame_rate_n,
            frame_rate_d: frame.frame_rate_d,
            picture_aspect_ratio: frame.picture_aspect_ratio,
            frame_format_type: frame.frame_format_type,
            timecode: frame.timecode,
            p_data: frame.data.as_ptr() as *mut u8,
            line_stride_in_bytes: frame.line_stride_in_bytes,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        };
        // SAFETY: `frame.data` outlives the call, which copies before it
        // returns (this is the synchronous send, not send_video_async_v2).
        unsafe { (self.api.send_video_v2)(self.instance, &raw) }
    }

    pub fn send_audio(&mut self, frame: &AudioFrame) {
        let raw = AudioFrameRaw {
            sample_rate: frame.sample_rate,
            no_channels: frame.no_channels,
            no_samples: frame.no_samples,
            timecode: frame.timecode,
            four_cc: FOURCC_FLTP,
            p_data: frame.data.as_ptr() as *mut u8,
            // Our own buffer is tightly packed: one plane per channel, no
            // padding, so the stride is exactly the samples in a plane.
            channel_stride_in_bytes: frame.no_samples.max(0) * 4,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        };
        // SAFETY: as `send_video`.
        unsafe { (self.api.send_audio_v3)(self.instance, &raw) }
    }

    /// Returns `Err` only when the text cannot be passed to C at all (an
    /// interior NUL); the SDK's own send is infallible.
    pub fn send_metadata(&mut self, frame: &MetadataFrame) -> Result<(), SysError> {
        let text =
            CString::new(frame.data.as_str()).map_err(|_| SysError::BadString("metadata"))?;
        let raw = MetadataFrameRaw {
            // The SDK counts the terminating NUL in `length`.
            length: text.as_bytes_with_nul().len() as c_int,
            timecode: frame.timecode,
            p_data: text.as_ptr() as *mut c_char,
        };
        // SAFETY: `text` outlives the call, which copies what it needs.
        unsafe { (self.api.send_metadata)(self.instance, &raw) }
        Ok(())
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        // SAFETY: called once, on a handle this type exclusively owns.
        unsafe { (self.api.send_destroy)(self.instance) }
    }
}

/// SAFETY: `p` must be NUL-terminated or null.
unsafe fn cstr_to_string(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sizes *and field offsets*, checked against numbers printed by a C
    /// program compiled against the real SDK headers — not against arithmetic
    /// done by hand, which is what got this wrong the first time. Offsets
    /// matter as much as sizes: two same-typed fields in the wrong order give
    /// an identical size and corrupt every frame.
    ///
    ///     SourceRaw 16  FindCreate 24  SendCreate 24  RecvCreateV3 40
    ///     VideoFrameRaw 72  AudioFrameRaw 64  MetadataFrameRaw 24
    #[test]
    fn struct_layouts_match_the_c_headers() {
        assert_eq!(
            std::mem::size_of::<*const c_char>(),
            8,
            "these expectations assume a 64-bit target"
        );

        assert_eq!(std::mem::size_of::<SourceRaw>(), 16);
        assert_eq!(std::mem::size_of::<FindCreate>(), 24);
        assert_eq!(std::mem::size_of::<SendCreate>(), 24);
        assert_eq!(std::mem::size_of::<MetadataFrameRaw>(), 24);

        assert_eq!(std::mem::size_of::<RecvCreateV3>(), 40);
        assert_eq!(std::mem::offset_of!(RecvCreateV3, source_to_connect_to), 0);
        assert_eq!(std::mem::offset_of!(RecvCreateV3, color_format), 16);
        assert_eq!(std::mem::offset_of!(RecvCreateV3, bandwidth), 20);
        assert_eq!(std::mem::offset_of!(RecvCreateV3, allow_video_fields), 24);
        assert_eq!(std::mem::offset_of!(RecvCreateV3, p_ndi_recv_name), 32);

        assert_eq!(std::mem::size_of::<VideoFrameRaw>(), 72);
        assert_eq!(std::mem::align_of::<VideoFrameRaw>(), 8);
        assert_eq!(std::mem::offset_of!(VideoFrameRaw, xres), 0);
        assert_eq!(std::mem::offset_of!(VideoFrameRaw, four_cc), 8);
        assert_eq!(std::mem::offset_of!(VideoFrameRaw, timecode), 32);
        assert_eq!(std::mem::offset_of!(VideoFrameRaw, p_data), 40);
        assert_eq!(
            std::mem::offset_of!(VideoFrameRaw, line_stride_in_bytes),
            48
        );
        assert_eq!(std::mem::offset_of!(VideoFrameRaw, p_metadata), 56);
        assert_eq!(std::mem::offset_of!(VideoFrameRaw, timestamp), 64);

        assert_eq!(std::mem::size_of::<AudioFrameRaw>(), 64);
        assert_eq!(std::mem::align_of::<AudioFrameRaw>(), 8);
        assert_eq!(std::mem::offset_of!(AudioFrameRaw, sample_rate), 0);
        assert_eq!(std::mem::offset_of!(AudioFrameRaw, no_channels), 4);
        assert_eq!(std::mem::offset_of!(AudioFrameRaw, no_samples), 8);
        assert_eq!(std::mem::offset_of!(AudioFrameRaw, timecode), 16);
        assert_eq!(std::mem::offset_of!(AudioFrameRaw, four_cc), 24);
        assert_eq!(std::mem::offset_of!(AudioFrameRaw, p_data), 32);
        assert_eq!(
            std::mem::offset_of!(AudioFrameRaw, channel_stride_in_bytes),
            40
        );
        assert_eq!(std::mem::offset_of!(AudioFrameRaw, p_metadata), 48);
        assert_eq!(std::mem::offset_of!(AudioFrameRaw, timestamp), 56);
    }

    #[test]
    fn fltp_fourcc_matches_the_sdk_macro() {
        // NDI_LIB_FOURCC('F','L','T','p') packs little-endian, so the bytes
        // read F,L,T,p in memory. Note the lowercase 'p'.
        assert_eq!(FOURCC_FLTP.to_le_bytes(), *b"FLTp");
    }

    /// Only runs where a runtime is installed. This is the test that proves the
    /// bindings work against the real library rather than agreeing with
    /// themselves — a wrong layout usually shows up as a crash inside the SDK.
    #[test]
    fn round_trips_a_real_frame_through_the_runtime() {
        let Ok(api) = load() else {
            eprintln!("no NDI runtime installed — skipping");
            return;
        };
        assert!(!api.version.is_empty(), "runtime reported no version");

        let mut sender = Sender::new(api.clone(), "srt-router-sys-test")
            .expect("creating a sender should succeed");

        // Send one of each kind. The point is that the SDK accepts our struct
        // layouts without crashing or rejecting them.
        let (w, h) = (32_i32, 16_i32);
        let stride = w * 2; // UYVY: 2 bytes per pixel
        let video = VideoFrame {
            xres: w,
            yres: h,
            four_cc: u32::from_le_bytes(*b"UYVY"),
            frame_rate_n: 30,
            frame_rate_d: 1,
            picture_aspect_ratio: 0.0,
            frame_format_type: 1, // progressive
            timecode: SEND_TIMECODE_SYNTHESIZE,
            line_stride_in_bytes: stride,
            data: vec![0x80; (stride * h) as usize],
        };
        sender.send_video(&video);

        let audio = AudioFrame {
            sample_rate: 48_000,
            no_channels: 2,
            no_samples: 480,
            timecode: SEND_TIMECODE_SYNTHESIZE,
            data: vec![0.25; 2 * 480],
        };
        sender.send_audio(&audio);

        sender
            .send_metadata(&MetadataFrame {
                timecode: SEND_TIMECODE_SYNTHESIZE,
                data: "<test/>".into(),
            })
            .expect("metadata without an interior NUL should send");

        // And discovery finds the sender we just published.
        let mut finder = Finder::new(api, true).expect("creating a finder should succeed");
        let mut seen = false;
        for _ in 0..10 {
            finder.wait_for_sources(Duration::from_millis(500));
            if finder
                .current_sources()
                .iter()
                .any(|s| s.name.contains("srt-router-sys-test"))
            {
                seen = true;
                break;
            }
        }
        assert!(seen, "the finder never discovered our own sender");
    }

    #[test]
    fn metadata_with_an_interior_nul_is_rejected_not_truncated() {
        let Ok(api) = load() else { return };
        let mut sender = match Sender::new(api, "srt-router-nul-test") {
            Ok(s) => s,
            Err(_) => return,
        };
        let err = sender.send_metadata(&MetadataFrame {
            timecode: 0,
            data: "bad\0value".into(),
        });
        assert!(matches!(err, Err(SysError::BadString(_))));
    }
}
