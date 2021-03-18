// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use {
    analytics::{add_crash_event, add_launch_event, get_notice},
    anyhow::{anyhow, Context, Result},
    ffx_core::{ffx_bail, ffx_error, init_metrics_svc, FfxError},
    ffx_daemon::{find_next_daemon, is_daemon_running},
    ffx_lib_args::{from_env, Ffx},
    ffx_lib_sub_command::Subcommand,
    fidl::endpoints::create_proxy,
    fidl_fuchsia_developer_bridge::{
        DaemonError, DaemonProxy, FastbootError, FastbootMarker, FastbootProxy,
    },
    fidl_fuchsia_developer_remotecontrol::{RemoteControlMarker, RemoteControlProxy},
    fidl_fuchsia_overnet_protocol::NodeId,
    fuchsia_async::TimeoutExt,
    futures::lock::Mutex,
    futures::{Future, FutureExt},
    lazy_static::lazy_static,
    ring::digest::{Context as ShaContext, Digest, SHA256},
    std::error::Error,
    std::fs::File,
    std::io::{BufReader, Read},
    std::pin::Pin,
    std::sync::Arc,
    std::time::{Duration, Instant},
};

// Config key for event timeout.
const PROXY_TIMEOUT_SECS: &str = "proxy.timeout_secs";
// TODO: a nice way to focus this error message would be to get a list of targets from the daemon
// and be able to distinguish whether there are in fact 0 or multiple available targets.
const TARGET_FAILURE_MSG: &str = "\
We weren't able to open a connection to a target.

Use `ffx target list` to verify the state of connected devices. This error
probably means that either:

1) There are no available targets. Make sure your device is connected.
2) There are multiple available targets and you haven't specified a target or
provided a default.
Tip: You can use `ffx --target \"my-nodename\" <command>` to specify a target
for a particular command, or use `ffx target default set \"my-nodename\"` if
you always want to use a particular target.";

const CURRENT_EXE_HASH: &str = "current.hash";

const NON_FASTBOOT_MSG: &str = "\
This command needs to be run against a target in the Fastboot state.
Try rebooting the device into Fastboot with the command `ffx target
reboot --bootloader` and try re-running this command.";

const TARGET_IN_FASTBOOT: &str = "\
This command cannot be run against a target in the Fastboot state. Try
rebooting the device or flashing the device into a running state.";

lazy_static! {
    // Using a mutex to guard the spawning of the daemon - the value it contains is not used.
    static ref SPAWN_GUARD: Arc<Mutex<()>> = Arc::new(Mutex::new(()));
}

async fn get_daemon_proxy_single_link(
    exclusions: Option<Vec<NodeId>>,
) -> Result<(NodeId, DaemonProxy, Pin<Box<impl Future<Output = Result<()>>>>), FfxError> {
    // Start a race betwen:
    // - The unix socket link being lost
    // - A timeout
    // - Getting a FIDL proxy over the link

    let link = hoist::hoist().run_single_ascendd_link().fuse();
    let mut link = Box::pin(link);
    let find = find_next_daemon(exclusions).fuse();
    let mut find = Box::pin(find);
    let mut timeout = fuchsia_async::Timer::new(Duration::from_secs(1)).fuse();

    let res = futures::select! {
        r = link => {
            Err(ffx_error!("Daemon link lost while attempting to connect: {:?}\nRun `ffx doctor` for further diagnostics.", r))
        }
        _ = timeout => {
            Err(ffx_error!("Timed out waiting for Daemon connection.\nRun `ffx doctor` for futher diagnostics."))
        }
        proxy = find => proxy.map_err(|e| ffx_error!("Error connecting to Daemon: {}.\nRun `ffx doctor` for fruther diagnostics.", e)),
    };
    res.map(|(nodeid, proxy)| (nodeid, proxy, link))
}

