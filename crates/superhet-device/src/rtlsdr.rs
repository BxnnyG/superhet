//! RTL-SDR backend on top of the system librtlsdr (dynamically linked, GPL-2.0-or-later;
//! see `docs/licenses.md`). Samples are read with `rtlsdr_read_async` on a dedicated thread
//! and forwarded through a bounded channel; blocks that do not fit are dropped and counted.
#![allow(unsafe_code)] // FFI to librtlsdr; every unsafe block carries a SAFETY comment.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::Duration;

use num_complex::Complex32;

use crate::rtlsdr_config::{DeviceSelector, Gain, RtlSdrConfig, validate_rate};
use crate::{Device, Error, IqBlock, IqStream};

/// Minimal hand-written bindings for the parts of `rtl-sdr.h` we use.
mod ffi {
    use std::ffi::{c_char, c_int, c_void};

    /// Opaque `rtlsdr_dev_t`.
    #[repr(C)]
    pub struct RtlsdrDev {
        _private: [u8; 0],
    }

    pub type ReadAsyncCb = unsafe extern "C" fn(buf: *mut u8, len: u32, ctx: *mut c_void);

    #[link(name = "rtlsdr")]
    unsafe extern "C" {
        pub fn rtlsdr_get_device_count() -> u32;
        pub fn rtlsdr_get_device_name(index: u32) -> *const c_char;
        pub fn rtlsdr_get_device_usb_strings(
            index: u32,
            manufact: *mut c_char,
            product: *mut c_char,
            serial: *mut c_char,
        ) -> c_int;
        pub fn rtlsdr_get_index_by_serial(serial: *const c_char) -> c_int;
        pub fn rtlsdr_open(dev: *mut *mut RtlsdrDev, index: u32) -> c_int;
        pub fn rtlsdr_close(dev: *mut RtlsdrDev) -> c_int;
        pub fn rtlsdr_set_center_freq(dev: *mut RtlsdrDev, freq: u32) -> c_int;
        pub fn rtlsdr_get_center_freq(dev: *mut RtlsdrDev) -> u32;
        pub fn rtlsdr_set_sample_rate(dev: *mut RtlsdrDev, rate: u32) -> c_int;
        pub fn rtlsdr_get_sample_rate(dev: *mut RtlsdrDev) -> u32;
        pub fn rtlsdr_set_tuner_gain_mode(dev: *mut RtlsdrDev, manual: c_int) -> c_int;
        pub fn rtlsdr_set_tuner_gain(dev: *mut RtlsdrDev, gain: c_int) -> c_int;
        pub fn rtlsdr_get_tuner_gains(dev: *mut RtlsdrDev, gains: *mut c_int) -> c_int;
        pub fn rtlsdr_get_tuner_type(dev: *mut RtlsdrDev) -> c_int;
        pub fn rtlsdr_set_agc_mode(dev: *mut RtlsdrDev, on: c_int) -> c_int;
        pub fn rtlsdr_set_freq_correction(dev: *mut RtlsdrDev, ppm: c_int) -> c_int;
        pub fn rtlsdr_set_bias_tee(dev: *mut RtlsdrDev, on: c_int) -> c_int;
        pub fn rtlsdr_reset_buffer(dev: *mut RtlsdrDev) -> c_int;
        pub fn rtlsdr_read_async(
            dev: *mut RtlsdrDev,
            cb: ReadAsyncCb,
            ctx: *mut c_void,
            buf_num: u32,
            buf_len: u32,
        ) -> c_int;
        pub fn rtlsdr_cancel_async(dev: *mut RtlsdrDev) -> c_int;
    }
}

/// Bytes per USB transfer in async mode (multiple of 512; 16 ms at 2.048 MS/s).
const ASYNC_BUF_LEN: u32 = 65_536;
/// `rtlsdr_set_freq_correction` returns this when the value is unchanged.
const ERR_UNCHANGED: c_int = -2;

fn dev_err(what: &str, code: c_int) -> Error {
    Error::Io(io::Error::other(format!(
        "librtlsdr {what} failed ({code})"
    )))
}

