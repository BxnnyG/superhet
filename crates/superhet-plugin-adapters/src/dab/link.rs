//! The plugin end of the framed transport (`docs/plugin-contract.md`): connect to
//! `SUPERHET_SOCKET`, answer the host's hello, then send events, audio, status, media and tune
//! requests (any thread) while one thread reads what the host sends.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;

use serde_json::{Value, json};
use superhet_plugin_protocol::PROTOCOL_VERSION;
use superhet_plugin_protocol::framed::{
    self, Frame, FrameDecoder, FrameKind, HostHello, MAX_MEDIA, MAX_STATUS, MediaHeader,
    PluginHello, TuneRequest,
};

/// Sending side, shared between threads; every frame is written under the lock.
pub struct Link {
    writer: Mutex<UnixStream>,
}

/// Receiving side.
pub struct Incoming {
    stream: UnixStream,
    decoder: FrameDecoder,
    buf: Vec<u8>,
}

impl Incoming {
    /// The next frame; `None` when the host closed the socket.
    ///
    /// # Errors
    /// I/O error or a framing violation.
    pub fn read_frame(&mut self) -> io::Result<Option<Frame>> {
        loop {
            match self.decoder.next_frame() {
                Ok(Some(frame)) => return Ok(Some(frame)),
                Ok(None) => {}
                Err(err) => {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, err.to_string()));
                }
            }
            let n = self.stream.read(&mut self.buf)?;
            if n == 0 {
                return Ok(None);
            }
            self.decoder.push(&self.buf[..n]);
        }
    }
}

/// Connect and do the hello exchange. Returns the sending half, the receiving half and what the
/// host said about this instance.
///
/// # Errors
/// Connection problems, a wrong first frame or a different protocol version.
pub fn connect(path: &str) -> io::Result<(Link, Incoming, HostHello)> {
    let stream = UnixStream::connect(path)?;
    let mut incoming = Incoming {
        stream: stream.try_clone()?,
        decoder: FrameDecoder::new(),
        buf: vec![0; 256 * 1024],
    };
    let frame = incoming
        .read_frame()?
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "host closed the socket"))?;
    if frame.kind != FrameKind::Hello {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "first frame is not a hello",
        ));
    }
    let hello: HostHello = serde_json::from_slice(&frame.payload)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    if hello.protocol != PROTOCOL_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("host speaks protocol {}", hello.protocol),
        ));
    }
    let link = Link {
        writer: Mutex::new(stream),
    };
    link.send_json(
        FrameKind::Hello,
        &PluginHello {
            protocol: PROTOCOL_VERSION,
            capabilities: vec!["status".into(), "media".into(), "tune".into()],
        },
    )?;
    Ok((link, incoming, hello))
}

impl Link {
    fn send(&self, kind: FrameKind, payload: &[u8]) -> io::Result<()> {
        let wire = framed::encode(kind, payload)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        writer.write_all(&wire)
    }

    fn send_json<T: serde::Serialize>(&self, kind: FrameKind, value: &T) -> io::Result<()> {
        let body = serde_json::to_vec(value)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
        self.send(kind, &body)
    }

    /// Close the socket in both directions; the reading side then ends too.
    pub fn close(&self) {
        let writer = self
            .writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = writer.shutdown(std::net::Shutdown::Both);
    }

    /// Liveness for the host's watchdog.
    ///
    /// # Errors
    /// The socket is gone.
    pub fn heartbeat(&self) -> io::Result<()> {
        self.send(FrameKind::Heartbeat, &[])
    }

    /// A contract event (`type` dotted, `data` an object).
    ///
    /// # Errors
    /// The socket is gone.
    pub fn event(&self, event_type: &str, data: &Value) -> io::Result<()> {
        self.send_json(
            FrameKind::Event,
            &json!({"v": PROTOCOL_VERSION, "type": event_type, "data": data}),
        )
    }

    /// PCM s16le interleaved stereo at 48 kHz.
    ///
    /// # Errors
    /// The socket is gone.
    pub fn audio(&self, pcm: &[u8]) -> io::Result<()> {
        self.send(FrameKind::Audio, pcm)
    }

    /// The plugin's complete state (replaces the previous document).
    ///
    /// # Errors
    /// The socket is gone or the document is too large.
    pub fn status(&self, doc: &Value) -> io::Result<()> {
        let body = serde_json::to_vec(doc)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
        if body.len() > MAX_STATUS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "status too large",
            ));
        }
        self.send(FrameKind::Status, &body)
    }

    /// Store a binary object under `key` (a slideshow image).
    ///
    /// # Errors
    /// The socket is gone or key / size are not acceptable.
    pub fn media(&self, key: &str, mime: &str, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > MAX_MEDIA || !MediaHeader::valid_key(key) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "media not acceptable",
            ));
        }
        let payload = MediaHeader {
            key: key.to_owned(),
            mime: mime.to_owned(),
        }
        .encode(bytes)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "media header too long"))?;
        self.send(FrameKind::Media, &payload)
    }

    /// Ask the host to move the claimed window to `freq_hz`.
    ///
    /// # Errors
    /// The socket is gone.
    pub fn tune(&self, freq_hz: u64) -> io::Result<()> {
        self.send_json(FrameKind::Tune, &TuneRequest { freq_hz })
    }

    /// A line for the host log.
    ///
    /// # Errors
    /// The socket is gone.
    pub fn log(&self, text: &str) -> io::Result<()> {
        self.send(FrameKind::Log, text.as_bytes())
    }
}