// This could get called multiple times by the plugin system via multiple threads - so make sure
// the spawning only happens one thread at a time.
async fn get_daemon_proxy() -> Result<DaemonProxy> {
    let _guard = SPAWN_GUARD.lock().await;
    if !is_daemon_running().await {
        #[cfg(not(test))]
        ffx_daemon::spawn_daemon().await?;
    }

    let (nodeid, proxy, link) = get_daemon_proxy_single_link(None).await?;

    // Spawn off the link task, so that FIDL functions can be called (link IO makes progress).
    let link_task = fuchsia_async::Task::spawn(link.map(|_| ()));

    // TODO(fxb/67400) Create an e2e test.
    #[cfg(test)]
    let hash: String = "testcurrenthash".to_owned();
    #[cfg(not(test))]
    let hash: String = ffx_config::get((CURRENT_EXE_HASH, ffx_config::ConfigLevel::Runtime))
        .await
        .map_err(|_| ffx_error!("BUG: ffx version information missing - build/release bug!"))?;

    let daemon_hash = proxy.get_hash().await?;
    if hash == daemon_hash {
        link_task.detach();
        return Ok(proxy);
    }

    log::info!("Daemon is a different version.  Attempting to restart");

    // Tell the daemon to quit, and wait for the link task to finish.
    // TODO(raggi): add a timeout on this, if the daemon quit fails for some
    // reason, the link task would hang indefinitely.
    let (quit_result, _) = futures::future::join(proxy.quit(), link_task).await;

    if !quit_result.is_ok() {
        ffx_bail!(
            "FFX daemon upgrade failed unexpectedly with {:?}. \n\
            Try running `ffx doctor --force-daemon-restart` and then retrying your \
            command",
            quit_result
        )
    }

    #[cfg(not(test))]
    ffx_daemon::spawn_daemon().await?;

    let (_nodeid, proxy, link) = get_daemon_proxy_single_link(Some(vec![nodeid])).await?;

    fuchsia_async::Task::spawn(link.map(|_| ())).detach();

    Ok(proxy)
}

async fn proxy_timeout() -> Result<Duration> {
    let proxy_timeout: ffx_config::Value = ffx_config::get(PROXY_TIMEOUT_SECS).await?;
    Ok(Duration::from_millis(
        (proxy_timeout
            .as_f64()
            .ok_or(anyhow!("unable to convert to float: {:?}", proxy_timeout))?
            * 1000.0) as u64,
    ))
}

#[derive(Debug)]
struct TimeoutError {}
impl std::fmt::Display for TimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "timed out")
    }
}
impl Error for TimeoutError {}

async fn timeout<F, T>(t: Duration, f: F) -> Result<T, TimeoutError>
where
    F: Future<Output = T> + Unpin,
{
    // TODO(raggi): this could be made more efficient (avoiding the box) with some additional work,
    // but for the local use cases here it's not sufficiently important.
    let mut timer = fuchsia_async::Timer::new(t).boxed().fuse();
    let mut f = f.fuse();

    futures::select! {
        _ = timer => Err(TimeoutError{}),
        res = f => Ok(res),
    }
}

async fn get_fastboot_proxy() -> Result<FastbootProxy> {
    let daemon_proxy = get_daemon_proxy().await?;
    let (fastboot_proxy, fastboot_server_end) = create_proxy::<FastbootMarker>()?;
    let app: Ffx = argh::from_env();
    let result = timeout(
        proxy_timeout().await?,
        daemon_proxy
            .get_fastboot(app.target().await?.as_ref().map(|s| s.as_str()), fastboot_server_end),
    )
    .await
    .context("timeout")?
    .context("connecting to Fastboot")?;

    match result {
        Ok(_) => Ok(fastboot_proxy),
        Err(FastbootError::NonFastbootDevice) => Err(ffx_error!(NON_FASTBOOT_MSG).into()),
        Err(e) => Err(anyhow!("unexpected failure connecting to Fastboot: {:?}", e)),
    }
}

async fn get_remote_proxy() -> Result<RemoteControlProxy> {
    let daemon_proxy = get_daemon_proxy().await?;
    let (remote_proxy, remote_server_end) = create_proxy::<RemoteControlMarker>()?;
    let app: Ffx = argh::from_env();

    let result = timeout(
        proxy_timeout().await?,
        daemon_proxy.get_remote_control(
            app.target().await?.as_ref().map(|s| s.as_str()),
            remote_server_end,
        ),
    )
    .await
    .context("timeout")?
    .context("connecting to daemon")?;

    match result {
        Ok(_) => Ok(remote_proxy),
        Err(DaemonError::TargetCacheError) => Err(ffx_error!(TARGET_FAILURE_MSG).into()),
        Err(DaemonError::TargetInFastboot) => Err(ffx_error!(TARGET_IN_FASTBOOT).into()),
        Err(e) => Err(anyhow!("unexpected failure connecting to RCS: {:?}", e)),
    }
}

async fn is_experiment_subcommand_on(key: &'static str) -> bool {
    ffx_config::get(key).await.unwrap_or(false)
}

fn is_daemon(subcommand: &Option<Subcommand>) -> bool {
    if let Some(Subcommand::FfxDaemonPlugin(ffx_daemon_plugin_args::DaemonCommand {
        subcommand: ffx_daemon_plugin_sub_command::Subcommand::FfxDaemonStart(_),
    })) = subcommand
    {
        return true;
    }
    false
}

fn set_hash_config(overrides: Option<String>) -> Result<Option<String>> {
    let input = std::env::current_exe()?;
    let reader = BufReader::new(File::open(input)?);
    let digest = sha256_digest(reader)?;

    let runtime = format!("{}={}", CURRENT_EXE_HASH, hex::encode(digest.as_ref()));
    match overrides {
        Some(s) => {
            if s.is_empty() {
                Ok(Some(runtime))
            } else {
                let new_overrides = format!("{},{}", s, runtime);
                Ok(Some(new_overrides))
            }
        }
        None => Ok(Some(runtime)),
    }
}