fn check(what: &str, code: c_int) -> Result<(), Error> {
    if code < 0 {
        Err(dev_err(what, code))
    } else {
        Ok(())
    }
}

/// Owning wrapper around an open `rtlsdr_dev_t`; closes it on drop.
struct Handle {
    dev: NonNull<ffi::RtlsdrDev>,
}

// SAFETY: librtlsdr devices are not tied to the opening thread (libusb is thread-safe). We
// uphold the one usage rule that matters: control calls are made only from the owner (through
// `&mut RtlSdrDevice`), while the streaming thread only calls `rtlsdr_read_async` and
// `rtlsdr_cancel_async`, the same split used by upstream's `rtl_tcp`. `Drop` runs only when
// the last `Arc` is gone, i.e. after the streaming thread has finished.
unsafe impl Send for Handle {}
// SAFETY: see above; shared references are only used for the read/cancel and control split.
unsafe impl Sync for Handle {}

impl Handle {
    fn ptr(&self) -> *mut ffi::RtlsdrDev {
        self.dev.as_ptr()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: `dev` came from a successful `rtlsdr_open`, is closed exactly once here and
        // no other reference exists (Drop of the last Arc).
        unsafe {
            ffi::rtlsdr_close(self.dev.as_ptr());
        }
    }
}

/// State shared between control side and the callback.
#[derive(Debug)]
struct Shared {
    center_hz: AtomicU64,
    sample_rate: AtomicU32,
    stop: AtomicBool,
}

/// RTL-SDR device.
pub struct RtlSdrDevice {
    handle: Arc<Handle>,
    config: RtlSdrConfig,
    shared: Arc<Shared>,
    gains_tenth_db: Vec<i32>,
    tuner: &'static str,
    worker: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for RtlSdrDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RtlSdrDevice")
            .field("tuner", &self.tuner)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

fn tuner_name(kind: c_int) -> &'static str {
    match kind {
        1 => "E4000",
        2 => "FC0012",
        3 => "FC0013",
        4 => "FC2580",
        5 => "R820T/R820T2",
        6 => "R828D",
        _ => "unknown",
    }
}

/// Nearest supported gain (tenths of dB) to the request.
fn nearest_gain(supported: &[i32], requested_db: f32) -> Option<i32> {
    let want = requested_db * 10.0;
    supported.iter().copied().min_by(|a, b| {
        (*a as f32 - want)
            .abs()
            .total_cmp(&(*b as f32 - want).abs())
    })
}

