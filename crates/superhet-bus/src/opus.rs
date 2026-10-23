//! Minimal safe wrapper around the system libopus (BSD-3-Clause, dynamically linked) for what
//! Superhet needs: a 48 kHz mono or stereo float encoder (audio application, VBR) and a matching
//! decoder. Stereo PCM is interleaved (`L R L R ...`).
//!
//! Hand-written FFI like `superhet-device`'s librtlsdr backend; the unsafe code is confined to
//! this module. Enabled by the cargo feature `opus` (needs `libopus-dev` at build time).

#![allow(unsafe_code)] // FFI to libopus; every unsafe block carries a SAFETY comment.

use std::ffi::{CStr, c_char, c_int};
use std::ptr::NonNull;

mod ffi {
    use super::{c_char, c_int};

    #[repr(C)]
    pub struct OpusEncoder {
        _private: [u8; 0],
    }
    #[repr(C)]
    pub struct OpusDecoder {
        _private: [u8; 0],
    }

    pub const OPUS_APPLICATION_AUDIO: c_int = 2049;
    pub const OPUS_SET_BITRATE_REQUEST: c_int = 4002;
    pub const OPUS_SET_VBR_REQUEST: c_int = 4006;
    pub const OPUS_SET_COMPLEXITY_REQUEST: c_int = 4010;

    #[link(name = "opus")]
    unsafe extern "C" {
        pub fn opus_encoder_create(
            fs: i32,
            channels: c_int,
            application: c_int,
            error: *mut c_int,
        ) -> *mut OpusEncoder;
        pub fn opus_encoder_destroy(st: *mut OpusEncoder);
        pub fn opus_encoder_ctl(st: *mut OpusEncoder, request: c_int, ...) -> c_int;
        pub fn opus_encode_float(
            st: *mut OpusEncoder,
            pcm: *const f32,
            frame_size: c_int,
            data: *mut u8,
            max_data_bytes: i32,
        ) -> i32;
        pub fn opus_decoder_create(fs: i32, channels: c_int, error: *mut c_int)
        -> *mut OpusDecoder;
        pub fn opus_decoder_destroy(st: *mut OpusDecoder);
        pub fn opus_decode_float(
            st: *mut OpusDecoder,
            data: *const u8,
            len: i32,
            pcm: *mut f32,
            frame_size: c_int,
            decode_fec: c_int,
        ) -> c_int;
        pub fn opus_strerror(error: c_int) -> *const c_char;
    }
}

/// libopus reported an error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("opus: {message} (code {code})")]
pub struct OpusError {
    /// Negative libopus error code.
    pub code: i32,
    /// Text from `opus_strerror`.
    pub message: String,
}

impl OpusError {
    fn from_code(code: c_int) -> Self {
        // SAFETY: opus_strerror returns a pointer to a static NUL-terminated string (or NULL
        // defensively, which is handled).
        let message = unsafe {
            let ptr = ffi::opus_strerror(code);
            if ptr.is_null() {
                String::from("unknown error")
            } else {
                CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        };
        Self { code, message }
    }
}

fn check(code: c_int) -> Result<c_int, OpusError> {
    if code < 0 {
        Err(OpusError::from_code(code))
    } else {
        Ok(code)
    }
}

/// Sample rate of both coder types.
pub const SAMPLE_RATE: i32 = 48_000;

/// 48 kHz Opus encoder (application `audio`, VBR on), mono or stereo.
pub struct Encoder {
    ptr: NonNull<ffi::OpusEncoder>,
    channels: usize,
}

// SAFETY: the state is only reachable through `&mut self`/`&self` of the owning value; libopus
// encoders have no thread affinity.
unsafe impl Send for Encoder {}

impl Encoder {
    /// Create a mono encoder with the given bitrate in bits per second.
    ///
    /// # Errors
    /// Fails if libopus rejects the parameters or runs out of memory.
    pub fn new(bitrate: i32) -> Result<Self, OpusError> {
        Self::with_channels(1, bitrate)
    }

