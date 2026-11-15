//! Общая очередь скачивания. Параметры фиксируются при добавлении задания.

use crate::api::{Api, Track};
use crate::config::{AudioFormat, DownloadOptions};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Queued,
    Resolving,
    Downloading,
    Processing,
    Completed,
    Failed,
}

impl Status {
    pub fn active(self) -> bool {
        !matches!(self, Self::Completed | Self::Failed)
    }
}

#[derive(Clone, Debug)]
pub struct Job {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub options: DownloadOptions,
    pub status: Status,
    pub received: u64,
    pub total: Option<u64>,
    pub detail: String,
}

pub struct Snapshot {
    pub version: u64,
    pub jobs: Vec<Job>,
}

#[derive(Clone)]
struct Work {
    id: u64,
    api: Api,
    track: Track,
    directory: PathBuf,
    options: DownloadOptions,
}
struct Entry {
    work: Work,
    job: Job,
}
#[derive(Default)]
struct Queue {
    entries: Vec<Entry>,
    pending: VecDeque<u64>,
    next_id: u64,
    version: u64,
    running: bool,
}

impl Queue {
    fn add(&mut self, api: Api, directory: PathBuf, track: Track, options: DownloadOptions) -> bool {
        if self.entries.iter().any(|e| e.job.status.active() && e.work.directory == directory && e.work.track.id == track.id && e.work.options == options) {
            return false;
        }
        self.next_id += 1;
        let id = self.next_id;
        let job = Job {
            id,
            title: track.title.clone(),
            artist: track.artists.clone(),
            options,
            status: Status::Queued,
            received: 0,
            total: None,
            detail: String::new(),
        };
        self.entries.push(Entry { work: Work { id, api, track, directory, options }, job });
        self.pending.push_back(id);
        self.version += 1;
        true
    }
    fn claim(&mut self) -> Option<Work> {
        let id = self.pending.pop_front()?;
        let entry = self.entries.iter_mut().find(|e| e.job.id == id)?;
        entry.job.status = Status::Resolving;
        self.version += 1;
        Some(entry.work.clone())
    }
    fn finish(&mut self, id: u64, result: Result<PathBuf, String>) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.job.id == id) {
            match result {
                Ok(path) => {
                    e.job.status = Status::Completed;
                    e.job.detail = path.display().to_string();
                }
                Err(error) => {
                    e.job.status = Status::Failed;
                    e.job.detail = error;
                }
            }
            self.version += 1;
        }
    }
    fn retry(&mut self, id: u64) -> bool {
        let Some(index) = self.entries.iter().position(|e| e.job.id == id && e.job.status == Status::Failed) else {
            return false;
        };
        let work = &self.entries[index].work;
        if self
            .entries
            .iter()
            .any(|e| e.job.status.active() && e.work.directory == work.directory && e.work.track.id == work.track.id && e.work.options == work.options)
        {
            return false;
        }
        let job = &mut self.entries[index].job;
        job.status = Status::Queued;
        job.received = 0;
        job.total = None;
        job.detail.clear();
        self.pending.push_back(id);
        self.version += 1;
        true
    }
}

#[derive(Clone, Default)]
pub struct Manager {
    inner: Arc<Mutex<Queue>>,
}

impl Manager {
    pub fn new() -> Self {
        if let Err(error) = recover_temporary(&crate::config::dir().join("download-parts.json")) {
            crate::config::log(format!("Очистка прерванного скачивания: {error}"));
        }
        Self::default()
    }
    pub fn snapshot(&self) -> Snapshot {
        let q = self.inner.lock().unwrap();
        Snapshot { version: q.version, jobs: q.entries.iter().map(|e| e.job.clone()).collect() }
    }
    pub fn completed_path(&self, id: u64) -> Option<PathBuf> {
        self.inner
            .lock()
            .unwrap()
            .entries
            .iter()
            .find(|entry| entry.job.id == id && entry.job.status == Status::Completed)
            .map(|entry| PathBuf::from(&entry.job.detail))
    }
    pub fn version(&self) -> u64 {
        self.inner.lock().unwrap().version
    }
    pub fn enqueue(&self, api: Api, directory: PathBuf, tracks: Vec<Track>, options: DownloadOptions) -> usize {
        let mut q = self.inner.lock().unwrap();
        let mut added = 0;
        for track in tracks {
            added += usize::from(q.add(api.clone(), directory.clone(), track, options.normalized()));
        }
        drop(q);
        self.kick();
        added
    }
    pub fn retry(&self, id: u64) {
        self.inner.lock().unwrap().retry(id);
        self.kick();
    }
    pub fn clear_finished(&self) {
        let mut q = self.inner.lock().unwrap();
        q.entries.retain(|e| e.job.status.active());
        q.version += 1;
    }
    fn kick(&self) {
        let mut q = self.inner.lock().unwrap();
        if q.running || q.pending.is_empty() {
            return;
        }
        q.running = true;
        let inner = self.inner.clone();
        std::thread::spawn(move || loop {
            let work = {
                let mut q = inner.lock().unwrap();
                let Some(work) = q.claim() else {
                    q.running = false;
                    return;
                };
                work
            };
            let progress = |status, received, total| {
                let mut q = inner.lock().unwrap();
                if let Some(e) = q.entries.iter_mut().find(|e| e.job.id == work.id) {
                    e.job.status = status;
                    e.job.received = received;
                    e.job.total = total;
                    q.version += 1;
                }
            };
            let result = save_track(&work, progress);
            if let Err(error) = &result {
                crate::config::log(format!("скачивание {}: {error}", work.track.id));
            }
            inner.lock().unwrap().finish(work.id, result);
        });
    }
}

