//! The one loaded video, its work folder and its jobs. Holds no Tauri types,
//! so it is tested without a window.
//!
//! [`Shared`] keeps the [`Session`] behind one lock. A command holds it only
//! to check and change the session; ffmpeg runs on a job thread of its own,
//! which takes the lock again only to record what came out. Loading is the
//! exception: it probes the file while it holds the lock, so no command can
//! act on a video that is about to be replaced. The window's drop and file
//! chooser call it off the main thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tempfile::TempDir;
use video_optimizer_core::probe::{VideoInfo, probe};
use video_optimizer_core::run::{CancelFlag, RunError};
use video_optimizer_core::sample::{SampleResult, discard, figures, make_before, make_sample};
use video_optimizer_core::save::save;
use video_optimizer_core::settings::{Audio, Crf, Settings};
use video_optimizer_core::tools::Tools;

use crate::events::{
    BeforeReady, Event, Notice, SampleFailed, SaveProgress, file_name_of, io_reason, refusal,
    sample, save_finished, video_loaded, without_folders,
};

// The shared test-clip helper, included once; `media`'s tests use it from here.
#[cfg(test)]
#[path = "../../crates/core/tests/common/clips.rs"]
pub(crate) mod clips;
#[cfg(all(test, unix))]
#[path = "../../crates/core/tests/common/fake_ffmpeg.rs"]
mod fake_ffmpeg;

/// Sends an event to the window. Called from the commands' threads and from
/// the job threads.
pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

/// How long closing the window waits for the jobs to end.
pub const CLOSE_WAIT: Duration = Duration::from_secs(5);

/// How often a sample that waits for the before clip looks at its cancel
/// flag.
const WAIT_STEP: Duration = Duration::from_millis(100);

/// The answer of `choose_folder`: the name of the folder a save goes to, or
/// `null` for next to the original.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FolderAnswer {
    pub folder: Option<String>,
}

/// The before clip of the loaded video.
#[derive(Debug)]
enum Before {
    /// Being made; the flag stops it.
    Making(CancelFlag),
    Ready(PathBuf),
    /// Could not be made, for this reason.
    Failed(String),
}

/// The loaded video.
#[derive(Debug)]
struct Loaded {
    /// The id the window knows it by.
    id: u64,
    info: VideoInfo,
    /// Where its before clip and samples are made. Each job holds a clone, so
    /// the folder is deleted once the video is replaced (or the window
    /// closed) and the last job working in it has ended.
    work: Arc<TempDir>,
    before: Before,
    /// The newest finished sample, by request number: at most one.
    samples: HashMap<u64, PathBuf>,
    /// The highest request number asked for this video. A request that is not
    /// above it starts nothing, so each request's file name is its own.
    last_request: Option<u64>,
}

impl Loaded {
    /// Stop the before clip, if it is still being made.
    fn stop_before(&self) {
        if let Before::Making(flag) = &self.before {
            flag.cancel();
        }
    }
}

/// Everything the backend keeps between commands.
#[derive(Debug)]
struct Session {
    tools: Tools,
    /// The folder each video's work folder is made in.
    work_root: PathBuf,
    video: Option<Loaded>,
    /// The id the next loaded video gets; the first is 1.
    next_video: u64,
    /// The chosen output folder; `None` for next to the original.
    folder: Option<PathBuf>,
    /// The running sample of the current video: its request number and its
    /// flag. A newer request, a save, a load and closing cancel it.
    sample_job: Option<(u64, CancelFlag)>,
    /// The running save. Set while it runs; its thread clears it.
    save_job: Option<CancelFlag>,
    /// How many job threads have not ended yet.
    running: usize,
    /// The app is closing: no command starts anything any more, so
    /// [`Shared::close`] never waits for a job that started after it.
    closing: bool,
}

impl Session {
    /// Every folder the backend knows of: those of ffmpeg and ffprobe, the
    /// work root, the chosen output folder, and the current video's own
    /// folder and work folder. No reason sent to the window names them
    /// ([`without_folders`]).
    fn folders(&self) -> Vec<PathBuf> {
        let tools = [&self.tools.ffmpeg, &self.tools.ffprobe]
            .into_iter()
            .filter_map(|program| program.parent());
        let video = self
            .video
            .iter()
            .flat_map(|loaded| [loaded.info.path.parent(), Some(loaded.work.path())])
            .flatten();
        tools
            .chain(video)
            .chain([self.work_root.as_path()])
            .chain(self.folder.as_deref())
            .map(Path::to_path_buf)
            .collect()
    }
}

/// The session, shared by the commands, the window's events and the job
/// threads.
pub struct Shared {
    session: Mutex<Session>,
    /// Told whenever a job ends, and whenever a before clip is made or fails
    /// (that job ends right after).
    changed: Condvar,
    emit: Emit,
}

/// Counts a job thread as ended when it is dropped, also when the job panics.
struct JobEnded<'a>(&'a Shared);

impl Drop for JobEnded<'_> {
    fn drop(&mut self) {
        self.0.lock().running -= 1;
        self.0.changed.notify_all();
    }
}

/// The settings of a command, through the core's checks. The error is the
/// core's sentence, which answers the window's invoke.
fn settings_from_json(crf: &Value, audio: &Value) -> Result<Settings, String> {
    Ok(Settings {
        crf: Crf::from_json(crf).map_err(|error| error.to_string())?,
        audio: Audio::from_json(audio).map_err(|error| error.to_string())?,
    })
}

/// Ends the sample job of request `request` of `video`, if that is the
/// running one, and gives the loaded video it belongs to. `None` (and the job
/// left as it is) when the video was replaced, or a newer request or a save
/// took its place.
fn end_sample_job(session: &mut Session, video: u64, request: u64) -> Option<&mut Loaded> {
    let loaded = session.video.as_mut().filter(|loaded| loaded.id == video)?;
    if !matches!(&session.sample_job, Some((running, _)) if *running == request) {
        return None;
    }
    session.sample_job = None;
    Some(loaded)
}

impl Shared {
    /// A session with nothing loaded, that finds ffmpeg as `tools` says,
    /// makes each video's work folder in `work_root`, and tells the window
    /// through `emit`.
    pub fn new(tools: Tools, work_root: PathBuf, emit: Emit) -> Arc<Shared> {
        Arc::new(Shared {
            session: Mutex::new(Session {
                tools,
                work_root,
                video: None,
                next_video: 1,
                folder: None,
                sample_job: None,
                save_job: None,
                running: 0,
                closing: false,
            }),
            changed: Condvar::new(),
            emit,
        })
    }

    /// The session, also after a panic elsewhere poisoned the lock.
    fn lock(&self) -> MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn send(&self, event: Event) {
        (self.emit)(event);
    }

    /// Run `job` on a thread of its own, counted in `running` until it ends,
    /// so [`Shared::close`] can wait for it.
    fn spawn_job(
        self: &Arc<Self>,
        session: &mut Session,
        name: &str,
        job: impl FnOnce(&Shared) + Send + 'static,
    ) {
        session.running += 1;
        let shared = Arc::clone(self);
        thread::Builder::new()
            .name(format!("video-optimizer-{name}"))
            .spawn(move || {
                let _ended = JobEnded(&shared);
                job(&shared);
            })
            .expect("a job thread can be started");
    }

