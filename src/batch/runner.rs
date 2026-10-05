// SPDX-License-Identifier: GPL-3.0-only
//! One coordinator owns each job. The UI receives snapshots and acknowledges library writes.

use super::*;
use eframe::egui;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Control { pub revision: u64, pub mode: Mode }

pub(super) enum Command { Wake, Key(String), RetryUnconfirmed, RetryFailed }
pub(super) enum Event {
    Snapshot(Job),
    Activity(Activity),
    Import(Job, mpsc::Sender<SaveReport>),
    Error(String),
    Idle,
}
pub(super) struct SaveReport { pub saved: Vec<usize>, pub rejected: Vec<(usize, String)>, pub error: Option<String> }

/// How many ordinary label requests an OpenAI-style job keeps in flight.
type SendFn = dyn Fn(&str, Option<&Value>) -> Result<Value, Failure> + std::marker::Send + Sync;

/// Label requests that are out, and the sheets they carry. The workers only
/// send and answer; the coordinator remains the only writer of the job.
struct Pump {
    tx: mpsc::Sender<(usize, Result<Value, Failure>)>,
    rx: mpsc::Receiver<(usize, Result<Value, Failure>)>,
    busy: std::collections::BTreeSet<usize>,
}
impl Pump {
    fn new() -> Self { let (tx, rx) = mpsc::channel(); Self { tx, rx, busy: std::collections::BTreeSet::new() } }
    fn inflight(&self) -> usize { self.busy.len() }
}

/// Batch submissions that are out, keyed by their recovery reference. The
/// workers only send and answer; the coordinator remains the only writer.
struct Submits {
    tx: mpsc::Sender<(String, Result<Value, Failure>)>,
    rx: mpsc::Receiver<(String, Result<Value, Failure>)>,
    busy: std::collections::BTreeSet<String>,
}
impl Submits {
    fn new() -> Self { let (tx, rx) = mpsc::channel(); Self { tx, rx, busy: std::collections::BTreeSet::new() } }
    fn inflight(&self) -> usize { self.busy.len() }
}

/// What woke the idle coordinator, or that only time passed.
enum Woke { Stop, Key(String), Retry, RetryUnconfirmed, RetryFailed, Timeout }

/// Waits for a command. Commands are the only thing that wakes an idle job;
/// time alone never causes a busy loop.
fn wait(rx: &mpsc::Receiver<Command>, stopped: &AtomicBool) -> Woke {
    if stopped.load(Ordering::Relaxed) { return Woke::Stop; }
    match rx.recv_timeout(Duration::from_secs(1)) {
        Ok(Command::Key(value)) => Woke::Key(value),
        Ok(Command::Wake) => Woke::Retry,
        Ok(Command::RetryUnconfirmed) => Woke::RetryUnconfirmed,
        Ok(Command::RetryFailed) => Woke::RetryFailed,
        Err(mpsc::RecvTimeoutError::Disconnected) => Woke::Stop,
        Err(mpsc::RecvTimeoutError::Timeout) => Woke::Timeout,
    }
}

pub(super) struct Runner {
    pub events: mpsc::Receiver<Event>,
    commands: mpsc::Sender<Command>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Runner { fn drop(&mut self) { self.stop.store(true, Ordering::Relaxed); let _ = self.commands.send(Command::Wake); } }

pub(super) fn lock(dir: &Path) -> Result<Arc<std::fs::File>, String> {
    lock_file(&dir.join(".tilepicky-job.lock"))
}

pub(super) fn lock_file(path: &Path) -> Result<Arc<std::fs::File>, String> {
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)
        .map_err(|e| e.to_string())?;
    file.try_lock().map_err(|_| "Another Tilepicky window owns this library job. Close that window before continuing.".to_string())?;
    Ok(Arc::new(file))
}

