//! Native SCM lifecycle. Stopping the service cancels the shared Agent loop,
//! allowing SQLite transactions and HTTP clients to drop before reporting Stopped.
use std::{ffi::OsString, time::Duration};
use windows_service::{
    define_windows_service,
    service::*,
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};
const NAME: &str = "EasyManageAgent";
define_windows_service!(ffi_main, service_main);
pub fn dispatch() -> windows_service::Result<()> {
    service_dispatcher::start(NAME, ffi_main)
}
fn status(state: ServiceState, code: u32) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(code),
        checkpoint: 0,
        wait_hint: Duration::from_secs(15),
        process_id: None,
    }
}
fn service_main(_: Vec<OsString>) {
    let (stop, mut receive) = tokio::sync::mpsc::channel(1);
    let Ok(handle) = service_control_handler::register(NAME, move |event| match event {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop.try_send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    }) else {
        return;
    };
    let _ = handle.set_service_status(status(ServiceState::StartPending, 0));
    let result = (|| -> anyhow::Result<()> {
        // Absolute config is provided as an SCM argument, never a credential.
        let args: Vec<_> = std::env::args_os().collect();
        if let Some(index) = args.iter().position(|arg| arg == "--config") {
            let path = args
                .get(index + 1)
                .ok_or_else(|| anyhow::anyhow!("missing service config"))?;
            // Load before spawning runtime threads; dotenvy does the same for console mode.
            dotenvy::from_path(path)?;
        }
        super::initialize_crypto_provider();
        super::initialize_tracing();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        handle.set_service_status(status(ServiceState::Running, 0))?;
        runtime.block_on(async { tokio::select! {
            result = crate::app::run() => result,
            _ = receive.recv() => { let _ = handle.set_service_status(status(ServiceState::StopPending, 0)); Ok(()) }
        } })
    })();
    let _ = handle.set_service_status(status(
        ServiceState::Stopped,
        if result.is_ok() { 0 } else { 1 },
    ));
}
