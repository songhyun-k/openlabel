use crate::{Error, Result};
use std::{
    fs::{File, OpenOptions},
    future::Future,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{sync::Notify, time::Instant};

#[derive(Clone)]
pub struct Control {
    cancel: Arc<AtomicBool>,
    notify: Arc<Notify>,
    deadline: Instant,
}
impl Default for Control {
    fn default() -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
            deadline: Instant::now() + Duration::from_secs(180),
        }
    }
}
impl Control {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
    pub fn check(&self) -> Result<()> {
        if self.cancel.load(Ordering::SeqCst) {
            Err(Error::localized("cancelled", "err.printCancelled", &[]))
        } else if Instant::now() >= self.deadline {
            Err(Error::localized(
                "transport_timeout",
                "err.operationTimeout",
                &[],
            ))
        } else {
            Ok(())
        }
    }
    async fn bounded<T>(&self, f: impl Future<Output = Result<T>>, limit: Duration) -> Result<T> {
        let notified = self.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        self.check()?;
        tokio::select! {biased;_=&mut notified=>Err(Error::localized("cancelled", "err.operationCancelled", &[])),result=tokio::time::timeout_at(self.deadline.min(Instant::now()+limit),f)=>result.map_err(|_|Error::localized("transport_timeout", "err.transportTimeout", &[]))?}
    }
    pub async fn operation<T>(&self, f: impl Future<Output = Result<T>>) -> Result<T> {
        self.bounded(f, Duration::from_secs(12)).await
    }
    pub async fn sleep(&self, duration: Duration) -> Result<()> {
        self.bounded(
            async {
                tokio::time::sleep(duration).await;
                Ok(())
            },
            duration + Duration::from_secs(1),
        )
        .await
    }
}
pub struct PrintLock(File);
impl PrintLock {
    pub fn at(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| Error::new("job_busy", e.to_string()))?;
        fs2::FileExt::try_lock_exclusive(&file)
            .map_err(|_| Error::localized("job_busy", "err.printLocked", &[]))?;
        Ok(Self(file))
    }
    pub(crate) fn acquire() -> Result<Self> {
        let dir = dirs::data_local_dir()
            .ok_or_else(|| Error::localized("job_busy", "err.dataDirectory", &[]))?
            .join("com.openlabel.shared");
        std::fs::create_dir_all(&dir).map_err(|e| Error::new("job_busy", e.to_string()))?;
        Self::at(&dir.join("print.lock"))
    }
}
impl Drop for PrintLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}
