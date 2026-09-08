use crate::{Error, Result, operation::Control};
#[cfg(target_os = "macos")]
use btleplug::{api::RetrievePeripheralsOptions, platform::PeripheralId};
use btleplug::{
    api::{
        Central, CentralState, CharPropFlags, Characteristic, Manager as _, Peripheral as _,
        PeripheralProperties, ScanFilter, WriteType,
    },
    platform::{Adapter, Manager, Peripheral},
};
use serde::{Deserialize, Serialize};
use std::{future::Future, time::Duration};
use uuid::Uuid;
const SERVICE: Uuid = Uuid::from_u128(0x0000ff00_0000_1000_8000_00805f9b34fb);
const WRITE: Uuid = Uuid::from_u128(0x0000ff02_0000_1000_8000_00805f9b34fb);
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: Option<String>,
    pub rssi: Option<i16>,
    pub advertised_services: Vec<Uuid>,
    pub supported: bool,
    pub model_allowed: bool,
    pub evidence: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Evidence {
    pub device_id: String,
    pub model: String,
    pub service: String,
    pub characteristic: String,
    pub write_type: String,
    pub mtu: u16,
}
pub(crate) fn map_error(e: btleplug::Error) -> Error {
    let code = match e {
        btleplug::Error::PermissionDenied => "permission_required",
        btleplug::Error::DeviceNotFound => "device_not_found",
        btleplug::Error::NotConnected => "disconnected",
        _ => "bluetooth_error",
    };
    Error::localized(code, "err.bluetooth", &[("e", e.to_string())])
}
pub fn choose_write(properties: CharPropFlags) -> Result<WriteType> {
    if properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE) {
        Ok(WriteType::WithoutResponse)
    } else if properties.contains(CharPropFlags::WRITE) {
        Ok(WriteType::WithResponse)
    } else {
        Err(Error::localized(
            "unsupported_device",
            "err.writeProperty",
            &[],
        ))
    }
}
pub fn model_allowed(name: Option<&str>) -> bool {
    let Some(name) = name else { return true };
    let upper = name.to_ascii_uppercase();
    if ["NIIMBOT", "BROTHER", "DYMO"]
        .iter()
        .any(|m| upper.contains(m))
    {
        return false;
    }
    // Advertising names are hints; explicit body-model attestation and GATT are still required.
    for word in upper.split(|c: char| !c.is_ascii_alphanumeric()) {
        if word.len() > 1
            && b"MDP".contains(&word.as_bytes()[0])
            && word.as_bytes()[1].is_ascii_digit()
            && word != "M110"
        {
            return false;
        }
    }
    true
}
async fn adapter(control: &Control) -> Result<Adapter> {
    let manager = control
        .operation(async { Manager::new().await.map_err(map_error) })
        .await?;
    let adapters = control
        .operation(async { manager.adapters().await.map_err(map_error) })
        .await?;
    let adapter = adapters
        .into_iter()
        .next()
        .ok_or_else(|| Error::localized("adapter_off", "err.adapterOn", &[]))?;
    check_adapter_state(&adapter, control).await?;
    Ok(adapter)
}
async fn check_adapter_state(adapter: &Adapter, control: &Control) -> Result<()> {
    match control
        .operation(async { adapter.adapter_state().await.map_err(map_error) })
        .await?
    {
        CentralState::PoweredOn => Ok(()),
        CentralState::PoweredOff => {
            Err(Error::localized("adapter_off", "err.systemBluetooth", &[]))
        }
        _ => Err(Error::localized(
            "permission_required",
            "err.bluetoothPermission",
            &[],
        )),
    }
}
async fn discover(adapter: &Adapter, control: &Control) -> Result<Vec<Peripheral>> {
    let result = async {
        control
            .operation(async {
                adapter
                    .start_scan(ScanFilter::default())
                    .await
                    .map_err(map_error)
            })
            .await?;
        control.sleep(Duration::from_secs(4)).await?;
        control
            .operation(async { adapter.peripherals().await.map_err(map_error) })
            .await
    }
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(2), adapter.stop_scan()).await;
    result
}
pub async fn devices(control: &Control) -> Result<Vec<Device>> {
    let mut devices = Vec::new();
    for p in discover(&adapter(control).await?, control).await? {
        let props = control
            .operation(async { p.properties().await.map_err(map_error) })
            .await?;
        devices.push(candidate(p.id().to_string(), props));
    }
    devices.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(devices)
}
fn candidate(id: String, properties: Option<PeripheralProperties>) -> Device {
    let props = properties.unwrap_or_default();
    Device {
        model_allowed: model_allowed(props.local_name.as_deref()),
        id,
        name: props.local_name,
        rssi: props.rssi,
        advertised_services: props.services,
        supported: false,
        evidence: "Discovery candidate: confirm M110 on the body; FF00/FF02 verification required when printing".into(),
    }
}
pub(crate) struct Connection {
    pub evidence: Evidence,
    transport: ConnectedTransport,
}
enum ConnectedTransport {
    Native(Peripheral, Characteristic, WriteType),
    #[cfg(test)]
    Fake(std::sync::Arc<FakeConnection>),
}
#[cfg(test)]
#[derive(Default)]
pub(crate) struct FakeConnection {
    pub connected: std::sync::atomic::AtomicBool,
    pub connects: std::sync::atomic::AtomicUsize,
    pub writes: std::sync::atomic::AtomicUsize,
    pub cleanups: std::sync::atomic::AtomicUsize,
    pub fail_write: std::sync::atomic::AtomicBool,
    pub connect_delay: Duration,
    pub cleanup_delay: Duration,
    pub probes: std::sync::atomic::AtomicUsize,
    pub probe_delay_ms: std::sync::atomic::AtomicU64,
}
#[cfg(target_os = "macos")]
type TargetId = PeripheralId;
#[cfg(not(target_os = "macos"))]
type TargetId = String;

