//! A stand-in `rtl_tcp` server: `welle-cli -F rtl_tcp,127.0.0.1:<port>` connects to it and reads
//! the IQ the host feeds the plugin.
//!
//! The protocol is the plain one of `rtl_tcp`: a 12 byte dongle info (`"RTL0"`, tuner type and
//! gain count, big endian) followed by an endless stream of unsigned 8 bit I/Q pairs; the client
//! may send 5 byte commands (set frequency, sample rate, gain), which are read and ignored
//! because the device daemon already tuned the window and fixed the rate (2.048 MS/s).

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// R820T, the tuner of most RTL-SDR sticks (welle only prints it).
const TUNER_R820T: u32 = 5;
/// Gain steps of the R820T (welle only prints it).
const GAIN_COUNT: u32 = 29;
/// A client that does not take data for this long is dropped.
const WRITE_TIMEOUT: Duration = Duration::from_millis(500);

/// The listening side.
pub struct FakeRtl {
    port: u16,
    sink: Arc<Mutex<Option<TcpStream>>>,
}

impl FakeRtl {
    /// Listen on a free loopback port and accept clients in the background.
    ///
    /// # Errors
    /// The port cannot be bound.
    pub fn start() -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let sink: Arc<Mutex<Option<TcpStream>>> = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&sink);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if accept(&stream, &slot).is_err() {
                    continue;
                }
            }
        });
        Ok(Self { port, sink })
    }

    /// The port welle-cli has to connect to.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Whether a client is connected.
    #[must_use]
    pub fn connected(&self) -> bool {
        self.sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    /// Send IQ bytes to the client (dropped silently when nobody is connected). A client that
    /// cannot keep up is disconnected, welle-cli then reconnects or is restarted.
    pub fn push(&self, data: &[u8]) {
        let mut slot = self
            .sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(stream) = slot.as_mut()
            && stream.write_all(data).is_err()
        {
            *slot = None;
        }
    }

    /// Disconnect the client (the next welle-cli gets a fresh connection).
    pub fn disconnect(&self) {
        *self
            .sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

fn accept(stream: &TcpStream, slot: &Arc<Mutex<Option<TcpStream>>>) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    let mut info = Vec::with_capacity(12);
    info.extend_from_slice(b"RTL0");
    info.extend_from_slice(&TUNER_R820T.to_be_bytes());
    info.extend_from_slice(&GAIN_COUNT.to_be_bytes());
    writer.write_all(&info)?;
    // Commands are read and thrown away; a closed connection ends the reader.
    let mut reader = stream.try_clone()?;
    std::thread::spawn(move || {
        let mut sink = [0u8; 256];
        while reader.read(&mut sink).is_ok_and(|n| n > 0) {}
    });
    *slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(writer);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_gets_the_dongle_info_then_the_stream() {
        let fake = FakeRtl::start().expect("start");
        fake.push(b"dropped: nobody is connected");
        let mut client = TcpStream::connect(("127.0.0.1", fake.port())).expect("connect");
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let mut info = [0u8; 12];
        client.read_exact(&mut info).expect("info");
        assert_eq!(&info[..4], b"RTL0");
        assert_eq!(u32::from_be_bytes([info[4], info[5], info[6], info[7]]), 5);
        // Commands from welle are accepted silently.
        client
            .write_all(&[1, 0, 0x0a, 0x9c, 0x40])
            .expect("command");
        for _ in 0..100 {
            if fake.connected() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        fake.push(&[1, 2, 3, 4]);
        let mut data = [0u8; 4];
        client.read_exact(&mut data).expect("data");
        assert_eq!(data, [1, 2, 3, 4]);
        fake.disconnect();
        assert!(!fake.connected());
    }
}