impl RtlSdrDevice {
    /// Open and configure the dongle selected by `config`.
    ///
    /// # Errors
    /// Fails if no matching dongle exists, it cannot be opened (e.g. udev permissions or
    /// kernel driver claimed) or librtlsdr rejects a setting.
    pub fn open(config: RtlSdrConfig) -> Result<Self, Error> {
        // SAFETY: plain query without arguments.
        let count = unsafe { ffi::rtlsdr_get_device_count() };
        if count == 0 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "no RTL-SDR device found (check USB, udev rules and the dvb_usb_rtl28xxu blacklist)",
            )));
        }
        let index = match &config.selector {
            DeviceSelector::Index(i) if *i < count => *i,
            DeviceSelector::Index(i) => {
                return Err(Error::Config(format!(
                    "RTL-SDR index {i} out of range ({count} device(s) found)"
                )));
            }
            DeviceSelector::Serial(s) => {
                let c = CString::new(s.as_str())
                    .map_err(|_| Error::Config("serial contains NUL".into()))?;
                // SAFETY: `c` is a valid NUL-terminated string for the duration of the call.
                let idx = unsafe { ffi::rtlsdr_get_index_by_serial(c.as_ptr()) };
                u32::try_from(idx).map_err(|_| {
                    Error::Config(format!(
                        "no RTL-SDR with serial {s:?} (librtlsdr code {idx})"
                    ))
                })?
            }
        };
        log_device(index);

        let mut raw: *mut ffi::RtlsdrDev = std::ptr::null_mut();
        // SAFETY: `raw` is a valid out-pointer; on success librtlsdr stores a device handle.
        let rc = unsafe { ffi::rtlsdr_open(&raw mut raw, index) };
        check("open", rc)?;
        let dev = NonNull::new(raw).ok_or_else(|| dev_err("open (null handle)", 0))?;
        let handle = Arc::new(Handle { dev });
        let p = handle.ptr();

        // SAFETY: `p` is a valid open device for all calls below (kept alive by `handle`).
        let (tuner, gains) = unsafe {
            let tuner = tuner_name(ffi::rtlsdr_get_tuner_type(p));
            let n = ffi::rtlsdr_get_tuner_gains(p, std::ptr::null_mut());
            let mut gains = vec![0 as c_int; usize::try_from(n).unwrap_or(0)];
            if !gains.is_empty() {
                ffi::rtlsdr_get_tuner_gains(p, gains.as_mut_ptr());
            }
            (tuner, gains)
        };

        let mut dev = Self {
            handle,
            shared: Arc::new(Shared {
                center_hz: AtomicU64::new(config.center_hz),
                sample_rate: AtomicU32::new(config.sample_rate),
                stop: AtomicBool::new(false),
            }),
            config,
            gains_tenth_db: gains,
            tuner,
            worker: None,
        };
        dev.apply_config()?;
        tracing::info!(
            tuner = dev.tuner,
            rate = dev.sample_rate(),
            freq = dev.center_freq(),
            gains = ?dev.gains_tenth_db,
            "RTL-SDR opened"
        );
        Ok(dev)
    }

    /// Tuner chip name, e.g. `R820T/R820T2`.
    pub fn tuner(&self) -> &str {
        self.tuner
    }

    /// Supported tuner gains in tenths of dB.
    pub fn supported_gains(&self) -> &[i32] {
        &self.gains_tenth_db
    }

    fn apply_config(&mut self) -> Result<(), Error> {
        let p = self.handle.ptr();
        let cfg = self.config.clone();
        self.set_sample_rate(cfg.sample_rate)?;
        self.set_center_freq(cfg.center_hz)?;
        if cfg.ppm != 0 {
            // SAFETY: `p` is a valid open device.
            let rc = unsafe { ffi::rtlsdr_set_freq_correction(p, cfg.ppm) };
            if rc != ERR_UNCHANGED {
                check("set_freq_correction", rc)?;
            }
        }
        match cfg.gain {
            Gain::Auto => {
                // SAFETY: `p` is a valid open device.
                check("set_tuner_gain_mode", unsafe {
                    ffi::rtlsdr_set_tuner_gain_mode(p, 0)
                })?;
            }
            Gain::Db(db) => {
                let tenth = nearest_gain(&self.gains_tenth_db, db)
                    .ok_or_else(|| Error::Config("tuner reports no gain steps".into()))?;
                // SAFETY: `p` is a valid open device.
                unsafe {
                    check("set_tuner_gain_mode", ffi::rtlsdr_set_tuner_gain_mode(p, 1))?;
                    check("set_tuner_gain", ffi::rtlsdr_set_tuner_gain(p, tenth))?;
                }
                tracing::info!(
                    requested_db = db,
                    applied_db = f64::from(tenth) / 10.0,
                    "gain"
                );
            }
        }
        // SAFETY: `p` is a valid open device.
        unsafe {
            check("set_agc_mode", ffi::rtlsdr_set_agc_mode(p, 0))?;
            check(
                "set_bias_tee",
                ffi::rtlsdr_set_bias_tee(p, c_int::from(cfg.bias_tee)),
            )?;
            check("reset_buffer", ffi::rtlsdr_reset_buffer(p))?;
        }
        Ok(())
    }
}

