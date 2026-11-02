//! Reading an import's source for the Import Images window, off the
//! interface thread: the photos of a folder (and its sub-folders), each
//! one's capture time for the duplicate check, and on request the
//! thumbnails on screen and a large preview of one photo.
//!
//! Two threads: one lists the folder and reads the capture times; the
//! other serves the pictures, the newest request first (a list of
//! thumbnails replaces the one still waiting, as the window scrolls).

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;

use raw_io::Thumbnail;

/// Longer side of a thumbnail of the import window.
pub const THUMB_SIDE: u32 = 240;
/// Longer side of the large preview.
pub const PREVIEW_SIDE: u32 = 1600;

/// What the scan reports.
#[derive(Debug, Clone)]
pub enum ScanEvent {
    /// The photos of the source, in order (sent once, before their capture times).
    Listed(Vec<PathBuf>),
    /// A photo's capture time (`None`: the file has none or cannot be read).
    Captured {
        /// The photo.
        path: PathBuf,
        /// As the camera writes it.
        captured: Option<String>,
    },
    /// Every capture time was read.
    Done,
    /// A thumbnail asked for.
    Thumbnail {
        /// The photo.
        path: PathBuf,
        /// Upright pixels, or why there are none.
        result: Result<Arc<Thumbnail>, String>,
    },
    /// The large preview asked for.
    Preview {
        /// The photo.
        path: PathBuf,
        /// Upright pixels, or why there are none.
        result: Result<Arc<Thumbnail>, String>,
    },
}

/// How the scan reads files (the real files, or a test's stand-in).
pub trait Reader: Send + Sync + 'static {
    /// The photos of `dir` (and its sub-folders when `subfolders`), in order.
    fn list(&self, dir: &Path, subfolders: bool) -> Vec<PathBuf>;
    /// The capture time of `path`.
    fn captured(&self, path: &Path) -> Option<String>;
    /// An upright picture of `path`, at most `side` on its longer side.
    ///
    /// # Errors
    /// A message when there is none.
    fn picture(&self, path: &Path, side: u32) -> Result<Thumbnail, String>;
}

/// The files themselves.
#[derive(Debug, Clone, Copy, Default)]
pub struct Files;

impl Reader for Files {
    fn list(&self, dir: &Path, subfolders: bool) -> Vec<PathBuf> {
        crate::catalogs::raw_files(dir, subfolders)
    }

    fn captured(&self, path: &Path) -> Option<String> {
        raw_io::read_info(path).ok().and_then(|i| i.captured)
    }

    fn picture(&self, path: &Path, side: u32) -> Result<Thumbnail, String> {
        raw_io::read_thumbnail(path, side).map_err(|e| e.to_string())
    }
}

/// Pictures waiting to be read: the preview first, then the thumbnails.
#[derive(Debug, Default)]
struct Wanted {
    preview: Option<PathBuf>,
    thumbs: VecDeque<PathBuf>,
    stop: bool,
}

#[derive(Debug, Default)]
struct Queue {
    wanted: Mutex<Wanted>,
    wake: Condvar,
}

/// A source being read. Dropping it stops its threads.
#[derive(Debug)]
pub struct ScanJob {
    rx: Receiver<ScanEvent>,
    stop: Arc<AtomicBool>,
    queue: Arc<Queue>,
}

impl ScanJob {
    /// Starts reading `dir` from the files; `notify` is called when there
    /// is news.
    ///
    /// # Errors
    /// A thread cannot start.
    pub fn start(dir: PathBuf, subfolders: bool, notify: impl Fn() + Send + Sync + 'static) -> io::Result<Self> {
        Self::start_with(Arc::new(Files), dir, subfolders, Arc::new(notify))
    }

    /// [`Self::start`] with `reader` reading the files.
    ///
    /// # Errors
    /// A thread cannot start.
    pub fn start_with(
        reader: Arc<dyn Reader>,
        dir: PathBuf,
        subfolders: bool,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> io::Result<Self> {
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let queue = Arc::new(Queue::default());
        {
            let (tx, stop, reader, notify) = (tx.clone(), Arc::clone(&stop), Arc::clone(&reader), Arc::clone(&notify));
            thread::Builder::new().name("aktis-import-scan".into()).spawn(move || {
                scan(&*reader, &dir, subfolders, &stop, &tx, &*notify);
            })?;
        }
        {
            let queue = Arc::clone(&queue);
            thread::Builder::new().name("aktis-import-pictures".into()).spawn(move || {
                pictures(&*reader, &queue, &tx, &*notify);
            })?;
        }
        Ok(Self { rx, stop, queue })
    }

    /// What happened since the last call.
    pub fn poll(&self) -> Vec<ScanEvent> {
        let mut out = Vec::new();
        while let Ok(e) = self.rx.try_recv() {
            out.push(e);
        }
        out
    }

    /// Asks for the thumbnails of `paths`, first ones first, in place of
    /// those still waiting.
    pub fn request_thumbnails(&self, paths: Vec<PathBuf>) {
        let mut w = self.queue.wanted.lock().unwrap_or_else(PoisonError::into_inner);
        w.thumbs = paths.into();
        drop(w);
        self.queue.wake.notify_all();
    }

    /// Asks for the large preview of `path`, before any thumbnail, in place
    /// of one still waiting.
    pub fn request_preview(&self, path: PathBuf) {
        let mut w = self.queue.wanted.lock().unwrap_or_else(PoisonError::into_inner);
        w.preview = Some(path);
        drop(w);
        self.queue.wake.notify_all();
    }
}

impl Drop for ScanJob {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.queue.wanted.lock().unwrap_or_else(PoisonError::into_inner).stop = true;
        self.queue.wake.notify_all();
    }
}