fn sha256_digest<R: Read>(mut reader: R) -> Result<Digest> {
    let mut context = ShaContext::new(&SHA256);
    let mut buffer = [0; 1024];

    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        context.update(&buffer[..count]);
    }

    Ok(context.finish())
}

fn get_launch_args() -> String {
    let args: Vec<String> = std::env::args().collect();
    // drop arg[0]: executable with hard path
    // TODO(fxb/71028): do we want to break out subcommands for analytics?
    let args_str = &args[1..].join(" ");
    format!("{}", &args_str)
}

async fn run() -> Result<()> {
    hoist::disable_autoconnect();
    let app: Ffx = from_env();

    // Configuration initialization must happen before ANY calls to the config (or the cache won't
    // properly have the runtime parameters.
    let overrides = set_hash_config(app.runtime_config_overrides())?;
    ffx_config::init_config(&app.config, &overrides, &app.env)?;
    let log_to_stdio = app.verbose || is_daemon(&app.subcommand);
    ffx_config::logging::init(log_to_stdio).await?;

    log::info!("starting command: {:?}", std::env::args().collect::<Vec<String>>());

    // HACK(64402): hoist uses a lazy static initializer obfuscating access to inject
    // this value by other means, so:
    let _ = ffx_config::get("overnet.socket").await.map(|sockpath: String| {
        std::env::set_var("ASCENDD", sockpath);
    });

    init_metrics_svc().await; // one time call to initialize app analytics
    if let Some(note) = get_notice().await {
        eprintln!("{}", note);
    }

    let analytics_start = Instant::now();

    let analytics_task = fuchsia_async::Task::spawn(async {
        let launch_args = get_launch_args();
        if let Err(e) = add_launch_event(Some(&launch_args)).await {
            log::error!("metrics submission failed: {}", e);
        }
        Instant::now()
    });

    let command_start = Instant::now();
    let res = ffx_lib_suite::ffx_plugin_impl(
        get_daemon_proxy,
        get_remote_proxy,
        get_fastboot_proxy,
        is_experiment_subcommand_on,
        app,
    )
    .await;
    let command_done = Instant::now();
    log::info!("Command completed. Success: {}", res.is_ok());

    let analytics_done = analytics_task
        // TODO(66918): make configurable, and evaluate chosen time value.
        .on_timeout(Duration::from_secs(2), || {
            log::error!("metrics submission timed out");
            // Metrics timeouts should not impact user flows.
            Instant::now()
        })
        .await;

    log::info!(
        "Run finished. success: {}, command time: {}, analytics time: {}",
        res.is_ok(),
        (command_done - command_start).as_secs_f32(),
        (analytics_done - analytics_start).as_secs_f32()
    );
    res
}

