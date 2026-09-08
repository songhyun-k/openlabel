use crate::{
    Error, Result, ble,
    label::{self, Request},
    m110,
    operation::{Control, PrintLock},
};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::path::Path;
use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio::time::Instant;
#[derive(Debug, Serialize)]
pub struct DeviceCheck {
    pub state: &'static str,
    pub sent_bytes: usize,
    pub evidence: ble::Evidence,
    pub cleanup_ok: bool,
}

/// Validates a selected target and checks its connection without sending printer bytes.
pub async fn check_device(id: &str, model: &str, control: &Control) -> Result<DeviceCheck> {
    ble::validate_target(id, model)?;
    check_locked(PrintLock::acquire(), control, async {
        let connection = ble::connect(id, model, control).await?;
        Ok((connection.evidence.clone(), async move {
            connection.cleanup().await
        }))
    })
    .await
}

async fn check_locked<Cleanup: Future<Output = Result<()>>>(
    lock: Result<PrintLock>,
    control: &Control,
    connect: impl Future<Output = Result<(ble::Evidence, Cleanup)>>,
) -> Result<DeviceCheck> {
    let result = async {
        let _lock = lock.map_err(|error| {
            if error.code == "job_busy" {
                error.with_context("err.checkBusy")
            } else {
                error
            }
        })?;
        control.check()?;
        let (evidence, cleanup) = connect.await?;
        tokio::time::timeout(Duration::from_secs(2), cleanup)
            .await
            .map_err(|_| Error::localized("transport_timeout", "err.cleanupTimeout", &[]))??;
        control.check()?;
        Ok(DeviceCheck {
            state: "connection_checked",
            sent_bytes: 0,
            evidence,
            cleanup_ok: true,
        })
    }
    .await;
    result.map_err(|error: Error| {
        if error.code == "cancelled" {
            Error::localized("cancelled", "err.checkCancelled", &[])
        } else {
            error
        }
    })
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrintRequest {
    pub label: Request,
    pub device: String,
    pub model: String,
    pub copies: u8,
    pub expect_sha256: String,
    pub expect_input_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: u64,
    pub state: String,
    pub finished: bool,
    pub sent_bytes: usize,
    pub total_bytes: usize,
    pub copies: u8,
    pub device: String,
    pub sha256: String,
    pub input_sha256: String,
    pub settings: Option<crate::settings::Settings>,
    pub evidence: Option<ble::Evidence>,
    pub error: Option<Error>,
    pub origin: String,
}
impl Job {
    pub fn terminal(&self) -> bool {
        ["completed", "failed", "cancelled"].contains(&self.state.as_str())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionStatus {
    pub state: String,
    pub device: Option<String>,
    pub evidence: Option<ble::Evidence>,
    pub error: Option<Error>,
}
impl Default for ConnectionStatus {
    fn default() -> Self {
        Self {
            state: "disconnected".into(),
            device: None,
            evidence: None,
            error: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Runtime {
    pub app_running: bool,
    pub revision: u64,
    pub connection: ConnectionStatus,
    pub job: Option<Job>,
    pub desktop_job: Option<Job>,
    pub busy: bool,
    pub closing: bool,
}
#[derive(Default)]
struct State {
    current: Option<(watch::Sender<Job>, Control)>,
    desktop: Option<watch::Receiver<Job>>,
    connection: ConnectionStatus,
    link_control: Option<Control>,
    busy: bool,
    closing: bool,
    revision: u64,
}
struct Session {
    connection: ble::Connection,
    lock: PrintLock,
}
#[derive(Default)]
pub struct Jobs {
    inner: Mutex<State>,
    transport: AsyncMutex<Option<Session>>,
    persistent: bool,
    #[cfg(test)]
    test: Option<TestSession>,
}
#[cfg(test)]
struct TestSession {
    path: std::path::PathBuf,
    connection: Arc<ble::FakeConnection>,
    render_busy: std::sync::atomic::AtomicUsize,
    renders: std::sync::atomic::AtomicUsize,
}
fn busy() -> Error {
    Error::localized("job_busy", "err.jobBusy", &[])
}
fn different_target() -> Error {
    Error::localized("connection_target_mismatch", "err.differentTarget", &[])
}
impl Jobs {
    pub fn persistent() -> Self {
        Self {
            persistent: true,
            ..Self::default()
        }
    }
    #[cfg(test)]
    pub(crate) fn synthetic(
        path: std::path::PathBuf,
        connection: Arc<ble::FakeConnection>,
    ) -> Self {
        Self {
            test: Some(TestSession {
                path,
                connection,
                render_busy: 0.into(),
                renders: 0.into(),
            }),
            ..Self::persistent()
        }
    }
    pub fn status(&self) -> Option<Job> {
        self.inner
            .lock()
            .unwrap()
            .current
            .as_ref()
            .map(|(sender, _)| sender.borrow().clone())
    }
    pub fn runtime(&self) -> Runtime {
        let state = self.inner.lock().unwrap();
        Runtime {
            app_running: true,
            revision: state.revision,
            connection: state.connection.clone(),
            job: state
                .current
                .as_ref()
                .map(|(sender, _)| sender.borrow().clone()),
            desktop_job: state
                .desktop
                .as_ref()
                .map(|receiver| receiver.borrow().clone()),
            busy: state.busy,
            closing: state.closing,
        }
    }
    pub fn cancel(&self, id: u64) {
        if let Some((sender, control)) = &self.inner.lock().unwrap().current
            && sender.borrow().id == id
            && !sender.borrow().terminal()
        {
            control.cancel();
        }
    }
    fn update(&self, id: u64, f: impl FnOnce(&mut Job)) {
        let mut state = self.inner.lock().unwrap();
        if let Some((sender, _)) = &state.current {
            let mut job = sender.borrow().clone();
            if job.id == id && !job.terminal() {
                f(&mut job);
                sender.send_replace(job);
                state.revision += 1;
            }
        }
    }
    pub fn start(self: &Arc<Self>, request: PrintRequest) -> Result<Job> {
        self.start_admitted(request, Control::default(), "desktop")
            .map(|(job, _)| job)
    }
    pub fn start_with_control(
        self: &Arc<Self>,
        request: PrintRequest,
        control: Control,
    ) -> Result<Job> {
        self.start_admitted(request, control, "cli")
            .map(|(job, _)| job)
    }
    pub fn start_observed(
        self: &Arc<Self>,
        request: PrintRequest,
        control: Control,
        origin: &str,
    ) -> Result<watch::Receiver<Job>> {
        self.start_admitted(request, control, origin)
            .map(|(_, receiver)| receiver)
    }
    fn start_admitted(
        self: &Arc<Self>,
        request: PrintRequest,
        control: Control,
        origin: &str,
    ) -> Result<(Job, watch::Receiver<Job>)> {
        control.check()?;
        if request.device.trim().is_empty() || request.model != "M110" {
            return Err(Error::localized(
                "unsupported_device",
                "err.targetRequired",
                &[],
            ));
        }
        if !(1..=10).contains(&request.copies)
            || (request.label.test_pattern && request.copies != 1)
        {
            return Err(Error::localized("invalid_settings", "err.copiesRange", &[]));
        }
        if [&request.expect_sha256, &request.expect_input_sha256]
            .iter()
            .any(|h| h.len() != 64 || !h.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            return Err(Error::localized("hash_mismatch", "err.hashRequired", &[]));
        }
        let mut inner = self.inner.lock().unwrap();
        if inner.busy || inner.closing {
            return Err(busy());
        }
        if inner
            .connection
            .device
            .as_ref()
            .is_some_and(|id| *id != request.device)
        {
            return Err(different_target());
        }
        let id = inner
            .current
            .as_ref()
            .map_or(1, |(sender, _)| sender.borrow().id + 1);
        let job = Job {
            id,
            state: "preparing".into(),
            finished: false,
            sent_bytes: 0,
            total_bytes: 0,
            copies: request.copies,
            device: request.device.clone(),
            sha256: request.expect_sha256.clone(),
            input_sha256: request.expect_input_sha256.clone(),
            settings: None,
            evidence: None,
            error: None,
            origin: origin.into(),
        };
        let (sender, receiver) = watch::channel(job.clone());
        if origin == "desktop" {
            inner.desktop = Some(receiver.clone());
        }
        inner.current = Some((sender, control.clone()));
        inner.busy = true;
        inner.revision += 1;
        drop(inner);
        let jobs = self.clone();
        tokio::spawn(async move {
            if let Err(error) = jobs.run(id, request, &control).await {
                jobs.update(id, |j| {
                    j.state = if error.code == "cancelled" {
                        "cancelled"
                    } else {
                        "failed"
                    }
                    .into();
                    j.error = Some(error);
                });
            }
            let mut state = jobs.inner.lock().unwrap();
            if let Some((sender, _)) = &state.current {
                let mut job = sender.borrow().clone();
                if job.id == id {
                    job.finished = true;
                    sender.send_replace(job);
                    state.busy = false;
                    state.revision += 1;
                }
            }
        });
        Ok((job, receiver))
    }
    async fn run(&self, id: u64, request: PrintRequest, control: &Control) -> Result<()> {
        let mut session = self.transport.lock().await;
        let mut lock = if session.is_none() {
            Some(self.acquire_lock()?)
        } else {
            None
        };
        control.check()?;
        let deadline = Instant::now() + Duration::from_secs(8);
        let preview = loop {
            control.check()?;
            #[cfg(not(test))]
            let result = label::preview_controlled(request.label.clone(), control).await;
            #[cfg(test)]
            let result = {
                use std::sync::atomic::Ordering;
                if let Some(test) = &self.test {
                    test.renders.fetch_add(1, Ordering::SeqCst);
                }
                if self.test.as_ref().is_some_and(|test| {
                    test.render_busy
                        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                        .is_ok()
                }) {
                    Err(Error::new("render_busy", "synthetic contention"))
                } else {
                    label::preview(&request.label)
                }
            };
            match result {
                Err(error) if error.code == "render_busy" && Instant::now() < deadline => {
                    control
                        .sleep(
                            Duration::from_millis(200)
                                .min(deadline.saturating_duration_since(Instant::now())),
                        )
                        .await?;
                }
                result => break result?,
            }
        };
        control.check()?;
        if preview.input_sha256 != request.expect_input_sha256
            || preview.sha256 != request.expect_sha256
        {
            return Err(Error::localized("hash_mismatch", "err.previewChanged", &[]));
        }
        let steps = m110::sequence(
            &preview.packed,
            preview.geometry.height as u16,
            &preview.settings.printer,
        )?;
        let total =
            steps.iter().map(|s| s.bytes.len()).sum::<usize>() * usize::from(request.copies);
        self.update(id, |j| {
            j.total_bytes = total;
            j.settings = Some(preview.settings);
        });
        if let Some(retained) = session.as_ref()
            && !Self::reuse_connected(&retained.connection, control).await?
        {
            let retained = session.take().unwrap();
            let _ = self.cleanup(&retained.connection, None).await;
            lock = Some(retained.lock);
        }
        control.check()?;
        if session.is_none() {
            self.connection_state("connecting", Some(request.device.clone()), None, None);
            match self.connect(&request.device, &request.model, control).await {
                Ok(connection) => {
                    self.connection_state(
                        "connected",
                        Some(request.device.clone()),
                        Some(connection.evidence.clone()),
                        None,
                    );
                    *session = Some(Session {
                        connection,
                        lock: lock.take().unwrap(),
                    });
                }
                Err(error) => {
                    self.connection_state("disconnected", None, None, Some(error.clone()));
                    return Err(error);
                }
            }
        }
        let connection = &session.as_ref().unwrap().connection;
        self.update(id, |j| {
            j.state = "sending".into();
            j.evidence = Some(connection.evidence.clone());
        });
        let result = send_sequence(
            &steps,
            request.copies,
            control,
            |bytes| connection.write(bytes),
            |n| self.update(id, |j| j.sent_bytes += n),
        )
        .await;
        match &result {
            Ok(()) => self.update(id, |j| j.state = "completed".into()),
            Err(error) => self.update(id, |j| {
                j.state = if error.code == "cancelled" {
                    "cancelled"
                } else {
                    "failed"
                }
                .into();
                j.error = Some(error.clone());
            }),
        }
        if !self.persistent || result.is_err() {
            let retained = session.take().unwrap();
            let _ = self
                .cleanup(&retained.connection, result.as_ref().err().cloned())
                .await;
        }
        result
    }
    fn acquire_lock(&self) -> Result<PrintLock> {
        #[cfg(test)]
        if let Some(test) = &self.test {
            return PrintLock::at(&test.path);
        }
        PrintLock::acquire()
    }
    async fn connect(&self, id: &str, model: &str, control: &Control) -> Result<ble::Connection> {
        #[cfg(test)]
        if let Some(test) = &self.test {
            return ble::Connection::fake(id, test.connection.clone(), control).await;
        }
        ble::connect(id, model, control).await
    }
    fn connection_state(
        &self,
        phase: &str,
        device: Option<String>,
        evidence: Option<ble::Evidence>,
        error: Option<Error>,
    ) {
        let mut state = self.inner.lock().unwrap();
        state.connection = ConnectionStatus {
            state: phase.into(),
            device,
            evidence,
            error,
        };
        state.revision += 1;
    }
    async fn cleanup(&self, connection: &ble::Connection, error: Option<Error>) -> Result<()> {
        self.connection_state(
            "disconnecting",
            Some(connection.evidence.device_id.clone()),
            Some(connection.evidence.clone()),
            error.clone(),
        );
        let cleanup = tokio::time::timeout(Duration::from_secs(2), connection.cleanup())
            .await
            .unwrap_or_else(|_| {
                Err(Error::localized(
                    "transport_timeout",
                    "err.cleanupTimeout",
                    &[],
                ))
            });
        self.connection_state(
            "disconnected",
            None,
            None,
            error.or_else(|| cleanup.as_ref().err().cloned()),
        );
        cleanup
    }
    fn admit_link(&self, device: &str, model: Option<&str>, control: &Control) -> Result<()> {
        if !self.persistent {
            return Err(Error::localized("app_required", "err.appRequired", &[]));
        }
        if let Some(model) = model {
            ble::validate_target(device, model)?;
        }
        control.check()?;
        let mut state = self.inner.lock().unwrap();
        if state.busy || state.closing {
            return Err(busy());
        }
        if state
            .connection
            .device
            .as_ref()
            .is_some_and(|id| id != device)
        {
            return Err(different_target());
        }
        if model.is_none() && state.connection.device.as_deref() != Some(device) {
            return Err(Error::localized("disconnected", "err.connectedId", &[]));
        }
        state.busy = true;
        state.link_control = Some(control.clone());
        state.revision += 1;
        Ok(())
    }
    pub async fn link(
        &self,
        device: String,
        model: Option<String>,
        control: Control,
    ) -> Result<ConnectionStatus> {
        self.admit_link(&device, model.as_deref(), &control)?;
        let result = self.run_link(&device, model.as_deref(), &control).await;
        let mut state = self.inner.lock().unwrap();
        state.busy = false;
        state.link_control = None;
        if let Err(error) = &result {
            state.connection.error = Some(error.clone());
        }
        state.revision += 1;
        result.map(|()| state.connection.clone())
    }
    async fn run_link(&self, device: &str, model: Option<&str>, control: &Control) -> Result<()> {
        let mut session = self.transport.lock().await;
        let mut lock = None;
        if let Some(retained) = session.as_ref() {
            if model.is_some() && Self::reuse_connected(&retained.connection, control).await? {
                return Ok(());
            }
            let retained = session.take().unwrap();
            let cleanup = self.cleanup(&retained.connection, None).await;
            lock = Some(retained.lock);
            if model.is_none() {
                cleanup?;
                control.check()?;
            }
        }
        if let Some(model) = model {
            control.check()?;
            let lock = match lock {
                Some(lock) => lock,
                None => self.acquire_lock()?,
            };
            self.connection_state("connecting", Some(device.into()), None, None);
            let result = self.connect(device, model, control).await;
            match result {
                Ok(connection) => {
                    if let Err(error) = control.check() {
                        let _ = self.cleanup(&connection, Some(error.clone())).await;
                        return Err(error);
                    }
                    self.connection_state(
                        "connected",
                        Some(device.into()),
                        Some(connection.evidence.clone()),
                        None,
                    );
                    *session = Some(Session { connection, lock });
                }
                Err(error) => {
                    self.connection_state("disconnected", None, None, Some(error.clone()));
                    return Err(error);
                }
            }
        }
        Ok(())
    }
    async fn reuse_connected(connection: &ble::Connection, control: &Control) -> Result<bool> {
        let connected = control.operation(connection.is_connected()).await;
        control.check()?;
        Ok(connected.unwrap_or(false))
    }
    pub async fn poll_connection(&self) {
        let Ok(mut session) = self.transport.try_lock() else {
            return;
        };
        {
            let state = self.inner.lock().unwrap();
            if state.busy || state.closing || session.is_none() {
                return;
            }
        }
        let connected = tokio::time::timeout(
            Duration::from_secs(1),
            session.as_ref().unwrap().connection.is_connected(),
        )
        .await;
        if !matches!(connected, Ok(Ok(true))) {
            let error = match connected {
                Ok(Err(error)) => error,
                _ => Error::localized("disconnected", "err.connectionLost", &[]),
            };
            let retained = session.take().unwrap();
            let _ = self.cleanup(&retained.connection, Some(error)).await;
        }
    }
    pub fn begin_close(&self) -> bool {
        let mut state = self.inner.lock().unwrap();
        if state.closing {
            return false;
        }
        state.closing = true;
        state.revision += 1;
        if let Some((sender, control)) = &state.current
            && !sender.borrow().terminal()
        {
            control.cancel();
        }
        if let Some(control) = &state.link_control {
            control.cancel();
        }
        true
    }
    pub async fn shutdown(&self) {
        self.begin_close();
        while self.inner.lock().unwrap().busy {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let mut session = self.transport.lock().await;
        if let Some(retained) = session.take() {
            let _ = self.cleanup(&retained.connection, None).await;
        }
    }
}
pub(crate) async fn send_sequence<'a, F, Fut>(
    steps: &'a [m110::Step],
    copies: u8,
    control: &Control,
    mut write: F,
    mut progress: impl FnMut(usize),
) -> Result<()>
where
    F: FnMut(&'a [u8]) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    for _ in 0..copies {
        for step in steps {
            control.check()?;
            if !step.bytes.is_empty() {
                control.operation(write(&step.bytes)).await?;
                progress(step.bytes.len());
            }
            if step.delay_ms > 0 {
                control.sleep(Duration::from_millis(step.delay_ms)).await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn synthetic_request(dir: &Path) -> PrintRequest {
        let path = dir.join("tiny.svg");
        std::fs::write(&path, b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40mm\" height=\"10mm\"><rect width=\"4\" height=\"4\"/></svg>").unwrap();
        let label = Request {
            path: Some(path),
            ..Default::default()
        };
        let preview = label::preview(&label).unwrap();
        PrintRequest {
            label,
            device: "00000000-0000-0000-0000-000000000007".into(),
            model: "M110".into(),
            copies: 1,
            expect_sha256: preview.sha256,
            expect_input_sha256: preview.input_sha256,
        }
    }
    async fn finished(mut receiver: watch::Receiver<Job>) -> Job {
        loop {
            let job = receiver.borrow().clone();
            if job.finished {
                return job;
            }
            receiver.changed().await.unwrap();
        }
    }
    #[tokio::test(start_paused = true)]
    async fn retained_session_reuses_target_watches_and_global_lock_until_disconnect() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("print.lock");
        let fake = Arc::new(ble::FakeConnection::default());
        let jobs = Arc::new(Jobs::synthetic(path.clone(), fake.clone()));
        let request = synthetic_request(dir.path());
        jobs.link(
            request.device.clone(),
            Some("M110".into()),
            Control::default(),
        )
        .await
        .unwrap();
        jobs.link(
            request.device.clone(),
            Some("M110".into()),
            Control::default(),
        )
        .await
        .unwrap();
        assert_eq!(fake.connects.load(Ordering::SeqCst), 1);
        assert_eq!(fake.writes.load(Ordering::SeqCst), 0);
        assert!(PrintLock::at(&path).is_err());
        let mut other = request.clone();
        other.device = "00000000-0000-0000-0000-000000000008".into();
        assert_eq!(
            jobs.link(
                other.device.clone(),
                Some("M110".into()),
                Control::default()
            )
            .await
            .unwrap_err()
            .code,
            "connection_target_mismatch"
        );
        assert_eq!(
            jobs.start(other).unwrap_err().code,
            "connection_target_mismatch"
        );
        let first = jobs
            .start_observed(request.clone(), Control::default(), "desktop")
            .unwrap();
        assert_eq!(
            jobs.link(request.device.clone(), None, Control::default())
                .await
                .unwrap_err()
                .code,
            "job_busy"
        );
        assert_eq!(finished(first.clone()).await.state, "completed");
        // A fresh Control works even long after the connect operation's deadline.
        tokio::time::advance(Duration::from_secs(181)).await;
        let second = jobs
            .start_observed(request.clone(), Control::default(), "cli")
            .unwrap();
        assert_eq!(first.borrow().state, "completed");
        assert_eq!(jobs.runtime().desktop_job.unwrap().id, first.borrow().id);
        assert_eq!(finished(second).await.state, "completed");
        assert_eq!(fake.connects.load(Ordering::SeqCst), 1);
        assert_eq!(fake.cleanups.load(Ordering::SeqCst), 0);
        let writes = fake.writes.load(Ordering::SeqCst);
        let mut stale = request.clone();
        stale.expect_sha256 = "0".repeat(64);
        assert_eq!(
            finished(
                jobs.start_observed(stale, Control::default(), "cli")
                    .unwrap()
            )
            .await
            .error
            .unwrap()
            .code,
            "hash_mismatch"
        );
        assert_eq!(fake.writes.load(Ordering::SeqCst), writes);
        assert_eq!(jobs.runtime().connection.state, "connected");
        fake.connected.store(false, Ordering::SeqCst);
        jobs.poll_connection().await;
        assert_eq!(jobs.runtime().connection.state, "disconnected");
        assert_eq!(fake.cleanups.load(Ordering::SeqCst), 1);
        assert!(PrintLock::at(&path).is_ok());
        jobs.link(
            request.device.clone(),
            Some("M110".into()),
            Control::default(),
        )
        .await
        .unwrap();
        jobs.link(request.device, None, Control::default())
            .await
            .unwrap();
        assert_eq!(fake.cleanups.load(Ordering::SeqCst), 2);
        assert!(PrintLock::at(&path).is_ok());
    }
    #[tokio::test(start_paused = true)]
    async fn cancelled_or_expired_reuse_probe_keeps_untouched_session() {
        use std::sync::atomic::Ordering;
        for print in [false, true] {
            for expired in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("lock");
                let fake = Arc::new(ble::FakeConnection::default());
                let jobs = Arc::new(Jobs::synthetic(path.clone(), fake.clone()));
                let request = synthetic_request(dir.path());
                jobs.link(
                    request.device.clone(),
                    Some("M110".into()),
                    Control::default(),
                )
                .await
                .unwrap();
                let control = Control::default();
                if expired {
                    tokio::time::advance(Duration::from_secs(179)).await;
                }
                fake.probe_delay_ms.store(2000, Ordering::SeqCst);
                let task = if print {
                    let receiver = jobs
                        .start_observed(request.clone(), control.clone(), "cli")
                        .unwrap();
                    tokio::spawn(async move { finished(receiver).await.error.unwrap() })
                } else {
                    let jobs = jobs.clone();
                    let device = request.device.clone();
                    let control = control.clone();
                    tokio::spawn(async move {
                        jobs.link(device, Some("M110".into()), control)
                            .await
                            .unwrap_err()
                    })
                };
                while fake.probes.load(Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
                if expired {
                    tokio::time::advance(Duration::from_secs(1)).await;
                } else {
                    control.cancel();
                }
                let error = task.await.unwrap();
                assert_eq!(
                    error.code,
                    if expired {
                        "transport_timeout"
                    } else {
                        "cancelled"
                    }
                );
                assert_eq!(fake.writes.load(Ordering::SeqCst), 0);
                assert_eq!(fake.cleanups.load(Ordering::SeqCst), 0);
                assert_eq!(jobs.runtime().connection.state, "connected");
                assert!(PrintLock::at(&path).is_err());
                fake.probe_delay_ms.store(0, Ordering::SeqCst);
                jobs.link(request.device, None, Control::default())
                    .await
                    .unwrap();
                assert_eq!(fake.cleanups.load(Ordering::SeqCst), 1);
                assert!(PrintLock::at(&path).is_ok());
            }
        }
    }
    #[tokio::test(start_paused = true)]
    async fn h2_false_reuse_probe_replaces_session_for_connect_and_print() {
        use std::sync::atomic::Ordering;
        for print in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("lock");
            let fake = Arc::new(ble::FakeConnection::default());
            let jobs = Arc::new(Jobs::synthetic(path.clone(), fake.clone()));
            let request = synthetic_request(dir.path());
            jobs.link(
                request.device.clone(),
                Some("M110".into()),
                Control::default(),
            )
            .await
            .unwrap();
            assert!(PrintLock::at(&path).is_err());
            let probes = fake.probes.load(Ordering::SeqCst);
            // Leave the retained session intact: exercise reuse, not idle polling.
            fake.connected.store(false, Ordering::SeqCst);
            if print {
                let job = finished(
                    jobs.start_observed(request.clone(), Control::default(), "cli")
                        .unwrap(),
                )
                .await;
                assert_eq!(job.state, "completed");
                assert!(job.finished);
                assert!(fake.writes.load(Ordering::SeqCst) > 0);
            } else {
                jobs.link(
                    request.device.clone(),
                    Some("M110".into()),
                    Control::default(),
                )
                .await
                .unwrap();
                assert_eq!(fake.writes.load(Ordering::SeqCst), 0);
            }
            assert!(fake.probes.load(Ordering::SeqCst) > probes);
            assert_eq!(fake.cleanups.load(Ordering::SeqCst), 1);
            assert_eq!(fake.connects.load(Ordering::SeqCst), 2);
            assert_eq!(jobs.runtime().connection.state, "connected");
            assert!(PrintLock::at(&path).is_err());
            jobs.link(request.device, None, Control::default())
                .await
                .unwrap();
            assert_eq!(fake.cleanups.load(Ordering::SeqCst), 2);
            assert!(PrintLock::at(&path).is_ok());
        }
    }
    #[tokio::test]
    async fn check_busy_points_to_retained_connection_without_connecting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");
        let _held = PrintLock::at(&path).unwrap();
        let called = std::sync::atomic::AtomicBool::new(false);
        for (lock, original) in [
            (PrintLock::at(&path), None),
            (
                Err(Error::new("job_busy", "synthetic useful I/O detail")),
                Some("synthetic useful I/O detail"),
            ),
        ] {
            let error = check_locked(lock, &Control::default(), async {
                called.store(true, std::sync::atomic::Ordering::SeqCst);
                Err::<(ble::Evidence, std::future::Ready<Result<()>>), _>(Error::new(
                    "unexpected",
                    "must not connect",
                ))
            })
            .await
            .unwrap_err();
            assert_eq!(error.code, "job_busy");
            assert!(error.detail.contains("openlabel connection"));
            assert!(
                error
                    .detail
                    .contains("app is holding the printer connection")
            );
            if let Some(original) = original {
                assert!(error.detail.contains(original));
            }
        }
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
    }
    #[tokio::test(start_paused = true)]
    async fn retained_send_failure_cancel_and_shutdown_are_bounded_without_retry() {
        use std::sync::atomic::Ordering;
        for outcome in [
            "failure",
            "cancel",
            "close_send",
            "close_connect",
            "completed_cleanup",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("print.lock");
            let fake = Arc::new(ble::FakeConnection {
                connect_delay: if outcome == "close_connect" {
                    Duration::from_secs(2)
                } else {
                    Duration::ZERO
                },
                cleanup_delay: Duration::from_secs(20),
                ..Default::default()
            });
            let mut instance = Jobs::synthetic(path.clone(), fake.clone());
            if outcome == "completed_cleanup" {
                instance.persistent = false;
            }
            let jobs = Arc::new(instance);
            let request = synthetic_request(dir.path());
            if outcome == "close_connect" {
                let operation = tokio::spawn({
                    let jobs = jobs.clone();
                    async move {
                        jobs.link(request.device, Some("M110".into()), Control::default())
                            .await
                    }
                });
                while fake.connects.load(Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
                assert!(jobs.begin_close());
                assert!(!jobs.begin_close());
                jobs.shutdown().await;
                assert_eq!(operation.await.unwrap().unwrap_err().code, "cancelled");
            } else {
                fake.fail_write
                    .store(outcome == "failure", Ordering::SeqCst);
                let receiver = jobs
                    .start_observed(request, Control::default(), "desktop")
                    .unwrap();
                while fake.writes.load(Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
                if outcome == "cancel" {
                    jobs.cancel(receiver.borrow().id);
                }
                if outcome == "close_send" {
                    jobs.begin_close();
                }
                if outcome == "completed_cleanup" {
                    let mut observe = receiver.clone();
                    while !observe.borrow().terminal() {
                        observe.changed().await.unwrap();
                    }
                    assert!(!observe.borrow().finished);
                    assert!(PrintLock::at(&path).is_err());
                    jobs.begin_close();
                }
                let result = finished(receiver).await;
                assert_eq!(
                    result.state,
                    match outcome {
                        "failure" => "failed",
                        "completed_cleanup" => "completed",
                        _ => "cancelled",
                    }
                );
                if outcome != "completed_cleanup" {
                    assert_eq!(fake.writes.load(Ordering::SeqCst), 1);
                }
                jobs.shutdown().await;
            }
            assert_eq!(fake.connects.load(Ordering::SeqCst), 1);
            assert_eq!(fake.cleanups.load(Ordering::SeqCst), 1);
            assert!(PrintLock::at(&path).is_ok());
        }
    }
    #[tokio::test(start_paused = true)]
    async fn controlled_render_contention_uses_real_job_loop_hashes_and_cancellation() {
        use std::sync::atomic::Ordering;
        for outcome in ["eventual", "cancel", "close", "exhausted"] {
            let dir = tempfile::tempdir().unwrap();
            let fake = Arc::new(ble::FakeConnection::default());
            let jobs = Arc::new(Jobs::synthetic(dir.path().join("lock"), fake.clone()));
            jobs.test.as_ref().unwrap().render_busy.store(
                if outcome == "eventual" { 3 } else { usize::MAX },
                Ordering::SeqCst,
            );
            let before = Instant::now();
            let receiver = jobs
                .start_observed(synthetic_request(dir.path()), Control::default(), "desktop")
                .unwrap();
            while jobs.test.as_ref().unwrap().renders.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
            if outcome == "cancel" {
                jobs.cancel(receiver.borrow().id);
            }
            if outcome == "close" {
                jobs.begin_close();
            }
            let result = finished(receiver).await;
            if outcome == "eventual" {
                assert_eq!(result.state, "completed");
                assert_eq!(
                    jobs.test.as_ref().unwrap().renders.load(Ordering::SeqCst),
                    4
                );
                assert_eq!(fake.connects.load(Ordering::SeqCst), 1);
            } else {
                assert_eq!(
                    result.error.unwrap().code,
                    if outcome == "exhausted" {
                        "render_busy"
                    } else {
                        "cancelled"
                    }
                );
                assert_eq!(fake.writes.load(Ordering::SeqCst), 0);
                assert_eq!(fake.connects.load(Ordering::SeqCst), 0);
                if outcome == "exhausted" {
                    assert_eq!(Instant::now() - before, Duration::from_secs(8));
                } else {
                    assert_eq!(
                        jobs.test.as_ref().unwrap().renders.load(Ordering::SeqCst),
                        1
                    );
                }
            }
            jobs.shutdown().await;
        }
    }
    #[tokio::test(start_paused = true)]
    async fn device_check_holds_isolated_lock_through_cleanup_without_printing() {
        use std::cell::Cell;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("check.lock");
        for outcome in [
            "busy",
            "success",
            "connection_failure",
            "cleanup_failure",
            "transport_timeout",
            "cancelled",
            "cancelled_before_connect",
            "cancelled_during_connect",
        ] {
            let busy = (outcome == "busy").then(|| PrintLock::at(&path).unwrap());
            let control = Control::default();
            if outcome == "cancelled_before_connect" {
                control.cancel();
            }
            let connected = Cell::new(false);
            let cleaned = Cell::new(false);
            let result = check_locked(PrintLock::at(&path), &control, async {
                connected.set(true);
                assert!(PrintLock::at(&path).is_err(), "lock before connection");
                if outcome == "cancelled_during_connect" {
                    control.cancel();
                    control.check()?;
                }
                if outcome == "connection_failure" {
                    return Err(Error::new(outcome, "synthetic"));
                }
                let evidence = ble::Evidence {
                    device_id: "synthetic".into(),
                    model: "M110".into(),
                    service: "FF00".into(),
                    characteristic: "FF02".into(),
                    write_type: "without_response".into(),
                    mtu: 131,
                };
                Ok((evidence, async {
                    assert!(PrintLock::at(&path).is_err(), "lock during cleanup");
                    if outcome == "cancelled" {
                        control.cancel();
                    }
                    if outcome == "transport_timeout" {
                        std::future::pending::<()>().await;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    assert!(
                        PrintLock::at(&path).is_err(),
                        "cancel cannot release lock before cleanup"
                    );
                    cleaned.set(true);
                    if outcome == "cleanup_failure" {
                        return Err(Error::new(outcome, "synthetic"));
                    }
                    Ok(())
                }))
            })
            .await;
            if outcome == "success" {
                let result = result.unwrap();
                assert_eq!(result.sent_bytes, 0);
                assert_eq!(result.state, "connection_checked");
                assert!(result.cleanup_ok);
            } else {
                let error = result.unwrap_err();
                if outcome.starts_with("cancelled") {
                    assert_eq!(
                        error.detail,
                        "Connection check cancelled. No print data was sent to the printer."
                    );
                    assert_eq!(
                        control.check().unwrap_err().detail,
                        "Print cancelled. Data already sent may still print."
                    );
                }
                assert_eq!(
                    error.code,
                    if outcome == "busy" {
                        "job_busy"
                    } else if outcome.starts_with("cancelled") {
                        "cancelled"
                    } else {
                        outcome
                    }
                );
            }
            assert_eq!(
                connected.get(),
                !["busy", "cancelled_before_connect"].contains(&outcome)
            );
            assert_eq!(
                cleaned.get(),
                ["success", "cleanup_failure", "cancelled"].contains(&outcome)
            );
            drop(busy);
            assert!(PrintLock::at(&path).is_ok());
        }
        assert_eq!(
            check_device("synthetic", "M220", &Control::default())
                .await
                .unwrap_err()
                .code,
            "unsupported_device"
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            check_device("synthetic", "M110", &Control::default())
                .await
                .unwrap_err()
                .code,
            "invalid_arguments"
        );
    }
    #[tokio::test(start_paused = true)]
    async fn print_job_pacing_success_cancel_failure_timeout() {
        let steps = m110::sequence(
            &[0x80; 144],
            3,
            &crate::settings::Printer {
                density: 10,
                speed: 1,
            },
        )
        .unwrap();
        let control = Control::default();
        let now = Instant::now();
        let mut sent = Vec::new();
        let mut count = 0;
        send_sequence(
            &steps,
            1,
            &control,
            |bytes| {
                sent.push(bytes.to_vec());
                std::future::ready(Ok(()))
            },
            |n| count += n,
        )
        .await
        .unwrap();
        assert_eq!(Instant::now() - now, Duration::from_millis(930));
        assert_eq!(sent.len(), 7);
        assert_eq!(count, 171);
        let control = Control::default();
        let cancel = control.clone();
        let mut calls = 0;
        let result = send_sequence(
            &steps,
            1,
            &control,
            |_| {
                calls += 1;
                cancel.cancel();
                std::future::ready(Ok(()))
            },
            |_| {},
        )
        .await;
        assert_eq!(result.unwrap_err().code, "cancelled");
        assert_eq!(calls, 1);
        let mut calls = 0;
        let result = send_sequence(
            &steps,
            2,
            &Control::default(),
            |_| {
                calls += 1;
                std::future::ready(Err(Error::new("disconnected", "fake")))
            },
            |_| {},
        )
        .await;
        assert_eq!(result.unwrap_err().code, "disconnected");
        assert_eq!(calls, 1);
        let result = send_sequence(
            &steps,
            1,
            &Control::default(),
            |_| std::future::pending::<Result<()>>(),
            |_| {},
        )
        .await;
        assert_eq!(result.unwrap_err().code, "transport_timeout");
    }
    #[tokio::test]
    async fn print_job_admission_race_and_terminal_immutable() {
        let jobs = Arc::new(Jobs::default());
        let request = PrintRequest {
            label: Request::default(),
            device: "test".into(),
            model: "M110".into(),
            copies: 1,
            expect_sha256: "0".repeat(64),
            expect_input_sha256: "0".repeat(64),
        };
        let job = jobs.start(request.clone()).unwrap();
        assert_eq!(jobs.start(request).unwrap_err().code, "job_busy");
        jobs.update(job.id, |j| j.state = "completed".into());
        jobs.cancel(job.id);
        jobs.update(job.id, |j| j.state = "failed".into());
        assert_eq!(jobs.status().unwrap().state, "completed");
    }
    #[test]
    fn print_job_cross_process_lock() {
        if let Ok(path) = std::env::var("OPENLABEL_LOCK_TEST_PATH") {
            assert_eq!(
                PrintLock::at(Path::new(&path)).err().unwrap().code,
                "job_busy"
            );
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");
        let lock = PrintLock::at(&path).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "job::tests::print_job_cross_process_lock"])
            .env("OPENLABEL_LOCK_TEST_PATH", &path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        drop(lock);
        assert!(PrintLock::at(&path).is_ok());
        assert!(path.exists());
    }
}
