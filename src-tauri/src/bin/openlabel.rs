use clap::{Args, Parser, Subcommand};
use openlabel_core::{
    Error, Result, ble,
    ipc::{self, Endpoint, Operation, Reply},
    job::{Job, Jobs, PrintRequest, check_device},
    label::{self, Request},
    operation::Control,
    settings::{self, Overrides},
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
#[cfg(target_os = "macos")]
tauri::embed_plist::embed_info_plist!("../../Info.plist");
#[derive(Parser)]
#[command(
    version,
    about = "Preview and print M110 labels. stdout contains one JSON object."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    #[arg(long, global = true)]
    json: bool,
}
#[derive(Subcommand)]
enum Command {
    Devices,
    CheckDevice(CheckArgs),
    Preview(PreviewArgs),
    Print(PrintArgs),
    TestPrint(TestArgs),
    Connection,
    Connect(CheckArgs),
    Disconnect(DisconnectArgs),
}
#[derive(Args)]
struct DisconnectArgs {
    #[arg(long)]
    device: String,
}
#[derive(Args)]
struct CheckArgs {
    #[arg(long)]
    device: String,
    #[arg(long)]
    model: String,
}
#[derive(Args)]
struct Input {
    /// SVG, PNG, JPEG, WebP, BMP, GIF (first frame), or TIFF (first page).
    path: Option<PathBuf>,
    #[arg(long)]
    test_pattern: bool,
    #[arg(long, conflicts_with = "defaults")]
    settings: Option<PathBuf>,
    #[arg(long)]
    defaults: bool,
    #[command(flatten)]
    overrides: Overrides,
}
impl Input {
    fn request(self) -> Request {
        Request {
            path: self.path,
            test_pattern: self.test_pattern,
            settings: self.settings,
            defaults: self.defaults,
            overrides: self.overrides,
            snapshot: None,
        }
    }
}
#[derive(Args)]
struct PreviewArgs {
    #[command(flatten)]
    input: Input,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    settings_out: Option<PathBuf>,
    #[arg(long)]
    overwrite: bool,
}
#[derive(Args)]
struct Target {
    #[arg(long)]
    device: String,
    #[arg(long)]
    model: String,
    #[arg(long)]
    expect_sha256: String,
    #[arg(long)]
    expect_input_sha256: String,
}
#[derive(Args)]
struct PrintArgs {
    #[command(flatten)]
    input: Input,
    #[command(flatten)]
    target: Target,
    #[arg(long, default_value_t = 1)]
    copies: u8,
}
#[derive(Args)]
struct TestArgs {
    #[arg(long)]
    device: String,
    #[arg(long)]
    model: String,
    #[arg(long)]
    expect_sha256: Option<String>,
    #[arg(long)]
    expect_input_sha256: Option<String>,
    #[command(flatten)]
    overrides: Overrides,
}
fn install_signals(control: Control) -> Result<tokio::task::JoinHandle<()>> {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .map_err(|e| Error::new("signal_error", e.to_string()))?;
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                .map_err(|e| Error::new("signal_error", e.to_string()))?;
        Ok(tokio::spawn(async move {
            tokio::select! {_=interrupt.recv()=>{},_=term.recv()=>{}}
            control.cancel();
        }))
    }
    #[cfg(not(unix))]
    {
        let signal = tokio::signal::windows::ctrl_c()
            .map_err(|e| Error::new("signal_error", e.to_string()))?;
        Ok(tokio::spawn(async move {
            let mut signal = signal;
            signal.recv().await;
            control.cancel();
        }))
    }
}
async fn print(request: Request, target: Target, copies: u8, control: &Control) -> Result<Value> {
    let request = PrintRequest {
        label: request,
        device: target.device,
        model: target.model,
        copies,
        expect_sha256: target.expect_sha256,
        expect_input_sha256: target.expect_input_sha256,
    };
    request.label.overrides.validate_finite()?;
    #[cfg(debug_assertions)]
    let transient = std::env::var("OPENLABEL_TEST_TRANSIENT").is_ok_and(|value| value == "1");
    #[cfg(not(debug_assertions))]
    let transient = false;
    let mut previous = String::new();
    if !transient {
        let response = ipc::call(
            &Endpoint::current()?,
            Operation::Print {
                request: Box::new(request.clone()),
            },
            control,
            |job| {
                show_progress(job, &mut previous);
            },
        )
        .await?;
        if let Some(response) = response {
            return shared_result(response);
        }
    }
    let jobs = Arc::new(Jobs::default());
    jobs.start_with_control(request, control.clone())?;
    loop {
        let job = jobs.status().unwrap();
        show_progress(&job, &mut previous);
        if job.finished {
            return job_result(job);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
fn show_progress(job: &Job, previous: &mut String) -> bool {
    if job.state == *previous {
        return false;
    }
    previous.clone_from(&job.state);
    eprintln!("{} ({}/{})", job.state, job.sent_bytes, job.total_bytes);
    true
}
fn job_result(job: Job) -> Result<Value> {
    if let Some(error) = job.error {
        Err(error)
    } else {
        Ok(json!(job))
    }
}
fn shared_result(response: Reply) -> Result<Value> {
    match response {
        Reply::Final { job } => job_result(job),
        Reply::Runtime { runtime } => Ok(json!(runtime)),
        Reply::Connection { connection } => Ok(json!(connection)),
        _ => Err(Error::localized("ipc_error", "err.ipcResponse", &[])),
    }
}
fn output(result: Result<Value>) -> (Value, i32) {
    match result {
        Ok(result) => (json!({"version":1,"ok":true,"result":result}), 0),
        Err(error) => {
            let exit = if error.code == "invalid_arguments" {
                2
            } else {
                1
            };
            (json!({"version":1,"ok":false,"error":error}), exit)
        }
    }
}
async fn run(command: Command, control: &Control) -> Result<Value> {
    control.check()?;
    match command {
        Command::Connection | Command::Connect(_) | Command::Disconnect(_) => {
            let operation = match command {
                Command::Connect(args) => Operation::Connect {
                    device: args.device,
                    model: args.model,
                },
                Command::Disconnect(args) => Operation::Disconnect {
                    device: args.device,
                },
                _ => Operation::Runtime {},
            };
            connection(&Endpoint::current()?, operation, control).await
        }
        Command::CheckDevice(args) => Ok(json!(
            check_device(&args.device, &args.model, control).await?
        )),
        Command::Devices => {
            let result = ble::devices(control).await;
            Ok(json!({"devices":result?}))
        }
        Command::Preview(args) => {
            let mut request = args.input.request();
            if request.test_pattern && args.settings_out.is_some() {
                return Err(Error::localized(
                    "invalid_settings",
                    "err.patternSidecar",
                    &[],
                ));
            }
            let mut preview = label::preview_controlled(request.clone(), control).await?;
            let mut protected: Vec<PathBuf> = request.path.iter().cloned().collect();
            if let Some(path) = &preview.settings_path {
                protected.push(path.clone());
            }
            if let Some(path) = args.settings_out {
                let bytes = serde_json::to_vec_pretty(&preview.settings).unwrap();
                control.check()?;
                settings::write_output(
                    &path,
                    &bytes,
                    args.overwrite,
                    &request.path.iter().cloned().collect::<Vec<_>>(),
                )?;
                request.defaults = false;
                request.settings = Some(path.clone());
                preview = label::preview_controlled(request, control).await?;
                protected.push(path);
            }
            control.check()?;
            settings::write_output(&args.output, &preview.png, args.overwrite, &protected)?;
            let mut value = json!(preview);
            value["output"] = json!(args.output);
            Ok(value)
        }
        Command::Print(args) => {
            if args.input.test_pattern {
                return Err(Error::localized(
                    "invalid_settings",
                    "err.patternCommand",
                    &[],
                ));
            }
            print(args.input.request(), args.target, args.copies, control).await
        }
        Command::TestPrint(args) => {
            let request = Request {
                test_pattern: true,
                overrides: args.overrides,
                ..Default::default()
            };
            let preview = label::preview_controlled(request.clone(), control).await?;
            let target = Target {
                device: args.device,
                model: args.model,
                expect_sha256: args.expect_sha256.unwrap_or(preview.sha256),
                expect_input_sha256: args.expect_input_sha256.unwrap_or(preview.input_sha256),
            };
            print(request, target, 1, control).await
        }
    }
}
async fn connection(endpoint: &Endpoint, operation: Operation, control: &Control) -> Result<Value> {
    let runtime = matches!(operation, Operation::Runtime {});
    match ipc::call(endpoint, operation, control, |_| {}).await? {
        Some(response) => shared_result(response),
        None if runtime => Ok(
            json!({"app_running":false,"connection":{"state":"disconnected","device":null,"evidence":null,"error":null},"job":null,"desktop_job":null,"busy":false,"closing":false,"revision":0}),
        ),
        None => Err(Error::localized("app_required", "err.openAppFirst", &[])),
    }
}
#[tokio::main]
async fn main() {
    if label::render_worker() {
        return;
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let help = matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            );
            println!(
                "{}",
                if help {
                    json!({"version":1,"ok":true,"result":{"help":e.to_string()}})
                } else {
                    json!({"version":1,"ok":false,"error":{"code":"invalid_arguments","detail":e.to_string()}})
                }
            );
            std::process::exit(if help { 0 } else { 2 });
        }
    };
    let control = Control::default();
    let result = match install_signals(control.clone()) {
        Ok(task) => {
            let result = run(cli.command, &control).await;
            task.abort();
            result
        }
        Err(error) => Err(error),
    };
    let (value, exit) = output(result);
    println!("{value}");
    std::process::exit(exit);
}

#[cfg(all(test, unix))]
mod shared_client_tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    };
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn no_host_uses_production_connection_mapping_and_state_only_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = Endpoint {
            directory: dir.path().join("absent"),
        };
        let (value, exit) =
            output(connection(&endpoint, Operation::Runtime {}, &Control::default()).await);
        assert_eq!(exit, 0);
        assert_eq!(value["version"], 1);
        assert_eq!(value["result"]["app_running"], false);
        assert_eq!(value["result"]["connection"]["state"], "disconnected");
        assert!(value["result"]["job"].is_null());
        assert!(value["result"]["desktop_job"].is_null());
        assert_eq!(value["result"]["busy"], false);
        for operation in [
            Operation::Connect {
                device: "synthetic".into(),
                model: "M110".into(),
            },
            Operation::Disconnect {
                device: "synthetic".into(),
            },
        ] {
            let (value, exit) = output(connection(&endpoint, operation, &Control::default()).await);
            assert_eq!(exit, 1);
            assert_eq!(value["version"], 1);
            assert_eq!(value["error"]["code"], "app_required");
        }
        assert!(!endpoint.directory.exists());
        let mut previous = String::new();
        let mut lines = 0;
        for state in [
            "preparing",
            "sending",
            "sending",
            "sending",
            "completed",
            "completed",
        ] {
            lines += usize::from(show_progress(
                &job(state, state == "completed", None),
                &mut previous,
            ));
        }
        assert_eq!(lines, 3);
    }

    #[tokio::test]
    async fn busy_progress_and_unary_operations_receive_one_prompt_cancel() {
        for print in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let endpoint = Endpoint {
                directory: dir.path().join("host"),
            };
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&endpoint.directory)
                .unwrap();
            let lock = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(endpoint.directory.join("host.lock"))
                .unwrap();
            fs2::FileExt::try_lock_exclusive(&lock).unwrap();
            let listener =
                tokio::net::UnixListener::bind(endpoint.directory.join("app.sock")).unwrap();
            let control = Control::default();
            let cancel = control.clone();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let _: ipc::Request = ipc::read_frame(&mut stream, ipc::REQUEST_LIMIT)
                    .await
                    .unwrap();
                let (mut reader, mut writer) = tokio::io::split(stream);
                {
                    let followup = ipc::read_frame::<ipc::Request>(&mut reader, ipc::REQUEST_LIMIT);
                    tokio::pin!(followup);
                    let mut ticker = tokio::time::interval(Duration::from_millis(20));
                    let deadline = tokio::time::sleep(Duration::from_millis(800));
                    tokio::pin!(deadline);
                    loop {
                        tokio::select! {
                            _ = &mut deadline => panic!("client cancellation starved by progress"),
                            request = &mut followup => { assert!(matches!(request.unwrap().request, Operation::Cancel {})); break; }
                            _ = ticker.tick(), if print => { ipc::write_frame(&mut writer, &ipc::Response { version: 1, response: Reply::Progress { job: job("sending", false, None) } }, ipc::RESPONSE_LIMIT).await.unwrap(); }
                        }
                    }
                }
                // Keep the final pending across another client cancellation tick; no second Cancel.
                assert!(
                    tokio::time::timeout(
                        Duration::from_millis(120),
                        ipc::read_frame::<ipc::Request>(&mut reader, ipc::REQUEST_LIMIT)
                    )
                    .await
                    .is_err()
                );
                ipc::write_frame(
                    &mut writer,
                    &ipc::Response {
                        version: 1,
                        response: Reply::Error {
                            error: Error::new("cancelled", "scripted cancellation"),
                        },
                    },
                    ipc::RESPONSE_LIMIT,
                )
                .await
                .unwrap();
            });
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                cancel.cancel();
            });
            let operation = if print {
                operation()
            } else {
                Operation::Connect {
                    device: "synthetic-target".into(),
                    model: "M110".into(),
                }
            };
            let result = ipc::call(&endpoint, operation, &control, |_| {}).await;
            server.await.unwrap();
            assert_eq!(result.unwrap_err().code, "cancelled");
        }
    }

    fn job(state: &str, finished: bool, error: Option<Error>) -> Job {
        serde_json::from_value(json!({"id":7,"origin":"cli","state":state,"finished":finished,"sent_bytes":30747,"total_bytes":30747,"copies":1,"device":"synthetic-target","sha256":"raster","input_sha256":"input","settings":null,"evidence":null,"error":error})).unwrap()
    }
    fn operation() -> Operation {
        Operation::Print {
            request: Box::new(PrintRequest {
                label: Request {
                    path: Some("synthetic.svg".into()),
                    settings: Some("synthetic.json".into()),
                    ..Default::default()
                },
                device: "synthetic-target".into(),
                model: "M110".into(),
                copies: 1,
                expect_sha256: "a".repeat(64),
                expect_input_sha256: "b".repeat(64),
            }),
        }
    }
    // The executable uses its production framed client and output formatter here; no fake BLE exports.
    #[tokio::test]
    async fn scripted_host_progress_final_error_and_unknown_outcome_use_cli_mapping() {
        for mode in 0..5 {
            let dir = tempfile::tempdir().unwrap();
            let endpoint = Endpoint {
                directory: dir.path().join("host"),
            };
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&endpoint.directory)
                .unwrap();
            let lock = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(endpoint.directory.join("host.lock"))
                .unwrap();
            fs2::FileExt::try_lock_exclusive(&lock).unwrap();
            let listener =
                tokio::net::UnixListener::bind(endpoint.directory.join("app.sock")).unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request: ipc::Request = ipc::read_frame(&mut stream, ipc::REQUEST_LIMIT)
                    .await
                    .unwrap();
                assert_eq!(request.version, 1);
                let Operation::Print { request } = request.request else {
                    panic!("print expected")
                };
                assert!(request.label.path.unwrap().is_absolute());
                assert!(request.label.settings.unwrap().is_absolute());
                if mode == 3 {
                    return;
                }
                if mode == 4 {
                    stream.write_u32(100).await.unwrap();
                    stream.write_all(b"{").await.unwrap();
                    return;
                }
                ipc::write_frame(
                    &mut stream,
                    &ipc::Response {
                        version: 1,
                        response: Reply::Accepted {
                            job: job("preparing", false, None),
                        },
                    },
                    ipc::RESPONSE_LIMIT,
                )
                .await
                .unwrap();
                ipc::write_frame(
                    &mut stream,
                    &ipc::Response {
                        version: 1,
                        response: Reply::Progress {
                            job: job("completed", false, None),
                        },
                    },
                    ipc::RESPONSE_LIMIT,
                )
                .await
                .unwrap();
                let error = match mode {
                    1 => Some(Error::new("invalid_label", "x".repeat(70 * 1024))),
                    2 => Some(Error::new("cancelled", "synthetic cancellation")),
                    _ => None,
                };
                let state = if mode == 1 {
                    "failed"
                } else if mode == 2 {
                    "cancelled"
                } else {
                    "completed"
                };
                let response = ipc::Response {
                    version: 1,
                    response: Reply::Final {
                        job: job(state, true, error),
                    },
                };
                let bytes = serde_json::to_vec(&response).unwrap();
                stream.write_u32(bytes.len() as u32).await.unwrap();
                // Cross client timer ticks while a single frame is partially read.
                for part in bytes.chunks(bytes.len().div_ceil(3)) {
                    stream.write_all(part).await.unwrap();
                    tokio::time::sleep(Duration::from_millis(65)).await;
                }
            });
            let mut progress = vec![];
            let result = ipc::call(&endpoint, operation(), &Control::default(), |job| {
                progress.push((job.state.clone(), job.finished))
            })
            .await
            .and_then(|reply| shared_result(reply.unwrap()));
            let (value, exit) = output(result);
            assert_eq!(value["version"], 1);
            match mode {
                0 => {
                    assert_eq!(exit, 0);
                    assert_eq!(value["result"]["state"], "completed");
                    assert_eq!(progress.last().unwrap(), &("completed".into(), false));
                }
                1 => {
                    assert_eq!(exit, 1);
                    assert_eq!(value["error"]["detail"].as_str().unwrap().len(), 70 * 1024);
                }
                2 => {
                    assert_eq!(exit, 1);
                    assert_eq!(value["error"]["code"], "cancelled");
                }
                _ => {
                    assert_eq!(exit, 1);
                    assert_eq!(value["error"]["code"], "unknown_outcome");
                }
            }
            server.await.unwrap();
        }
        assert_eq!(
            output(Err(Error::new("invalid_arguments", "synthetic"))).1,
            2
        );
    }
}