pub(crate) fn validate_target(id: &str, model: &str) -> Result<TargetId> {
    if model != "M110" || id.trim().is_empty() {
        return Err(Error::localized(
            "unsupported_device",
            "err.confirmM110",
            &[],
        ));
    }
    #[cfg(target_os = "macos")]
    return Uuid::parse_str(id)
        .map(Into::into)
        .map_err(|_| Error::localized("invalid_arguments", "err.macDeviceId", &[]));
    #[cfg(not(target_os = "macos"))]
    Ok(id.into())
}

fn check_name(name: Option<&str>) -> Result<()> {
    if model_allowed(name) {
        Ok(())
    } else {
        Err(Error::localized(
            "unsupported_device",
            "err.modelMismatch",
            &[],
        ))
    }
}

#[cfg(any(target_os = "macos", test))]
fn retrieved<T>(result: btleplug::Result<Vec<T>>) -> Result<Option<Vec<T>>> {
    match result {
        Ok(values) => Ok(Some(values)),
        Err(btleplug::Error::NotSupported(_)) => Ok(None),
        Err(error) => Err(map_error(error)),
    }
}

#[cfg(any(target_os = "macos", test))]
async fn resolve_target<T, Scan: Future<Output = Result<Vec<T>>>>(
    retrieval: impl Future<Output = Result<Option<Vec<T>>>>,
    scan: impl FnOnce() -> Scan,
    matches: impl Fn(&T) -> bool,
) -> Result<T> {
    if let Some(target) = retrieval
        .await?
        .unwrap_or_default()
        .into_iter()
        .find(&matches)
    {
        return Ok(target);
    }
    scan()
        .await?
        .into_iter()
        .find(matches)
        .ok_or_else(missing_target)
}

fn missing_target() -> Error {
    Error::localized("device_not_found", "err.deviceMissing", &[])
}

// Cleanup also runs when admission fails after a connection attempt.
async fn admit<T>(
    operation: impl Future<Output = Result<(T, Option<String>)>>,
    cleanup: impl Future<Output = Result<()>>,
) -> Result<T> {
    let result = operation.await.and_then(|(value, name)| {
        check_name(name.as_deref())?;
        Ok(value)
    });
    if result.is_err() {
        let _ = cleanup.await;
    }
    result
}

