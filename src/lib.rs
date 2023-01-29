/*
 * This file is part of rust-gdb.
 *
 * rust-gdb is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * rust-gdb is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with rust-gdb.  If not, see <http://www.gnu.org/licenses/>.
 */

extern crate regex;

mod dbg;
mod msg;
mod parser;
use std::future::Future;

use sysinfo::Signal;
#[cfg(not(windows))]
use sysinfo::{Pid, ProcessExt, System, SystemExt};
use tokio::runtime::Runtime;

/// Helper function to bridge between the async <-> sync code
pub fn run_async(future: impl Future) {
    let rt = Runtime::new().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, future);
}

#[cfg(target_os = "windows")]
use winapi::um::winbase::DebugBreakProcess;

/// Send `sigid` to process with ID `pid`
/// Return true on success
#[cfg(target_os = "windows")]
pub fn signal(pid: usize, sigid: Signal) -> bool {
    unsafe {
        match sigid {
            Signal::Interrupt => {
                let open_mode: u32 = 2097151; // PROCESS_ALL_ACCESS
                let process_handle =
                    winapi::um::processthreadsapi::OpenProcess(open_mode, 0, pid as u32);
                DebugBreakProcess(process_handle) == 1
            }
            Signal::Kill => {
                let open_mode: u32 = 2097151; // PROCESS_ALL_ACCESS
                let process_handle =
                    winapi::um::processthreadsapi::OpenProcess(open_mode, 0, pid as u32);
                winapi::um::processthreadsapi::TerminateProcess(process_handle, 0);
                true
            }
            _ => false,
        }
    }
}

#[cfg(not(windows))]
pub fn signal(pid: usize, sigid: Signal) -> bool {
    let mut s = System::new();
    s.refresh_processes();
    if let Some(process) = s.process(Pid::from(pid)) {
        tracing::debug!("sending signal {} to process {}", sigid, pid);
        if let Some(res) = process.kill_with(sigid) {
            tracing::debug!("success");
            return res;
        }
    }
    return false;
}

#[cfg(test)]
mod tests {
    use super::run_async;
    use super::*;

    #[test]
    fn test_debug_session() {
        tracing_subscriber::fmt::init();
        run_async(async move {
            let (mut dbg, mut rx) = dbg::Debugger::start().await.unwrap();
            assert!(dbg.can_send_commands());

            if let Ok(test_exe) = std::env::var("TEST_EXE") {
                // load the executable
                let test_exe = test_exe.replace("\\", "/");
                dbg.send_cmd_raw(&format!(r#"-file-exec-and-symbols "{test_exe}""#))
                    .await;

                let resp = dbg.read_result_record(&mut rx).await;
                assert_eq!(msg::ResultClass::Done, resp.class);

                dbg.send_cmd_raw("-exec-run").await;
                let resp = dbg.read_result_record(&mut rx).await;

                tracing::debug!("{:?}", resp);
                assert_eq!(msg::ResultClass::Running, resp.class);

                // let the process a chance to start
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                // Make sure we CANNOT send commands
                assert!(!dbg.can_send_commands());

                // we should have the debugiee process id by now
                assert!(dbg.get_debuggee_pid().is_some(), "no debuggee process id");

                // interrupt the process
                assert!(dbg.interrupt());

                // let the process a chance to start
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                // Make sure we CAN send commands
                assert!(dbg.can_send_commands());
            }
        });
    }

    #[test]
    fn parse_stuff() {
        let resp = parser::parse_line("789^done,this=\"that\"\n").unwrap();
        match resp {
            msg::Record::Result(msg) => {
                println!("{:?}", msg);
            }
            _ => panic!("wrong type :("),
        };

        let resp = parser::parse_line("=stopped,this=\"that\"\n").unwrap();
        match resp {
            msg::Record::Async(msg) => {
                println!("{:?}", msg);
            }
            _ => panic!("wrong type :("),
        };
        let resp = parser::parse_line("~\"yadda yadda\"\n").unwrap();
        match resp {
            msg::Record::Stream(msg) => {
                println!("{:?}", msg);
            }
            _ => panic!("wrong type :("),
        };
    }
}

pub use dbg::*;
pub use msg::*;