    /// A drop, or the file chooser's answer. Refused with `saving` while a
    /// save runs, and with `several-files` unless there is exactly one path.
    /// The file is probed before anything is replaced, so a refused file
    /// (`refused`, `too-large`) or a work folder that cannot be made
    /// (`failed`) leaves the current video as it is. Otherwise the old video
    /// and its jobs are stopped and dropped, the new one gets the next id and
    /// a work folder, `video` is sent, and its before clip is made on a job
    /// thread (`before-ready`, or `failed` when it cannot be made). Once the
    /// app is closing, nothing happens at all.
    pub fn load(self: &Arc<Self>, paths: &[PathBuf]) {
        let mut guard = self.lock();
        let session = &mut *guard;
        if session.closing {
            return;
        }
        if session.save_job.is_some() {
            self.send(Event::Notice(Notice::Saving));
            return;
        }
        let [path] = paths else {
            self.send(Event::Notice(Notice::SeveralFiles));
            return;
        };
        let info = match probe(&session.tools, path) {
            Ok(info) => info,
            Err(error) => {
                self.send(Event::Notice(refusal(path, &error, &session.folders())));
                return;
            }
        };
        let work = match tempfile::Builder::new()
            .prefix("video-optimizer-")
            .tempdir_in(&session.work_root)
        {
            Ok(work) => Arc::new(work),
            Err(error) => {
                // tempfile's text names the folder; only the system's
                // sentence goes to the window.
                self.send(Event::Notice(Notice::Failed {
                    reason: format!("no work folder could be made: {}", io_reason(&error)),
                }));
                return;
            }
        };
        if let Some(old) = session.video.take() {
            old.stop_before();
        }
        if let Some((_, sample)) = session.sample_job.take() {
            sample.cancel();
        }
        let id = session.next_video;
        session.next_video += 1;
        let cancel = CancelFlag::new();
        session.video = Some(Loaded {
            id,
            info: info.clone(),
            work: Arc::clone(&work),
            before: Before::Making(cancel.clone()),
            samples: HashMap::new(),
            last_request: None,
        });
        self.send(Event::Video(video_loaded(id, &info)));
        let tools = session.tools.clone();
        self.spawn_job(session, "before", move |shared| {
            shared.run_before(id, &tools, &info, work, &cancel);
        });
    }

    /// The before clip job of video `id`. Its outcome is kept only while the
    /// video is still the current one. A failure's reason names no folder.
    fn run_before(
        &self,
        id: u64,
        tools: &Tools,
        info: &VideoInfo,
        work: Arc<TempDir>,
        cancel: &CancelFlag,
    ) {
        let result = make_before(tools, info, work.path(), cancel);
        // A replaced video's folder goes as soon as no job holds it.
        drop(work);
        let mut session = self.lock();
        let folders = session.folders();
        let Some(loaded) = session.video.as_mut().filter(|loaded| loaded.id == id) else {
            return;
        };
        match result {
            Ok(path) => {
                loaded.before = Before::Ready(path);
                self.send(Event::BeforeReady(BeforeReady { video: id }));
            }
            Err(error) => {
                let reason = without_folders(&error.to_string(), &folders);
                loaded.before = Before::Failed(reason.clone());
                self.send(Event::Notice(Notice::Failed { reason }));
            }
        }
    }

    /// Make the sample of request `request` of `video` at `crf` and `audio`
    /// (JSON values, checked by the core; a refusal is the error and starts
    /// nothing). Nothing starts once the app is closing, nor while a save
    /// runs (a request asked then does not count as asked), nor for a `video`
    /// that is not the current one or a request that is not above the last
    /// one for this video. Otherwise the running sample is cancelled and this
    /// one starts on a job thread, once the before clip exists: `sample`, or
    /// `sample-failed`.
    pub fn make_sample(
        self: &Arc<Self>,
        video: u64,
        request: u64,
        crf: &Value,
        audio: &Value,
    ) -> Result<(), String> {
        let settings = settings_from_json(crf, audio)?;
        let mut guard = self.lock();
        let session = &mut *guard;
        if session.closing || session.save_job.is_some() {
            return Ok(());
        }
        let Some(loaded) = session.video.as_mut().filter(|loaded| loaded.id == video) else {
            return Ok(());
        };
        if loaded.last_request.is_some_and(|last| request <= last) {
            return Ok(());
        }
        loaded.last_request = Some(request);
        let cancel = CancelFlag::new();
        if let Some((_, older)) = session.sample_job.replace((request, cancel.clone())) {
            older.cancel();
        }
        self.spawn_job(session, "sample", move |shared| {
            shared.run_sample(video, request, settings, &cancel);
        });
        Ok(())
    }

    /// The sample job of request `request`.
    fn run_sample(&self, video: u64, request: u64, settings: Settings, cancel: &CancelFlag) {
        let Some((tools, info, work, before)) = self.wait_for_before(video, request, cancel) else {
            return;
        };
        let name = format!("sample-{request}");
        let result = make_sample(&tools, &info, settings, &before, work.path(), &name, cancel);
        drop(work);
        self.finish_sample(video, request, settings, result);
    }

