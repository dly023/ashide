#[allow(deprecated)]
use cocoa::base::id;
use warpui::{platform::mac::make_nsstring, AppContext};

use crate::channel::ChannelState;

use {
    std::fs::{self, File, OpenOptions},
    std::io::{self, Read, Write},
    std::os::fd::AsRawFd,
    std::os::unix::fs::PermissionsExt,
    std::os::unix::net::{UnixListener, UnixStream},
    std::path::PathBuf,
    std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    std::thread::{self, JoinHandle},
    std::time::Duration,
    warpui::{Entity, ModelContext, SingletonEntity},
};

extern "C" {
    /// ObjC function to create and register the NSServices provider for the
    /// application.
    fn warp_register_services_provider();
}

const STARTUP_IPC_RETRY_COUNT: usize = 200;
const STARTUP_IPC_RETRY_DELAY: Duration = Duration::from_millis(50);
const STARTUP_IPC_IO_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_IPC_MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
const STARTUP_IPC_ACK: u8 = 1;
const STARTUP_IPC_REJECTED: u8 = 0;

static GUI_OWNER_LOCK: OnceLock<File> = OnceLock::new();
static GUI_OWNER_LISTENER: OnceLock<Mutex<Option<UnixListener>>> = OnceLock::new();

/// Initializes application services.
pub fn init(ctx: &mut AppContext) {
    unsafe {
        warp_register_services_provider();
    }

    ctx.add_singleton_model(MacSingleInstanceHost::new);
}

pub fn teardown(ctx: &mut AppContext) {
    MacSingleInstanceHost::handle(ctx).update(ctx, |host, _| host.terminate());
}

/// Attempts to become the sole GUI owner, or forwards this launch to the owner.
///
/// Only [`StartupArgsForwardingError::NoExistingInstance`] authorizes the caller
/// to continue GUI initialization. Every owner-detected IPC failure is fatal so
/// startup cannot silently create a second GUI process.
pub fn pass_startup_args_to_existing_instance(
    args: &warp_cli::AppArgs,
) -> Result<(), StartupArgsForwardingError> {
    if args.finish_update {
        for _ in 0..STARTUP_IPC_RETRY_COUNT {
            if try_become_gui_owner()? {
                return Err(StartupArgsForwardingError::NoExistingInstance);
            }
            thread::sleep(STARTUP_IPC_RETRY_DELAY);
        }
        return Err(StartupArgsForwardingError::PreviousOwnerDidNotExit);
    }

    if try_become_gui_owner()? {
        return Err(StartupArgsForwardingError::NoExistingInstance);
    }

    forward_startup_urls(&startup_urls(args))
}