fn file_component(text: &str, max: usize) -> String {
    let text: String = text.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).take(max).collect();
    let text = text.trim().trim_end_matches(['.', ' ']);
    if text.is_empty() {
        "_".into()
    } else {
        text.into()
    }
}

fn destination(work: &Work) -> PathBuf {
    let o = work.options;
    let format = if o.format == AudioFormat::Flac { "FLAC".into() } else { format!("MP3 {}", o.bitrate) };
    work.directory.join(format!(
        "{} [{}; {}; {}].{}",
        file_component(&format!("{} — {}", work.track.artists, work.track.title), 100),
        file_component(&work.track.id, 30),
        format,
        o.cover_size,
        if o.format == AudioFormat::Flac { "flac" } else { "mp3" }
    ))
}

struct Temporary(Vec<PathBuf>);

fn recover_temporary(journal: &Path) -> Result<(), String> {
    let data = match std::fs::read(journal) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let paths: Vec<PathBuf> = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    for path in paths {
        let owned = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(".yamusic-rs-") && n.ends_with(".part"));
        if owned {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
    }
    std::fs::remove_file(journal).map_err(|e| e.to_string())
}

struct Journal(PathBuf);
impl Journal {
    fn create(path: PathBuf, files: &[PathBuf]) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        // Журнал синхронизирован до создания первого временного аудиофайла.
        use std::io::Write;
        let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec(files).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        Ok(Self(path))
    }
}
impl Drop for Journal {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn save_track(work: &Work, mut progress: impl FnMut(Status, u64, Option<u64>)) -> Result<PathBuf, String> {
    std::fs::create_dir_all(&work.directory).map_err(|e| e.to_string())?;
    let path = destination(work);
    if std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() > 0) {
        return Ok(path);
    }
    let part = work.directory.join(format!(
        ".yamusic-rs-{}-{}-{}.{}.part",
        file_component(&work.track.id, 30),
        std::process::id(),
        work.id,
        work.options.format.extension()
    ));
    let _journal = Journal::create(crate::config::dir().join("download-parts.json"), &[part.clone(), part.with_extension("tagged.part")])?;
    let _cleanup = Temporary(vec![part.clone()]);
    let mut last = String::new();
    let mut received = 0;
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_secs(2 * attempt));
        }
        progress(Status::Resolving, 0, None);
        let result = work.api.download_url(&work.track.id, &work.options).and_then(|url| {
            work.api.download_to_progress(&url, &part, |bytes, total| {
                received = bytes;
                progress(Status::Downloading, bytes, total);
            })
        });
        match result {
            Ok(n) if n > 0 => {
                last.clear();
                break;
            }
            Ok(_) => last = "Сервер вернул пустой файл".into(),
            Err(error) => last = error,
        }
    }
    if !last.is_empty() {
        return Err(last);
    }
    progress(Status::Processing, received, Some(received));
    let cover = if let Some(url) = work.track.cover_url(work.options.cover_size) {
        let bytes = work.api.fetch_bytes(&url).map_err(|e| format!("Обложка: {e}"))?;
        let image = image::load_from_memory(&bytes).map_err(|e| format!("Обложка: {e}"))?;
        let size = work.options.cover_size;
        let image = if image.width() == size && image.height() == size { image } else { image.resize_exact(size, size, image::imageops::FilterType::Lanczos3) };
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90).encode_image(&image.to_rgb8()).map_err(|e| format!("Обложка: {e}"))?;
        Some(jpeg)
    } else {
        None
    };
    finish_download(work, &part, cover.as_deref())
}