    /// What the sample of request `request` needs, once the before clip of
    /// `video` exists: the tools, the video, its work folder and the before
    /// clip. `None` when the request is cancelled or its video replaced while
    /// it waits, and when the before clip failed; then `sample-failed` has
    /// been sent with the before clip's reason.
    fn wait_for_before(
        &self,
        video: u64,
        request: u64,
        cancel: &CancelFlag,
    ) -> Option<(Tools, VideoInfo, Arc<TempDir>, PathBuf)> {
        let mut session = self.lock();
        loop {
            let loaded = session
                .video
                .as_ref()
                .filter(|loaded| loaded.id == video && !cancel.is_cancelled())?;
            match &loaded.before {
                Before::Ready(before) => {
                    let needs = (
                        session.tools.clone(),
                        loaded.info.clone(),
                        Arc::clone(&loaded.work),
                        before.clone(),
                    );
                    return Some(needs);
                }
                Before::Failed(reason) => {
                    let reason = reason.clone();
                    if end_sample_job(&mut session, video, request).is_some() {
                        self.send(Event::SampleFailed(SampleFailed {
                            video,
                            request,
                            reason,
                        }));
                    }
                    return None;
                }
                Before::Making(_) => {
                    session = self
                        .changed
                        .wait_timeout(session, WAIT_STEP)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            }
        }
    }

    /// Record how request `request` of `video` ended. Only the running
    /// request is told: its sample replaces (and deletes) the older sample
    /// file, or `sample-failed` is sent. A request that was replaced, or whose
    /// video was, is not told; a sample it finished all the same (its cancel
    /// came too late) is deleted. A failure's reason names no folder.
    fn finish_sample(
        &self,
        video: u64,
        request: u64,
        settings: Settings,
        result: Result<SampleResult, RunError>,
    ) {
        let mut session = self.lock();
        let folders = session.folders();
        let Some(loaded) = end_sample_job(&mut session, video, request) else {
            if let Ok(late) = result {
                discard(&late.path);
            }
            return;
        };
        match result {
            Ok(done) => {
                for (_, older) in loaded.samples.drain() {
                    discard(&older);
                }
                loaded.samples.insert(request, done.path);
                let figures = figures(&loaded.info, done.bytes);
                self.send(Event::Sample(sample(
                    video,
                    request,
                    settings.crf,
                    &figures,
                    done.psnr,
                )));
            }
            Err(error) => self.send(Event::SampleFailed(SampleFailed {
                video,
                request,
                reason: without_folders(&error.to_string(), &folders),
            })),
        }
    }

    /// Save the whole of `video` at `crf` and `audio` (checked as for
    /// [`Shared::make_sample`]) in the chosen folder, or next to the
    /// original. Nothing starts once the app is closing, for a `video` that
    /// is not the current one, or while a save runs. Otherwise the running
    /// sample is cancelled and the save runs on a job thread:
    /// `save-progress`, then `save-done`, `save-cancelled` or `save-failed`.
    pub fn save(self: &Arc<Self>, video: u64, crf: &Value, audio: &Value) -> Result<(), String> {
        let settings = settings_from_json(crf, audio)?;
        let mut guard = self.lock();
        let session = &mut *guard;
        if session.closing {
            return Ok(());
        }
        let Some(loaded) = session.video.as_ref().filter(|loaded| loaded.id == video) else {
            return Ok(());
        };
        if session.save_job.is_some() {
            return Ok(());
        }
        let info = loaded.info.clone();
        if let Some((_, sample)) = session.sample_job.take() {
            sample.cancel();
        }
        let cancel = CancelFlag::new();
        session.save_job = Some(cancel.clone());
        let tools = session.tools.clone();
        let folder = session.folder.clone();
        let folders = session.folders();
        self.spawn_job(session, "save", move |shared| {
            shared.run_save(
                &tools,
                &info,
                settings,
                folder.as_deref(),
                &cancel,
                &folders,
            );
        });
        Ok(())
    }

    /// The save job. `save_job` is cleared before the outcome is sent, so a
    /// drop that follows `save-done` is loaded. A failure's reason names none
    /// of `folders` (those known when the save started, its own included).
    fn run_save(
        &self,
        tools: &Tools,
        info: &VideoInfo,
        settings: Settings,
        folder: Option<&Path>,
        cancel: &CancelFlag,
        folders: &[PathBuf],
    ) {
        let result = save(tools, info, settings, folder, cancel, &mut |percent| {
            self.send(Event::SaveProgress(SaveProgress { percent }));
        });
        self.lock().save_job = None;
        self.send(save_finished(info.bytes, result, folders));
    }

    /// Stop the running save, if there is one. Its thread removes the partial
    /// file and sends `save-cancelled`, unless it had already finished.
    pub fn cancel_save(&self) {
        if let Some(save) = &self.lock().save_job {
            save.cancel();
        }
    }

    /// The folder dialog's answer: a picked folder becomes the one a save
    /// goes to; a closed dialog (`None`) changes nothing. The answer names
    /// the folder in effect afterwards.
    pub fn choose_folder(&self, picked: Option<PathBuf>) -> FolderAnswer {
        let mut session = self.lock();
        if let Some(folder) = picked {
            session.folder = Some(folder);
        }
        FolderAnswer {
            folder: session.folder.as_deref().map(file_name_of),
        }
    }

    /// Back to saving next to the original.
    pub fn clear_folder(&self) {
        self.lock().folder = None;
    }

    /// The window is closing or the app is ending: from now on no command
    /// starts anything; stop every job, wait at most `wait` for them to end,
    /// then drop the video, which deletes its work folder. A stopped save
    /// removes its partial file before its thread ends, so the folder it was
    /// saving in is as it was once this returns (unless a job outlasted
    /// `wait`). Calling it again finds nothing to stop.
    pub fn close(&self, wait: Duration) {
        let mut session = self.lock();
        session.closing = true;
        if let Some(save) = &session.save_job {
            save.cancel();
        }
        if let Some((_, sample)) = &session.sample_job {
            sample.cancel();
        }
        if let Some(loaded) = &session.video {
            loaded.stop_before();
        }
        let (mut session, _) = self.wait_for_jobs(session, wait);
        session.video = None;
    }

    /// Wait at most `wait` until no job runs; whether none does. The lock is
    /// let go while waiting, so the jobs can finish.
    fn wait_for_jobs<'a>(
        &'a self,
        session: MutexGuard<'a, Session>,
        wait: Duration,
    ) -> (MutexGuard<'a, Session>, bool) {
        let (session, waited) = self
            .changed
            .wait_timeout_while(session, wait, |session| session.running > 0)
            .unwrap_or_else(PoisonError::into_inner);
        (session, !waited.timed_out())
    }

    /// The before clip of `video`, once it is made and while `video` is the
    /// current one.
    pub fn before_path(&self, video: u64) -> Option<PathBuf> {
        let session = self.lock();
        let loaded = session.video.as_ref().filter(|loaded| loaded.id == video)?;
        match &loaded.before {
            Before::Ready(path) => Some(path.clone()),
            Before::Making(_) | Before::Failed(_) => None,
        }
    }

    /// The sample of request `request` of `video`, while it is the newest
    /// finished one of the current video.
    pub fn sample_path(&self, video: u64, request: u64) -> Option<PathBuf> {
        let session = self.lock();
        let loaded = session.video.as_ref().filter(|loaded| loaded.id == video)?;
        loaded.samples.get(&request).cloned()
    }
}

#[cfg(test)]
impl Shared {
    /// Wait at most `wait` until no job runs; whether none does.
    fn wait_idle(&self, wait: Duration) -> bool {
        self.wait_for_jobs(self.lock(), wait).1
    }

    fn is_closing(&self) -> bool {
        self.lock().closing
    }

    fn current_video(&self) -> Option<u64> {
        self.lock().video.as_ref().map(|loaded| loaded.id)
    }

    fn sample_job(&self) -> Option<(u64, CancelFlag)> {
        self.lock().sample_job.clone()
    }

    fn saving(&self) -> bool {
        self.lock().save_job.is_some()
    }