#[derive(Debug, thiserror::Error)]
pub enum StartupArgsForwardingError {
    #[error("there is no existing Ashide GUI instance")]
    NoExistingInstance,
    #[error("failed to create the single-instance cache directory {path}: {source}")]
    CreateCacheDirectory { path: PathBuf, source: io::Error },
    #[error("failed to open the GUI owner lock {path}: {source}")]
    OpenOwnerLock { path: PathBuf, source: io::Error },
    #[error("failed to acquire the GUI owner lock {path}: {source}")]
    AcquireOwnerLock { path: PathBuf, source: io::Error },
    #[error("failed to remove stale startup IPC socket {path}: {source}")]
    RemoveStaleSocket { path: PathBuf, source: io::Error },
    #[error("failed to bind startup IPC socket {path}: {source}")]
    BindStartupSocket { path: PathBuf, source: io::Error },
    #[error("GUI owner resources were initialized more than once")]
    OwnerResourcesAlreadyInitialized,
    #[error("existing GUI owner did not expose startup IPC before the retry budget expired: {0}")]
    OwnerIpcUnavailable(io::Error),
    #[error("failed to serialize startup intent: {0}")]
    SerializeStartupIntent(#[from] serde_json::Error),
    #[error("startup intent payload is {size} bytes, exceeding the {max} byte protocol limit")]
    StartupIntentTooLarge { size: usize, max: usize },
    #[error("failed to send startup intent: {0}")]
    SendStartupIntent(io::Error),
    #[error("existing GUI owner rejected the startup intent")]
    StartupIntentRejected,
    #[error("the previous GUI owner did not exit after auto-update")]
    PreviousOwnerDidNotExit,
}

fn single_instance_paths() -> (PathBuf, PathBuf) {
    let root = warp_core::paths::application_identity_dir().join("single-instance");
    (root.join("gui-owner.lock"), root.join("startup.sock"))
}

fn try_become_gui_owner() -> Result<bool, StartupArgsForwardingError> {
    let (lock_path, socket_path) = single_instance_paths();
    let root = lock_path
        .parent()
        .expect("single-instance lock must have a parent directory");
    fs::create_dir_all(root).map_err(|source| {
        StartupArgsForwardingError::CreateCacheDirectory {
            path: root.to_path_buf(),
            source,
        }
    })?;

    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|source| StartupArgsForwardingError::OpenOwnerLock {
            path: lock_path.clone(),
            source,
        })?;
    let lock_result = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if lock_result != 0 {
        let source = io::Error::last_os_error();
        if source.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Ok(false);
        }
        return Err(StartupArgsForwardingError::AcquireOwnerLock {
            path: lock_path,
            source,
        });
    }

    match fs::remove_file(&socket_path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(StartupArgsForwardingError::RemoveStaleSocket {
                path: socket_path,
                source,
            });
        }
    }
    let listener = UnixListener::bind(&socket_path).map_err(|source| {
        StartupArgsForwardingError::BindStartupSocket {
            path: socket_path.clone(),
            source,
        }
    })?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600)).map_err(|source| {
        StartupArgsForwardingError::BindStartupSocket {
            path: socket_path,
            source,
        }
    })?;

    GUI_OWNER_LOCK
        .set(lock_file)
        .map_err(|_| StartupArgsForwardingError::OwnerResourcesAlreadyInitialized)?;
    GUI_OWNER_LISTENER
        .set(Mutex::new(Some(listener)))
        .map_err(|_| StartupArgsForwardingError::OwnerResourcesAlreadyInitialized)?;
    Ok(true)
}

fn startup_urls(args: &warp_cli::AppArgs) -> Vec<String> {
    if !args.urls.is_empty() {
        return args.urls.iter().map(ToString::to_string).collect();
    }

    let mut url = url::Url::parse(&format!(
        "{}://action/new_window",
        ChannelState::url_scheme()
    ))
    .expect("channel URL scheme must produce a valid URL");
    if let Ok(current_dir) = std::env::current_dir() {
        url.query_pairs_mut()
            .append_pair("path", &current_dir.to_string_lossy());
    }
    vec![url.to_string()]
}

fn forward_startup_urls(urls: &[String]) -> Result<(), StartupArgsForwardingError> {
    let (_, socket_path) = single_instance_paths();
    let mut last_error = io::Error::new(io::ErrorKind::NotFound, "startup IPC is not ready");
    let frame = startup_intent_frame(urls)?;

    for _ in 0..STARTUP_IPC_RETRY_COUNT {
        match UnixStream::connect(&socket_path) {
            Ok(mut stream) => {
                stream
                    .set_read_timeout(Some(STARTUP_IPC_IO_TIMEOUT))
                    .map_err(StartupArgsForwardingError::SendStartupIntent)?;
                stream
                    .set_write_timeout(Some(STARTUP_IPC_IO_TIMEOUT))
                    .map_err(StartupArgsForwardingError::SendStartupIntent)?;
                stream
                    .write_all(&frame)
                    .map_err(StartupArgsForwardingError::SendStartupIntent)?;
                let mut ack = [STARTUP_IPC_REJECTED];
                stream
                    .read_exact(&mut ack)
                    .map_err(StartupArgsForwardingError::SendStartupIntent)?;
                return if ack[0] == STARTUP_IPC_ACK {
                    Ok(())
                } else {
                    Err(StartupArgsForwardingError::StartupIntentRejected)
                };
            }
            Err(error) => {
                last_error = error;
                thread::sleep(STARTUP_IPC_RETRY_DELAY);
            }
        }
    }

    Err(StartupArgsForwardingError::OwnerIpcUnavailable(last_error))
}

