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

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use tokio::runtime::Runtime;

    /// Helper function to bridge between the async <-> sync code
    fn run_async(future: impl Future) {
        let rt = Runtime::new().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, future);
    }

    #[test]
    fn test_debug_session() {
        tracing_subscriber::fmt::init();
        run_async(async move {
            let mut dbg = dbg::Debugger::start().await.unwrap();
            assert!(dbg.can_send_commands());

            if let Ok(test_exe) = std::env::var("TEST_EXE") {
                // load the executable
                let test_exe = test_exe.replace("\\", "/");
                dbg.send_cmd_raw(&format!(r#"-file-exec-and-symbols "{test_exe}""#))
                    .await;
                let resp = dbg.read_result_record().await;
                assert_eq!(msg::ResultClass::Done, resp.class);

                dbg.send_cmd_raw("-exec-run").await;
                let resp = dbg.read_result_record().await;

                tracing::debug!("{:?}", resp);
                assert_eq!(msg::ResultClass::Running, resp.class);

                // let the process a chance to start
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;

                // Make sure we can not send commands
                assert!(!dbg.can_send_commands());

                // we should have the debugiee process id by now
                assert!(dbg.get_debuggee_pid().is_some(), "no debuggee process id");

                // interrupt the process
                //assert!(dbg.interrupt());
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