    /// Create an encoder for 1 (mono) or 2 (interleaved stereo) channels.
    ///
    /// # Errors
    /// Fails if libopus rejects the parameters (e.g. other channel counts) or runs out of memory.
    pub fn with_channels(channels: usize, bitrate: i32) -> Result<Self, OpusError> {
        let ch = c_int::try_from(channels).map_err(|_| OpusError::from_code(-1))?;
        let mut error: c_int = 0;
        // SAFETY: `error` is a valid out pointer; the returned state is checked for NULL.
        let raw = unsafe {
            ffi::opus_encoder_create(SAMPLE_RATE, ch, ffi::OPUS_APPLICATION_AUDIO, &raw mut error)
        };
        check(error)?;
        let ptr = NonNull::new(raw).ok_or_else(|| OpusError::from_code(-7))?;
        let mut encoder = Self { ptr, channels };
        encoder.set_vbr(true)?;
        encoder.set_bitrate(bitrate)?;
        Ok(encoder)
    }

    fn ctl(&mut self, request: c_int, value: i32) -> Result<(), OpusError> {
        // SAFETY: `ptr` is a live encoder; the requests used here take a single opus_int32.
        check(unsafe { ffi::opus_encoder_ctl(self.ptr.as_ptr(), request, value) }).map(|_| ())
    }

    /// Set the target bitrate in bits per second.
    ///
    /// # Errors
    /// Fails if libopus rejects the value.
    pub fn set_bitrate(&mut self, bitrate: i32) -> Result<(), OpusError> {
        self.ctl(ffi::OPUS_SET_BITRATE_REQUEST, bitrate)
    }

    /// Enable or disable variable bitrate.
    ///
    /// # Errors
    /// Fails if libopus rejects the value.
    pub fn set_vbr(&mut self, enabled: bool) -> Result<(), OpusError> {
        self.ctl(ffi::OPUS_SET_VBR_REQUEST, i32::from(enabled))
    }

    /// Set the computational complexity (0..=10).
    ///
    /// # Errors
    /// Fails if libopus rejects the value.
    pub fn set_complexity(&mut self, complexity: u8) -> Result<(), OpusError> {
        self.ctl(ffi::OPUS_SET_COMPLEXITY_REQUEST, i32::from(complexity))
    }

    /// Channel count the encoder was created with.
    #[must_use]
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Encode one frame (`pcm.len() / channels` must be 120, 240, 480, 960, 1920 or 2880 samples
    /// per channel; stereo PCM is interleaved) into `out` and return the packet length.
    ///
    /// # Errors
    /// Fails on an invalid frame size or a too small output buffer.
    pub fn encode_float(&mut self, pcm: &[f32], out: &mut [u8]) -> Result<usize, OpusError> {
        if pcm.len() % self.channels != 0 {
            return Err(OpusError::from_code(-1));
        }
        let frame =
            c_int::try_from(pcm.len() / self.channels).map_err(|_| OpusError::from_code(-1))?;
        let max = i32::try_from(out.len()).unwrap_or(i32::MAX);
        // SAFETY: `pcm` holds `frame` samples and `out` at least `max` bytes; the encoder is live.
        let n = check(unsafe {
            ffi::opus_encode_float(
                self.ptr.as_ptr(),
                pcm.as_ptr(),
                frame,
                out.as_mut_ptr(),
                max,
            )
        })?;
        Ok(n as usize)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: `ptr` came from opus_encoder_create and is destroyed exactly once.
        unsafe { ffi::opus_encoder_destroy(self.ptr.as_ptr()) };
    }
}

/// 48 kHz Opus decoder, mono or stereo.
pub struct Decoder {
    ptr: NonNull<ffi::OpusDecoder>,
    channels: usize,
}

// SAFETY: see `Encoder`.
unsafe impl Send for Decoder {}

impl Decoder {
    /// Create a mono decoder.
    ///
    /// # Errors
    /// Fails if libopus runs out of memory.
    pub fn new() -> Result<Self, OpusError> {
        Self::with_channels(1)
    }

    /// Create a decoder for 1 (mono) or 2 (interleaved stereo) channels.
    ///
    /// # Errors
    /// Fails if libopus rejects the channel count or runs out of memory.
    pub fn with_channels(channels: usize) -> Result<Self, OpusError> {
        let ch = c_int::try_from(channels).map_err(|_| OpusError::from_code(-1))?;
        let mut error: c_int = 0;
        // SAFETY: `error` is a valid out pointer; the returned state is checked for NULL.
        let raw = unsafe { ffi::opus_decoder_create(SAMPLE_RATE, ch, &raw mut error) };
        check(error)?;
        let ptr = NonNull::new(raw).ok_or_else(|| OpusError::from_code(-7))?;
        Ok(Self { ptr, channels })
    }

