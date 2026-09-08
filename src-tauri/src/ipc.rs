use crate::{
    Error, Result,
    job::{ConnectionStatus, Job, Jobs, PrintRequest, Runtime},
    operation::Control,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Mutex, Semaphore, watch},
    task::{JoinHandle, JoinSet},
    time::Instant,
};

pub const REQUEST_LIMIT: usize = 64 * 1024;
pub const RESPONSE_LIMIT: usize = 8 * 1024 * 1024;
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Print { request: Box<PrintRequest> },
    Runtime {},
    Connect { device: String, model: String },
    Disconnect { device: String },
    Cancel {},
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u8,
    pub request: Operation,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reply {
    Accepted { job: Job },
    Progress { job: Job },
    Final { job: Job },
    Runtime { runtime: Box<Runtime> },
    Connection { connection: ConnectionStatus },
    Error { error: Error },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub version: u8,
    pub response: Reply,
}
fn protocol(detail: impl Into<String>) -> Error {
    Error::new("ipc_error", detail)
}
fn security(detail: impl Into<String>) -> Error {
    Error::new("ipc_security", detail)
}
fn unknown() -> Error {
    Error::localized("unknown_outcome", "err.unknownOutcome", &[])
}
pub async fn read_frame<T: DeserializeOwned>(
    reader: &mut (impl AsyncRead + Unpin),
    limit: usize,
) -> Result<T> {
    let size = reader
        .read_u32()
        .await
        .map_err(|e| protocol(e.to_string()))? as usize;
    if size == 0 || size > limit {
        return Err(Error::localized("ipc_error", "err.ipcFrameLimit", &[]));
    }
    let mut bytes = vec![0; size];
    reader
        .read_exact(&mut bytes)
        .await
        .map_err(|e| protocol(e.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|e| protocol(e.to_string()))
}
pub async fn write_frame(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &impl Serialize,
    limit: usize,
) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| protocol(e.to_string()))?;
    if bytes.is_empty() || bytes.len() > limit {
        return Err(Error::localized("ipc_error", "err.ipcFrameLimit", &[]));
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        writer.write_u32(bytes.len() as u32).await?;
        writer.write_all(&bytes).await
    })
    .await
    .map_err(|_| Error::localized("ipc_error", "err.ipcWriteTimeout", &[]))?
    .map_err(|e| protocol(e.to_string()))
}
async fn reply(writer: &mut (impl AsyncWrite + Unpin), response: Reply) -> Result<()> {
    write_frame(
        writer,
        &Response {
            version: 1,
            response,
        },
        RESPONSE_LIMIT,
    )
    .await
}
pub use platform::Endpoint;

pub struct Host {
    stop: watch::Sender<bool>,
    task: Mutex<Option<JoinHandle<()>>>,
}
impl Host {
    pub async fn start(endpoint: &Endpoint, jobs: Arc<Jobs>) -> Result<Arc<Self>> {
        let mut listener = platform::Listener::bind(endpoint)?;
        let (stop, mut stopped) = watch::channel(false);
        let mut poll_stop = stopped.clone();
        let poll_jobs = jobs.clone();
        let poll = tokio::spawn(async move {
            loop {
                tokio::select! { biased; _ = poll_stop.changed() => break, _ = tokio::time::sleep(Duration::from_secs(1)) => poll_jobs.poll_connection().await }
            }
        });
        let task = tokio::spawn(async move {
            let slots = Arc::new(Semaphore::new(4));
            let mut handlers = JoinSet::new();
            loop {
                tokio::select! { biased;
                    _ = stopped.changed() => break,
                    _ = handlers.join_next(), if !handlers.is_empty() => {},
                    accepted = listener.accept() => {
                        let Ok(mut stream) = accepted else { continue; };
                        if platform::authorize(&stream).is_err() { continue; }
                        let Ok(permit) = slots.clone().try_acquire_owned() else {
                            let _ = reply(&mut stream, Reply::Error { error: Error::localized("ipc_busy", "err.ipcBusy", &[]) }).await;
                            continue;
                        };
                        let jobs = jobs.clone(); let stopped = stopped.clone();
                        handlers.spawn(async move { let _permit = permit; serve(stream, jobs, stopped).await; });
                    }
                }
            }
            let _ = poll.await;
            while handlers.join_next().await.is_some() {}
        });
        Ok(Arc::new(Self {
            stop,
            task: Mutex::new(Some(task)),
        }))
    }
    pub fn stop_admission(&self) {
        self.stop.send_replace(true);
    }
    pub async fn stopped(&self) {
        self.stop_admission();
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }
}
async fn serve(stream: platform::ServerStream, jobs: Arc<Jobs>, stopped: watch::Receiver<bool>) {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let first = tokio::time::timeout(
        Duration::from_secs(5),
        read_frame::<Request>(&mut reader, REQUEST_LIMIT),
    )
    .await;
    let request = match first {
        Ok(Ok(request)) if request.version == 1 && !*stopped.borrow() => request.request,
        _ => {
            let _ = reply(
                &mut writer,
                Reply::Error {
                    error: Error::localized("ipc_error", "err.ipcRequestFormat", &[]),
                },
            )
            .await;
            return;
        }
    };
    match request {
        Operation::Runtime {} => {
            let _ = reply(
                &mut writer,
                Reply::Runtime {
                    runtime: Box::new(jobs.runtime()),
                },
            )
            .await;
        }
        Operation::Print { request } => {
            let control = Control::default();
            let mut receiver = match jobs.start_observed(*request, control.clone(), "cli") {
                Ok(receiver) => receiver,
                Err(error) => {
                    let _ = reply(&mut writer, Reply::Error { error }).await;
                    return;
                }
            };
            let initial = receiver.borrow().clone();
            let mut previous = initial.state.clone();
            let mut writable = reply(&mut writer, Reply::Accepted { job: initial })
                .await
                .is_ok();
            if !writable {
                control.cancel();
            }
            let followup = read_frame::<Request>(&mut reader, REQUEST_LIMIT);
            tokio::pin!(followup);
            let mut followed = false;
            loop {
                let job = receiver.borrow().clone();
                if job.finished {
                    if writable {
                        let _ = reply(&mut writer, Reply::Final { job }).await;
                    }
                    break;
                }
                tokio::select! { biased;
                    request = &mut followup, if !followed => {
                        followed = true;
                        // This stream owns only this Control; EOF/malformed input also cancels it.
                        let _valid_cancel = matches!(request, Ok(Request { version: 1, request: Operation::Cancel {} }));
                        control.cancel();
                    }
                    changed = receiver.changed() => {
                        if changed.is_err() { break; }
                        let job = receiver.borrow().clone();
                        if job.state != previous {
                            previous = job.state.clone();
                            if writable && reply(&mut writer, Reply::Progress { job }).await.is_err() {
                                writable = false; control.cancel();
                            }
                        }
                    }
                }
            }
        }
        Operation::Connect { device, model } => {
            serve_link(&jobs, device, Some(model), &mut reader, &mut writer).await
        }
        Operation::Disconnect { device } => {
            serve_link(&jobs, device, None, &mut reader, &mut writer).await
        }
        Operation::Cancel {} => {
            let _ = reply(
                &mut writer,
                Reply::Error {
                    error: Error::localized("ipc_error", "err.ipcCancelStream", &[]),
                },
            )
            .await;
        }
    }
}
async fn serve_link(
    jobs: &Jobs,
    device: String,
    model: Option<String>,
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
) {
    let control = Control::default();
    let operation = jobs.link(device, model, control.clone());
    tokio::pin!(operation);
    let mut byte = [0];
    let result = tokio::select! { biased;
        result = &mut operation => result,
        _ = reader.read(&mut byte) => { control.cancel(); operation.await }
    };
    let response = match result {
        Ok(connection) => Reply::Connection { connection },
        Err(error) => Reply::Error { error },
    };
    let _ = reply(writer, response).await;
}