async fn disconnect(peripheral: &Peripheral) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(2), peripheral.disconnect())
        .await
        .map_err(|_| Error::localized("transport_timeout", "err.cleanupTimeout", &[]))?
        .map_err(map_error)
}

#[cfg(target_os = "macos")]
async fn retrieve(
    adapter: &Adapter,
    id: &PeripheralId,
    control: &Control,
) -> Result<Option<Vec<Peripheral>>> {
    control
        .operation(async {
            retrieved(
                adapter
                    .retrieve_peripherals(RetrievePeripheralsOptions {
                        identifiers: Some(vec![id.clone()]),
                        services: None,
                    })
                    .await,
            )
        })
        .await
}

#[cfg(any(target_os = "macos", test))]
type NameObserver<T> = std::sync::OnceLock<tokio::sync::watch::Receiver<Option<Result<T>>>>;

#[cfg(any(target_os = "macos", test))]
async fn name_observer<T: Clone + Send + Sync + 'static>(
    observer: &NameObserver<T>,
    control: &Control,
    factory: impl Future<Output = Result<T>> + Send + 'static,
) -> Result<T> {
    control.check()?;
    let mut receiver = observer
        .get_or_init(|| {
            let (sender, receiver) = tokio::sync::watch::channel(None);
            // ponytail: btleplug cannot stop its backend thread; one attempt per process,
            // including failed initialization, until the backend supports shutdown.
            tokio::spawn(async move {
                let result = tokio::time::timeout(Duration::from_secs(12), factory)
                    .await
                    .unwrap_or_else(|_| {
                        Err(Error::localized(
                            "transport_timeout",
                            "err.observerTimeout",
                            &[],
                        ))
                    })
                    .map_err(|error| error.with_context("err.observerRestart"));
                sender.send_replace(Some(result));
            });
            receiver
        })
        .clone();
    control
        .operation(async move {
            loop {
                if let Some(result) = receiver.borrow().clone() {
                    return result;
                }
                receiver
                    .changed()
                    .await
                    .map_err(|_| Error::localized("bluetooth_error", "err.observerStopped", &[]))?;
            }
        })
        .await
}

