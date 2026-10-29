//! An import running on its own thread, as Capture One's: for each photo
//! it reads the capture time, leaves out the shots already in the catalog,
//! copies the file into the dated sub-folder when the import copies, and
//! makes the backup copy. The window adds each photo it reports to the
//! catalog, so photos appear while the import goes on.
//!
//! The job never writes to the catalog itself: the catalog belongs to the
//! window, and the job only reports.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;
use std::time::UNIX_EPOCH;

use catalog::{Day, Destination, ImportedPhoto, ShotKey, free_name};

/// What to import and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRequest {
    /// The files, in the order they are imported.
    pub files: Vec<PathBuf>,
    /// Add them where they are, or copy them.
    pub destination: Destination,
    /// A second copy of every original below this folder (same sub-folders
    /// as the copies, or the source folder's name when not copying).
    pub backup: Option<PathBuf>,
    /// Leave out shots already in the catalog.
    pub skip_duplicates: bool,
    /// The shots in the catalog when the import started.
    pub known: HashSet<ShotKey>,
}

/// Why a photo was left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skipped {
    /// The same shot (file name and capture time) is in the catalog, or
    /// earlier in this import.
    Duplicate,
    /// It could not be copied (the message says why); the others go on.
    Failed(String),
}

/// What the job reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportProgress {
    /// A photo to add to the catalog.
    Imported(ImportedPhoto),
    /// A photo left out.
    Skipped {
        /// The source file.
        path: PathBuf,
        /// Why.
        why: Skipped,
    },
    /// The backup copy of a photo failed; the photo itself is imported.
    BackupFailed {
        /// The source file.
        path: PathBuf,
        /// Why.
        why: String,
    },
    /// Every photo was done, or the import was stopped.
    Finished {
        /// Stopped by the user before the end.
        stopped: bool,
    },
}

/// A running import.
#[derive(Debug)]
pub struct ImportJob {
    rx: Receiver<ImportProgress>,
    stop: Arc<AtomicBool>,
    /// Photos to import.
    pub total: usize,
    /// Photos done (imported or left out).
    pub done: usize,
}

impl ImportJob {
    /// Starts importing; `notify` is called after each photo so the window
    /// redraws.
    ///
    /// # Errors
    /// The thread cannot start.
    pub fn start(request: ImportRequest, notify: impl Fn() + Send + 'static) -> io::Result<Self> {
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let total = request.files.len();
        let flag = Arc::clone(&stop);
        thread::Builder::new().name("rne-import".into()).spawn(move || {
            let stopped = run(&request, &flag, &mut |p| {
                let _ = tx.send(p);
                notify();
            });
            let _ = tx.send(ImportProgress::Finished { stopped });
            notify();
        })?;
        Ok(Self { rx, stop, total, done: 0 })
    }

    /// Asks the import to stop after the photo it is on.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// What happened since the last call; counts the photos done.
    pub fn poll(&mut self) -> Vec<ImportProgress> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(p) => {
                    if matches!(p, ImportProgress::Imported(_) | ImportProgress::Skipped { .. }) {
                        self.done += 1;
                    }
                    out.push(p);
                }
                Err(TryRecvError::Empty) => break,
                // The thread ended without saying so (it cannot, but never hang).
                Err(TryRecvError::Disconnected) => {
                    if !out.iter().any(|p| matches!(p, ImportProgress::Finished { .. })) {
                        out.push(ImportProgress::Finished { stopped: true });
                    }
                    break;
                }
            }
        }
        out
    }
}

/// Imports every file of `request`, reporting each one; returns whether it
/// was stopped. Runs on the calling thread (the job's thread, or a test).
pub fn run(request: &ImportRequest, stop: &AtomicBool, report: &mut dyn FnMut(ImportProgress)) -> bool {
    run_with(request, stop, &|p| raw_io::read_info(p).ok().and_then(|i| i.captured), report)
}

