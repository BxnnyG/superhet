//! Audio of the selected service: welle-cli serves it as an MP3 stream (`/mp3/<sid>`, 32, 24 or
//! 48 kHz stereo, whatever the service uses); this module decodes it to PCM and converts it to
//! what the plugin contract promises (`[output.audio]`: 48 kHz, stereo, s16le).

use std::io::Read;

use superhet_dsp::resample::Resampler;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::formats::TrackType;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::{MediaSourceStream, ReadOnlySource};
use symphonia::core::meta::MetadataOptions;

/// Sample rate of the plugin's audio output.
pub const OUT_RATE: u32 = 48_000;

/// Decode an MP3 byte stream and hand over PCM (s16le, interleaved stereo, 48 kHz) as it comes.
/// Returns when the stream ends (`Ok`), the sink says stop (`Ok`), or decoding fails for good.
///
/// # Errors
/// The stream is not MP3 or cannot be read.
pub fn decode_stream<R: Read + Send + Sync + 'static>(
    reader: R,
    mut sink: impl FnMut(&[u8]) -> bool,
) -> Result<(), String> {
    let source = MediaSourceStream::new(Box::new(ReadOnlySource::new(reader)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|err| format!("not an MP3 stream: {err}"))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or("the stream has no audio track")?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or("the stream has no audio parameters")?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|err| format!("cannot decode MP3: {err}"))?;

    let mut converter = Converter::default();
    let mut samples: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => return Ok(()),
            Err(Error::IoError(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(());
            }
            Err(err) => return Err(format!("reading the MP3 stream: {err}")),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buffer) => {
                samples.resize(buffer.samples_interleaved(), 0.0);
                buffer.copy_to_slice_interleaved(&mut samples);
                let spec = buffer.spec();
                let pcm = converter.convert(&samples, spec.rate(), spec.channels().count());
                if !pcm.is_empty() && !sink(&pcm) {
                    return Ok(());
                }
            }
            // A damaged frame (the radio link is not perfect): skip it.
            Err(Error::DecodeError(_)) => {}
            Err(err) => return Err(format!("decoding MP3: {err}")),
        }
    }
}

/// Interleaved f32 of any channel count and rate to s16le stereo at [`OUT_RATE`].
#[derive(Default)]
pub struct Converter {
    rate: u32,
    left: Option<Resampler>,
    right: Option<Resampler>,
    scratch: [Vec<f32>; 2],
    out: [Vec<f32>; 2],
}

impl Converter {
    /// Convert one block.
    pub fn convert(&mut self, interleaved: &[f32], rate: u32, channels: usize) -> Vec<u8> {
        if rate == 0 || channels == 0 {
            return Vec::new();
        }
        if rate != self.rate {
            self.rate = rate;
            let make =
                || (rate != OUT_RATE).then(|| Resampler::new(f64::from(rate), f64::from(OUT_RATE)));
            self.left = make();
            self.right = make();
        }
        for plane in &mut self.scratch {
            plane.clear();
        }
        for frame in interleaved.chunks_exact(channels) {
            let left = frame[0];
            // Mono is played on both sides; more than two channels: front left / right.
            let right = if channels > 1 { frame[1] } else { left };
            self.scratch[0].push(left);
            self.scratch[1].push(right);
        }
        for (side, resampler) in [(0, &mut self.left), (1, &mut self.right)] {
            self.out[side].clear();
            match resampler {
                Some(resampler) => resampler.process(&self.scratch[side], &mut self.out[side]),
                None => self.out[side].extend_from_slice(&self.scratch[side]),
            }
        }
        let frames = self.out[0].len().min(self.out[1].len());
        let mut pcm = Vec::with_capacity(frames * 4);
        for i in 0..frames {
            for side in 0..2 {
                let value = (self.out[side][i] * 32767.0)
                    .round()
                    .clamp(-32768.0, 32767.0);
                pcm.extend_from_slice(&(value as i16).to_le_bytes());
            }
        }
        pcm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TONE: &[u8] = include_bytes!("../../tests/dab/tone-32k.mp3");

    /// Frequency of the strongest component of one channel, by zero crossings.
    fn frequency(pcm: &[u8], channel: usize) -> f64 {
        let samples: Vec<i16> = pcm
            .as_chunks::<4>()
            .0
            .iter()
            .map(|f| i16::from_le_bytes([f[channel * 2], f[channel * 2 + 1]]))
            .collect();
        let crossings = samples
            .windows(2)
            .filter(|w| (w[0] < 0) != (w[1] < 0))
            .count();
        crossings as f64 / 2.0 / (samples.len() as f64 / f64::from(OUT_RATE))
    }

    #[test]
    fn a_32k_stereo_mp3_becomes_48k_stereo_pcm_with_the_right_pitch() {
        let mut pcm = Vec::new();
        decode_stream(TONE, |chunk| {
            pcm.extend_from_slice(chunk);
            true
        })
        .expect("decode");
        let seconds = pcm.len() as f64 / 4.0 / f64::from(OUT_RATE);
        assert!((1.0..1.4).contains(&seconds), "{seconds} s");
        // Skip the encoder delay at the start before measuring.
        let settled = &pcm[4 * 4800..];
        let left = frequency(settled, 0);
        let right = frequency(settled, 1);
        assert!((left - 440.0).abs() < 15.0, "left {left} Hz");
        assert!((right - 880.0).abs() < 25.0, "right {right} Hz");
    }

    #[test]
    fn the_sink_can_stop_the_stream() {
        let mut calls = 0;
        decode_stream(TONE, |_| {
            calls += 1;
            false
        })
        .expect("decode");
        assert_eq!(calls, 1);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode_stream(&[0x13u8; 5000][..], |_| true).is_err());
    }

    #[test]
    fn mono_is_duplicated_and_48k_passes_through() {
        let mut converter = Converter::default();
        let pcm = converter.convert(&[0.5, -0.5, 0.25], 48_000, 1);
        let samples: Vec<i16> = pcm
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
            .collect();
        assert_eq!(samples, [16384, 16384, -16384, -16384, 8192, 8192]);
        assert!(converter.convert(&[0.0; 4], 0, 2).is_empty());
    }
}