fn finish_download(work: &Work, part: &Path, cover: Option<&[u8]>) -> Result<PathBuf, String> {
    use std::io::{Read, Seek, Write};
    let tagged = part.with_extension("tagged.part");
    let _cleanup = Temporary(vec![part.to_path_buf(), tagged.clone()]);
    let title = if work.track.version.is_empty() { work.track.title.clone() } else { format!("{} ({})", work.track.title, work.track.version) };
    let tags = crate::tags::Tags {
        title: &title,
        artist: &work.track.artists,
        album: &work.track.album_title,
        year: work.track.year,
        track: work.track.position,
        cover,
    };
    let mut output = std::fs::File::create(&tagged).map_err(|e| e.to_string())?;
    match work.options.format {
        AudioFormat::Flac => {
            let bytes = std::fs::read(part).map_err(|e| e.to_string())?;
            let flac = crate::flac::prepare(&bytes, &tags)?;
            output.write_all(&flac).map_err(|e| e.to_string())?;
        }
        AudioFormat::Mp3 => {
            let mut input = std::fs::File::open(part).map_err(|e| e.to_string())?;
            let mut head = [0u8; 10];
            let n = input.read(&mut head).map_err(|e| e.to_string())?;
            let skip = crate::tags::existing_id3_len(&head[..n]) as u64;
            input.seek(std::io::SeekFrom::Start(skip)).map_err(|e| e.to_string())?;
            let mut frame = [0u8; 4];
            input.read_exact(&mut frame).map_err(|_| "Аудиофайл слишком короткий".to_string())?;
            if frame[0] != 0xff
                || frame[1] & 0xe0 != 0xe0
                || frame[1] & 0x18 == 0x08
                || frame[1] & 0x06 == 0
                || frame[2] & 0xf0 == 0xf0
                || frame[2] & 0x0c == 0x0c
            {
                return Err("Сервер вернул аудио не в формате MP3".into());
            }
            input.seek(std::io::SeekFrom::Start(skip)).map_err(|e| e.to_string())?;
            output.write_all(&crate::tags::id3(&tags)).map_err(|e| e.to_string())?;
            std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
        }
    }
    output.sync_all().map_err(|e| e.to_string())?;
    drop(output);
    let path = destination(work);
    std::fs::rename(&tagged, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveal_returns_only_successfully_completed_paths() {
        let mut q = Queue::default();
        add(&mut q, "1", DownloadOptions::default());
        let work = q.claim().unwrap();
        let manager = Manager { inner: Arc::new(Mutex::new(q)) };
        assert_eq!(manager.completed_path(work.id), None);
        let path = PathBuf::from("C:/Музыка/Исполнитель — Песня.mp3");
        manager.inner.lock().unwrap().finish(work.id, Ok(path.clone()));
        assert_eq!(manager.completed_path(work.id), Some(path));
        manager.inner.lock().unwrap().finish(work.id, Err("failed".into()));
        assert_eq!(manager.completed_path(work.id), None);
        assert_eq!(manager.completed_path(999), None);
    }

    fn track(id: &str) -> Track {
        Track {
            id: id.into(),
            album_id: None,
            title: "Песня".into(),
            version: String::new(),
            artists: "Исполнитель".into(),
            artist_list: vec![],
            duration_ms: 1000,
            cover: None,
            explicit: false,
            lyrics: false,
            color: None,
            album_title: String::new(),
            year: None,
            position: None,
        }
    }
    fn add(q: &mut Queue, id: &str, options: DownloadOptions) -> bool {
        q.add(Api::new(""), PathBuf::from("music"), track(id), options)
    }

    #[test]
    fn fifo_deduplicates_only_active_identical_jobs_and_captures_options() {
        let mut q = Queue::default();
        let options = DownloadOptions::default();
        assert!(add(&mut q, "1", options));
        assert!(!add(&mut q, "1", options));
        assert!(add(&mut q, "2", options));
        assert!(add(&mut q, "1", DownloadOptions { bitrate: 192, ..options }));
        let first = q.claim().unwrap();
        assert_eq!(first.track.id, "1");
        assert_eq!(first.options.bitrate, 320);
        assert!(!add(&mut q, "1", options));
        q.finish(first.id, Ok(PathBuf::from("music/one.mp3")));
        assert!(add(&mut q, "1", options));
        assert_eq!(q.claim().unwrap().track.id, "2");
        let third = q.claim().unwrap();
        assert_eq!((third.track.id.as_str(), third.options.bitrate), ("1", 192));
    }

    #[test]
    fn failure_does_not_block_next_and_retry_resets_progress() {
        let mut q = Queue::default();
        add(&mut q, "1", DownloadOptions::default());
        add(&mut q, "2", DownloadOptions::default());
        let first = q.claim().unwrap();
        q.entries[0].job.received = 123;
        q.finish(first.id, Err("HTTP 404".into()));
        assert_eq!(q.entries[0].job.status, Status::Failed);
        assert_eq!(q.claim().unwrap().track.id, "2");
        assert!(q.retry(first.id));
        assert!(!q.retry(first.id));
        assert_eq!(q.entries[0].job.received, 0);
        assert_eq!(q.claim().unwrap().track.id, "1");
    }

    #[test]
    fn filenames_separate_ids_and_each_quality_and_cannot_escape_folder() {
        let mut q = Queue::default();
        add(&mut q, "1", DownloadOptions::default());
        add(&mut q, "2", DownloadOptions::default());
        add(&mut q, "1", DownloadOptions { cover_size: 1000, ..DownloadOptions::default() });
        add(&mut q, "1", DownloadOptions { format: AudioFormat::Flac, ..DownloadOptions::default() });
        let paths: std::collections::HashSet<_> = q.entries.iter().map(|e| destination(&e.work)).collect();
        assert_eq!(paths.len(), 4);
        let mut work = q.entries[0].work.clone();
        work.track.title = "../CON:<title>?".into();
        work.track.id = "../../42".into();
        assert_eq!(destination(&work).parent(), Some(Path::new("music")));
        assert_eq!(destination(&q.entries[3].work).extension().unwrap(), "flac");
    }

    fn temp_work() -> Work {
        let dir = std::env::temp_dir().join(format!(
            "yamusic-save-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Work { id: 1, api: Api::new(""), track: track("123"), directory: dir, options: DownloadOptions::default() }
    }

    #[test]
    fn finalize_mp3_replaces_old_tag_preserves_audio_and_removes_temporary() {
        let work = temp_work();
        let part = work.directory.join("track.part");
        let audio = [0xff, 0xfb, 0x90, 0x64, 1, 2, 3, 4, 5];
        let mut source = b"ID3\x03\0\0\0\0\0\0".to_vec();
        source.extend(audio);
        std::fs::write(&part, source).unwrap();
        let path = finish_download(&work, &part, None).unwrap();
        let output = std::fs::read(&path).unwrap();
        let skip = crate::tags::existing_id3_len(&output);
        assert!(skip > 10);
        assert_eq!(&output[skip..], &audio);
        assert!(!part.exists());
        std::fs::remove_dir_all(&work.directory).unwrap();
    }

    #[test]
    fn wrong_format_never_creates_final_file_and_cleans_parts() {
        let mut work = temp_work();
        let part = work.directory.join("track.part");
        for (format, bytes) in [(AudioFormat::Mp3, b"<html>error</html>".as_slice()), (AudioFormat::Flac, b"\xff\xfb\x90\x64bad mp3".as_slice())] {
            work.options.format = format;
            std::fs::write(&part, bytes).unwrap();
            assert!(finish_download(&work, &part, None).is_err());
            assert!(!destination(&work).exists());
            assert!(!part.exists());
        }
        std::fs::remove_dir_all(&work.directory).unwrap();
    }

    #[test]
    fn next_launch_removes_journaled_interrupted_parts_but_preserves_final_files() {
        let work = temp_work();
        let part = work.directory.join(".yamusic-rs-123-1.mp3.part");
        let tagged = work.directory.join(".yamusic-rs-123-1.mp3.tagged.part");
        let final_file = work.directory.join("123.mp3");
        let unrelated = work.directory.join("other.part");
        for path in [&part, &tagged, &final_file, &unrelated] {
            std::fs::write(path, b"bytes").unwrap();
        }
        let journal = work.directory.join("download-parts.json");
        std::fs::write(&journal, serde_json::to_vec(&[part.clone(), tagged.clone(), final_file.clone(), unrelated.clone()]).unwrap()).unwrap();
        recover_temporary(&journal).unwrap();
        assert!(!part.exists());
        assert!(!tagged.exists());
        assert!(final_file.exists());
        assert!(unrelated.exists());
        assert!(!journal.exists());
        recover_temporary(&journal).unwrap();
        std::fs::remove_dir_all(&work.directory).unwrap();
    }
}