    /// Decode one packet into `pcm` (which bounds the frame size; stereo is interleaved) and
    /// return the number of samples written per channel.
    ///
    /// # Errors
    /// Fails on a corrupt packet or a too small output buffer.
    pub fn decode_float(&mut self, packet: &[u8], pcm: &mut [f32]) -> Result<usize, OpusError> {
        let len = i32::try_from(packet.len()).map_err(|_| OpusError::from_code(-1))?;
        let frame =
            c_int::try_from(pcm.len() / self.channels).map_err(|_| OpusError::from_code(-1))?;
        // SAFETY: `packet` has `len` bytes and `pcm` room for `frame` samples; the decoder is live.
        let n = check(unsafe {
            ffi::opus_decode_float(
                self.ptr.as_ptr(),
                packet.as_ptr(),
                len,
                pcm.as_mut_ptr(),
                frame,
                0,
            )
        })?;
        Ok(n as usize)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: `ptr` came from opus_decoder_create and is destroyed exactly once.
        unsafe { ffi::opus_decoder_destroy(self.ptr.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frame: usize, offset: usize) -> Vec<f32> {
        (0..frame)
            .map(|i| {
                let t = (offset + i) as f32 / 48_000.0;
                0.5 * (std::f32::consts::TAU * 1_000.0 * t).sin()
            })
            .collect()
    }

    #[test]
    fn round_trips_a_tone() {
        let mut encoder = Encoder::new(24_000).expect("encoder");
        encoder.set_complexity(5).expect("complexity");
        let mut decoder = Decoder::new().expect("decoder");
        let mut packet = [0u8; 1500];
        let mut pcm = vec![0f32; 960];
        let mut rms = 0.0;
        for frame in 0..20 {
            let n = encoder
                .encode_float(&sine(960, frame * 960), &mut packet)
                .expect("encode");
            assert!((1..400).contains(&n), "packet of {n} bytes");
            let decoded = decoder
                .decode_float(&packet[..n], &mut pcm)
                .expect("decode");
            assert_eq!(decoded, 960);
            rms = (pcm.iter().map(|x| x * x).sum::<f32>() / 960.0).sqrt();
        }
        // 0.5 amplitude sine → rms 0.354.
        assert!((rms - 0.354).abs() < 0.08, "rms {rms}");
    }

    #[test]
    fn round_trips_a_stereo_signal_keeping_the_channels_apart() {
        let mut encoder = Encoder::with_channels(2, 96_000).expect("encoder");
        assert_eq!(encoder.channels(), 2);
        let mut decoder = Decoder::with_channels(2).expect("decoder");
        let mut packet = [0u8; 1500];
        let mut pcm = vec![0f32; 1920];
        let (mut left, mut right) = (0.0f32, 0.0f32);
        for frame in 0..20 {
            // 1 kHz in the left channel only.
            let input: Vec<f32> = sine(960, frame * 960)
                .into_iter()
                .flat_map(|s| [s, 0.0])
                .collect();
            let n = encoder.encode_float(&input, &mut packet).expect("encode");
            let decoded = decoder
                .decode_float(&packet[..n], &mut pcm)
                .expect("decode");
            assert_eq!(decoded, 960);
            left = pcm.iter().step_by(2).map(|x| x * x).sum::<f32>() / 960.0;
            right = pcm.iter().skip(1).step_by(2).map(|x| x * x).sum::<f32>() / 960.0;
        }
        assert!(left.sqrt() > 0.25, "left rms {}", left.sqrt());
        assert!(right.sqrt() < 0.03, "right rms {}", right.sqrt());
        // An odd number of samples cannot be interleaved stereo.
        assert!(encoder.encode_float(&[0.0; 961], &mut packet).is_err());
    }

    #[test]
    fn silence_is_tiny_and_errors_are_mapped() {
        let mut encoder = Encoder::new(64_000).expect("encoder");
        let mut packet = [0u8; 1500];
        let mut last = usize::MAX;
        for _ in 0..10 {
            last = encoder
                .encode_float(&[0.0; 960], &mut packet)
                .expect("encode");
        }
        assert!(last < 20, "silence packet of {last} bytes");

        let err = encoder
            .encode_float(&[0.0; 100], &mut packet)
            .expect_err("bad frame size");
        assert!(err.code < 0 && !err.message.is_empty(), "{err}");
        let mut decoder = Decoder::new().expect("decoder");
        // A buffer smaller than the packet's frame is rejected.
        let n = encoder
            .encode_float(&sine(960, 0), &mut packet)
            .expect("encode");
        assert!(decoder.decode_float(&packet[..n], &mut [0.0; 100]).is_err());
    }
}
