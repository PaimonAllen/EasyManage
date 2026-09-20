#![forbid(unsafe_code)]

use easymanage_protocol::PROTOCOL_VERSION;
use easymanage_runtime::privilege::ensure_administrator;
use std::process::ExitCode;

fn main() -> ExitCode {
    if let Err(error) = ensure_administrator("controller") {
        eprintln!("[FATAL] {error}");
        return ExitCode::FAILURE;
    }

    println!("EasyManage controller scaffold (protocol v{PROTOCOL_VERSION})");
    ExitCode::SUCCESS
}