#[fuchsia_async::run_singlethreaded]
async fn main() {
    match run().await {
        Ok(_) => {
            // TODO add event for timing here at end
            std::process::exit(0)
        }
        Err(err) => {
            let error_code = if let Some(ffx_err) = err.downcast_ref::<FfxError>() {
                eprintln!("{}", ffx_err);
                match ffx_err {
                    FfxError::Error(_, code) => *code,
                }
            } else {
                eprintln!("BUG: An internal command error occurred.\n{:?}", err);
                1
            };
            let err_msg = format!("{}", err);
            // TODO(66918): make configurable, and evaluate chosen time value.
            if let Err(e) = add_crash_event(&err_msg)
                .on_timeout(Duration::from_secs(2), || {
                    log::error!("analytics timed out reporting crash event");
                    Ok(())
                })
                .await
            {
                log::error!("analytics failed to submit crash event: {}", e);
            }
            std::process::exit(error_code);
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use ascendd;
    use async_std::os::unix::net::UnixListener;
    use async_std::sync::Mutex;
    use fidl::endpoints::{ClientEnd, RequestStream, ServiceMarker};
    use fidl_fuchsia_developer_bridge::{DaemonMarker, DaemonRequest, DaemonRequestStream};
    use fidl_fuchsia_overnet::{ServiceProviderRequest, ServiceProviderRequestStream};
    use fuchsia_async::Task;
    use futures::AsyncReadExt;
    use futures::TryStreamExt;
    use hoist::OvernetInstance;
    use std::path::PathBuf;
    use tempfile;

    fn setup_ascendd_temp() -> tempfile::TempPath {
        let path = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        std::fs::remove_file(&path).unwrap();
        std::env::set_var("ASCENDD", &path);
        path
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_get_daemon_proxy_link_lost() {
        let sockpath = setup_ascendd_temp();

        // Start a listener that accepts and immediately closes the socket..
        let listener = UnixListener::bind(sockpath.to_owned()).await.unwrap();
        let _listen_task = Task::spawn(async move {
            loop {
                drop(listener.accept().await.unwrap());
            }
        });

        let res = get_daemon_proxy().await;
        let str = format!("{}", res.err().unwrap());
        assert!(str.contains("link lost"));
        assert!(str.contains("ffx doctor"));
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_get_daemon_proxy_timeout_no_connection() {
        let sockpath = setup_ascendd_temp();

        // Start a listener that never accepts the socket.
        let _listener = UnixListener::bind(sockpath.to_owned()).await.unwrap();

        let res = get_daemon_proxy().await;
        let str = format!("{}", res.err().unwrap());
        assert!(str.contains("Timed out"));
        assert!(str.contains("ffx doctor"));
    }

    async fn test_daemon(sockpath: PathBuf, hash: &str) {
        let daemon_hoist = Arc::new(hoist::Hoist::new().unwrap());

        let (s, p) = fidl::Channel::create().unwrap();
        daemon_hoist.publish_service(DaemonMarker::NAME, ClientEnd::new(p)).unwrap();

        let link_tasks = Arc::new(Mutex::new(Vec::<Task<()>>::new()));
        let link_tasks1 = link_tasks.clone();

        let listener = UnixListener::bind(sockpath.to_owned()).await.unwrap();
        let listen_task = Task::spawn(async move {
            // let (sock, _addr) = listener.accept().await.unwrap();
            let mut stream = listener.incoming();
            while let Some(sock) = stream.try_next().await.unwrap_or(None) {
                let hoist_clone = daemon_hoist.clone();
                link_tasks1.lock().await.push(Task::spawn(async move {
                    let (mut rx, mut tx) = sock.split();
                    ascendd::run_stream(
                        hoist_clone.node(),
                        &mut rx,
                        &mut tx,
                        Some("fake daemon".to_string()),
                        None,
                    )
                    .map(|r| eprintln!("link error: {:?}", r))
                    .await;
                }));
            }
        });

        let mut stream = ServiceProviderRequestStream::from_channel(
            fidl::AsyncChannel::from_channel(s).unwrap(),
        );

        while let Some(ServiceProviderRequest::ConnectToService { chan, .. }) =
            stream.try_next().await.unwrap_or(None)
        {
            let link_tasks = link_tasks.clone();
            let mut stream =
                DaemonRequestStream::from_channel(fidl::AsyncChannel::from_channel(chan).unwrap());
            while let Some(request) = stream.try_next().await.unwrap_or(None) {
                match request {
                    DaemonRequest::GetHash { responder, .. } => responder.send(hash).unwrap(),
                    DaemonRequest::Quit { responder, .. } => {
                        std::fs::remove_file(sockpath).unwrap();
                        listen_task.cancel().await;
                        responder.send(true).unwrap();
                        // This is how long the daemon sleeps for, which
                        // is a workaround for the fact that we have no
                        // way to "flush" the response over overnet due
                        // to the constraints of mesh routing.
                        fuchsia_async::Timer::new(Duration::from_millis(20)).await;
                        link_tasks.lock().await.clear();
                        return;
                    }
                    _ => {
                        panic!("unimplemented stub for request: {:?}", request);
                    }
                }
            }
        }
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_get_daemon_proxy_hash_matches() {
        let sockpath = setup_ascendd_temp();

        let sockpath1 = sockpath.to_owned();
        let daemons_task = Task::spawn(async move {
            test_daemon(sockpath1.to_owned(), "testcurrenthash").await;
        });

        // wait until daemon binds the socket path
        while std::fs::metadata(&sockpath).is_err() {
            fuchsia_async::Timer::new(Duration::from_millis(20)).await
        }

        let proxy = get_daemon_proxy().await.unwrap();
        proxy.quit().await.unwrap();
        daemons_task.await;
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_get_daemon_proxy_upgrade() {
        let sockpath = setup_ascendd_temp();

        // Spawn two daemons, the first out of date, the second is up to date.
        let sockpath1 = sockpath.to_owned();
        let daemons_task = Task::spawn(async move {
            test_daemon(sockpath1.to_owned(), "oldhash").await;
            // Note: testcurrenthash is explicitly expected by #cfg in get_daemon_proxy
            test_daemon(sockpath1.to_owned(), "testcurrenthash").await;
        });

        // wait until daemon binds the socket path
        while std::fs::metadata(&sockpath).is_err() {
            fuchsia_async::Timer::new(Duration::from_millis(20)).await
        }

        let proxy = get_daemon_proxy().await.unwrap();
        proxy.quit().await.unwrap();
        daemons_task.await;
    }
}