fn scan(
    reader: &dyn Reader,
    dir: &Path,
    subfolders: bool,
    stop: &AtomicBool,
    tx: &Sender<ScanEvent>,
    notify: &(dyn Fn() + Send + Sync),
) {
    let photos = reader.list(dir, subfolders);
    let _ = tx.send(ScanEvent::Listed(photos.clone()));
    notify();
    for path in photos {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let captured = reader.captured(&path);
        if tx.send(ScanEvent::Captured { path, captured }).is_err() {
            return;
        }
        notify();
    }
    let _ = tx.send(ScanEvent::Done);
    notify();
}

fn pictures(reader: &dyn Reader, queue: &Queue, tx: &Sender<ScanEvent>, notify: &(dyn Fn() + Send + Sync)) {
    loop {
        let next = {
            let guard = queue.wanted.lock().unwrap_or_else(PoisonError::into_inner);
            let mut w = queue
                .wake
                .wait_while(guard, |w| w.preview.is_none() && w.thumbs.is_empty() && !w.stop)
                .unwrap_or_else(PoisonError::into_inner);
            if w.stop {
                return;
            }
            match w.preview.take() {
                Some(p) => Some((p, true)),
                None => w.thumbs.pop_front().map(|p| (p, false)),
            }
        };
        let Some((path, large)) = next else { continue };
        let side = if large { PREVIEW_SIDE } else { THUMB_SIDE };
        let result = reader.picture(&path, side).map(Arc::new);
        let event = if large { ScanEvent::Preview { path, result } } else { ScanEvent::Thumbnail { path, result } };
        if tx.send(event).is_err() {
            return;
        }
        notify();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    struct Fake;

    impl Reader for Fake {
        fn list(&self, dir: &Path, _subfolders: bool) -> Vec<PathBuf> {
            vec![dir.join("a.NEF"), dir.join("b.JPG")]
        }
        fn captured(&self, path: &Path) -> Option<String> {
            path.ends_with("a.NEF").then(|| "2026:10:29 10:00:00".to_owned())
        }
        fn picture(&self, _path: &Path, side: u32) -> Result<Thumbnail, String> {
            Ok(Thumbnail { width: side, height: 1, rgb: vec![0; side as usize * 3] })
        }
    }

    fn wait(job: &ScanJob, until: impl Fn(&[ScanEvent]) -> bool) -> Vec<ScanEvent> {
        let mut events = Vec::new();
        let started = std::time::Instant::now();
        while !until(&events) && started.elapsed().as_secs() < 10 {
            events.extend(job.poll());
            thread::sleep(std::time::Duration::from_millis(2));
        }
        events
    }

    #[test]
    fn the_source_is_listed_then_its_capture_times_come() {
        let job = ScanJob::start_with(Arc::new(Fake), PathBuf::from("card"), true, Arc::new(|| {})).unwrap();
        let events = wait(&job, |e| e.iter().any(|e| matches!(e, ScanEvent::Done)));
        assert!(matches!(&events[0], ScanEvent::Listed(p) if p.len() == 2));
        let times: Vec<Option<String>> = events
            .iter()
            .filter_map(|e| match e {
                ScanEvent::Captured { captured, .. } => Some(captured.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(times, [Some("2026:10:29 10:00:00".to_owned()), None]);
    }

    #[test]
    fn pictures_come_on_request_the_preview_first() {
        let job = ScanJob::start_with(Arc::new(Fake), PathBuf::from("card"), false, Arc::new(|| {})).unwrap();
        job.request_thumbnails(vec![PathBuf::from("card/a.NEF")]);
        job.request_preview(PathBuf::from("card/b.JPG"));
        let events = wait(&job, |e| {
            e.iter().any(|e| matches!(e, ScanEvent::Preview { .. }))
                && e.iter().any(|e| matches!(e, ScanEvent::Thumbnail { .. }))
        });
        let preview = events.iter().find_map(|e| match e {
            ScanEvent::Preview { result, .. } => Some(result.as_ref().unwrap().width),
            _ => None,
        });
        assert_eq!(preview, Some(PREVIEW_SIDE));
    }
}