#[cfg(any(target_os = "macos", test))]
async fn check_observed_name<T, Name: Future<Output = Result<Option<String>>>>(
    retrieval: impl Future<Output = Result<Option<Vec<T>>>>,
    matches: impl Fn(&T) -> bool,
    name: impl FnOnce(T) -> Name,
) -> Result<()> {
    if let Some(peripherals) = retrieval.await? {
        let peripheral = peripherals
            .into_iter()
            .find(matches)
            .ok_or_else(missing_target)?;
        check_name(name(peripheral).await?.as_deref())?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
async fn check_native_name(id: &PeripheralId, control: &Control) -> Result<()> {
    static OBSERVER: NameObserver<Adapter> = std::sync::OnceLock::new();
    let observer = name_observer(&OBSERVER, control, async {
        let manager = Manager::new().await.map_err(map_error)?;
        manager
            .adapters()
            .await
            .map_err(map_error)?
            .into_iter()
            .next()
            .ok_or_else(|| Error::localized("adapter_off", "err.adapterMissing", &[]))
    })
    .await?;
    check_adapter_state(&observer, control).await?;
    check_observed_name(
        retrieve(&observer, id, control),
        |p| p.id() == *id,
        |p| async move {
            control
                .operation(async { p.properties().await.map_err(map_error) })
                .await
                .map(|properties| properties.and_then(|p| p.local_name))
        },
    )
    .await
}

pub(crate) async fn connect(id: &str, model: &str, control: &Control) -> Result<Connection> {
    let target = validate_target(id, model)?;
    let adapter = adapter(control).await?;
    #[cfg(target_os = "macos")]
    let p = resolve_target(
        retrieve(&adapter, &target, control),
        || discover(&adapter, control),
        |p| p.id() == target,
    )
    .await?;
    #[cfg(not(target_os = "macos"))]
    let p = discover(&adapter, control)
        .await?
        .into_iter()
        .find(|p| p.id().to_string() == target)
        .ok_or_else(missing_target)?;
    let (characteristic, write_type, evidence) = admit(
        async {
            let props = control
                .operation(async { p.properties().await.map_err(map_error) })
                .await?;
            check_name(props.as_ref().and_then(|p| p.local_name.as_deref()))?;
            control
                .operation(async { p.connect().await.map_err(map_error) })
                .await?;
            control
                .operation(async { p.discover_services().await.map_err(map_error) })
                .await?;
            #[cfg(target_os = "macos")]
            check_native_name(&target, control).await?;
            let props = control
                .operation(async { p.properties().await.map_err(map_error) })
                .await?;
            let characteristic = p
                .characteristics()
                .into_iter()
                .find(|c| c.service_uuid == SERVICE && c.uuid == WRITE)
                .ok_or_else(|| Error::localized("unsupported_device", "err.gattMissing", &[]))?;
            let write_type = choose_write(characteristic.properties)?;
            if p.mtu() < 131 {
                return Err(Error::localized("unsupported_device", "err.mtuSmall", &[]));
            }
            let evidence = Evidence {
                device_id: id.into(),
                model: model.into(),
                service: SERVICE.to_string(),
                characteristic: WRITE.to_string(),
                write_type: match write_type {
                    WriteType::WithoutResponse => "without_response",
                    WriteType::WithResponse => "with_response",
                }
                .into(),
                mtu: p.mtu(),
            };
            Ok((
                (characteristic, write_type, evidence),
                props.and_then(|p| p.local_name),
            ))
        },
        disconnect(&p),
    )
    .await?;
    Ok(Connection {
        transport: ConnectedTransport::Native(p, characteristic, write_type),
        evidence,
    })
}
impl Connection {
    pub async fn is_connected(&self) -> Result<bool> {
        match &self.transport {
            ConnectedTransport::Native(p, _, _) => p.is_connected().await.map_err(map_error),
            #[cfg(test)]
            ConnectedTransport::Fake(fake) => {
                fake.probes
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(
                    fake.probe_delay_ms
                        .load(std::sync::atomic::Ordering::SeqCst),
                ))
                .await;
                Ok(fake.connected.load(std::sync::atomic::Ordering::SeqCst))
            }
        }
    }
    pub async fn write(&self, bytes: &[u8]) -> Result<()> {
        match &self.transport {
            ConnectedTransport::Native(p, characteristic, write_type) => p
                .write(characteristic, bytes, *write_type)
                .await
                .map_err(map_error),
            #[cfg(test)]
            ConnectedTransport::Fake(fake) => {
                use std::sync::atomic::Ordering;
                fake.writes.fetch_add(1, Ordering::SeqCst);
                if fake.fail_write.load(Ordering::SeqCst) {
                    Err(Error::new("disconnected", "synthetic write failure"))
                } else {
                    Ok(())
                }
            }
        }
    }
    pub async fn cleanup(&self) -> Result<()> {
        match &self.transport {
            ConnectedTransport::Native(p, _, _) => disconnect(p).await,
            #[cfg(test)]
            ConnectedTransport::Fake(fake) => {
                use std::sync::atomic::Ordering;
                fake.cleanups.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(fake.cleanup_delay).await;
                fake.connected.store(false, Ordering::SeqCst);
                Ok(())
            }
        }
    }
    #[cfg(test)]
    pub(crate) async fn fake(
        id: &str,
        fake: std::sync::Arc<FakeConnection>,
        control: &Control,
    ) -> Result<Self> {
        use std::sync::atomic::Ordering;
        fake.connects.fetch_add(1, Ordering::SeqCst);
        let connection = Self {
            evidence: Evidence {
                device_id: id.into(),
                model: "M110".into(),
                service: SERVICE.to_string(),
                characteristic: WRITE.to_string(),
                write_type: "without_response".into(),
                mtu: 131,
            },
            transport: ConnectedTransport::Fake(fake.clone()),
        };
        if let Err(error) = control.sleep(fake.connect_delay).await {
            let _ = tokio::time::timeout(Duration::from_secs(2), connection.cleanup()).await;
            return Err(error);
        }
        fake.connected.store(true, Ordering::SeqCst);
        Ok(connection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn name_observer_initializes_once_across_cancelled_and_concurrent_waiters() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let observer: NameObserver<i32> = NameObserver::new();
        let control = Control::default();
        control.cancel();
        assert_eq!(
            name_observer(&observer, &control, async {
                panic!("already cancelled: no factory");
            })
            .await
            .unwrap_err()
            .code,
            "cancelled"
        );
        assert!(observer.get().is_none());
        for outcome in ["success", "failure", "timeout", "panic"] {
            let observer = Arc::new(NameObserver::new());
            let attempts = Arc::new(AtomicUsize::new(0));
            let control = Control::default();
            let first = tokio::spawn({
                let observer = observer.clone();
                let attempts = attempts.clone();
                let control = control.clone();
                async move {
                    name_observer(&observer, &control, async move {
                        attempts.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        match outcome {
                            "success" => Ok(7),
                            "failure" => Err(Error::new("permission_required", "synthetic")),
                            "timeout" => std::future::pending().await,
                            _ => panic!("synthetic initializer panic"),
                        }
                    })
                    .await
                }
            });
            while attempts.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
            let second = tokio::spawn({
                let observer = observer.clone();
                async move {
                    name_observer(&observer, &Control::default(), async {
                        panic!("second factory must not run");
                    })
                    .await
                }
            });
            control.cancel();
            assert_eq!(first.await.unwrap().unwrap_err().code, "cancelled");
            let concurrent = second.await.unwrap();
            let reused = name_observer(&observer, &Control::default(), async {
                panic!("cached factory must not run");
            })
            .await;
            if outcome == "success" {
                assert_eq!(concurrent.unwrap(), 7);
                assert_eq!(reused.unwrap(), 7);
            } else {
                let expected = match outcome {
                    "failure" => "permission_required",
                    "timeout" => "transport_timeout",
                    _ => "bluetooth_error",
                };
                assert_eq!(concurrent.unwrap_err().code, expected);
                let error = reused.unwrap_err();
                assert_eq!(error.code, expected);
                assert!(error.detail.contains("Restart"));
            }
            assert_eq!(attempts.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn independent_native_name_cannot_be_hidden_by_transport_advertisement() {
        use std::{cell::Cell, future::ready};
        // Transport properties retain the advertisement. Observer retrieval has its own source.
        for (advertised, native, expected) in [
            (Some("M110"), Some("M220"), Some("unsupported_device")),
            (Some("M110"), Some("M110"), None),
            (Some("M110"), None, None),
            (Some("M220"), Some("M110"), Some("unsupported_device")),
        ] {
            let observed = Cell::new(0);
            let cleaned = Cell::new(false);
            let admitted = admit(
                async {
                    check_observed_name(
                        ready(retrieved(Ok(vec![(7, native)]))),
                        |p| p.0 == 7,
                        |p| {
                            observed.set(observed.get() + 1);
                            ready(Ok(p.1.map(str::to_owned)))
                        },
                    )
                    .await?;
                    Ok(((), advertised.map(str::to_owned)))
                },
                async {
                    cleaned.set(true);
                    Ok(())
                },
            )
            .await;
            assert_eq!(admitted.err().map(|e| e.code), expected.map(str::to_owned));
            assert_eq!(observed.get(), 1);
            assert_eq!(cleaned.get(), expected.is_some());
            // Only admitted values could expose the original connection's writer.
        }
        for (retrieval, expected) in [
            (Ok(Some(vec![8])), Some("device_not_found")),
            (Ok(Some(vec![])), Some("device_not_found")),
            (
                retrieved(Err(btleplug::Error::NotSupported("synthetic".into()))),
                None,
            ),
            (
                Err(Error::new("bluetooth_error", "synthetic")),
                Some("bluetooth_error"),
            ),
            (Err(Error::new("cancelled", "synthetic")), Some("cancelled")),
            (
                Err(Error::new("transport_timeout", "synthetic")),
                Some("transport_timeout"),
            ),
        ] {
            let cleaned = Cell::new(false);
            let result = admit(
                async {
                    check_observed_name(
                        ready(retrieval),
                        |id| *id == 7,
                        |_| async {
                            panic!("no exact observer result: no properties call");
                        },
                    )
                    .await?;
                    Ok(((), Some("M110".into())))
                },
                async {
                    cleaned.set(true);
                    Ok(())
                },
            )
            .await;
            assert_eq!(result.err().map(|e| e.code), expected.map(str::to_owned));
            assert_eq!(cleaned.get(), expected.is_some());
        }
    }

    #[tokio::test]
    async fn selected_target_retrieval_and_admission_never_retry() {
        use std::{cell::Cell, future::ready};
        let selected = 7;
        for (retrieval, fallback, expected, scans) in [
            (Ok(vec![selected]), vec![], Ok(selected), 0),
            (Ok(vec![]), vec![selected], Ok(selected), 1),
            (
                Err(btleplug::Error::NotSupported("retrieve".into())),
                vec![selected],
                Ok(selected),
                1,
            ),
            (Ok(vec![8]), vec![8], Err("device_not_found"), 1),
            (
                Err(btleplug::Error::PermissionDenied),
                vec![selected],
                Err("permission_required"),
                0,
            ),
        ] {
            let count = Cell::new(0);
            let result = resolve_target(
                ready(retrieved(retrieval)),
                || {
                    count.set(count.get() + 1);
                    ready(Ok(fallback))
                },
                |id| *id == selected,
            )
            .await;
            assert_eq!(result.map_err(|e| e.code), expected.map_err(str::to_owned));
            assert_eq!(count.get(), scans);
        }
        for code in ["cancelled", "transport_timeout", "bluetooth_error"] {
            let result = resolve_target(
                ready(Err(Error::new(code, "synthetic"))),
                || async {
                    panic!("failed retrieval must never scan");
                    #[allow(unreachable_code)]
                    Ok(vec![7])
                },
                |id| *id == selected,
            )
            .await;
            assert_eq!(result.unwrap_err().code, code);
        }
        for initial in [None, Some("M110")] {
            let cleaned = Cell::new(false);
            let result = admit(
                async {
                    check_name(initial)?;
                    Ok(((), Some("M220".into())))
                },
                async {
                    cleaned.set(true);
                    Ok(())
                },
            )
            .await;
            assert_eq!(result.unwrap_err().code, "unsupported_device");
            assert!(cleaned.get());
        }
        assert!(validate_target("synthetic", "M220").is_err());
        #[cfg(target_os = "macos")]
        {
            assert_eq!(
                validate_target("synthetic", "M110").unwrap_err().code,
                "invalid_arguments"
            );
            assert!(validate_target("00000000-0000-0000-0000-000000000007", "M110").is_ok());
        }
    }

    #[test]
    fn discovery_retains_all_candidates_without_claiming_support() {
        let unrelated = Uuid::from_u128(0x0000180f_0000_1000_8000_00805f9b34fb);
        for (name, services, present, allowed) in [
            (Some("SYNTHETIC123456"), vec![unrelated], true, true),
            (Some("Renamed device"), vec![], true, true),
            (None, vec![unrelated], true, true),
            (None, vec![], false, true),
            (Some("M220"), vec![SERVICE], true, false),
        ] {
            let props = present.then(|| PeripheralProperties {
                local_name: name.map(str::to_owned),
                rssi: Some(-60),
                services: services.clone(),
                ..Default::default()
            });
            let device = candidate("synthetic-id".into(), props);
            assert_eq!(device.id, "synthetic-id");
            assert_eq!(device.name.as_deref(), name);
            assert_eq!(device.rssi, present.then_some(-60));
            assert_eq!(device.advertised_services, services);
            assert!(!device.supported);
            assert_eq!(device.model_allowed, allowed);
        }
    }
}