/// [`run`], with the capture time of each file read by `captured`.
fn run_with(
    request: &ImportRequest,
    stop: &AtomicBool,
    captured_of: &dyn Fn(&Path) -> Option<String>,
    report: &mut dyn FnMut(ImportProgress),
) -> bool {
    let mut seen = HashSet::new();
    for source in &request.files {
        if stop.load(Ordering::Relaxed) {
            return true;
        }
        let captured = captured_of(source);
        let shot = ShotKey::new(source, captured.as_deref());
        if request.skip_duplicates
            && let Some(key) = &shot
            && (request.known.contains(key) || !seen.insert(key.clone()))
        {
            report(ImportProgress::Skipped { path: source.clone(), why: Skipped::Duplicate });
            continue;
        }
        let day = captured.as_deref().and_then(Day::from_exif).or_else(|| modified_day(source));
        let path = match &request.destination {
            Destination::InPlace => source.clone(),
            Destination::Copy { folder, pattern } => match copy_into(source, &folder.join(pattern.folder_for(day))) {
                Ok(copy) => copy,
                Err(e) => {
                    report(ImportProgress::Skipped { path: source.clone(), why: Skipped::Failed(e) });
                    continue;
                }
            },
        };
        if let Some(root) = &request.backup {
            let below = match &request.destination {
                Destination::Copy { pattern, .. } => pattern.folder_for(day),
                Destination::InPlace => {
                    source.parent().and_then(Path::file_name).map(PathBuf::from).unwrap_or_default()
                }
            };
            if let Err(why) = copy_into(source, &root.join(below)) {
                report(ImportProgress::BackupFailed { path: source.clone(), why });
            }
        }
        report(ImportProgress::Imported(ImportedPhoto { path, captured }));
    }
    false
}

/// The day a file was last changed, for photos without a capture date.
fn modified_day(path: &Path) -> Option<Day> {
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok()?;
    let secs = modified.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(Day::from_unix(i64::try_from(secs).ok()?))
}

/// Copies `source` into folder `dir` (made when missing) and returns the
/// copy. A file already there with the same content, under its name or a
/// numbered one ("a (2).NEF"), is taken as the copy (an import run again);
/// another file is never replaced: the copy gets a free name. The copy is written under a
/// temporary name and renamed when whole, and keeps the original's
/// modification time.
fn copy_into(source: &Path, dir: &Path) -> Result<PathBuf, String> {
    let fail = |what: &str, e: io::Error| format!("{what}: {e}");
    let name = source.file_name().ok_or_else(|| format!("{} has no file name", source.display()))?;
    let name = name.to_string_lossy();
    if dir.join(&*name) == source {
        return Ok(source.to_owned());
    }
    let size = fs::metadata(source).map_err(|e| fail("cannot read the file", e))?.len();
    // The free name skips every file already there, unless one is this photo.
    let mut copied = None;
    let target = free_name(dir, &name, |p| {
        if copied.is_none() && same_content(source, size, p) {
            copied = Some(p.to_owned());
        }
        copied.is_none() && p.exists()
    });
    if let Some(copy) = copied {
        return Ok(copy);
    }
    fs::create_dir_all(dir).map_err(|e| fail(&format!("cannot make {}", dir.display()), e))?;
    let part = PathBuf::from(format!("{}.part", target.display()));
    let written = (|| -> io::Result<()> {
        fs::copy(source, &part)?;
        if let Ok(modified) = fs::metadata(source).and_then(|m| m.modified()) {
            fs::File::options().write(true).open(&part)?.set_modified(modified)?;
        }
        fs::rename(&part, &target)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&part);
        return Err(fail(&format!("cannot copy to {}", dir.display()), e));
    }
    Ok(target)
}

/// Whether `other` is a file holding the same bytes as `source` (of `size` bytes).
fn same_content(source: &Path, size: u64, other: &Path) -> bool {
    if !fs::metadata(other).is_ok_and(|m| m.is_file() && m.len() == size) {
        return false;
    }
    let (Ok(mut a), Ok(mut b)) = (fs::File::open(source), fs::File::open(other)) else { return false };
    let (mut x, mut y) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let (Ok(n), Ok(m)) = (fill(&mut a, &mut x), fill(&mut b, &mut y)) else { return false };
        if n != m || x[..n] != y[..m] {
            return false;
        }
        if n == 0 {
            return true;
        }
    }
}