fn startup_intent_frame(urls: &[String]) -> Result<Vec<u8>, StartupArgsForwardingError> {
    let payload = serde_json::to_vec(urls)?;
    if payload.len() > STARTUP_IPC_MAX_PAYLOAD_BYTES {
        return Err(StartupArgsForwardingError::StartupIntentTooLarge {
            size: payload.len(),
            max: STARTUP_IPC_MAX_PAYLOAD_BYTES,
        });
    }

    let payload_len = u32::try_from(payload.len()).expect("startup IPC limit must fit u32");
    let mut frame = Vec::with_capacity(size_of::<u32>() + payload.len());
    frame.extend_from_slice(&payload_len.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

#[derive(Debug, thiserror::Error)]
enum StartupIpcReceiveError {
    #[error("startup intent frame IO failed: {0}")]
    Io(#[from] io::Error),
    #[error("startup intent frame declares {size} bytes, exceeding the {max} byte protocol limit")]
    PayloadTooLarge { size: usize, max: usize },
    #[error("startup intent JSON is invalid: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("startup intent UI queue is unavailable")]
    QueueUnavailable,
}

fn read_startup_intent_frame(
    stream: &mut impl Read,
) -> Result<Vec<String>, StartupIpcReceiveError> {
    let mut header = [0; size_of::<u32>()];
    stream.read_exact(&mut header)?;
    let payload_len = u32::from_be_bytes(header) as usize;
    if payload_len > STARTUP_IPC_MAX_PAYLOAD_BYTES {
        return Err(StartupIpcReceiveError::PayloadTooLarge {
            size: payload_len,
            max: STARTUP_IPC_MAX_PAYLOAD_BYTES,
        });
    }

    let mut payload = vec![0; payload_len];
    stream.read_exact(&mut payload)?;
    Ok(serde_json::from_slice(&payload)?)
}

fn receive_startup_intent(
    stream: &mut UnixStream,
    tx: &async_channel::Sender<Vec<String>>,
) -> Result<(), StartupIpcReceiveError> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(STARTUP_IPC_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(STARTUP_IPC_IO_TIMEOUT))?;
    let urls = read_startup_intent_frame(stream)?;
    tx.try_send(urls)
        .map_err(|_| StartupIpcReceiveError::QueueUnavailable)
}

struct MacSingleInstanceHost {
    stop: Arc<AtomicBool>,
    server_thread: Option<JoinHandle<()>>,
    socket_path: PathBuf,
}

impl MacSingleInstanceHost {
    fn new(ctx: &mut ModelContext<Self>) -> Self {
        let listener = GUI_OWNER_LISTENER
            .get()
            .and_then(|listener| listener.lock().ok()?.take())
            .expect("release GUI owner must bind startup IPC before App initialization");
        listener
            .set_nonblocking(true)
            .expect("startup IPC listener must support nonblocking mode");
        let (_, socket_path) = single_instance_paths();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let (tx, rx) = async_channel::unbounded::<Vec<String>>();
        let server_thread = thread::Builder::new()
            .name("ashide-startup-ipc".to_owned())
            .spawn(move || {
                while !server_stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let accepted = match receive_startup_intent(&mut stream, &tx) {
                                Ok(()) => true,
                                Err(error) => {
                                    log::warn!("macOS startup IPC rejected intent: {error}");
                                    false
                                }
                            };
                            let ack = if accepted {
                                STARTUP_IPC_ACK
                            } else {
                                STARTUP_IPC_REJECTED
                            };
                            let _ = stream.write_all(&[ack]);
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(25));
                        }
                        Err(error) => {
                            log::error!("macOS startup IPC accept failed: {error}");
                            thread::sleep(Duration::from_millis(100));
                        }
                    }
                }
            })
            .expect("startup IPC server thread must spawn");

        ctx.spawn_stream_local(
            rx,
            |_, urls, ctx| {
                for uri in urls {
                    match url::Url::parse(&uri) {
                        Ok(uri) => crate::uri::handle_incoming_uri(&uri, ctx),
                        Err(error) => {
                            log::warn!("Failed to parse URI from macOS startup IPC: {error:#}")
                        }
                    }
                }
            },
            |_, _| {},
        );

        Self {
            stop,
            server_thread: Some(server_thread),
            socket_path,
        }
    }

    fn terminate(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(server_thread) = self.server_thread.take() {
            let _ = server_thread.join();
        }
        if let Err(error) = fs::remove_file(&self.socket_path) {
            if error.kind() != io::ErrorKind::NotFound {
                log::warn!(
                    "Failed to remove macOS startup IPC socket {}: {error}",
                    self.socket_path.display()
                );
            }
        }
    }
}

