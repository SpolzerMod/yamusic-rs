//! Декодер трека на symphonia: MP3, AAC и FLAC, в том числе внутри MP4.
//!
//! Декодер rodio не открывает MP4 (паника «Seek errors should not occur during
//! initialization»): поток с перемоткой symphonia открывает, только зная длину файла, а rodio
//! её не передаёт. Lossless Яндекс отдаёт только как `flac-mp4`, поэтому свой декодер. Сэмплы
//! идут в f32 как есть: в 16 битах, как у rodio, lossless в 24 бита терял бы младшие биты ещё до
//! эквалайзера.

use rodio::source::SeekError;
use rodio::Source;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use symphonia::core::audio::{Channels, SampleBuffer, SignalSpec};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Сколько битых пакетов подряд пропустить, прежде чем считать трек оборвавшимся (как у rodio).
const MAX_DECODE_RETRIES: usize = 3;

/// Поток трека для symphonia. Пока формат открывается, перемотки «нет»: с ней демультиплексор
/// MP4 обходит все атомы до конца файла и ждёт, пока докачается весь трек (FLAC — 30–40 МБ), а
/// без неё читает заголовок до `mdat` и начинает играть. MP3 и AAC без длины файла открываются
/// так же, как у rodio. После открытия перемотка нужна для seek.
struct Input<R> {
    inner: R,
    seekable: Arc<AtomicBool>,
}

impl<R: Read> Read for Input<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Seek> Seek for Input<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl<R: Read + Seek + Send + Sync> MediaSource for Input<R> {
    fn is_seekable(&self) -> bool {
        self.seekable.load(Ordering::Relaxed)
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

pub struct Decoded {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track: u32,
    spec: SignalSpec,
    buffer: SampleBuffer<f32>,
    at: usize,
    total: Option<Duration>,
}

impl Decoded {
    /// Формат определяется по первым байтам, как у rodio.
    pub fn open<R: Read + Seek + Send + Sync + 'static>(inner: R) -> Result<Decoded, String> {
        let seekable = Arc::new(AtomicBool::new(false));
        let input = Input { inner, seekable: seekable.clone() };
        let mss = MediaSourceStream::new(Box::new(input), Default::default());
        let opts = FormatOptions { enable_gapless: true, ..Default::default() };
        let probed = symphonia::default::get_probe()
            .format(&Hint::new(), mss, &opts, &MetadataOptions::default())
            .map_err(|e| format!("формат: {e}"))?;
        seekable.store(true, Ordering::Relaxed);
        let format = probed.format;
        let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or("нет звуковой дорожки")?;
        let total = track.codec_params.time_base.zip(track.codec_params.n_frames).map(|(base, n)| {
            let t = base.calc_time(n);
            Duration::from_secs(t.seconds) + Duration::from_secs_f64(t.frac)
        });
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| format!("кодек: {e}"))?;
        let track = track.id;
        // Заготовка до первого пакета: у SampleBuffer каналов не может быть ноль.
        let spec = SignalSpec::new(44100, Channels::FRONT_LEFT | Channels::FRONT_RIGHT);
        let mut d = Decoded { format, decoder, track, spec, buffer: SampleBuffer::new(0, spec), at: 0, total };
        if !d.decode_next() {
            return Err("в треке нет звука".into());
        }
        Ok(d)
    }

    /// Следующий пакет своей дорожки в буфер. `false` — трек кончился или оборвался.
    fn decode_next(&mut self) -> bool {
        let mut errors = 0;
        loop {
            let Ok(packet) = self.format.next_packet() else { return false };
            if packet.track_id() != self.track {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    self.spec = *decoded.spec();
                    if self.buffer.capacity() < decoded.capacity() * self.spec.channels.count() {
                        self.buffer = SampleBuffer::new(decoded.capacity() as u64, self.spec);
                    }
                    self.buffer.copy_interleaved_ref(decoded);
                    self.at = 0;
                    return true;
                }
                Err(Error::DecodeError(_)) if errors < MAX_DECODE_RETRIES => errors += 1,
                Err(_) => return false,
            }
        }
    }
}

impl Iterator for Decoded {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.at >= self.buffer.len() && !self.decode_next() {
            return None;
        }
        let x = *self.buffer.samples().get(self.at)?;
        self.at += 1;
        Some(x)
    }
}

impl Source for Decoded {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.buffer.len().saturating_sub(self.at).max(1))
    }

    fn channels(&self) -> u16 {
        self.spec.channels.count().max(1) as u16
    }

    fn sample_rate(&self) -> u32 {
        self.spec.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.total
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let time = pos.as_secs_f64().into();
        let seeked = self
            .format
            .seek(SeekMode::Accurate, SeekTo::Time { time, track_id: Some(self.track) })
            .map_err(|_| SeekError::NotSupported { underlying_source: "decode::Decoded" })?;
        self.decoder.reset();
        // Точно на место, как у rodio: после перехода к ближайшему пакету лишние кадры
        // пропускаются.
        let mut skip = seeked.required_ts.saturating_sub(seeked.actual_ts);
        loop {
            if !self.decode_next() {
                self.buffer.clear();
                self.at = 0;
                return Ok(());
            }
            let frames = (self.buffer.len() / self.channels() as usize) as u64;
            if frames > skip {
                self.at = skip as usize * self.channels() as usize;
                return Ok(());
            }
            skip -= frames;
        }
    }
}