impl Runner {
    #[cfg(test)]
    pub(super) fn disconnected() -> Self {
        let (_, events) = mpsc::channel(); let (commands, _) = mpsc::channel();
        Self { events, commands, stop: Arc::new(AtomicBool::new(false)), thread: None }
    }
    pub fn finish(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.commands.send(Command::Wake);
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
    pub fn command(&self, command: Command) { let _ = self.commands.send(command); }
    pub fn start(job: Job, root: PathBuf, key: String, secret: Option<String>, lock: Arc<std::fs::File>, ctx: egui::Context, log: crate::ai_log::Log) -> Self {
        let (tx, events) = mpsc::channel();
        let (commands, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            let _lock = lock;
            let _scope = log.enter();
            log.resume();
            log.event("batch_resume", json!({"provider":job.provider.name,"model":job.model,"mode":job.mode}));
            let dir = root.clone();
            let notify = |event| { let _ = tx.send(event); ctx.request_repaint(); };
            let mut coordinator = Coordinator { job, root, dir, book: None };
            for group in &mut coordinator.job.groups {
                if let Remote::Waiting(id) = &group.remote { group.tracking.id = id.clone(); }
            }
            coordinator.job.poll_ms = coordinator.job.next_poll();
            if coordinator.job.provider.kind == Kind::OpenAi
                && let Some(store) = coordinator.job.provider.store.clone() {
                match secret {
                    Some(secret) => match crate::s3::Client::new(store, secret) {
                        Ok(client) => coordinator.job.objects = Some(Arc::new(openrouter::Storage::new(client))),
                        Err(error) => coordinator.job.issue("upload", error, now_ms()),
                    },
                    None => coordinator.job.issue("upload", "No object-storage secret key is set for this provider.".into(), now_ms()),
                }
            }
            let mut key = key;
            let mut retry = false;
            let mut dirty = true;
            let mut pump = Pump::new();
            let mut label_send: Option<Arc<SendFn>> = None;
            let mut label_key = String::new();
            let mut submits = Submits::new();
            let mut batch_send: Option<Arc<SendFn>> = None;
            let mut batch_key = String::new();
            while !stopped.load(Ordering::Relaxed) {
                while let Ok(command) = rx.try_recv() {
                    match command {
                        Command::Wake => retry = true,
                        Command::Key(value) => { key = value; retry = true; }
                        command @ (Command::RetryUnconfirmed | Command::RetryFailed) => {
                            coordinator.retry(command); retry = true;
                        }
                    }
                }
                match coordinator.control() {
                    Ok(changed) => dirty |= changed,
                    Err(error) => { notify(Event::Error(error)); break; }
                }
                if retry {
                    for issue in coordinator.job.issues.values_mut() { issue.retry_ms = 0; }
                    coordinator.job.poll_ms = 0; coordinator.job.recovery_ms = 0;
                    retry = false; dirty = true;
                }
                // No operation can start until every previous state change is durable.
                if dirty && let Err(error) = coordinator.job.save(&coordinator.dir) {
                    notify(Event::Error(error));
                    if let Ok(Command::Key(value)) = rx.recv_timeout(Duration::from_secs(2)) { key = value; }
                    continue;
                }
                if dirty {
                    if coordinator.job.done() { log.complete(); }
                    notify(Event::Snapshot(coordinator.job.clone())); dirty = false;
                }
                let now = now_ms();
                if coordinator.job.pending_save() && coordinator.job.due("save", now) {
                    let (reply, saved) = mpsc::channel();
                    notify(Event::Import(coordinator.job.clone(), reply));
                    loop {
                        match saved.recv_timeout(Duration::from_millis(200)) {
                            Ok(report) => { coordinator.saved(report, now_ms()); dirty = true; break; }
                            Err(mpsc::RecvTimeoutError::Timeout) if !stopped.load(Ordering::Relaxed) => continue,
                            _ => return,
                        }
                    }
                    continue;
                }
                if coordinator.job.provider.kind == Kind::OpenAi && coordinator.job.provider.store.is_none() {
                    if !key.is_empty() && (label_send.is_none() || label_key != key) {
                        match Transport::new(&coordinator.job.provider, key.clone()) {
                            Ok(transport) => {
                                let transport = Arc::new(transport);
                                let send: Arc<SendFn> = Arc::new(move |path, body| transport.send(path, body));
                                label_send = Some(send); label_key = key.clone();
                            }
                            Err(error) => { coordinator.job.issue("upload", error, now); dirty = true; label_send = None; }
                        }
                    }
                    let ready = label_send.is_some() && coordinator.job.mode == Mode::Running
                        && !coordinator.job.pending_save();
                    if ready && let Some(send) = &label_send {
                        if pump.inflight() == 0 { notify(Event::Activity(Activity::before(Operation::Label))); }
                        dirty |= coordinator.dispatch_labels(&mut pump, send, &log, now);
                    }
                    if pump.inflight() > 0 {
                        match pump.rx.recv_timeout(Duration::from_millis(200)) {
                            Ok((i, reply)) => { pump.busy.remove(&i); coordinator.collect((i, reply), now_ms()); dirty = true; }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                        }
                        if pump.inflight() == 0 { notify(Event::Idle); }
                        continue;
                    }
                    if dirty { continue; }
                    if key.is_empty() && coordinator.job.untaken() && coordinator.job.due("upload", now) {
                        coordinator.job.issue("upload", "The provider key is missing. Set it in Settings.".into(), now);
                        dirty = true;
                        continue;
                    }
                    match wait(&rx, &stopped) {
                        Woke::Stop => break,
                        Woke::Key(value) => { key = value; retry = true; }
                        Woke::Retry => retry = true,
                        Woke::RetryUnconfirmed => { coordinator.retry(Command::RetryUnconfirmed); retry = true; }
                        Woke::RetryFailed => { coordinator.retry(Command::RetryFailed); retry = true; }
                        Woke::Timeout => {}
                    }
                    continue;
                }
                if !key.is_empty() && (batch_send.is_none() || batch_key != key) {
                    match Transport::new(&coordinator.job.provider, key.clone()) {
                        Ok(transport) => {
                            let transport = Arc::new(transport);
                            let send: Arc<SendFn> = Arc::new(move |path, body| transport.send(path, body));
                            batch_send = Some(send); batch_key = key.clone();
                        }
                        Err(error) => { coordinator.job.issue("upload", error, now); dirty = true; batch_send = None; }
                    }
                }
                let batch_ready = batch_send.is_some() && coordinator.job.mode == Mode::Running
                    && !coordinator.job.pending_save();
                if batch_ready && coordinator.job.provider.kind == Kind::Gemini && let Some(send) = &batch_send {
                    dirty |= coordinator.dispatch_submissions(&mut submits, send, &log, now);
                }
                if submits.inflight() > 0 {
                    match submits.rx.recv_timeout(Duration::from_millis(200)) {
                        Ok((reference, reply)) => {
                            submits.busy.remove(&reference);
                            coordinator.collect_submission(&reference, reply, now_ms());
                            dirty = true;
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    if submits.inflight() == 0 { notify(Event::Idle); }
                    continue;
                }
                if dirty { continue; }
                if key.is_empty() && coordinator.job.untaken() && coordinator.job.due("upload", now) {
                    coordinator.job.issue("upload", "The provider key is missing. Set it in Settings.".into(), now);
                    dirty = true;
                    continue;
                }
                if let Some(operation) = coordinator.job.next_operation(now) {
                    notify(Event::Activity(Activity::before(operation)));
                    let result = if key.is_empty() { Err("The provider key is missing. Set it in Settings.".into()) }
                        else { Transport::new(&coordinator.job.provider, key.clone()) };
                    match result {
                        Ok(transport) => coordinator.step(operation, now, |path, body| {
                            notify(Event::Activity(Activity::request(path, body)));
                            transport.send(path, body)
                        }),
                        Err(error) => coordinator.job.issue(operation.name(), error, now),
                    }
                    dirty = true;
                    notify(Event::Idle);
                } else {
                    match wait(&rx, &stopped) {
                        Woke::Stop => break,
                        Woke::Key(value) => { key = value; retry = true; }
                        Woke::Retry => retry = true,
                        Woke::RetryUnconfirmed => { coordinator.retry(Command::RetryUnconfirmed); retry = true; }
                        Woke::RetryFailed => { coordinator.retry(Command::RetryFailed); retry = true; }
                        Woke::Timeout => {}
                    }
                }
            }
        });
        Self { events, commands, stop, thread: Some(thread) }
    }
}

pub(super) struct Coordinator { pub job: Job, pub root: PathBuf, pub dir: PathBuf, book: Option<crate::sidecar::Book> }
impl Coordinator {
    /// The label the book holds for one sheet. The book is read once and
    /// kept until labels are saved, so a library of many sheets does not
    /// re-read it for each request.
    fn guard_label(&mut self, i: usize) -> Option<Label> {
        if self.book.is_none() { self.book = crate::sidecar::load_book(&self.root).ok(); }
        let rel = &self.job.sheets[i].rel;
        self.book.as_ref().and_then(|book| book.sheets.get(rel)).and_then(|side| side.label.clone())
    }
    fn retry(&mut self, command: Command) {
        self.job.mode = Mode::Running;
        match command {
            Command::RetryUnconfirmed => {
                for i in (0..self.job.groups.len()).rev() {
                    if self.job.groups[i].remote == Remote::Submitting { self.job.send_again(i); }
                }
            }
            Command::RetryFailed => {
                for sheet in &mut self.job.sheets {
                    if !sheet.error.is_empty() && !sheet.cancelled {
                        sheet.taken = false; sheet.error.clear(); sheet.label = None; sheet.imported = false; sheet.guard = None; sheet.unknown = 0;
                    }
                }
                self.job.groups.retain(|g| g.remote != Remote::Done);
            }
            _ => {}
        }
    }
    fn control(&mut self) -> Result<bool, String> {
        let control: Control = store::control(&self.dir)?;
        if control.revision > self.job.control_revision {
            self.job.control_revision = control.revision;
            self.job.mode = control.mode;
            if control.mode == Mode::Cancelling {
                for sheet in self.job.sheets.iter_mut().filter(|s| !s.taken) { sheet.taken = true; sheet.cancelled = true; }
            }
            return Ok(true);
        }
        Ok(false)
    }
    fn saved(&mut self, report: SaveReport, now: u64) {
        self.book = None;
        for i in report.saved { self.job.sheets[i].imported = true; }
        for (i, error) in report.rejected {
            self.job.sheets[i].error = error; self.job.sheets[i].label = None; self.job.sheets[i].imported = false;
        }
        if let Some(error) = report.error { self.job.issue("save", error, now); }
        else { self.job.issues.remove("save"); }
    }

    /// Sends up to the model's concurrency of label requests at once, each
    /// on its own worker. The job is written here and by `collect`, never
    /// by a worker.
    fn dispatch_labels(&mut self, pump: &mut Pump, send: &Arc<SendFn>, log: &crate::ai_log::Log, now: u64) -> bool {
        if self.job.groups.iter().any(|g| g.remote != Remote::Done) { self.job.release_groups(); }
        let flight = self.job.concurrency.max(1) as usize;
        let mut worked = false;
        while pump.inflight() < flight {
            let Some(i) = self.job.sheets.iter().enumerate()
                .find(|(i, s)| !s.taken && s.retry_ms <= now && !pump.busy.contains(i)).map(|(i, _)| i) else { break };
            crate::ai_log::event("batch_sheet_start", json!({"sheet":self.job.sheets[i].rel, "provider":self.job.provider.name,
                "model":self.job.model, "attempt":self.job.sheets[i].unknown + 1, "tags_requested":self.job.tag_list}));
            let existing = self.guard_label(i);
            let request = match image_request(&mut self.job, &self.root, i, existing) {
                Ok(request) => request,
                Err(error) => { self.job.sheets[i].error = error; self.job.sheets[i].taken = true; worked = true; continue; }
            };
            pump.busy.insert(i);
            let (tx, send, log) = (pump.tx.clone(), send.clone(), log.clone());
            std::thread::spawn(move || {
                let _scope = log.enter();
                let reply = send("chat/completions", Some(&request));
                let _ = tx.send((i, reply));
            });
            worked = true;
        }
        worked
    }

    /// Reads one worker's reply into the job, as one ordinary request does.
    fn collect(&mut self, (i, reply): (usize, Result<Value, Failure>), now: u64) {
        let response = reply.is_ok() || matches!(&reply, Err(Failure::Status(_, _)));
        let auth = matches!(&reply, Err(Failure::Status(401 | 403, _)));
        let _ = finish_one(&mut self.job, &self.dir, i, reply, now);
        if response { self.job.last_response_ms = now; }
        if auth { self.job.mode = Mode::Paused; }
    }

    /// Prepares and sends Gemini batches, up to the model's concurrency.
    /// A submission that is out or awaiting confirmation counts against the
    /// same number, so a flaky connection cannot pile up unconfirmed work.
    fn dispatch_submissions(&mut self, submits: &mut Submits, send: &Arc<SendFn>, log: &crate::ai_log::Log, now: u64) -> bool {
        let limit = self.job.concurrency.max(1) as usize;
        let mut worked = false;
        while self.job.untaken() {
            let outstanding = submits.inflight() + self.job.groups.iter()
                .filter(|g| g.remote == Remote::Submitting)
                .filter(|g| g.recovery.as_ref().is_some_and(|r| !submits.busy.contains(&r.reference)))
                .count();
            if outstanding >= limit { break; }
            match prepare_submission(&mut self.job, &self.root, &self.dir) {
                Ok(Some((reference, path, body))) => {
                    submits.busy.insert(reference.clone());
                    let (tx, send, log) = (submits.tx.clone(), send.clone(), log.clone());
                    std::thread::spawn(move || {
                        let _scope = log.enter();
                        let reply = send(&path, Some(&body));
                        let _ = tx.send((reference, reply));
                    });
                    worked = true;
                }
                Ok(None) => break,
                Err(error) => { self.job.issue("upload", error, now); worked = true; break; }
            }
        }
        worked
    }

    /// Reads one submission worker's reply into its group.
    fn collect_submission(&mut self, reference: &str, reply: Result<Value, Failure>, now: u64) {
        let response = reply.is_ok() || matches!(&reply, Err(Failure::Status(_, _)));
        let auth = matches!(&reply, Err(Failure::Status(401 | 403, _)));
        let result = finish_submission(&mut self.job, &self.dir, reference, reply);
        if response { self.job.last_response_ms = now; }
        if auth { self.job.mode = Mode::Paused; }
        match result {
            Ok(()) => { self.job.issues.remove("upload"); }
            Err(error) => self.job.issue("upload", error, now),
        }
    }
    fn step(&mut self, operation: Operation, now: u64, mut send: impl FnMut(&str, Option<&Value>) -> Result<Value, Failure>) {
        let polled = if operation == Operation::Poll {
            self.job.groups.iter().filter(|g| matches!(g.remote, Remote::Waiting(_)))
                .min_by_key(|g| g.tracking.checked_ms).map(|g| g.remote.clone())
        } else { None };
        let target = match operation {
            Operation::Poll => polled.as_ref().and_then(|remote| self.job.groups.iter().find(|g| &g.remote == remote)),
            Operation::Cancel => self.job.groups.iter().find(|g| matches!(g.remote, Remote::Waiting(_)) && !g.tracking.cancel_sent),
            Operation::Recover => self.job.groups.iter().find(|g| g.remote == Remote::Submitting && g.recovery.is_some()),
            _ => None,
        }.map(Group::reference);
        let mut response = false;
        let mut auth = false;
        let (job, result) = advance_operation(self.job.clone(), &self.root, &self.dir, operation, &mut |path, body| {
            let result = send(path, body);
            response |= result.is_ok() || matches!(&result, Err(Failure::Status(_, _)));
            auth |= matches!(&result, Err(Failure::Status(401 | 403, _)));
            result
        });
        self.job = job;
        if response { self.job.last_response_ms = now; }
        if auth { self.job.mode = Mode::Paused; }
        if let Some(target) = target && let Some(group) = self.job.groups.iter_mut().find(|g| g.reference() == target) {
            group.tracking.error = result.as_ref().err().cloned().unwrap_or_default();
        }
        match result {
            Ok(()) => { self.job.issues.remove(operation.name()); }
            Err(error) => self.job.issue(operation.name(), error, now),
        }
        if self.job.remote_done() { self.job.issues.remove("cancel"); }
        if operation == Operation::Recover && !self.job.uncertain() && !self.job.untaken() { self.job.issues.remove("upload"); }
        if operation == Operation::Poll {
            if let Some(group) = self.job.groups.iter_mut().find(|g| Some(&g.remote) == polled.as_ref()) {
                group.tracking.checked_ms = now;
            }
            self.job.poll_ms = self.job.next_poll();
        }
        if matches!(operation, Operation::Submit | Operation::Recover) { self.job.poll_ms = self.job.next_poll(); }
        if operation == Operation::Recover {
            let more_pages = self.job.groups.iter().any(|g| g.recovery.as_ref().is_some_and(|r| !r.page.is_empty()));
            let repeated = self.job.groups.iter().any(|g| g.remote == Remote::Submitting && g.tracking.recoveries >= 5);
            self.job.recovery_ms = now + if more_pages { 1_000 } else if repeated { 300_000 } else { 60_000 };
        }
    }
}

impl Group {
    fn reference(&self) -> String {
        if let Some(recovery) = &self.recovery { recovery.reference.clone() }
        else if let Remote::Waiting(id) = &self.remote { id.clone() }
        else { self.tracking.id.clone() }
    }
}

impl Operation {
    fn name(self) -> &'static str {
        match self { Self::Label | Self::Submit => "upload", Self::Poll => "check", Self::Recover => "recovery", Self::Cancel => "cancel" }
    }
}
impl Job {
    fn next_poll(&self) -> u64 {
        self.groups.iter().filter(|g| matches!(g.remote, Remote::Waiting(_)))
            .map(|g| if g.tracking.checked_ms == 0 { 0 } else { g.tracking.checked_ms + POLL.as_millis() as u64 })
            .min().unwrap_or(0)
    }
    pub(super) fn issue(&mut self, operation: &str, message: String, now: u64) {
        let issue = self.issues.entry(operation.into()).or_default();
        issue.attempts = issue.attempts.saturating_add(1);
        issue.retry_ms = now + 30_000 * (1 << issue.attempts.min(5).saturating_sub(1));
        issue.message = message;
        crate::ai_log::event("batch_issue", json!({"operation":operation, "message":issue.message, "retry_ms":issue.retry_ms}));
    }
    fn due(&self, operation: &str, now: u64) -> bool { self.issues.get(operation).is_none_or(|issue| issue.retry_ms <= now) }
    fn next_operation(&self, now: u64) -> Option<Operation> {
        if self.done() { return None; }
        let waiting = self.groups.iter().any(|g| matches!(g.remote, Remote::Waiting(_)));
        let recoverable = self.groups.iter().any(|g| g.remote == Remote::Submitting && g.recovery.is_some());
        let allowed = |op: Operation| self.due(op.name(), now) && match op {
            Operation::Label => self.provider.kind == Kind::OpenAi && self.provider.store.is_none() && self.mode == Mode::Running && self.untaken() && !self.pending_save(),
            // Gemini submissions run on the dispatcher's own pool. An OpenRouter
            // batch submission runs through this ordinary operation path.
            Operation::Submit => self.provider.store.is_some() && self.mode == Mode::Running && self.untaken() && !self.pending_save()
                && !self.groups.iter().any(|g| g.remote == Remote::Submitting),
            Operation::Poll => waiting && self.poll_ms <= now,
            Operation::Recover => recoverable && self.recovery_ms <= now,
            Operation::Cancel => self.mode == Mode::Cancelling
                && self.groups.iter().any(|g| matches!(g.remote, Remote::Waiting(_)) && !g.tracking.cancel_sent),
        };
        let preferred = if self.mode == Mode::Cancelling { Operation::Cancel } else { self.operation() };
        [preferred, Operation::Poll, Operation::Recover, Operation::Submit, Operation::Label].into_iter().find(|op| allowed(*op))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    struct Fixture { folder: crate::storage::tests::Folder, root: PathBuf, dir: PathBuf }
    impl Fixture {
        fn new() -> Self {
            let folder = crate::storage::tests::Folder::new();
            let root = folder.0.join("library"); let dir = root.clone();
            std::fs::create_dir_all(&root).unwrap();
            RgbaImage::from_pixel(8, 8, Rgba([80, 90, 100, 255])).save(root.join("sheet.png")).unwrap();
            Self { folder, root, dir }
        }
        fn job(&self) -> Job {
            let provider = Provider { name: "test".into(), kind: Kind::Gemini, skip: None, store: None, key_env: vec![],
                url: "https://example.invalid/v1beta".into() };
            prepare(&Index::scan(&self.root, [16, 16]), provider, "test".into(), Scope::Unlabeled).unwrap()
        }
        fn coordinator(&self) -> Coordinator { Coordinator { job: self.job(), root: self.root.clone(), dir: self.dir.clone(), book: None } }
        fn reopen(&self) -> Coordinator {
            Coordinator { job: Job::load(&self.dir).unwrap().unwrap(), root: self.root.clone(), dir: self.dir.clone(), book: None }
        }
        fn control(&self, mode: Mode, revision: u64) {
            store::set_control(&self.dir, &Control { mode, revision }).unwrap();
        }
    }
    fn result() -> Value {
        json!({"done":true, "response":{"inlinedResponses":{"inlinedResponses":[{"metadata":{"key":"sheet-0"},
            "response":{"candidates":[{"finishReason":"STOP", "content":{"parts":[{"text":
                "{\"status\":\"labeled\",\"caption\":\"Stone wall\",\"tags\":[\"stone\"],\"listed\":{}}"}]}}]}}]}}})
    }
    fn receive(c: &mut Coordinator) {
        c.step(Operation::Submit, 1_000, |_, _| Ok(json!({"name":"batches/accepted"})));
        c.step(Operation::Poll, 2_000, |_, _| Ok(result()));
    }

    /// An OpenAI-style job fills the pipe before it reads any reply, and ends
    /// when every sheet had its turn.
    #[test]
    fn an_openai_job_keeps_several_requests_in_flight() {
        use crate::labels::tests::{completion, labeled};
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
        let f = Fixture::new();
        for name in ["b.png", "c.png", "d.png", "e.png"] {
            RgbaImage::from_pixel(8, 8, Rgba([1, 2, 3, 255])).save(f.root.join(name)).unwrap();
        }
        let mut job = prepare(&Index::scan(&f.root, [16, 16]), super::super::tests::provider(Kind::OpenAi),
            "test:batch".into(), Scope::Unlabeled).unwrap();
        job.concurrency = 4;
        let flight = job.concurrency as usize;
        assert!(job.sheets.len() >= flight);
        let mut c = Coordinator { job, root: f.root.clone(), dir: f.dir.clone(), book: None };
        let mut pump = Pump::new();
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let send: Arc<SendFn> = Arc::new(move |_path, _body| {
            counted.fetch_add(1, AtomicOrdering::Relaxed);
            Ok(completion(labeled("Forest")))
        });
        let log = crate::ai_log::Log::single("test");
        assert!(c.dispatch_labels(&mut pump, &send, &log, 1_000));
        assert_eq!(pump.inflight(), flight, "the pipe fills before any reply is read");
        while !c.job.remote_done() {
            let reply = pump.rx.recv_timeout(Duration::from_secs(5)).expect("a worker answered");
            pump.busy.remove(&reply.0);
            c.collect(reply, 1_000);
            if pump.inflight() < flight { c.dispatch_labels(&mut pump, &send, &log, 1_000); }
        }
        assert_eq!(calls.load(AtomicOrdering::Relaxed), c.job.sheets.len());
        assert!(c.job.sheets.iter().all(|s| s.taken && s.label.is_some()));
    }

    #[test]
    fn lost_acceptance_recovers_after_restart_without_another_submission() {
        let f = Fixture::new(); let mut c = f.coordinator(); let mut submissions = 0;
        c.step(Operation::Submit, 1_000, |_, body| { assert!(body.is_some()); submissions += 1; Err(Failure::Unknown("Lost reply".into())) });
        c.job.save(&f.dir).unwrap();
        let mut c = f.reopen();
        let reference = c.job.groups[0].recovery.as_ref().unwrap().reference.clone();
        c.step(Operation::Recover, 31_000, |path, body| {
            assert!(path.starts_with("batches?")); assert!(body.is_none());
            Ok(json!({"operations":[{"name":"batches/accepted", "metadata":{"displayName":reference,"model":"models/test"}}]}))
        });
        c.step(Operation::Poll, 32_000, |_, body| { assert!(body.is_none()); Ok(result()) });
        assert!(c.job.pending_save()); assert!(!c.job.done());
        let (report, labels) = save_labels(&c.job, &f.root);
        assert_eq!(labels.len(), 1);
        c.saved(report, 33_000); c.job.save(&f.dir).unwrap();
        let c = f.reopen(); assert!(c.job.done()); assert_eq!(c.job.saved(), 1);
        assert_eq!(submissions, 1); assert!(c.job.next_operation(100_000).is_none());
        assert_eq!(crate::sidecar::load_book(&f.root).unwrap().sheets["sheet.png"].label.as_ref().unwrap().caption, "Stone wall");
    }

    #[test]
    fn a_failed_library_save_retries_without_another_model_request() {
        let f = Fixture::new(); let mut c = f.coordinator(); receive(&mut c);
        let original = std::fs::read(f.root.join(crate::sidecar::BOOK)).unwrap();
        std::fs::write(f.root.join(crate::sidecar::BOOK), "broken").unwrap();
        let (report, labels) = save_labels(&c.job, &f.root);
        assert!(labels.is_empty()); assert!(report.error.is_some()); c.saved(report, 3_000);
        assert!(!c.job.done()); assert!(c.job.next_operation(50_000).is_none());
        assert!(c.job.save(&f.dir).is_err());
        std::fs::write(f.root.join(crate::sidecar::BOOK), original).unwrap();
        c.job.save(&f.dir).unwrap(); let mut c = f.reopen();
        let (report, _) = save_labels(&c.job, &f.root); c.saved(report, 50_000);
        assert!(c.job.done()); assert_eq!(c.job.saved(), 1); assert!(!c.job.issues.contains_key("save"));
    }

    #[test]
    fn a_crash_after_the_book_write_does_not_repeat_or_replace_it() {
        let f = Fixture::new(); let mut c = f.coordinator(); receive(&mut c); c.job.save(&f.dir).unwrap();
        let (_, labels) = save_labels(&c.job, &f.root); assert_eq!(labels.len(), 1);
        let mut c = f.reopen();
        let (report, labels) = save_labels(&c.job, &f.root);
        assert!(labels.is_empty()); assert_eq!(report.saved, vec![0]);
        c.saved(report, 3_000); assert!(c.job.done());
    }

    #[test]
    fn changed_images_and_newer_labels_are_never_overwritten() {
        for change_image in [false, true] {
            let f = Fixture::new(); let mut c = f.coordinator(); receive(&mut c);
            if change_image { RgbaImage::new(4, 4).save(f.root.join("sheet.png")).unwrap(); }
            else {
                let mut label = c.job.sheets[0].label.clone().unwrap(); label.caption = "Newer label".into();
                crate::sidecar::store_labels(&f.root, [("sheet.png", Some(label))]).unwrap();
            }
            let (report, labels) = save_labels(&c.job, &f.root);
            assert!(labels.is_empty()); assert_eq!(report.rejected.len(), 1);
            c.saved(report, 3_000); assert!(c.job.done()); assert_eq!(c.job.saved(), 0);
            if !change_image {
                assert_eq!(crate::sidecar::load_book(&f.root).unwrap().sheets["sheet.png"].label.as_ref().unwrap().caption, "Newer label");
            }
        }
    }

    #[test]
    fn cancellation_survives_lost_acceptance_and_waits_for_confirmation() {
        let f = Fixture::new(); let mut c = f.coordinator();
        c.step(Operation::Submit, 1_000, |_, _| Err(Failure::Unknown("Lost reply".into()))); c.job.save(&f.dir).unwrap();
        f.control(Mode::Cancelling, 2_000);
        let mut c = f.reopen(); c.control().unwrap(); assert_eq!(c.job.next_operation(3_000), Some(Operation::Recover));
        let reference = c.job.groups[0].recovery.as_ref().unwrap().reference.clone();
        c.step(Operation::Recover, 3_000, |_, _| Ok(json!({"operations":[
            {"name":"batches/accepted", "metadata":{"displayName":reference,"model":"models/test"}}
        ]})));
        c.step(Operation::Cancel, 4_000, |path, _| { assert_eq!(path, "batches/accepted:cancel"); Ok(json!({})) });
        c.job.save(&f.dir).unwrap(); assert!(!c.job.done());
        let mut c = f.reopen(); assert_eq!(c.job.next_operation(5_000), Some(Operation::Poll));
        c.step(Operation::Poll, 5_000, |_, _| Ok(json!({"done":true,"metadata":{"state":"BATCH_STATE_CANCELLED"}})));
        c.job.save(&f.dir).unwrap(); assert!(f.reopen().job.done()); assert!(c.job.sheets[0].cancelled);
    }

    #[test]
    fn polling_success_does_not_erase_a_cancellation_failure() {
        let f = Fixture::new(); let mut c = f.coordinator();
        c.step(Operation::Submit, 1_000, |_, _| Ok(json!({"name":"batches/accepted"})));
        c.job.mode = Mode::Cancelling;
        c.step(Operation::Cancel, 2_000, |_, _| Err(Failure::Status(503, "Temporarily unavailable".into())));
        c.step(Operation::Poll, 3_000, |_, _| Ok(json!({"metadata":{"state":"BATCH_STATE_RUNNING"}})));
        assert!(c.job.issues.contains_key("cancel")); assert!(!c.job.done());
        assert_eq!(c.job.next_operation(32_000), Some(Operation::Cancel));
    }

    #[test]
    fn pause_prevents_uploads_but_keeps_results_checks() {
        let f = Fixture::new(); let mut c = f.coordinator();
        f.control(Mode::Paused, 1_000); c.control().unwrap();
        assert!(c.job.next_operation(2_000).is_none());
        f.control(Mode::Running, 3_000); c.control().unwrap(); assert!(c.job.next_operation(3_000).is_none());
        c.step(Operation::Submit, 3_000, |_, _| Ok(json!({"name":"batches/accepted"})));
        f.control(Mode::Paused, 4_000); c.control().unwrap(); assert_eq!(c.job.next_operation(4_000), Some(Operation::Poll));
        c.job.save(&f.dir).unwrap(); assert!(f.reopen().job.mode == Mode::Paused);
    }

    #[test]
    fn authentication_failure_pauses_without_failing_the_sheets() {
        let f = Fixture::new(); let mut c = f.coordinator();
        c.step(Operation::Submit, 1_000, |_, _| Err(Failure::Status(401, "Invalid key".into())));
        c.job.save(&f.dir).unwrap(); let c = f.reopen();
        assert!(c.job.mode == Mode::Paused); assert!(c.job.groups.is_empty());
        assert!(c.job.sheets[0].error.is_empty()); assert!(c.job.next_operation(1_000_000).is_none());
    }

    #[test]
    fn a_failed_cancellation_does_not_block_other_groups_or_lose_its_error() {
        let f = Fixture::new(); let mut c = f.coordinator();
        c.step(Operation::Submit, 1_000, |_, _| Ok(json!({"name":"batches/first"})));
        let mut other = c.job.groups[0].clone(); other.remote = Remote::Waiting("batches/second".into());
        other.recovery.as_mut().unwrap().reference = "second".into(); c.job.groups.push(other); c.job.mode = Mode::Cancelling;
        c.step(Operation::Cancel, 2_000, |path, _| {
            assert_eq!(path, "batches/first:cancel"); Err(Failure::Status(503, "Try later".into()))
        });
        c.step(Operation::Cancel, 32_000, |path, _| { assert_eq!(path, "batches/second:cancel"); Ok(json!({})) });
        assert!(c.job.groups.iter().any(|g| g.tracking.error == "Try later"));
        assert!(c.job.groups.iter().any(|g| g.tracking.cancel_sent));
    }

    #[test]
    fn each_provider_batch_has_its_own_poll_interval() {
        let f = Fixture::new(); let mut c = f.coordinator();
        c.step(Operation::Submit, 1_000, |_, _| Ok(json!({"name":"batches/first"})));
        let mut other = c.job.groups[0].clone(); other.remote = Remote::Waiting("batches/second".into()); c.job.groups.push(other);
        c.step(Operation::Poll, 2_000, |path, _| { assert_eq!(path, "batches/first"); Ok(json!({"done":false})) });
        assert_eq!(c.job.next_operation(2_001), Some(Operation::Poll));
        c.step(Operation::Poll, 2_001, |path, _| { assert_eq!(path, "batches/second"); Ok(json!({"done":false})) });
        assert!(c.job.next_operation(2_002).is_none());
        assert_eq!(c.job.next_operation(32_000), Some(Operation::Poll));
    }

    #[test]
    fn old_journals_keep_accepted_ids_and_only_check_existing_work() {
        let f = Fixture::new(); let mut c = f.coordinator();
        c.step(Operation::Submit, 1_000, |_, _| Ok(json!({"name":"batches/old-accepted"})));
        let mut old = serde_json::to_value(&c.job).unwrap();
        for field in ["mode", "control_revision", "issues", "last_response_ms", "poll_ms", "recovery_ms", "prompt"] {
            old.as_object_mut().unwrap().remove(field);
        }
        for group in old["groups"].as_array_mut().unwrap() { group.as_object_mut().unwrap().remove("tracking"); }
        for sheet in old["sheets"].as_array_mut().unwrap() {
            for field in ["guard", "cancelled"] { sheet.as_object_mut().unwrap().remove(field); }
        }
        let legacy = f.folder.0.join("legacy");
        crate::storage::write_private(&legacy.join("state.json"), &old).unwrap();
        crate::sidecar::update_book(&f.root, |book| { book.ai_batch = None; Ok(()) }).unwrap();
        store::migrate(&f.root, &legacy).unwrap();
        let mut c = f.reopen(); assert_eq!(c.job.next_operation(10_000), Some(Operation::Poll));
        c.step(Operation::Poll, 10_000, |path, body| {
            assert_eq!(path, "batches/old-accepted"); assert!(body.is_none());
            Ok(json!({"metadata":{"state":"BATCH_STATE_RUNNING"}}))
        });
        assert_eq!(c.job.groups[0].remote, Remote::Waiting("batches/old-accepted".into()));
    }

    #[test]
    fn cancellation_still_runs_when_a_library_save_is_waiting() {
        let f = Fixture::new(); let mut c = f.coordinator(); receive(&mut c);
        let mut waiting = c.job.groups[0].clone(); waiting.remote = Remote::Waiting("batches/other".into());
        c.job.groups.push(waiting); c.job.mode = Mode::Cancelling;
        c.job.issue("save", "Library is read only".into(), 3_000);
        assert_eq!(c.job.next_operation(4_000), Some(Operation::Cancel));
    }

    /// An unconfirmed submission counts against the model's concurrency, so
    /// a flaky connection cannot pile up more paid work than the limit.
    #[test]
    fn unconfirmed_submissions_fill_the_concurrency() {
        let f = Fixture::new(); let mut c = f.coordinator();
        c.job.concurrency = 1;
        c.step(Operation::Submit, 1_000, |_, _| Err(Failure::Unknown("Lost reply".into())));
        let mut queued = c.job.sheets[0].clone(); queued.taken = false; c.job.sheets.push(queued);
        let mut submits = Submits::new();
        let log = crate::ai_log::Log::single("test");
        let send: Arc<SendFn> = Arc::new(|_, _| Ok(json!({"name":"batches/next"})));
        assert!(!c.dispatch_submissions(&mut submits, &send, &log, 2_000));
        assert_eq!(submits.inflight(), 0);
        assert_eq!(c.job.groups.iter().filter(|g| g.remote == Remote::Submitting).count(), 1);
    }

    /// A prepared submission goes out on a worker, and its reply turns the
    /// group into one the provider is working on.
    #[test]
    fn a_submission_is_sent_and_collected_on_the_pool() {
        let f = Fixture::new(); let mut c = f.coordinator();
        let mut submits = Submits::new();
        let log = crate::ai_log::Log::single("test");
        let send: Arc<SendFn> = Arc::new(|_, _| Ok(json!({"name":"batches/accepted"})));
        assert!(c.dispatch_submissions(&mut submits, &send, &log, 1_000));
        assert_eq!(submits.inflight(), 1);
        let (reference, reply) = submits.rx.recv_timeout(Duration::from_secs(5)).expect("the worker answered");
        submits.busy.remove(&reference);
        c.collect_submission(&reference, reply, 1_000);
        assert!(matches!(&c.job.groups[0].remote, Remote::Waiting(_)));
        assert!(!c.job.untaken());
    }

    /// The book is read once and kept, so a library of many sheets does not
    /// re-read it for each request. A saved label refreshes the copy.
    #[test]
    fn the_guard_label_is_kept_and_refreshes_after_a_save() {
        let f = Fixture::new(); let mut c = f.coordinator();
        let label = |caption: &str| Label { provider: "test".into(), model: "test".into(), status: Status::Labeled,
            caption: caption.into(), tags: vec![], tag_list: None };
        crate::sidecar::store_labels(&f.root, [("sheet.png", Some(label("Old")))]).unwrap();
        assert_eq!(c.guard_label(0).unwrap().caption, "Old");
        crate::sidecar::store_labels(&f.root, [("sheet.png", Some(label("New")))]).unwrap();
        assert_eq!(c.guard_label(0).unwrap().caption, "Old", "the book is kept between reads");
        c.saved(SaveReport { saved: vec![], rejected: vec![], error: None }, 1_000);
        assert_eq!(c.guard_label(0).unwrap().caption, "New", "a save refreshes the copy");
    }

    #[test]
    fn only_one_coordinator_can_own_the_library() {
        let f = Fixture::new(); let first = lock(&f.dir).unwrap();
        assert!(lock(&f.dir).is_err()); drop(first); assert!(lock(&f.dir).is_ok());
        assert!(f.folder.0.exists());
    }

    #[test]
    fn the_runner_waits_for_the_library_acknowledgement() {
        let f = Fixture::new(); let mut c = f.coordinator(); receive(&mut c);
        let runner = Runner::start(c.job, f.root.clone(), String::new(), None, lock(&f.dir).unwrap(), egui::Context::default(),
            crate::ai_log::Log::batch(&f.root, "test"));
        let mut reply = None;
        for _ in 0..4 {
            match runner.events.recv_timeout(Duration::from_secs(2)).unwrap() {
                Event::Import(job, channel) => { assert!(!job.done()); reply = Some(channel); break; }
                Event::Activity(_) => panic!("The runner started another operation before saving labels."),
                _ => {}
            }
        }
        let reply = reply.unwrap();
        assert!(runner.events.recv_timeout(Duration::from_millis(50)).is_err());
        let job = Job::load(&f.dir).unwrap().unwrap();
        let (report, _) = save_labels(&job, &f.root); reply.send(report).unwrap();
        let Event::Snapshot(job) = runner.events.recv_timeout(Duration::from_secs(2)).unwrap() else { panic!("Expected saved state") };
        assert!(job.done()); runner.finish(); assert!(Job::load(&f.dir).unwrap().unwrap().done());
    }
}