fn log_device(index: u32) {
    // SAFETY: `get_device_name` returns a pointer to static storage (or null) for a valid index.
    let name = unsafe {
        let p = ffi::rtlsdr_get_device_name(index);
        if p.is_null() {
            String::new()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    };
    let mut manufact: [c_char; 256] = [0; 256];
    let mut product: [c_char; 256] = [0; 256];
    let mut serial: [c_char; 256] = [0; 256];
    // SAFETY: librtlsdr documents 256-byte buffers for all three strings.
    let rc = unsafe {
        ffi::rtlsdr_get_device_usb_strings(
            index,
            manufact.as_mut_ptr(),
            product.as_mut_ptr(),
            serial.as_mut_ptr(),
        )
    };
    if rc == 0 {
        // SAFETY: on success librtlsdr wrote NUL-terminated strings into the buffers.
        let (m, p, s) = unsafe {
            (
                CStr::from_ptr(manufact.as_ptr()).to_string_lossy(),
                CStr::from_ptr(product.as_ptr()).to_string_lossy(),
                CStr::from_ptr(serial.as_ptr()).to_string_lossy(),
            )
        };
        tracing::info!(index, %name, manufacturer = %m, product = %p, serial = %s, "RTL-SDR found");
    } else {
        tracing::info!(index, %name, "RTL-SDR found (USB strings unreadable)");
    }
}

impl Device for RtlSdrDevice {
    fn name(&self) -> &str {
        "rtlsdr"
    }

    fn center_freq(&self) -> u64 {
        self.shared.center_hz.load(Ordering::Relaxed)
    }

    fn sample_rate(&self) -> u32 {
        self.shared.sample_rate.load(Ordering::Relaxed)
    }

    fn set_center_freq(&mut self, hz: u64) -> Result<(), Error> {
        let f = u32::try_from(hz)
            .map_err(|_| Error::Config(format!("frequency {hz} Hz exceeds 32 bit")))?;
        // SAFETY: valid open device; control calls only come from the owner (`&mut self`).
        unsafe {
            check(
                "set_center_freq",
                ffi::rtlsdr_set_center_freq(self.handle.ptr(), f),
            )?;
            let actual = ffi::rtlsdr_get_center_freq(self.handle.ptr());
            self.shared.center_hz.store(
                if actual == 0 { hz } else { u64::from(actual) },
                Ordering::Relaxed,
            );
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<(), Error> {
        validate_rate(hz)?;
        // SAFETY: valid open device; control calls only come from the owner (`&mut self`).
        unsafe {
            check(
                "set_sample_rate",
                ffi::rtlsdr_set_sample_rate(self.handle.ptr(), hz),
            )?;
            let actual = ffi::rtlsdr_get_sample_rate(self.handle.ptr());
            self.shared
                .sample_rate
                .store(if actual == 0 { hz } else { actual }, Ordering::Relaxed);
        }
        Ok(())
    }

    fn start(&mut self, queue_blocks: usize) -> Result<IqStream, Error> {
        if self.worker.is_some() {
            return Err(Error::State("already streaming"));
        }
        self.shared.stop.store(false, Ordering::Relaxed);
        // SAFETY: valid open device; clears stale USB data before streaming.
        check("reset_buffer", unsafe {
            ffi::rtlsdr_reset_buffer(self.handle.ptr())
        })?;
        let (tx, rx) = sync_channel(queue_blocks.max(1));
        let dropped = Arc::new(AtomicU64::new(0));
        let handle = Arc::clone(&self.handle);
        let ctx = CallbackCtx {
            handle: Arc::clone(&handle),
            tx,
            shared: Arc::clone(&self.shared),
            dropped: Arc::clone(&dropped),
            block_len: self.config.block_len,
            pending: Vec::with_capacity(self.config.block_len),
            lut: build_lut(),
        };
        let worker = std::thread::Builder::new()
            .name("rtlsdr-device".into())
            .spawn(move || run_async(handle, ctx))?;
        self.worker = Some(worker);
        Ok(IqStream::new(rx, dropped))
    }

    fn stop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        let Some(worker) = self.worker.take() else {
            return;
        };
        // A cancel issued before `read_async` has started is ignored by librtlsdr, so repeat it
        // until the thread is gone; the callback also checks the stop flag.
        while !worker.is_finished() {
            // SAFETY: valid open device; `cancel_async` is the documented cross-thread call.
            unsafe {
                ffi::rtlsdr_cancel_async(self.handle.ptr());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if worker.join().is_err() {
            tracing::warn!("rtlsdr device thread panicked");
        }
    }
}

impl Drop for RtlSdrDevice {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 8-bit sample to float, same mapping as the file backend: `(x - 127.5) / 127.5`.
fn build_lut() -> Box<[f32; 256]> {
    let mut lut = Box::new([0.0f32; 256]);
    for (i, v) in lut.iter_mut().enumerate() {
        *v = (i as f32 - 127.5) / 127.5;
    }
    lut
}

struct CallbackCtx {
    handle: Arc<Handle>,
    tx: SyncSender<IqBlock>,
    shared: Arc<Shared>,
    dropped: Arc<AtomicU64>,
    block_len: usize,
    pending: Vec<Complex32>,
    lut: Box<[f32; 256]>,
}

impl CallbackCtx {
    fn on_data(&mut self, data: &[u8]) {
        if self.shared.stop.load(Ordering::Relaxed) {
            self.cancel();
            return;
        }
        let (pairs, _) = data.as_chunks::<2>();
        for pair in pairs {
            self.pending.push(Complex32::new(
                self.lut[usize::from(pair[0])],
                self.lut[usize::from(pair[1])],
            ));
            if self.pending.len() == self.block_len {
                let samples =
                    std::mem::replace(&mut self.pending, Vec::with_capacity(self.block_len));
                let block = IqBlock {
                    center_hz: self.shared.center_hz.load(Ordering::Relaxed),
                    sample_rate: self.shared.sample_rate.load(Ordering::Relaxed),
                    samples,
                };
                match self.tx.try_send(block) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        self.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        self.cancel();
                        return;
                    }
                }
            }
        }
    }

    fn cancel(&self) {
        // SAFETY: valid open device; librtlsdr allows `cancel_async` from within the callback.
        unsafe {
            ffi::rtlsdr_cancel_async(self.handle.ptr());
        }
    }
}

unsafe extern "C" fn on_read(buf: *mut u8, len: u32, ctx: *mut c_void) {
    if buf.is_null() || ctx.is_null() {
        return;
    }
    // SAFETY: `ctx` is the `&mut CallbackCtx` passed to `read_async` in `run_async`, which
    // outlives the call and is only touched from this (single) callback thread. `buf` is valid
    // for `len` bytes for the duration of the callback.
    let (ctx, data) = unsafe {
        (
            &mut *ctx.cast::<CallbackCtx>(),
            std::slice::from_raw_parts(buf, len as usize),
        )
    };
    // A panic must not unwind across the C frames of librtlsdr.
    if catch_unwind(AssertUnwindSafe(|| ctx.on_data(data))).is_err() {
        ctx.shared.stop.store(true, Ordering::Relaxed);
        ctx.cancel();
    }
}

fn run_async(handle: Arc<Handle>, mut ctx: CallbackCtx) {
    // SAFETY: valid open device; `ctx` lives on this stack frame until `read_async` returns
    // (it blocks until cancelled) and `on_read` is the only user of the pointer.
    let rc = unsafe {
        ffi::rtlsdr_read_async(
            handle.ptr(),
            on_read,
            (&raw mut ctx).cast::<c_void>(),
            0,
            ASYNC_BUF_LEN,
        )
    };
    if rc < 0 && !ctx.shared.stop.load(Ordering::Relaxed) {
        tracing::error!(code = rc, "rtlsdr read_async failed; stopping stream");
    }
    // Dropping `ctx.tx` closes the channel so the consumer sees the end of the stream.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_nearest_gain() {
        let g = [
            0, 9, 14, 27, 37, 77, 87, 125, 144, 157, 166, 197, 207, 229, 254, 280, 297, 328, 338,
            364, 372, 386, 402, 421, 434, 439, 445, 480, 496,
        ];
        assert_eq!(nearest_gain(&g, 28.0), Some(280));
        assert_eq!(nearest_gain(&g, 0.0), Some(0));
        assert_eq!(nearest_gain(&g, 100.0), Some(496));
        assert_eq!(nearest_gain(&[], 10.0), None);
    }

    #[test]
    fn lut_matches_file_backend() {
        let lut = build_lut();
        assert!((lut[0] + 1.0).abs() < 1e-6);
        assert!((lut[255] - 1.0).abs() < 1e-6);
    }
}
