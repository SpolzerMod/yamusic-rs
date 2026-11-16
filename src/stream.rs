//! Потоковый HTTP-источник для rodio: Range-запросы вместо полной загрузки файла.
//! Веб-плеер оригинала начинает играть после первого буфера, а не ждёт весь mp3 —
//! здесь то же: первый кусок 256 КБ, дальше докачка по мере чтения. Поддерживает
//! Seek (перемотка шлёт отдельный Range-запрос), поэтому годится для symphonia.
//! Если сервер не умеет Range (ответ 200 на запрос куска) — забираем весь файл сразу.
//!
//! Чтение с аудиопотока не должно блокироваться сетью, иначе будут заикания на
//! границах кусков. Поэтому следующий кусок качается заранее в фоновом потоке.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::mpsc::Receiver;

/// Размер куска: хватает на несколько секунд звука, качается быстро.
const CHUNK: u64 = 256 * 1024;
/// За сколько до конца куска начинаем качать следующий (~2.7 с звука при 192 кбит/с).
const AHEAD: u64 = 64 * 1024;

/// Результат фоновой докачки: (начало куска, байты или ошибка).
type AheadChunk = (u64, std::sync::Mutex<Receiver<(u64, Result<Vec<u8>, String>)>>);

pub struct HttpStream {
    client: reqwest::blocking::Client,
    url: String,
    pos: u64,
    len: u64,
    chunk: Vec<u8>,
    chunk_start: u64,
    /// false — сервер отдал весь файл одним ответом, докачка не нужна.
    ranged: bool,
    /// Заранее качаемый следующий кусок: (начало, результат из фонового потока).
    /// Mutex нужен, чтобы источник был Sync (требование rodio); доступ всё равно
    /// только с аудиопотока.
    ahead: Option<AheadChunk>,
}

fn fetch_range(client: &reqwest::blocking::Client, url: &str, start: u64, end: u64) -> Result<Vec<u8>, String> {
    let resp = client
        .get(url)
        .header("Range", format!("bytes={start}-{end}"))
        .send()
        .map_err(|e| format!("сеть: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.bytes().map(|b| b.to_vec()).map_err(|e| format!("сеть: {e}"))
}

impl HttpStream {
    pub fn open(client: reqwest::blocking::Client, url: String) -> Result<Self, String> {
        let resp = client
            .get(&url)
            .header("Range", format!("bytes=0-{}", CHUNK - 1))
            .send()
            .map_err(|e| format!("сеть: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("HTTP {status}"));
        }
        if status == reqwest::StatusCode::PARTIAL_CONTENT {
            // Content-Range: bytes 0-262143/13252440 — полная длина после слэша.
            let total = resp
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.rsplit('/').next())
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or("нет длины в Content-Range")?;
            let chunk = resp.bytes().map_err(|e| format!("сеть: {e}"))?.to_vec();
            Ok(HttpStream { client, url, pos: 0, len: total, chunk, chunk_start: 0, ranged: true, ahead: None })
        } else {
            let chunk = resp.bytes().map_err(|e| format!("сеть: {e}"))?.to_vec();
            let len = chunk.len() as u64;
            Ok(HttpStream { client, url, pos: 0, len, chunk, chunk_start: 0, ranged: false, ahead: None })
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    fn chunk_end(&self) -> u64 {
        self.chunk_start + self.chunk.len() as u64
    }

    fn fetch_chunk(&mut self, start: u64) -> io::Result<()> {
        self.ahead = None;
        if !self.ranged {
            return Ok(());
        }
        let end = (start + CHUNK - 1).min(self.len.saturating_sub(1));
        self.chunk = fetch_range(&self.client, &self.url, start, end).map_err(io::Error::other)?;
        self.chunk_start = start;
        Ok(())
    }

    /// Запустить фоновую докачку следующего куска, когда подходим к концу текущего.
    fn maybe_start_ahead(&mut self) {
        if !self.ranged || self.ahead.is_some() {
            return;
        }
        let end = self.chunk_end();
        if end >= self.len || self.pos + AHEAD < end {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let client = self.client.clone();
        let url = self.url.clone();
        let len = self.len;
        std::thread::spawn(move || {
            let stop = (end + CHUNK - 1).min(len - 1);
            let res = fetch_range(&client, &url, end, stop);
            let _ = tx.send((end, res));
        });
        self.ahead = Some((end, std::sync::Mutex::new(rx)));
    }

    /// Взять заранее скачанный кусок, если он начинается там, где нужно.
    fn take_ahead(&mut self, want_start: u64) -> Option<Vec<u8>> {
        let (start, rx) = self.ahead.take()?;
        if start != want_start {
            return None;
        }
        let rx = rx.into_inner().ok()?;
        match rx.recv() {
            Ok((_, Ok(bytes))) => Some(bytes),
            _ => None,
        }
    }
}

impl Read for HttpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let end = self.chunk_end();
        if self.pos >= end {
            // Граница куска: сначала пробуем заранее скачанное, иначе качаем сейчас.
            let want = end;
            match self.take_ahead(want) {
                Some(bytes) => {
                    self.chunk = bytes;
                    self.chunk_start = want;
                }
                None => self.fetch_chunk(self.pos)?,
            }
        } else if self.pos < self.chunk_start {
            self.fetch_chunk(self.pos)?;
        }
        let off = (self.pos - self.chunk_start) as usize;
        if off >= self.chunk.len() {
            return Ok(0);
        }
        let avail = &self.chunk[off..];
        let n = buf.len().min(avail.len()).min((self.len - self.pos) as usize);
        buf[..n].copy_from_slice(&avail[..n]);
        self.pos += n as u64;
        self.maybe_start_ahead();
        Ok(n)
    }
}

impl Seek for HttpStream {
    fn seek(&mut self, s: SeekFrom) -> io::Result<u64> {
        let new = match s {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::End(n) => self.len as i64 + n,
            SeekFrom::Current(n) => self.pos as i64 + n,
        };
        if new < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek до начала файла"));
        }
        self.pos = new as u64;
        Ok(self.pos)
    }
}