/// Reads into `buf` until it is full or the file ends; returns the bytes read.
fn fill(file: &mut fs::File, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match io::Read::read(file, &mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use catalog::FolderPattern;

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rne-import-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn request(files: Vec<PathBuf>, destination: Destination) -> ImportRequest {
        ImportRequest { files, destination, backup: None, skip_duplicates: true, known: HashSet::new() }
    }

    fn collect(request: &ImportRequest) -> Vec<ImportProgress> {
        let mut out = Vec::new();
        run(request, &AtomicBool::new(false), &mut |p| out.push(p));
        out
    }

    #[test]
    fn photos_added_in_place_are_reported_as_they_are() {
        let dir = temp("in-place");
        let a = dir.join("a.NEF");
        fs::write(&a, b"raw").unwrap();
        let events = collect(&request(vec![a.clone()], Destination::InPlace));
        assert_eq!(events, [ImportProgress::Imported(ImportedPhoto { path: a, captured: None })]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copies_go_in_dated_folders_and_never_replace_another_file() {
        let dir = temp("copy");
        let card = dir.join("card");
        fs::create_dir_all(&card).unwrap();
        let a = card.join("a.NEF");
        fs::write(&a, b"raw").unwrap();
        let out = dir.join("Photos");
        // Not a real RAW file: its day is the file's modification day.
        let day = modified_day(&a).unwrap();
        let sub = FolderPattern::default().folder_for(Some(day));
        fs::create_dir_all(out.join(&sub)).unwrap();
        fs::write(out.join(&sub).join("a.NEF"), b"another photo").unwrap();
        let destination = Destination::Copy { folder: out.clone(), pattern: FolderPattern::default() };
        let mut req = request(vec![a.clone()], destination);
        req.backup = Some(dir.join("Backup"));
        let events = collect(&req);
        let copy = out.join(&sub).join("a (2).NEF");
        assert_eq!(events, [ImportProgress::Imported(ImportedPhoto { path: copy.clone(), captured: None })]);
        assert_eq!(fs::read(&copy).unwrap(), b"raw");
        assert_eq!(fs::read(dir.join("Backup").join(&sub).join("a.NEF")).unwrap(), b"raw");
        assert!(!PathBuf::from(format!("{}.part", copy.display())).exists());
        // Run again: the copy already there is taken, nothing is copied twice.
        let again = collect(&req);
        assert_eq!(again, [ImportProgress::Imported(ImportedPhoto { path: copy, captured: None })]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shots_already_in_the_catalog_or_earlier_in_the_import_are_left_out() {
        let dir = temp("duplicates");
        let files: Vec<PathBuf> = ["a.NEF", "b.NEF", "sub/b.NEF", "c.NEF"].iter().map(|n| dir.join(n)).collect();
        fs::create_dir_all(dir.join("sub")).unwrap();
        for f in &files {
            fs::write(f, b"raw").unwrap();
        }
        let when = |p: &Path| (!p.ends_with("c.NEF")).then(|| "2026:10:29 10:00:00".to_owned());
        let mut req = request(files.clone(), Destination::InPlace);
        req.known.insert(ShotKey::new(Path::new("old/A.nef"), Some("2026:10:29 10:00:00")).unwrap());
        let mut out = Vec::new();
        run_with(&req, &AtomicBool::new(false), &when, &mut |p| out.push(p));
        let dup = |i: usize| ImportProgress::Skipped { path: files[i].clone(), why: Skipped::Duplicate };
        let added = |i: usize, c: Option<&str>| {
            ImportProgress::Imported(ImportedPhoto { path: files[i].clone(), captured: c.map(str::to_owned) })
        };
        assert_eq!(out, [dup(0), added(1, Some("2026:10:29 10:00:00")), dup(2), added(3, None)]);
        // Without the check every photo comes in.
        req.skip_duplicates = false;
        let mut all = Vec::new();
        run_with(&req, &AtomicBool::new(false), &when, &mut |p| all.push(p));
        assert_eq!(all.len(), 4);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_stopped_import_reports_nothing_more() {
        let dir = temp("stop");
        let a = dir.join("a.NEF");
        fs::write(&a, b"raw").unwrap();
        let mut out = Vec::new();
        let stopped = run(&request(vec![a], Destination::InPlace), &AtomicBool::new(true), &mut |p| out.push(p));
        assert!(stopped && out.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_job_reports_on_its_own_thread_until_finished() {
        let dir = temp("job");
        let a = dir.join("a.NEF");
        fs::write(&a, b"raw").unwrap();
        let mut job = ImportJob::start(request(vec![a], Destination::InPlace), || {}).unwrap();
        let mut events = Vec::new();
        let started = std::time::Instant::now();
        while !events.iter().any(|e| matches!(e, ImportProgress::Finished { .. })) && started.elapsed().as_secs() < 10 {
            events.extend(job.poll());
            thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(job.done, 1);
        assert_eq!(events.last(), Some(&ImportProgress::Finished { stopped: false }));
        fs::remove_dir_all(&dir).unwrap();
    }
}