/// NoHost is returned only before submission. All failures after a write attempt are terminal.
pub async fn call(
    endpoint: &Endpoint,
    operation: Operation,
    control: &Control,
    mut progress: impl FnMut(&Job),
) -> Result<Option<Reply>> {
    let mut operation = operation;
    if let Operation::Print { request } = &mut operation {
        request.label.overrides.validate_finite()?;
        let cwd = std::env::current_dir().map_err(|e| protocol(e.to_string()))?;
        for path in [&mut request.label.path, &mut request.label.settings]
            .into_iter()
            .flatten()
        {
            if path.is_relative() {
                *path = cwd.join(&*path);
            }
        }
    }
    let request = Request {
        version: 1,
        request: operation,
    };
    // Reject local serialization/size errors before any submission attempt.
    if serde_json::to_vec(&request)
        .map_err(|e| protocol(e.to_string()))?
        .len()
        > REQUEST_LIMIT
    {
        return Err(Error::localized("ipc_error", "err.ipcRequestLimit", &[]));
    }
    control.check()?;
    let Some(stream) = control.operation(platform::connect(endpoint)).await? else {
        return Ok(None);
    };
    platform::authorize_server(&stream)?;
    let (mut reader, mut writer) = tokio::io::split(stream);
    write_frame(&mut writer, &request, REQUEST_LIMIT)
        .await
        .map_err(|_| unknown())?;
    let print = matches!(request.request, Operation::Print { .. });
    let mut cancelled = false;
    let mut deadline = Instant::now() + Duration::from_secs(190);
    let mut ticker = tokio::time::interval(Duration::from_millis(50));
    loop {
        let response = read_frame::<Response>(&mut reader, RESPONSE_LIMIT);
        tokio::pin!(response);
        let response = loop {
            tokio::select! { biased;
                _ = ticker.tick() => {
                    if Instant::now() >= deadline { return Err(unknown()); }
                    if !cancelled && control.check().is_err() {
                        cancelled = true; deadline = Instant::now() + Duration::from_secs(10);
                        write_frame(&mut writer, &Request { version: 1, request: Operation::Cancel {} }, REQUEST_LIMIT).await.map_err(|_| unknown())?;
                    }
                }
                result = &mut response => break result.map_err(|_| unknown())?,
            }
        };
        if response.version != 1 {
            return Err(unknown());
        }
        match response.response {
            Reply::Accepted { job } | Reply::Progress { job } if print => progress(&job),
            Reply::Final { job } if print && job.finished => return Ok(Some(Reply::Final { job })),
            Reply::Error { error } => return Err(error),
            response @ (Reply::Runtime { .. } | Reply::Connection { .. }) if !print => {
                return Ok(Some(response));
            }
            _ => return Err(unknown()),
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::{Error, Result, security};
    use std::{
        fs::{File, OpenOptions},
        os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt},
        path::PathBuf,
    };
    pub type ServerStream = tokio::net::UnixStream;
    pub type ClientStream = tokio::net::UnixStream;
    #[derive(Clone)]
    pub struct Endpoint {
        pub directory: PathBuf,
    }
    impl Endpoint {
        pub fn current() -> Result<Self> {
            Ok(Self {
                directory: PathBuf::from(format!("/tmp/openlabel-{}", uid())),
            })
        }
    }
    fn uid() -> u32 {
        unsafe { libc::geteuid() }
    } // No arguments or owned resources.
    fn directory(endpoint: &Endpoint) -> Result<bool> {
        match std::fs::symlink_metadata(&endpoint.directory) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Ok(meta) if meta.is_dir() && meta.uid() == uid() && meta.mode() & 0o777 == 0o700 => {
                Ok(true)
            }
            _ => Err(Error::localized("ipc_security", "err.ipcDirectory", &[])),
        }
    }
    fn lock(endpoint: &Endpoint, create: bool) -> Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(create)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(endpoint.directory.join("host.lock"))
            .map_err(|e| security(e.to_string()))?;
        let meta = file.metadata().map_err(|e| security(e.to_string()))?;
        if !meta.is_file() || meta.uid() != uid() || meta.mode() & 0o077 != 0 {
            return Err(Error::localized("ipc_security", "err.ipcLock", &[]));
        }
        Ok(file)
    }
    fn acquire(file: &File) -> Result<()> {
        fs2::FileExt::try_lock_exclusive(file)
            .map_err(|_| Error::localized("ipc_busy", "err.appRunning", &[]))
    }
    fn socket(endpoint: &Endpoint) -> Result<bool> {
        match std::fs::symlink_metadata(endpoint.directory.join("app.sock")) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Ok(meta) if meta.file_type().is_socket() && meta.uid() == uid() => Ok(true),
            _ => Err(Error::localized("ipc_security", "err.ipcSocket", &[])),
        }
    }
    pub struct Listener {
        listener: tokio::net::UnixListener,
        _lock: File,
    }
    impl Listener {
        pub fn bind(endpoint: &Endpoint) -> Result<Self> {
            if !directory(endpoint)? {
                match std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&endpoint.directory)
                {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(security(e.to_string())),
                }
            }
            directory(endpoint)?;
            let lock = lock(endpoint, true)?;
            acquire(&lock)?;
            if socket(endpoint)? {
                std::fs::remove_file(endpoint.directory.join("app.sock"))
                    .map_err(|e| security(e.to_string()))?;
            }
            let listener = tokio::net::UnixListener::bind(endpoint.directory.join("app.sock"))
                .map_err(|e| security(e.to_string()))?;
            Ok(Self {
                listener,
                _lock: lock,
            })
        }
        pub async fn accept(&mut self) -> std::io::Result<ServerStream> {
            self.listener.accept().await.map(|(stream, _)| stream)
        }
    }
    pub async fn connect(endpoint: &Endpoint) -> Result<Option<ClientStream>> {
        if !directory(endpoint)? {
            return Ok(None);
        }
        let socket_exists = socket(endpoint)?;
        let lock_exists = match std::fs::symlink_metadata(endpoint.directory.join("host.lock")) {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(security(e.to_string())),
        };
        if !lock_exists {
            return if socket_exists {
                Err(Error::localized("ipc_security", "err.ipcLockMissing", &[]))
            } else {
                Ok(None)
            };
        }
        let lock = lock(endpoint, false)?;
        match tokio::net::UnixStream::connect(endpoint.directory.join("app.sock")).await {
            Ok(stream) => Ok(Some(stream)),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                acquire(&lock)?;
                Ok(None)
            }
            Err(e) => Err(security(e.to_string())),
        }
    }
    pub fn authorize(stream: &ServerStream) -> Result<()> {
        authorize_uid(
            stream
                .peer_cred()
                .map_err(|e| security(e.to_string()))?
                .uid(),
        )
    }
    fn authorize_uid(peer: u32) -> Result<()> {
        if peer != uid() {
            return Err(Error::localized("ipc_security", "err.ipcUser", &[]));
        }
        Ok(())
    }
    pub fn authorize_server(stream: &ClientStream) -> Result<()> {
        authorize(stream)
    }
    #[cfg(test)]
    #[test]
    fn foreign_peer_identity_fails_closed() {
        assert!(authorize_uid(uid()).is_ok());
        assert!(authorize_uid(uid().wrapping_add(1)).is_err());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    fn endpoint(directory: &std::path::Path) -> Endpoint {
        Endpoint {
            directory: directory.join("ipc"),
        }
    }
    fn request(directory: &std::path::Path) -> PrintRequest {
        let path = directory.join("tiny.svg");
        fs::write(&path, b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40mm\" height=\"10mm\"><rect width=\"4\" height=\"4\"/></svg>").unwrap();
        let label = crate::label::Request {
            path: Some(path),
            ..Default::default()
        };
        let preview = crate::label::preview(&label).unwrap();
        PrintRequest {
            label,
            device: "00000000-0000-0000-0000-000000000007".into(),
            model: "M110".into(),
            copies: 1,
            expect_sha256: preview.sha256,
            expect_input_sha256: preview.input_sha256,
        }
    }
    #[tokio::test]
    async fn endpoint_absence_ownership_stale_socket_and_duplicate_host() {
        use std::os::unix::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let endpoint = endpoint(dir.path());
        let control = Control::default();
        assert!(
            call(&endpoint, Operation::Runtime {}, &control, |_| {})
                .await
                .unwrap()
                .is_none()
        );
        assert!(!endpoint.directory.exists());
        fs::create_dir(&endpoint.directory).unwrap();
        fs::set_permissions(&endpoint.directory, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            call(&endpoint, Operation::Runtime {}, &control, |_| {})
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(fs::read_dir(&endpoint.directory).unwrap().count(), 0);
        let orphan = Endpoint {
            directory: dir.path().join("orphan"),
        };
        fs::create_dir(&orphan.directory).unwrap();
        fs::set_permissions(&orphan.directory, fs::Permissions::from_mode(0o700)).unwrap();
        let orphan_listener =
            tokio::net::UnixListener::bind(orphan.directory.join("app.sock")).unwrap();
        assert_eq!(
            call(&orphan, Operation::Runtime {}, &control, |_| {})
                .await
                .unwrap_err()
                .code,
            "ipc_security"
        );
        assert!(!orphan.directory.join("host.lock").exists());
        assert!(orphan.directory.join("app.sock").exists());
        assert_eq!(fs::read_dir(&orphan.directory).unwrap().count(), 1);
        drop(orphan_listener);
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(endpoint.directory.join("host.lock"))
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&lock).unwrap();
        assert_eq!(
            call(&endpoint, Operation::Runtime {}, &control, |_| {})
                .await
                .unwrap_err()
                .code,
            "ipc_busy"
        );
        drop(lock);
        let stale = tokio::net::UnixListener::bind(endpoint.directory.join("app.sock")).unwrap();
        drop(stale);
        assert!(
            call(&endpoint, Operation::Runtime {}, &control, |_| {})
                .await
                .unwrap()
                .is_none()
        );
        assert!(endpoint.directory.join("app.sock").exists());
        let jobs = Arc::new(Jobs::persistent());
        let host = Host::start(&endpoint, jobs.clone()).await.unwrap();
        assert!(Host::start(&endpoint, jobs.clone()).await.is_err());
        assert!(matches!(
            call(&endpoint, Operation::Runtime {}, &control, |_| {})
                .await
                .unwrap(),
            Some(Reply::Runtime { .. })
        ));
        host.stop_admission();
        jobs.shutdown().await;
        host.stopped().await;
        fs::set_permissions(&endpoint.directory, fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(
            call(&endpoint, Operation::Runtime {}, &control, |_| {})
                .await
                .unwrap_err()
                .code,
            "ipc_security"
        );
        let link = Endpoint {
            directory: dir.path().join("symlink"),
        };
        std::os::unix::fs::symlink(&endpoint.directory, &link.directory).unwrap();
        assert!(
            call(&link, Operation::Runtime {}, &control, |_| {})
                .await
                .is_err()
        );
        let unexpected = Endpoint {
            directory: dir.path().join("unexpected"),
        };
        fs::create_dir(&unexpected.directory).unwrap();
        fs::set_permissions(&unexpected.directory, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(unexpected.directory.join("app.sock"), b"do not delete").unwrap();
        assert!(
            Host::start(&unexpected, Arc::new(Jobs::persistent()))
                .await
                .is_err()
        );
        assert_eq!(
            fs::read(unexpected.directory.join("app.sock")).unwrap(),
            b"do not delete"
        );
        for symlink in [false, true] {
            let unsafe_lock = Endpoint {
                directory: dir.path().join(if symlink {
                    "linked-lock"
                } else {
                    "public-lock"
                }),
            };
            fs::create_dir(&unsafe_lock.directory).unwrap();
            fs::set_permissions(&unsafe_lock.directory, fs::Permissions::from_mode(0o700)).unwrap();
            let lock_path = unsafe_lock.directory.join("host.lock");
            if symlink {
                std::os::unix::fs::symlink(unexpected.directory.join("app.sock"), &lock_path)
                    .unwrap();
            } else {
                fs::write(&lock_path, b"private lock required").unwrap();
                fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o644)).unwrap();
            }
            assert!(
                call(&unsafe_lock, Operation::Runtime {}, &control, |_| {})
                    .await
                    .is_err()
            );
            assert!(
                Host::start(&unsafe_lock, Arc::new(Jobs::persistent()))
                    .await
                    .is_err()
            );
            assert!(!unsafe_lock.directory.join("app.sock").exists());
        }
    }
    #[tokio::test]
    async fn framing_bounds_versions_and_finite_overrides_precede_admission() {
        for operation in ["runtime", "cancel"] {
            let valid = format!(r#"{{"version":1,"request":{{"operation":"{operation}"}}}}"#);
            assert!(serde_json::from_str::<Request>(&valid).is_ok());
            let invalid =
                format!(r#"{{"version":1,"request":{{"operation":"{operation}","extra":true}}}}"#);
            assert!(serde_json::from_str::<Request>(&invalid).is_err());
        }
        for (size, limit) in [
            (0, REQUEST_LIMIT),
            (REQUEST_LIMIT + 1, REQUEST_LIMIT),
            (RESPONSE_LIMIT + 1, RESPONSE_LIMIT),
        ] {
            let mut bytes = (size as u32).to_be_bytes().as_slice().to_vec();
            assert!(
                read_frame::<serde_json::Value>(&mut bytes.as_slice(), limit)
                    .await
                    .is_err()
            );
            bytes.clear();
        }
        let dir = tempfile::tempdir().unwrap();
        let endpoint = endpoint(dir.path());
        let fake = Arc::new(crate::ble::FakeConnection::default());
        let jobs = Arc::new(Jobs::synthetic(dir.path().join("lock"), fake.clone()));
        let host = Host::start(&endpoint, jobs.clone()).await.unwrap();
        let mut oversized = tokio::net::UnixStream::connect(endpoint.directory.join("app.sock"))
            .await
            .unwrap();
        oversized
            .write_u32((REQUEST_LIMIT + 1) as u32)
            .await
            .unwrap();
        assert!(matches!(
            read_frame::<Response>(&mut oversized, RESPONSE_LIMIT)
                .await
                .unwrap()
                .response,
            Reply::Error { .. }
        ));
        let mut too_large = request(dir.path());
        too_large.device = "x".repeat(REQUEST_LIMIT);
        assert_eq!(
            call(
                &endpoint,
                Operation::Print {
                    request: Box::new(too_large)
                },
                &Control::default(),
                |_| {}
            )
            .await
            .unwrap_err()
            .code,
            "ipc_error"
        );
        assert!(jobs.status().is_none());
        assert_eq!(fake.writes.load(std::sync::atomic::Ordering::SeqCst), 0);
        for bytes in [
            br#"{"version":2,"request":{"operation":"runtime"}}"#.as_slice(),
            br#"{"version":1,"request":{"operation":"shell"}}"#,
            br#"{"version":1,"extra":true,"request":{"operation":"runtime"}}"#,
            br#"{"version":1,"request":{"operation":"runtime","extra":true}}"#,
            br#"{"version":1,"request":{"operation":"cancel","extra":true}}"#,
        ] {
            let mut stream = tokio::net::UnixStream::connect(endpoint.directory.join("app.sock"))
                .await
                .unwrap();
            stream.write_u32(bytes.len() as u32).await.unwrap();
            stream.write_all(bytes).await.unwrap();
            assert!(matches!(
                read_frame::<Response>(&mut stream, RESPONSE_LIMIT)
                    .await
                    .unwrap()
                    .response,
                Reply::Error { .. }
            ));
            assert!(jobs.status().is_none());
        }
        let mut invalid = request(dir.path());
        invalid.label.overrides.scale_percent = Some(f64::NAN);
        assert_eq!(
            call(
                &endpoint,
                Operation::Print {
                    request: Box::new(invalid)
                },
                &Control::default(),
                |_| {}
            )
            .await
            .unwrap_err()
            .code,
            "invalid_settings"
        );
        assert!(jobs.status().is_none());
        host.stop_admission();
        jobs.shutdown().await;
        host.stopped().await;
    }
    #[tokio::test]
    async fn shared_jobs_stream_finished_results_and_preserve_large_error_details() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let endpoint = endpoint(dir.path());
        let fake = Arc::new(crate::ble::FakeConnection::default());
        let jobs = Arc::new(Jobs::synthetic(dir.path().join("lock"), fake.clone()));
        let host = Host::start(&endpoint, jobs.clone()).await.unwrap();
        let request = request(dir.path());
        let result = call(
            &endpoint,
            Operation::Connect {
                device: request.device.clone(),
                model: "M110".into(),
            },
            &Control::default(),
            |_| {},
        )
        .await
        .unwrap();
        assert!(
            matches!(result, Some(Reply::Connection { connection }) if connection.state == "connected")
        );
        let mut progress = 0;
        let result = call(
            &endpoint,
            Operation::Print {
                request: Box::new(request.clone()),
            },
            &Control::default(),
            |_| progress += 1,
        )
        .await
        .unwrap();
        assert!(
            matches!(result, Some(Reply::Final { job }) if job.finished && job.state == "completed")
        );
        assert!(progress > 0);
        assert_eq!(fake.connects.load(Ordering::SeqCst), 1);
        assert_eq!(fake.cleanups.load(Ordering::SeqCst), 0);
        let path = request.label.path.as_ref().unwrap();
        fs::write(path, format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40mm\" height=\"10mm\" {}=\"x\"/>", "a".repeat(70 * 1024))).unwrap();
        let result = call(
            &endpoint,
            Operation::Print {
                request: Box::new(request),
            },
            &Control::default(),
            |_| {},
        )
        .await
        .unwrap();
        assert!(
            matches!(result, Some(Reply::Final { job }) if job.error.as_ref().is_some_and(|error| error.code == "invalid_label" && error.detail.len() > 70 * 1024 && error.message.as_ref().is_some_and(|message| message.key == "err.svgAttribute" && message.params["name"].len() == 70 * 1024)))
        );
        let result = call(
            &endpoint,
            Operation::Runtime {},
            &Control::default(),
            |_| {},
        )
        .await
        .unwrap();
        assert!(
            matches!(result, Some(Reply::Runtime { runtime }) if runtime.job.as_ref().unwrap().error.as_ref().is_some_and(|error| error.detail.len() > 70 * 1024 && error.message.as_ref().is_some_and(|message| message.key == "err.svgAttribute" && message.params["name"].len() == 70 * 1024)))
        );
        host.stop_admission();
        jobs.shutdown().await;
        host.stopped().await;
    }
    #[tokio::test]
    async fn nonreading_client_receives_bounded_states_without_cancelling_full_send() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = endpoint(dir.path());
        let fake = Arc::new(crate::ble::FakeConnection::default());
        let jobs = Arc::new(Jobs::synthetic(dir.path().join("lock"), fake.clone()));
        let mut request = request(dir.path());
        request.label.overrides.width_mm = Some(50.0);
        request.label.overrides.height_mm = Some(80.0);
        let preview = crate::label::preview(&request.label).unwrap();
        request.expect_sha256 = preview.sha256;
        request.expect_input_sha256 = preview.input_sha256;
        let host = Host::start(&endpoint, jobs.clone()).await.unwrap();
        let mut stream = tokio::net::UnixStream::connect(endpoint.directory.join("app.sock"))
            .await
            .unwrap();
        write_frame(
            &mut stream,
            &Request {
                version: 1,
                request: Operation::Print {
                    request: Box::new(request),
                },
            },
            REQUEST_LIMIT,
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let mut states = vec![];
        let final_job = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match read_frame::<Response>(&mut stream, RESPONSE_LIMIT)
                    .await
                    .unwrap()
                    .response
                {
                    Reply::Accepted { job } | Reply::Progress { job } => states.push(job.state),
                    Reply::Final { job } => break job,
                    _ => panic!("unexpected reply"),
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(final_job.state, "completed");
        assert!(final_job.finished);
        assert_eq!(final_job.sent_bytes, 30747);
        assert!(states.len() <= 3);
        assert_eq!(states.first().unwrap(), "preparing");
        assert!(states.windows(2).all(|pair| pair[0] != pair[1]));
        assert_eq!(fake.cleanups.load(std::sync::atomic::Ordering::SeqCst), 0);
        host.stop_admission();
        jobs.shutdown().await;
        host.stopped().await;
    }
    #[tokio::test]
    async fn client_cancel_interrupts_delayed_connect_and_releases_lock() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = endpoint(dir.path());
        let path = dir.path().join("lock");
        let fake = Arc::new(crate::ble::FakeConnection {
            connect_delay: Duration::from_secs(20),
            ..Default::default()
        });
        let jobs = Arc::new(Jobs::synthetic(path.clone(), fake.clone()));
        let host = Host::start(&endpoint, jobs.clone()).await.unwrap();
        let control = Control::default();
        let cancel = control.clone();
        let observed = fake.clone();
        let cancellation = tokio::spawn(async move {
            while observed.connects.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
            cancel.cancel();
        });
        let result = call(
            &endpoint,
            Operation::Connect {
                device: "00000000-0000-0000-0000-000000000007".into(),
                model: "M110".into(),
            },
            &control,
            |_| {},
        )
        .await;
        cancellation.await.unwrap();
        assert_eq!(result.unwrap_err().code, "cancelled");
        assert_eq!(fake.writes.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(fake.cleanups.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(!jobs.runtime().busy);
        assert!(crate::operation::PrintLock::at(&path).is_ok());
        host.stop_admission();
        jobs.shutdown().await;
        host.stopped().await;
    }
    #[tokio::test]
    async fn lost_acceptance_eof_and_fragmented_cancel_cleanup_only_owned_job() {
        use std::sync::atomic::Ordering;
        for fragmented in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let endpoint = endpoint(dir.path());
            let fake = Arc::new(crate::ble::FakeConnection::default());
            let jobs = Arc::new(Jobs::synthetic(dir.path().join("lock"), fake.clone()));
            let host = Host::start(&endpoint, jobs.clone()).await.unwrap();
            let mut stream = tokio::net::UnixStream::connect(endpoint.directory.join("app.sock"))
                .await
                .unwrap();
            write_frame(
                &mut stream,
                &Request {
                    version: 1,
                    request: Operation::Print {
                        request: Box::new(request(dir.path())),
                    },
                },
                REQUEST_LIMIT,
            )
            .await
            .unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                while jobs.status().is_none() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            if fragmented {
                let accepted = read_frame::<Response>(&mut stream, RESPONSE_LIMIT)
                    .await
                    .unwrap();
                assert!(matches!(accepted.response, Reply::Accepted { .. }));
                let bytes = serde_json::to_vec(&Request {
                    version: 1,
                    request: Operation::Cancel {},
                })
                .unwrap();
                let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
                frame.extend(bytes);
                for part in frame.chunks(3) {
                    stream.write_all(part).await.unwrap();
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
            drop(stream);
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if jobs.status().is_some_and(|job| job.finished) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(jobs.status().unwrap().state, "cancelled");
            assert!(fake.connects.load(Ordering::SeqCst) <= 1);
            assert!(fake.cleanups.load(Ordering::SeqCst) <= 1);
            assert_eq!(jobs.runtime().connection.state, "disconnected");
            host.stop_admission();
            jobs.shutdown().await;
            host.stopped().await;
        }
    }
    #[tokio::test(start_paused = true)]
    async fn first_frame_deadline_and_four_handler_cap_do_not_admit_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = endpoint(dir.path());
        let jobs = Arc::new(Jobs::persistent());
        let host = Host::start(&endpoint, jobs.clone()).await.unwrap();
        let mut streams = vec![];
        for _ in 0..4 {
            streams.push(
                tokio::net::UnixStream::connect(endpoint.directory.join("app.sock"))
                    .await
                    .unwrap(),
            );
            tokio::task::yield_now().await;
        }
        let mut fifth = tokio::net::UnixStream::connect(endpoint.directory.join("app.sock"))
            .await
            .unwrap();
        assert!(
            matches!(read_frame::<Response>(&mut fifth, RESPONSE_LIMIT).await.unwrap().response, Reply::Error { error } if error.code == "ipc_busy")
        );
        tokio::time::advance(Duration::from_secs(5)).await;
        for stream in &mut streams {
            assert!(matches!(
                read_frame::<Response>(stream, RESPONSE_LIMIT)
                    .await
                    .unwrap()
                    .response,
                Reply::Error { .. }
            ));
        }
        assert!(jobs.status().is_none());
        host.stop_admission();
        jobs.shutdown().await;
        host.stopped().await;
    }
    #[tokio::test]
    async fn dropped_unary_connect_cancels_control_and_waits_for_cleanup() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let endpoint = endpoint(dir.path());
        let fake = Arc::new(crate::ble::FakeConnection {
            connect_delay: Duration::from_secs(20),
            ..Default::default()
        });
        let jobs = Arc::new(Jobs::synthetic(dir.path().join("lock"), fake.clone()));
        let host = Host::start(&endpoint, jobs.clone()).await.unwrap();
        let mut stream = tokio::net::UnixStream::connect(endpoint.directory.join("app.sock"))
            .await
            .unwrap();
        write_frame(
            &mut stream,
            &Request {
                version: 1,
                request: Operation::Connect {
                    device: "00000000-0000-0000-0000-000000000007".into(),
                    model: "M110".into(),
                },
            },
            REQUEST_LIMIT,
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while fake.connects.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(stream);
        tokio::time::timeout(Duration::from_secs(3), async {
            while jobs.runtime().busy {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(fake.cleanups.load(Ordering::SeqCst), 1);
        assert_eq!(fake.writes.load(Ordering::SeqCst), 0);
        assert_eq!(jobs.runtime().connection.state, "disconnected");
        host.stop_admission();
        jobs.shutdown().await;
        host.stopped().await;
    }
}

#[cfg(windows)]
mod platform {
    use super::{Error, Result, security};
    use std::{
        ffi::c_void,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        ptr,
    };
    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, PipeMode, ServerOptions,
    };
    use windows_sys::Win32::{
        Foundation::{
            ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, ERROR_PIPE_BUSY, HANDLE, LocalFree,
        },
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            },
            GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        System::{
            Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId},
            Threading::{
                GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
    };
    pub type ServerStream = NamedPipeServer;
    pub type ClientStream = NamedPipeClient;
    #[derive(Clone)]
    pub struct Endpoint {
        pub name: String,
    }
    fn failure() -> Error {
        security(std::io::Error::last_os_error().to_string())
    }
    struct LocalAllocation(*mut c_void);
    impl Drop for LocalAllocation {
        fn drop(&mut self) {
            // These pointers are returned by the Convert* APIs and require LocalFree, not CloseHandle.
            unsafe {
                LocalFree(self.0);
            }
        }
    }
    fn token_sid(process: HANDLE) -> Result<String> {
        let mut raw = ptr::null_mut();
        // process is a live borrowed handle (possibly the current-process pseudo handle).
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut raw) } == 0 {
            return Err(failure());
        }
        // OpenProcessToken succeeded and returned a real owned token handle.
        let token = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut size = 0;
        // Zero-length query only discovers the required TOKEN_USER buffer length.
        let queried = unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                ptr::null_mut(),
                0,
                &mut size,
            )
        };
        if queried != 0
            || std::io::Error::last_os_error().raw_os_error()
                != Some(ERROR_INSUFFICIENT_BUFFER as i32)
            || size < std::mem::size_of::<TOKEN_USER>() as u32
            || size > 64 * 1024
        {
            return Err(Error::localized("ipc_security", "err.tokenUser", &[]));
        }
        // usize storage supplies pointer alignment required by TOKEN_USER and its embedded SID.
        let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        let capacity = size;
        // The writable buffer is at least capacity bytes and remains alive through SID conversion.
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                capacity,
                &mut size,
            )
        } == 0
            || size > capacity
        {
            return Err(failure());
        }
        // A successful TokenUser query initializes the aligned TOKEN_USER prefix.
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let mut sid = ptr::null_mut();
        // The returned SID is owned by the still-live token buffer; conversion allocates a UTF-16 string.
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) } == 0 {
            return Err(failure());
        }
        let allocation = LocalAllocation(sid.cast());
        let mut length = 0;
        // The successful native conversion returns a NUL-terminated SID string; cap scanning defensively.
        unsafe {
            while length < 256 && *sid.add(length) != 0 {
                length += 1;
            }
            if length == 256 {
                return Err(Error::localized("ipc_security", "err.sidLength", &[]));
            }
            let text = String::from_utf16(std::slice::from_raw_parts(sid, length))
                .map_err(|_| Error::localized("ipc_security", "err.sidString", &[]))?;
            drop(allocation);
            Ok(text)
        }
    }
    fn current_sid() -> Result<String> {
        // GetCurrentProcess returns a borrowed pseudo handle; it must never enter OwnedHandle.
        token_sid(unsafe { GetCurrentProcess() })
    }
    impl Endpoint {
        pub fn current() -> Result<Self> {
            Ok(Self {
                name: format!(r"\\.\pipe\openlabel-{}", current_sid()?),
            })
        }
    }
    fn create(endpoint: &Endpoint, first: bool) -> Result<NamedPipeServer> {
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{})", current_sid()?)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = ptr::null_mut();
        // NUL-terminated SDDL remains live; revision1 returns a LocalFree-owned security descriptor.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(failure());
        }
        let allocation = LocalAllocation(descriptor);
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let mut options = ServerOptions::new();
        options
            .pipe_mode(PipeMode::Byte)
            .reject_remote_clients(true)
            .first_pipe_instance(first);
        // Both attributes and protected same-user DACL remain valid until CreateNamedPipe returns.
        let pipe = unsafe {
            options.create_with_security_attributes_raw(
                &endpoint.name,
                (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
            )
        }
        .map_err(|e| security(e.to_string()));
        drop(allocation);
        pipe
    }
    pub struct Listener {
        server: NamedPipeServer,
        endpoint: Endpoint,
    }
    impl Listener {
        pub fn bind(endpoint: &Endpoint) -> Result<Self> {
            Ok(Self {
                server: create(endpoint, true)?,
                endpoint: endpoint.clone(),
            })
        }
        pub async fn accept(&mut self) -> std::io::Result<ServerStream> {
            self.server.connect().await?;
            // Keep the connected instance alive until its successor exists, preventing a name gap.
            let next = create(&self.endpoint, false).map_err(std::io::Error::other)?;
            Ok(std::mem::replace(&mut self.server, next))
        }
    }
    pub async fn connect(endpoint: &Endpoint) -> Result<Option<ClientStream>> {
        // Tokio's default SECURITY_IDENTIFICATION QoS remains unchanged; no impersonation/elevation.
        match ClientOptions::new().open(&endpoint.name) {
            Ok(stream) => Ok(Some(stream)),
            Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => Ok(None),
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                Err(Error::localized("ipc_busy", "err.ipcConnectionBusy", &[]))
            }
            Err(error) => Err(security(error.to_string())),
        }
    }
    fn authorize_process(pid: u32) -> Result<()> {
        // Limited query access only; a successful OpenProcess returns a real owned handle.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return Err(failure());
        }
        let process = unsafe { OwnedHandle::from_raw_handle(process) };
        if token_sid(process.as_raw_handle())? != current_sid()? {
            return Err(Error::localized("ipc_security", "err.ipcUser", &[]));
        }
        Ok(())
    }
    pub fn authorize(stream: &ServerStream) -> Result<()> {
        let mut pid = 0;
        // Live named-pipe server handle and writable process-id output; no impersonation.
        if unsafe { GetNamedPipeClientProcessId(stream.as_raw_handle(), &mut pid) } == 0 {
            return Err(failure());
        }
        authorize_process(pid)
    }
    pub fn authorize_server(stream: &ClientStream) -> Result<()> {
        let mut pid = 0;
        // Verify the live pipe server's process token before sending any request bytes.
        if unsafe { GetNamedPipeServerProcessId(stream.as_raw_handle(), &mut pid) } == 0 {
            return Err(failure());
        }
        authorize_process(pid)
    }
    #[cfg(test)]
    #[tokio::test]
    async fn isolated_sid_pipe_authorizes_peers_and_rejects_duplicate_host() {
        use super::{Control, Host, Jobs, Operation, Reply, call};
        use std::{
            sync::{Arc, atomic::Ordering},
            time::Duration,
        };
        let endpoint = Endpoint {
            name: format!(
                r"\\.\pipe\openlabel-{}-test-{}",
                current_sid().unwrap(),
                std::process::id()
            ),
        };
        let directory = tempfile::tempdir().unwrap();
        let lock = directory.path().join("lock");
        let fake = Arc::new(crate::ble::FakeConnection {
            connect_delay: Duration::from_secs(20),
            ..Default::default()
        });
        let jobs = Arc::new(Jobs::synthetic(lock.clone(), fake.clone()));
        let host = Host::start(&endpoint, jobs.clone()).await.unwrap();
        assert!(Host::start(&endpoint, jobs.clone()).await.is_err());
        assert!(matches!(
            call(
                &endpoint,
                Operation::Runtime {},
                &Control::default(),
                |_| {}
            )
            .await
            .unwrap(),
            Some(Reply::Runtime { .. })
        ));
        let control = Control::default();
        let cancel = control.clone();
        let observed = fake.clone();
        let cancellation = tokio::spawn(async move {
            while observed.connects.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
            cancel.cancel();
        });
        let result = call(
            &endpoint,
            Operation::Connect {
                device: "synthetic-target".into(),
                model: "M110".into(),
            },
            &control,
            |_| {},
        )
        .await;
        cancellation.await.unwrap();
        assert_eq!(result.unwrap_err().code, "cancelled");
        assert_eq!(fake.writes.load(Ordering::SeqCst), 0);
        assert_eq!(fake.cleanups.load(Ordering::SeqCst), 1);
        assert!(!jobs.runtime().busy);
        assert!(crate::operation::PrintLock::at(&lock).is_ok());
        host.stop_admission();
        jobs.shutdown().await;
        host.stopped().await;
        assert!(connect(&endpoint).await.unwrap().is_none());
        assert!(authorize_process(0).is_err());
    }
}