impl Entity for MacSingleInstanceHost {
    type Event = ();
}

impl SingletonEntity for MacSingleInstanceHost {}

/// Returns an NSString containing the custom URL scheme that this build of the
/// application will respond to.
///
/// Called synchronously from the NSServices dispatch path in
/// `services.m::forFilesFromPasteboard:performAction:`, which wraps the body in
/// an `@autoreleasepool` block. That ambient pool owns the returned NSString.
#[allow(deprecated)]
#[no_mangle]
extern "C-unwind" fn warp_services_provider_custom_url_scheme() -> id {
    make_nsstring(ChannelState::url_scheme())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    #[test]
    fn desktop_singleton_is_not_release_feature_gated() {
        let app_source = include_str!("../lib.rs");
        let services_source = include_str!("mod.rs");
        let mac_source = include_str!("mac.rs");

        for (label, source) in [
            ("run_internal", app_source),
            ("app_services", services_source),
            ("mac app services", mac_source),
        ] {
            assert!(
                !source.contains("feature = \"release_bundle\""),
                "desktop singleton correctness must not be release-feature gated in {label}"
            );
        }
    }

    #[test]
    fn desktop_singleton_ignores_data_profile_namespace() {
        let mac_source = include_str!("mac.rs");
        let path_contract = mac_source
            .split_once("fn single_instance_paths()")
            .expect("macOS singleton path helper must exist")
            .1
            .split_once("\n}")
            .expect("macOS singleton path helper must have a body")
            .0;

        assert!(path_contract.contains("application_identity_dir()"));
        for forbidden in ["cache_dir()", "data_profile()", "WARP_DATA_PROFILE"] {
            assert!(
                !path_contract.contains(forbidden),
                "GUI ownership must ignore data-profile namespace: {forbidden}"
            );
        }
    }

    #[test]
    fn desktop_singleton_arbitration_precedes_shared_initialization() {
        let app_source = include_str!("../lib.rs");
        let run_internal = app_source
            .split_once("fn run_internal")
            .expect("app source must contain run_internal")
            .1
            .split_once("SQLite 预热")
            .expect("pre-init startup section must end before SQLite prewarm")
            .0;
        let arbitration = run_internal
            .find("arbitrate_desktop_gui_singleton")
            .expect("desktop release must arbitrate singleton ownership");
        let shared_init = run_internal
            .find("init_common(&launch_mode")
            .expect("owner must initialize shared services");

        assert!(
            arbitration < shared_init,
            "desktop singleton arbitration must precede shared initialization"
        );
        assert!(!run_internal.contains("pass_startup_args_to_existing_instance"));
    }

    #[test]
    fn desktop_singleton_errors_are_fail_closed() {
        let services_source = include_str!("mod.rs");
        let app_source = include_str!("../lib.rs");
        let post_init = app_source
            .split_once("init_common(&launch_mode")
            .expect("app source must initialize shared services")
            .1;

        assert!(services_source.contains("arbitrate_desktop_gui_singleton"));
        assert!(services_source.contains("Linux GUI singleton startup failed closed"));
        assert!(services_source.contains("Windows GUI singleton startup failed closed"));
        assert!(!post_init.contains("pre_init_errors"));
        assert!(!post_init.contains("pass_startup_args_to_existing_instance"));
    }

    #[test]
    fn mac_single_instance_claim_precedes_shared_logging_initialization() {
        let app_source = include_str!("../lib.rs");
        let run_internal = app_source
            .split_once("fn run_internal")
            .expect("run_internal must exist")
            .1
            .split_once("// SQLite 预热")
            .expect("run_internal startup section must end before SQLite prewarm")
            .0;
        let singleton_claim = run_internal
            .find("arbitrate_desktop_gui_singleton")
            .expect("release GUI must arbitrate singleton startup");
        let shared_logging = run_internal
            .find("init_common(&launch_mode")
            .expect("owner must initialize shared process services");

        assert!(
            singleton_claim < shared_logging,
            "singleton ownership must be decided before shared logging initialization"
        );
    }

    #[test]
    fn mac_single_instance_contract_is_fail_closed() {
        let owner_detected_error = StartupArgsForwardingError::OwnerIpcUnavailable(io::Error::new(
            io::ErrorKind::TimedOut,
            "owner IPC not ready",
        ));
        assert!(!matches!(
            owner_detected_error,
            StartupArgsForwardingError::NoExistingInstance
        ));
    }

    #[test]
    fn release_bundle_declares_multiple_instances_prohibited() {
        for source in [
            include_str!("../../assets/resources/mac/CLI-Info.plist"),
            include_str!("../../../script/macos/run"),
            include_str!("../bin/ashide.rs"),
        ] {
            assert!(source.contains("LSMultipleInstancesProhibited"));
        }

        let release_bundle = include_str!("../../../script/macos/bundle");
        assert!(release_bundle
            .contains("plist_set_bool \"$plist_path\" \"LSMultipleInstancesProhibited\" true"));
        assert!(release_bundle.contains("assert_app_bundle_plist_contract"));
    }

    #[test]
    fn startup_ipc_nonblocking_listener_round_trips_one_framed_intent() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after epoch")
            .as_nanos();
        let socket_path = PathBuf::from(format!(
            "/tmp/ashide-startup-ipc-{}-{nonce}.sock",
            std::process::id()
        ));
        let listener = UnixListener::bind(&socket_path).expect("test listener must bind");
        listener
            .set_nonblocking(true)
            .expect("test listener must become nonblocking");

        let expected = vec!["file:///Users/test/project".to_owned()];
        let client_expected = expected.clone();
        let client_socket_path = socket_path.clone();
        let client = thread::spawn(move || {
            let mut stream = UnixStream::connect(client_socket_path)
                .expect("client must connect to test listener");
            stream
                .set_read_timeout(Some(STARTUP_IPC_IO_TIMEOUT))
                .expect("client read timeout must be configurable");
            stream
                .set_write_timeout(Some(STARTUP_IPC_IO_TIMEOUT))
                .expect("client write timeout must be configurable");
            let frame = startup_intent_frame(&client_expected).expect("intent must serialize");
            stream
                .write_all(&frame)
                .expect("client must write one complete frame");
            let mut ack = [STARTUP_IPC_REJECTED];
            stream
                .read_exact(&mut ack)
                .expect("client must receive ACK");
            assert_eq!(ack, [STARTUP_IPC_ACK]);
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "test listener did not accept client"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("test listener accept failed: {error}"),
            }
        };
        let (tx, rx) = async_channel::bounded(1);
        receive_startup_intent(&mut stream, &tx).expect("owner must decode framed intent");
        stream
            .write_all(&[STARTUP_IPC_ACK])
            .expect("owner must acknowledge accepted intent");

        assert_eq!(rx.try_recv().expect("intent must reach UI queue"), expected);
        client.join().expect("client thread must finish");
        fs::remove_file(socket_path).expect("test socket must be removed");
    }

    #[test]
    fn startup_ipc_rejects_oversized_frame() {
        let oversized = u32::try_from(STARTUP_IPC_MAX_PAYLOAD_BYTES + 1)
            .expect("startup IPC limit must fit u32")
            .to_be_bytes();
        let error = read_startup_intent_frame(&mut Cursor::new(oversized))
            .expect_err("oversized frame must be rejected before body allocation");
        assert!(matches!(
            error,
            StartupIpcReceiveError::PayloadTooLarge {
                size,
                max: STARTUP_IPC_MAX_PAYLOAD_BYTES,
            } if size == STARTUP_IPC_MAX_PAYLOAD_BYTES + 1
        ));
    }
}