    fn save_flag(&self) -> Option<CancelFlag> {
        self.lock().save_job.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::mpsc::{self, Receiver};
    use std::time::Instant;

    use serde_json::json;
    use video_optimizer_core::settings::Crf;

    use super::clips::{self, Clip};
    use super::*;
    use crate::events::{Notice, SampleFailed};

    /// The longest a test waits for one event.
    const WAIT: Duration = Duration::from_secs(120);

    fn real_tools() -> Tools {
        Tools::locate().expect(
            "ffmpeg and ffprobe are needed for these tests: put them on the PATH or set \
             VIDEO_OPTIMIZER_TOOLS_DIR",
        )
    }

    /// A session with real wiring: the tools it is given, a work root inside
    /// a test folder, and every event it sends collected.
    struct Harness {
        shared: Arc<Shared>,
        events: Receiver<Event>,
        /// Every event received so far, in order.
        log: Vec<Event>,
        dir: TempDir,
    }

    impl Harness {
        fn new() -> Harness {
            Harness::with_tools(real_tools())
        }

        fn with_tools(tools: Tools) -> Harness {
            let dir = TempDir::new().unwrap();
            let work_root = dir.path().join("work");
            fs::create_dir(&work_root).unwrap();
            Harness::with(tools, work_root, dir)
        }

        fn with(tools: Tools, work_root: PathBuf, dir: TempDir) -> Harness {
            let (sender, events) = mpsc::channel();
            let emit: Emit = Arc::new(move |event| {
                let _ = sender.send(event);
            });
            Harness {
                shared: Shared::new(tools, work_root, emit),
                events,
                log: Vec::new(),
                dir,
            }
        }

        /// A 2 second 640x360 H.264 clip without sound.
        fn clip(&self, name: &str) -> PathBuf {
            clips::make(
                self.dir.path(),
                Clip {
                    name: name.into(),
                    ..Clip::default()
                },
            )
        }

        /// A clip whose save at preset slow takes several seconds.
        fn long_clip(&self, name: &str) -> PathBuf {
            clips::make(
                self.dir.path(),
                Clip {
                    name: name.into(),
                    seconds: 20.0,
                    fps: 30,
                    width: 1280,
                    height: 720,
                    ..Clip::default()
                },
            )
        }

        /// The next event that `matches`, logging every event before it.
        fn next(&mut self, what: &str, matches: impl Fn(&Event) -> bool) -> Event {
            loop {
                let event = self
                    .events
                    .recv_timeout(WAIT)
                    .unwrap_or_else(|_| panic!("no {what} came; seen {:?}", self.log));
                self.log.push(event.clone());
                if matches(&event) {
                    return event;
                }
            }
        }

        /// Wait until no job runs, then log the events still queued.
        fn idle(&mut self) {
            assert!(self.shared.wait_idle(WAIT), "the jobs did not end");
            self.log.extend(self.events.try_iter());
        }

        /// Load `path` and wait for its before clip; its id.
        fn load_ready(&mut self, path: &Path) -> u64 {
            self.shared.load(&[path.to_path_buf()]);
            let id = self.next_video();
            self.next(
                "before-ready",
                |e| matches!(e, Event::BeforeReady(b) if b.video == id),
            );
            id
        }

        /// The id of the next `video` event.
        fn next_video(&mut self) -> u64 {
            match self.next("video", |e| matches!(e, Event::Video(_))) {
                Event::Video(video) => video.video,
                _ => unreachable!(),
            }
        }

        fn next_notice(&mut self) -> Notice {
            match self.next("notice", |e| matches!(e, Event::Notice(_))) {
                Event::Notice(notice) => notice,
                _ => unreachable!(),
            }
        }

        fn next_sample(&mut self, request: u64) -> crate::events::Sample {
            match self.next(
                "sample",
                |e| matches!(e, Event::Sample(s) if s.request == request),
            ) {
                Event::Sample(sample) => sample,
                _ => unreachable!(),
            }
        }

        fn next_sample_failed(&mut self, request: u64) -> SampleFailed {
            match self.next(
                "sample-failed",
                |e| matches!(e, Event::SampleFailed(s) if s.request == request),
            ) {
                Event::SampleFailed(failed) => failed,
                _ => unreachable!(),
            }
        }

        /// The events logged so far with this name.
        fn logged(&self, name: &str) -> Vec<&Event> {
            self.log.iter().filter(|e| e.name() == name).collect()
        }
    }

    fn keep() -> Value {
        json!("keep")
    }

    /// The names in `dir`, sorted.
    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// `reason` names no folder: not the test folder (where the clips, the
    /// work root and the outputs are), and no path separator at all.
    fn assert_no_path(h: &Harness, reason: &str) {
        let dir = h.dir.path().to_string_lossy();
        assert!(!reason.contains(dir.as_ref()), "{reason}");
        assert!(!reason.contains(std::path::MAIN_SEPARATOR), "{reason}");
        assert!(!reason.contains("file:"), "{reason}");
    }

    /// Wait until `done` holds, for at most [`WAIT`].
    fn wait_until(what: &str, done: impl Fn() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < WAIT, "{what} did not happen");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    // ---- loading ----

    #[test]
    fn a_valid_file_is_loaded() {
        let mut h = Harness::new();
        let path = h.clip("a.mp4");
        h.shared.load(std::slice::from_ref(&path));
        let Event::Video(video) = h.next("video", |e| matches!(e, Event::Video(_))) else {
            unreachable!()
        };
        assert_eq!(video.video, 1);
        assert_eq!(video.name, "a.mp4");
        assert_eq!((video.width, video.height), (640, 360));
        assert_eq!(video.fps, Some(10.0));
        let duration = video.duration.expect("a duration");
        assert!((duration - 2.0).abs() < 0.1, "{duration}");
        assert_eq!(video.bytes, fs::metadata(&path).unwrap().len());
        assert_eq!(video.audio, None);
        assert!(!video.display_copy);
        h.next(
            "before-ready",
            |e| matches!(e, Event::BeforeReady(b) if b.video == 1),
        );
        let before = h.shared.before_path(1).expect("the before clip is kept");
        assert!(before.is_file(), "{before:?}");
    }

    #[test]
    fn loading_a_second_video_replaces_the_first() {
        let mut h = Harness::new();
        let first = h.load_ready(&h.clip("a.mp4"));
        assert_eq!(first, 1);
        let first_work = h
            .shared
            .before_path(1)
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert!(first_work.is_dir());
        let second = h.load_ready(&h.clip("b.mp4"));
        assert_eq!(second, 2);
        assert!(!first_work.exists(), "the first work folder is deleted");
        assert_eq!(h.shared.before_path(1), None);
        assert!(h.shared.before_path(2).unwrap().is_file());
    }

    #[test]
    fn dropping_two_files_loads_neither() {
        let mut h = Harness::new();
        let a = h.clip("a.mp4");
        let b = h.clip("b.mp4");
        assert_eq!(h.load_ready(&a), 1);
        h.shared.load(&[a.clone(), b.clone()]);
        assert_eq!(h.next_notice(), Notice::SeveralFiles);
        h.shared.load(&[]);
        assert_eq!(h.next_notice(), Notice::SeveralFiles);
        h.idle();
        assert!(h.logged("video").len() == 1, "{:?}", h.log);
        assert_eq!(h.shared.current_video(), Some(1));
        assert!(h.shared.before_path(1).is_some());
    }

    #[test]
    fn a_refused_file_keeps_the_current_video() {
        let mut h = Harness::new();
        assert_eq!(h.load_ready(&h.clip("a.mp4")), 1);
        let text = h.dir.path().join("notes.mp4");
        fs::write(&text, "these are notes, not a video\n").unwrap();
        h.shared.load(&[text]);
        match h.next_notice() {
            Notice::Refused { name, reason } => {
                assert_eq!(name, "notes.mp4");
                // ffprobe's own sentence, without the path it puts first.
                assert_eq!(reason, "Invalid data found when processing input");
                assert_no_path(&h, &reason);
            }
            other => panic!("expected refused, got {other:?}"),
        }
        // No video stream (a sound file), and a folder.
        let sound = clips::make_audio(h.dir.path(), "tone.m4a", 1.0, false);
        h.shared.load(&[sound]);
        assert_eq!(
            h.next_notice(),
            Notice::Refused {
                name: "tone.m4a".into(),
                reason: "this file has no video".into()
            }
        );
        let folder = h.dir.path().join("a folder");
        fs::create_dir(&folder).unwrap();
        h.shared.load(&[folder]);
        assert_eq!(
            h.next_notice(),
            Notice::Refused {
                name: "a folder".into(),
                reason: "this is not a file".into()
            }
        );
        h.idle();
        assert_eq!(h.logged("video").len(), 1);
        assert_eq!(h.shared.current_video(), Some(1));
        assert!(h.shared.before_path(1).unwrap().is_file());
    }

    #[test]
    fn a_work_folder_that_cannot_be_made_is_a_failed_notice_and_keeps_the_video() {
        let mut h = Harness::new();
        assert_eq!(h.load_ready(&h.clip("a.mp4")), 1);
        let b = h.clip("b.mp4");
        // The work root goes away: the next load has nowhere to work.
        fs::remove_dir_all(h.dir.path().join("work")).unwrap();
        h.shared.load(&[b]);
        match h.next_notice() {
            Notice::Failed { reason } => {
                assert!(!reason.is_empty());
                // tempfile names the folder it tried; the window is not told.
                assert_no_path(&h, &reason);
            }
            other => panic!("expected failed, got {other:?}"),
        }
        h.idle();
        assert_eq!(h.logged("video").len(), 1);
        assert_eq!(h.shared.current_video(), Some(1));
    }

    #[test]
    fn a_drop_while_saving_is_refused() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.long_clip("long.mp4"));
        h.shared.save(id, &json!(28), &keep()).unwrap();
        h.shared.load(&[h.clip("b.mp4")]);
        assert_eq!(h.next_notice(), Notice::Saving);
        assert_eq!(h.shared.current_video(), Some(id));
        h.shared.cancel_save();
        h.next("save-cancelled", |e| matches!(e, Event::SaveCancelled(_)));
        h.idle();
        assert_eq!(h.logged("video").len(), 1);
    }

    #[test]
    fn a_drop_after_the_save_is_loaded() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        h.shared.save(id, &json!(28), &keep()).unwrap();
        h.next("save-done", |e| matches!(e, Event::SaveDone(_)));
        assert_eq!(h.load_ready(&h.clip("b.mp4")), 2);
        assert!(h.logged("notice").is_empty(), "{:?}", h.log);
    }

    // ---- samples ----

    #[test]
    fn a_single_request_finishes() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        h.shared.make_sample(id, 1, &json!(28), &keep()).unwrap();
        let sample = h.next_sample(1);
        assert_eq!((sample.video, sample.request, sample.crf), (id, 1, 28));
        assert!(sample.sample_bytes > 0);
        assert!(sample.original_bytes.is_some());
        assert!(sample.whole_bytes.is_some());
        let psnr = sample.psnr.expect("a PSNR");
        assert!(psnr.is_finite() && psnr > 20.0, "{psnr}");
        assert!(!sample.identical);
        let path = h.shared.sample_path(id, 1).expect("the sample is kept");
        assert_eq!(fs::metadata(&path).unwrap().len(), sample.sample_bytes);
    }

    #[test]
    fn a_newer_sample_cancels_the_older() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.long_clip("long.mp4"));
        h.shared.make_sample(id, 1, &json!(28), &keep()).unwrap();
        let (request, first) = h.shared.sample_job().expect("request 1 runs");
        assert_eq!(request, 1);
        assert!(!first.is_cancelled());
        h.shared.make_sample(id, 2, &json!(30), &keep()).unwrap();
        assert!(first.is_cancelled(), "request 2 cancels request 1");
        let (request, second) = h.shared.sample_job().expect("request 2 runs");
        assert_eq!(request, 2);
        assert!(!second.is_cancelled());
        h.next_sample(2);
        h.idle();
        let requests: Vec<u64> = h
            .log
            .iter()
            .filter_map(|e| match e {
                Event::Sample(s) => Some(s.request),
                Event::SampleFailed(s) => Some(s.request),
                _ => None,
            })
            .collect();
        assert_eq!(requests, [2]);
        let work = h
            .shared
            .before_path(id)
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert_eq!(listing(&work), ["before.mp4", "sample-2.mp4"]);
        assert_eq!(h.shared.sample_path(id, 1), None);
        assert!(h.shared.sample_path(id, 2).is_some());
    }

    #[test]
    fn make_sample_refuses_a_bad_crf_and_starts_nothing() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        for crf in [json!(37), json!("28"), json!(28.5), json!([28]), json!(19)] {
            assert_eq!(
                h.shared.make_sample(id, 1, &crf, &keep()),
                Err("CRF must be a whole number from 20 to 36".to_owned()),
                "{crf}"
            );
        }
        assert_eq!(
            h.shared.make_sample(id, 1, &json!(28), &json!("Keep")),
            Err("Audio must be keep or remove".to_owned())
        );
        assert!(h.shared.sample_job().is_none());
        h.idle();
        assert!(h.logged("sample").is_empty());
        assert!(h.logged("sample-failed").is_empty());
        // A refused request does not count: request 1 still starts.
        h.shared.make_sample(id, 1, &json!(28), &keep()).unwrap();
        h.next_sample(1);
    }

    #[test]
    fn make_sample_accepts_20_and_36_and_keeps_only_the_newest_file() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        h.shared.make_sample(id, 1, &json!(20), &keep()).unwrap();
        assert_eq!(h.next_sample(1).crf, 20);
        let first = h.shared.sample_path(id, 1).expect("request 1 is kept");
        assert!(first.is_file());
        h.shared
            .make_sample(id, 2, &json!(36), &json!("remove"))
            .unwrap();
        assert_eq!(h.next_sample(2).crf, 36);
        assert!(!first.exists(), "the older sample file is deleted");
        assert_eq!(h.shared.sample_path(id, 1), None);
        assert!(h.shared.sample_path(id, 2).unwrap().is_file());
    }

    #[test]
    fn a_stale_video_id_starts_nothing() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        assert_eq!(h.shared.make_sample(id + 1, 1, &json!(28), &keep()), Ok(()));
        assert!(h.shared.sample_job().is_none());
        assert_eq!(h.shared.save(id + 1, &json!(28), &keep()), Ok(()));
        assert!(!h.shared.saving());
        h.idle();
        assert!(h.logged("sample").is_empty());
        assert!(h.logged("save-done").is_empty());
        assert_eq!(listing(h.dir.path()), ["a.mp4", "work"]);
    }

    #[test]
    fn the_current_video_id_starts_a_job() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        h.shared.make_sample(id, 1, &json!(28), &keep()).unwrap();
        assert!(h.shared.sample_job().is_some());
        h.next_sample(1);
        h.shared.save(id, &json!(28), &keep()).unwrap();
        assert!(h.shared.saving());
        h.next("save-done", |e| matches!(e, Event::SaveDone(_)));
    }

    #[test]
    fn a_request_not_above_the_last_starts_nothing() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        h.shared.make_sample(id, 5, &json!(28), &keep()).unwrap();
        h.next_sample(5);
        for request in [5, 4] {
            assert_eq!(
                h.shared.make_sample(id, request, &json!(30), &keep()),
                Ok(())
            );
            assert!(h.shared.sample_job().is_none(), "{request}");
        }
        h.idle();
        assert_eq!(h.logged("sample").len(), 1);
        assert!(h.shared.sample_path(id, 5).unwrap().is_file());
    }

    #[test]
    fn a_sample_that_fails_sends_sample_failed() {
        let mut h = Harness::new();
        let path = h.clip("a.mp4");
        let id = h.load_ready(&path);
        // The original goes away: the sample's encode cannot read it.
        fs::remove_file(&path).unwrap();
        h.shared.make_sample(id, 1, &json!(28), &keep()).unwrap();
        let failed = h.next_sample_failed(1);
        assert_eq!(failed.video, id);
        assert!(
            failed.reason.starts_with("ffmpeg failed"),
            "{}",
            failed.reason
        );
        assert_no_path(&h, &failed.reason);
        h.idle();
        assert!(h.logged("sample").is_empty());
        assert_eq!(h.shared.sample_path(id, 1), None);
    }

    /// Request `request` of the current video made a sample file of its own.
    fn fake_sample(h: &Harness, id: u64, request: u64) -> SampleResult {
        let work = h
            .shared
            .before_path(id)
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let path = work.join(format!("sample-{request}.mp4"));
        fs::write(&path, b"sample").unwrap();
        SampleResult {
            path,
            bytes: 6,
            psnr: Some(40.0),
        }
    }

    #[test]
    fn a_sample_that_finishes_after_a_newer_request_is_discarded() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        // Request 2 is the running one; request 1's cancel came too late and
        // it finished all the same.
        h.shared.lock().sample_job = Some((2, CancelFlag::new()));
        let late = fake_sample(&h, id, 1);
        h.shared
            .finish_sample(id, 1, Settings::default(), Ok(late.clone()));
        assert!(!late.path.exists(), "the late sample file is deleted");
        assert_eq!(h.shared.sample_path(id, 1), None);
        assert_eq!(h.shared.sample_job().map(|(r, _)| r), Some(2));
        h.idle();
        assert!(h.logged("sample").is_empty());
    }

    #[test]
    fn a_sample_of_the_running_request_is_kept_and_sent() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        h.shared.lock().sample_job = Some((1, CancelFlag::new()));
        let done = fake_sample(&h, id, 1);
        h.shared
            .finish_sample(id, 1, Settings::default(), Ok(done.clone()));
        assert!(done.path.is_file());
        assert_eq!(h.shared.sample_path(id, 1), Some(done.path));
        assert!(h.shared.sample_job().is_none());
        let sample = h.next_sample(1);
        assert_eq!((sample.sample_bytes, sample.psnr), (6, Some(40.0)));
        assert_eq!(sample.crf, Crf::DEFAULT);
    }

    #[test]
    fn a_sample_of_a_replaced_video_is_discarded() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        h.shared.lock().sample_job = Some((1, CancelFlag::new()));
        let old = fake_sample(&h, id, 1);
        // Same request number, other video.
        h.shared
            .finish_sample(id + 1, 1, Settings::default(), Ok(old.clone()));
        assert!(!old.path.exists());
        assert_eq!(h.shared.sample_path(id, 1), None);
        h.idle();
        assert!(h.logged("sample").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_before_clip_that_fails_is_a_failed_notice_and_fails_the_samples() {
        let dir = TempDir::new().unwrap();
        let mut tools = super::fake_ffmpeg::script_tools(
            dir.path(),
            "echo 'the before clip broke' >&2\nexit 1",
        );
        tools.ffprobe = real_tools().ffprobe;
        let mut h = Harness::with_tools(tools);
        h.shared.load(&[h.clip("a.mp4")]);
        let id = h.next_video();
        assert_eq!(
            h.next_notice(),
            Notice::Failed {
                reason: "ffmpeg failed: the before clip broke".into()
            }
        );
        assert_eq!(h.shared.before_path(id), None);
        h.shared.make_sample(id, 1, &json!(28), &keep()).unwrap();
        let failed = h.next_sample_failed(1);
        assert_eq!(failed.reason, "ffmpeg failed: the before clip broke");
        h.idle();
        assert!(h.logged("before-ready").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_before_clip_failure_names_no_folder() {
        // ffmpeg's last line starts with the file it was writing, in the work
        // folder, as `file:<path>`.
        let dir = TempDir::new().unwrap();
        let mut tools = super::fake_ffmpeg::script_tools(
            dir.path(),
            "for a; do last=$a; done\necho \"$last: Disk quota exceeded\" >&2\nexit 1",
        );
        tools.ffprobe = real_tools().ffprobe;
        let mut h = Harness::with_tools(tools);
        h.shared.load(&[h.clip("a.mp4")]);
        let id = h.next_video();
        let Notice::Failed { reason } = h.next_notice() else {
            panic!("expected failed")
        };
        assert_eq!(reason, "ffmpeg failed: before.mp4: Disk quota exceeded");
        assert_no_path(&h, &reason);
        h.shared.make_sample(id, 1, &json!(28), &keep()).unwrap();
        let failed = h.next_sample_failed(1);
        assert_no_path(&h, &failed.reason);
    }

    #[test]
    fn a_sample_failure_names_no_folder() {
        let mut h = Harness::new();
        let path = h.clip("a.mp4");
        let id = h.load_ready(&path);
        h.shared.lock().sample_job = Some((1, CancelFlag::new()));
        let work = h
            .shared
            .before_path(id)
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let reason = format!(
            "file:{}: Invalid argument; input file:{}",
            work.join("sample-1.mp4").display(),
            path.display()
        );
        h.shared.finish_sample(
            id,
            1,
            Settings::default(),
            Err(RunError::Failed {
                code: Some(1),
                reason,
            }),
        );
        let failed = h.next_sample_failed(1);
        assert_eq!(
            failed.reason,
            "ffmpeg failed: sample-1.mp4: Invalid argument; input a.mp4"
        );
        assert_no_path(&h, &failed.reason);
    }

    /// Tools whose ffmpeg takes `seconds` and then fails, and the real
    /// ffprobe.
    #[cfg(unix)]
    fn slow_failing_tools(dir: &Path, seconds: u32) -> Tools {
        let mut tools = super::fake_ffmpeg::script_tools(
            dir,
            &format!("sleep {seconds}\necho 'slow and broken' >&2\nexit 1"),
        );
        tools.ffprobe = real_tools().ffprobe;
        tools
    }

    #[cfg(unix)]
    #[test]
    fn a_sample_waiting_for_the_before_clip_is_cancelled_by_a_newer_one() {
        let dir = TempDir::new().unwrap();
        let mut h = Harness::with_tools(slow_failing_tools(dir.path(), 1));
        h.shared.load(&[h.clip("a.mp4")]);
        let id = h.next_video();
        h.shared.make_sample(id, 1, &json!(28), &keep()).unwrap();
        h.shared.make_sample(id, 2, &json!(28), &keep()).unwrap();
        h.next_sample_failed(2);
        h.idle();
        let failed: Vec<u64> = h
            .log
            .iter()
            .filter_map(|e| match e {
                Event::SampleFailed(s) => Some(s.request),
                _ => None,
            })
            .collect();
        assert_eq!(failed, [2], "request 1 ends without an event");
    }

    #[cfg(unix)]
    #[test]
    fn a_sample_waiting_for_the_before_clip_of_a_replaced_video_ends_silently() {
        // The first ffmpeg run (the first video's before clip) takes 60
        // seconds unless it is stopped; every later run fails at once. `exec`
        // makes the process that a cancel kills the `sleep` itself, which
        // holds the pipes.
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("started");
        let mut tools = super::fake_ffmpeg::script_tools(
            dir.path(),
            &format!(
                "if [ -e '{0}' ]; then echo 'broken' >&2; exit 1; fi\ntouch '{0}'\nexec sleep 60",
                marker.display()
            ),
        );
        tools.ffprobe = real_tools().ffprobe;
        let mut h = Harness::with_tools(tools);
        h.shared.load(&[h.clip("a.mp4")]);
        let first = h.next_video();
        h.shared.make_sample(first, 1, &json!(28), &keep()).unwrap();
        wait_until("the first before clip started", || marker.exists());
        let start = Instant::now();
        h.shared.load(&[h.clip("b.mp4")]);
        let second = h.next_video();
        assert_eq!(second, first + 1);
        h.idle();
        // The first video's before clip was stopped, not waited for.
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "{:?}",
            start.elapsed()
        );
        assert!(
            !h.log
                .iter()
                .any(|e| matches!(e, Event::SampleFailed(s) if s.video == first)),
            "{:?}",
            h.log
        );
        // Only the second video's before clip failure is told.
        assert_eq!(
            h.logged("notice"),
            [&Event::Notice(Notice::Failed {
                reason: "ffmpeg failed: broken".into()
            })]
        );
    }

    // ---- saving ----

    #[test]
    fn a_save_writes_next_to_the_original_and_reports_progress() {
        let mut h = Harness::new();
        let path = h.clip("a.mp4");
        let id = h.load_ready(&path);
        h.shared.save(id, &json!(28), &keep()).unwrap();
        let Event::SaveDone(done) = h.next("save-done", |e| matches!(e, Event::SaveDone(_))) else {
            unreachable!()
        };
        assert_eq!(done.input_bytes, fs::metadata(&path).unwrap().len());
        let saved = h.dir.path().join(&done.name);
        assert!(done.name.starts_with("optimized-a."), "{}", done.name);
        assert_eq!(fs::metadata(&saved).unwrap().len(), done.bytes);
        let progress = h.logged("save-progress");
        assert!(!progress.is_empty());
        assert_eq!(
            progress.last(),
            Some(&&Event::SaveProgress(crate::events::SaveProgress {
                percent: Some(100)
            }))
        );
        h.idle();
        assert!(!h.shared.saving());
    }

    #[test]
    fn a_save_goes_to_the_chosen_folder_until_it_is_cleared() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        let exports = h.dir.path().join("Exports");
        fs::create_dir(&exports).unwrap();
        assert_eq!(
            h.shared.choose_folder(Some(exports.clone())),
            FolderAnswer {
                folder: Some("Exports".into())
            }
        );
        h.shared.save(id, &json!(28), &keep()).unwrap();
        h.next("save-done", |e| matches!(e, Event::SaveDone(_)));
        assert_eq!(listing(&exports), ["optimized-a.mp4"]);
        h.idle();

        h.shared.clear_folder();
        h.shared.save(id, &json!(28), &keep()).unwrap();
        h.next("save-done", |e| matches!(e, Event::SaveDone(_)));
        assert!(h.dir.path().join("optimized-a.mp4").is_file());
        assert_eq!(listing(&exports), ["optimized-a.mp4"]);
    }

    #[test]
    fn a_closed_folder_dialog_keeps_the_folder_in_effect() {
        let h = Harness::new();
        assert_eq!(h.shared.choose_folder(None), FolderAnswer { folder: None });
        let exports = h.dir.path().join("Exports");
        fs::create_dir(&exports).unwrap();
        h.shared.choose_folder(Some(exports));
        assert_eq!(
            h.shared.choose_folder(None),
            FolderAnswer {
                folder: Some("Exports".into())
            }
        );
    }

    #[test]
    fn save_refuses_a_bad_crf_or_audio_and_starts_nothing() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        assert_eq!(
            h.shared.save(id, &json!(37), &keep()),
            Err("CRF must be a whole number from 20 to 36".to_owned())
        );
        assert_eq!(
            h.shared.save(id, &json!(28), &json!(null)),
            Err("Audio must be keep or remove".to_owned())
        );
        assert!(!h.shared.saving());
        h.idle();
        assert!(h.logged("save-done").is_empty());
        assert_eq!(listing(h.dir.path()), ["a.mp4", "work"]);
    }

    #[test]
    fn a_save_that_fails_sends_save_failed_and_leaves_nothing() {
        let mut h = Harness::new();
        let path = h.clip("a.mp4");
        let id = h.load_ready(&path);
        fs::remove_file(&path).unwrap();
        h.shared.save(id, &json!(28), &keep()).unwrap();
        let Event::SaveFailed(failed) =
            h.next("save-failed", |e| matches!(e, Event::SaveFailed(_)))
        else {
            unreachable!()
        };
        // ffmpeg's bare reason, as in "No space left on device".
        assert!(
            !failed.reason.starts_with("ffmpeg failed"),
            "{}",
            failed.reason
        );
        assert!(
            failed.reason.contains("No such file or directory"),
            "{}",
            failed.reason
        );
        assert_no_path(&h, &failed.reason);
        h.idle();
        assert!(!h.shared.saving());
        assert_eq!(listing(h.dir.path()), ["work"]);
    }

    #[test]
    fn a_cancelled_save_sends_save_cancelled_and_leaves_nothing() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.long_clip("long.mp4"));
        h.shared.save(id, &json!(28), &keep()).unwrap();
        h.shared.cancel_save();
        h.next("save-cancelled", |e| matches!(e, Event::SaveCancelled(_)));
        h.idle();
        assert!(h.logged("save-done").is_empty());
        assert!(!h.shared.saving());
        assert_eq!(listing(h.dir.path()), ["long.mp4", "work"]);
    }

    #[test]
    fn cancel_save_without_a_save_does_nothing() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        h.shared.cancel_save();
        // A save started after it is not cancelled by it.
        h.shared.save(id, &json!(28), &keep()).unwrap();
        h.next("save-done", |e| matches!(e, Event::SaveDone(_)));
        assert!(h.logged("save-cancelled").is_empty());
    }

    #[test]
    fn a_save_while_saving_starts_nothing() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.long_clip("long.mp4"));
        h.shared.save(id, &json!(28), &keep()).unwrap();
        let first = h.shared.save_flag().expect("the save runs");
        assert_eq!(h.shared.save(id, &json!(30), &keep()), Ok(()));
        let still = h.shared.save_flag().expect("the first save still runs");
        // The same job: cancelling one cancels the other.
        still.cancel();
        assert!(first.is_cancelled());
        h.next("save-cancelled", |e| matches!(e, Event::SaveCancelled(_)));
        h.idle();
        assert_eq!(h.logged("save-cancelled").len(), 1);
        assert!(h.logged("save-done").is_empty());
    }

    #[test]
    fn a_save_cancels_the_running_sample() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.long_clip("long.mp4"));
        h.shared.make_sample(id, 1, &json!(28), &keep()).unwrap();
        let (_, sample) = h.shared.sample_job().expect("the sample runs");
        h.shared.save(id, &json!(28), &keep()).unwrap();
        assert!(sample.is_cancelled());
        assert!(h.shared.sample_job().is_none());
        h.shared.cancel_save();
        h.idle();
        assert!(h.logged("sample").is_empty());
        assert_eq!(h.shared.sample_path(id, 1), None);
    }

    #[test]
    fn a_sample_asked_during_a_save_starts_nothing() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.long_clip("long.mp4"));
        h.shared.save(id, &json!(28), &keep()).unwrap();
        assert_eq!(h.shared.make_sample(id, 1, &json!(30), &keep()), Ok(()));
        assert!(
            h.shared.sample_job().is_none(),
            "no sample runs next to the save"
        );
        h.shared.cancel_save();
        h.next("save-cancelled", |e| matches!(e, Event::SaveCancelled(_)));
        h.idle();
        assert!(h.logged("sample").is_empty());
        assert!(h.logged("sample-failed").is_empty());
        // The refused request did not count: request 1 still starts.
        h.shared.make_sample(id, 1, &json!(30), &keep()).unwrap();
        assert_eq!(h.next_sample(1).crf, 30);
    }

    #[test]
    fn a_sample_asked_after_the_save_starts() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        h.shared.save(id, &json!(28), &keep()).unwrap();
        h.next("save-done", |e| matches!(e, Event::SaveDone(_)));
        h.idle();
        h.shared.make_sample(id, 1, &json!(28), &keep()).unwrap();
        assert!(h.shared.sample_job().is_some());
        h.next_sample(1);
    }

    #[test]
    fn a_sample_started_without_a_save_is_not_cancelled() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        h.shared.make_sample(id, 1, &json!(28), &keep()).unwrap();
        let (_, sample) = h.shared.sample_job().expect("the sample runs");
        h.next_sample(1);
        assert!(!sample.is_cancelled());
    }

    // ---- closing ----

    #[test]
    fn closing_during_a_save_deletes_the_partial_file() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.long_clip("long.mp4"));
        let out = h.dir.path().join("out");
        fs::create_dir(&out).unwrap();
        h.shared.choose_folder(Some(out.clone()));
        h.shared.save(id, &json!(28), &keep()).unwrap();
        wait_until("the partial file", || {
            listing(&out).iter().any(|name| name.ends_with(".part"))
        });
        h.shared.close(CLOSE_WAIT);
        assert_eq!(listing(&out), Vec::<String>::new());
        assert!(!h.shared.saving());
        h.next("save-cancelled", |e| matches!(e, Event::SaveCancelled(_)));
    }

    #[test]
    fn closing_after_a_save_keeps_the_file_and_deletes_the_work_folder() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        let work = h
            .shared
            .before_path(id)
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        h.shared.save(id, &json!(28), &keep()).unwrap();
        h.next("save-done", |e| matches!(e, Event::SaveDone(_)));
        h.shared.close(CLOSE_WAIT);
        assert!(h.dir.path().join("optimized-a.mp4").is_file());
        assert!(!work.exists(), "the work folder is deleted");
        assert_eq!(h.shared.current_video(), None);
        h.idle();
        assert!(h.logged("save-cancelled").is_empty());
    }

    #[test]
    fn closing_stops_a_running_sample() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.long_clip("long.mp4"));
        h.shared.make_sample(id, 1, &json!(28), &keep()).unwrap();
        let (_, sample) = h.shared.sample_job().expect("the sample runs");
        let start = Instant::now();
        h.shared.close(CLOSE_WAIT);
        assert!(sample.is_cancelled());
        assert!(start.elapsed() < CLOSE_WAIT);
        assert!(
            h.shared.wait_idle(Duration::ZERO),
            "no job runs after close"
        );
        h.idle();
        assert!(h.logged("sample").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn nothing_starts_once_closing_has_begun() {
        // The before clip's "ffmpeg" is a shell whose `sleep` child keeps the
        // pipes open for 2 seconds after the shell is killed, so `close` is
        // still waiting for that job, with the video still loaded, while the
        // commands below arrive. `close` is called only once the script runs,
        // or the job would end on its cancel before starting it.
        let dir = TempDir::new().unwrap();
        let started = dir.path().join("started");
        let body = format!("touch '{}'\nsleep 2\nexit 1", started.display());
        let mut tools = super::fake_ffmpeg::script_tools(dir.path(), &body);
        tools.ffprobe = real_tools().ffprobe;
        let mut h = Harness::with_tools(tools);
        let b = h.clip("b.mp4");
        h.shared.load(&[h.clip("a.mp4")]);
        let id = h.next_video();
        wait_until("the before clip's ffmpeg runs", || started.exists());
        let closing = {
            let shared = Arc::clone(&h.shared);
            std::thread::spawn(move || shared.close(CLOSE_WAIT))
        };
        wait_until("close began", || h.shared.is_closing());
        assert_eq!(h.shared.current_video(), Some(id), "close still waits");
        h.shared.load(std::slice::from_ref(&b));
        assert_eq!(h.shared.make_sample(id, 1, &json!(28), &keep()), Ok(()));
        assert_eq!(h.shared.save(id, &json!(28), &keep()), Ok(()));
        assert!(h.shared.sample_job().is_none(), "no sample started");
        assert!(!h.shared.saving(), "no save started");
        assert_eq!(h.shared.current_video(), Some(id), "b was not loaded");
        closing.join().unwrap();
        assert!(
            h.shared.wait_idle(Duration::ZERO),
            "close left no job running"
        );
        h.idle();
        assert_eq!(h.logged("video").len(), 1, "{:?}", h.log);
        assert!(h.logged("sample").is_empty());
        assert!(h.logged("save-done").is_empty());
        assert!(h.logged("save-cancelled").is_empty());
    }

    #[test]
    fn the_same_commands_start_jobs_before_closing() {
        let mut h = Harness::new();
        let id = h.load_ready(&h.clip("a.mp4"));
        h.shared.make_sample(id, 1, &json!(28), &keep()).unwrap();
        assert!(h.shared.sample_job().is_some());
        h.next_sample(1);
        h.shared.save(id, &json!(28), &keep()).unwrap();
        assert!(h.shared.saving());
        h.next("save-done", |e| matches!(e, Event::SaveDone(_)));
        assert_eq!(h.load_ready(&h.clip("b.mp4")), 2);
    }

    #[test]
    fn the_settings_of_a_request_reach_the_encode() {
        // Audio "remove" on a clip with sound: the sample has no audio.
        let mut h = Harness::new();
        let path = clips::make(
            h.dir.path(),
            Clip {
                name: "sound.mp4".into(),
                audio: true,
                ..Clip::default()
            },
        );
        let id = h.load_ready(&path);
        h.shared
            .make_sample(id, 1, &json!(24), &json!("remove"))
            .unwrap();
        assert_eq!(h.next_sample(1).crf, 24);
        let sample = h.shared.sample_path(id, 1).unwrap();
        let info = video_optimizer_core::probe::probe(&real_tools(), &sample).unwrap();
        assert_eq!(info.audio, None);
    }
}
