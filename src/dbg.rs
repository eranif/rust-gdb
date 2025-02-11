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

use crate::msg;
use crate::msg::{AsyncClass, AsyncRecord, Record, ResultClass, Value};
use crate::parser;
use std::{
    convert::From,
    fmt,
    process::Stdio,
    result, str,
    sync::{
        atomic::Ordering,
        atomic::{AtomicBool, AtomicUsize},
        Arc,
    },
};
use tokio::process::Command;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
    sync::mpsc::{channel, Receiver, Sender},
};

#[derive(Debug)]
pub enum Error {
    IOError(std::io::Error),
    ParseError,
    IgnoredOutput,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            &Error::IOError(ref err) => write!(f, "{}", err.to_string()),
            &Error::ParseError => write!(f, "cannot parse response from gdb"),
            &Error::IgnoredOutput => write!(f, "ignored output"),
        }
    }
}

pub type Result<T> = result::Result<T, Error>;

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Error {
        Error::IOError(err)
    }
}

pub struct Debugger {
    /// We write to gdb raw string commands
    pub stdin: Sender<String>,
    /// gdb process ID
    pub gdb_pid: Arc<AtomicUsize>,
    /// The debugger state
    pub can_interact: Arc<AtomicBool>,
    /// The debugee pid
    pub debugee_pid: Arc<AtomicUsize>,
}

fn escape_command(cmd: &str) -> String {
    let cmd = cmd.replace("\r", "\\r");
    let cmd = cmd.replace("\n", "\\n");
    cmd
}

impl Debugger {
    /// start new gdb process. Return a pair:
    ///
    /// * A `Debugger` instsance
    /// * The receiver end of the debugger's output channel
    pub async fn start() -> Result<(Self, Receiver<msg::Record>)> {
        tracing::debug!("launching debugger");
        let name = ::std::env::var("GDB_BINARY").unwrap_or("gdb".to_string());
        let mut child = Command::new(name)
            .args(&["--interpreter=mi"])
            .stdout(Stdio::piped())
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // =======================
        // Handling stdout / Stdin
        // =======================
        let stdout = child
            .stdout
            .take()
            .expect("child did not have a handle to stdout");

        // start a tasks here that always listens to gdb, parses the output and put it inside a channel
        let (stdout_sender, output_channel) = channel::<msg::Record>(100);

        let stdin = child
            .stdin
            .take()
            .expect("child did not have a handle to stdin");
        let (stdin_sender, mut stdin_receiver) = channel::<String>(100);

        let can_interact = Arc::new(AtomicBool::new(true));
        let debugee_pid = Arc::new(AtomicUsize::new(usize::MAX));

        let can_interact_clone = can_interact.clone();
        let debugee_pid_clone = debugee_pid.clone();

        let mut reader = BufReader::new(stdout).lines();
        tracing::debug!("launching gdb reader task");
        tokio::task::spawn_local(async move {
            while let Ok(line) = reader.next_line().await {
                if let Some(line) = line {
                    // skip gdb prompt line
                    if line.starts_with("(gdb)") {
                        continue;
                    }
                    tracing::trace!("{}", escape_command(&line));
                    Self::process_line(
                        line,
                        &stdout_sender,
                        can_interact_clone.clone(),
                        debugee_pid_clone.clone(),
                    )
                    .await;
                }
            }
        });

        let mut writer = BufWriter::new(stdin);
        tracing::debug!("launching gdb writer task");
        // start a task that reads lines from the input channel `stdin_receiver` and writes
        // them to the gdb process
        tokio::task::spawn_local(async move {
            while let Some(line) = stdin_receiver.recv().await {
                tracing::debug!("will send command to gdb: {}", escape_command(&line));
                let buf = line.as_bytes();
                let _ = writer.write(buf).await;
                let _ = writer.flush().await;
                tracing::debug!("command sent!");
            }
        });

        tracing::debug!("gdb is up and running");
        Ok((
            Debugger {
                stdin: stdin_sender,
                gdb_pid: Arc::new(AtomicUsize::new(usize::MAX)),
                can_interact,
                debugee_pid,
            },
            output_channel,
        ))
    }

    /// Process gdb output line
    async fn process_line(
        mut line: String,
        sender: &Sender<msg::Record>,
        can_interact: Arc<AtomicBool>,
        debugee_pid: Arc<AtomicUsize>,
    ) {
        if !line.ends_with("\n") {
            line.push('\n');
        }
        match parser::parse_line(line.as_str()) {
            Ok(resp) => {
                match &resp {
                    Record::Async(async_record) => {
                        match async_record {
                            // keep track of "*stopped" messages and mark the debugger as
                            // "can_interact"
                            AsyncRecord::Exec(s) | AsyncRecord::Status(s) => {
                                tracing::trace!("pushing response (AsyncRecord::Exec) to queue");
                                if s.class == AsyncClass::Stopped {
                                    tracing::trace!(
                                        "debugger is stopped -> can_interact is set to TRUE"
                                    );
                                    can_interact.store(true, Ordering::Relaxed);
                                }
                            }
                            AsyncRecord::Notify(s) => {
                                // Looking for the process id
                                if s.class == AsyncClass::Other
                                    && debugee_pid.load(Ordering::Relaxed) == usize::MAX
                                {
                                    for var in &s.content {
                                        if var.name.eq("pid") {
                                            if let Value::String(str_value) = &var.value {
                                                // found the pid
                                                let stripped_value = str_value.replace("\"", "");
                                                if let Ok(pid) = stripped_value.parse() {
                                                    debugee_pid.store(pid, Ordering::Relaxed);
                                                    tracing::debug!("debuggee PID is {}", pid);
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Record::Result(res) => {
                        // keep track of records of type "*running"
                        if res.class == ResultClass::Running {
                            tracing::trace!("debugger is running -> can_interact is set to FALSE");
                            can_interact.store(false, Ordering::Relaxed);
                        }
                    }
                    _ => {}
                }
                let _ = sender.send(resp).await;
            }
            Err(_) => {
                //                tracing::trace!("error parsing line: `{}`", line.as_str());
                //                return;
            }
        };
    }

    /// Read the first `msg::ResultClass` from gdb output channel.
    /// This method discards everything until it finds the
    /// first `msg::ResultClass`
    pub async fn read_result_record(
        &self,
        output_channel: &mut Receiver<msg::Record>,
    ) -> msg::MessageRecord<msg::ResultClass> {
        loop {
            let record = self.read_message_record(output_channel).await;
            match record {
                msg::Record::Result(msg) => return msg,
                msg::Record::Stream(rec) => match rec {
                    msg::StreamRecord::Console(_msg)
                    | msg::StreamRecord::Target(_msg)
                    | msg::StreamRecord::Log(_msg) => {
                        //tracing::trace!("read stream: {}", escape_command(&msg))
                    }
                },
                _ => {}
            }
        }
    }

    /// Read the first `msg::ResultClass` from gdb output queue
    pub async fn read_message_record(
        &self,
        output_channel: &mut Receiver<msg::Record>,
    ) -> msg::Record {
        loop {
            if let Some(record) = &output_channel.recv().await {
                match record {
                    msg::Record::Result(message) => {
                        tracing::trace!("< {:?}", message);
                        return record.clone();
                    }
                    msg::Record::Stream(rec) => match rec {
                        msg::StreamRecord::Console(message)
                        | msg::StreamRecord::Target(message)
                        | msg::StreamRecord::Log(message) => {
                            tracing::trace!("< {}", escape_command(&message));
                            return record.clone();
                        }
                    },
                    msg::Record::Async(async_record) => {
                        tracing::trace!("< {:?}", async_record);
                        return record.clone();
                    }
                }
            }
        }
    }

    /// Send command to gdb
    pub async fn send_cmd_raw(&mut self, cmd: &str) {
        tracing::debug!("sending command: {} to gdb", escape_command(&cmd));
        if cmd.ends_with("\n") {
            let _ = self.stdin.send(cmd.to_string()).await;
        } else {
            let _ = self.stdin.send(cmd.to_string() + "\n").await;
        }
        tracing::debug!("done");
    }

    /// can we send commands to the debugger now?
    pub fn can_send_commands(&self) -> bool {
        self.can_interact.load(Ordering::Relaxed)
    }

    /// interrupt the running process
    /// If success, we should `can_send_commands()` returns `true`
    pub fn interrupt(&self) -> bool {
        if self.can_send_commands() {
            // nothing to be done more
            tracing::debug!(
                "no need to interrupt debugee process. it is already in an interactive mode"
            );
            return true;
        }

        if self.debugee_pid.load(Ordering::Relaxed) == usize::MAX {
            tracing::debug!("can not interrupt debugee process. I don't know its process id yet");
            return false;
        }
        signal(self.debugee_pid.load(Ordering::Relaxed), Signal::Interrupt)
    }

    pub fn get_debuggee_pid(&self) -> Option<usize> {
        if self.debugee_pid.load(Ordering::Relaxed) != usize::MAX {
            Some(self.debugee_pid.load(Ordering::Relaxed))
        } else {
            None
        }
    }

    pub fn terminate(&self) {
        tracing::debug!("terminating gdb...");
        // terminate gdb + debugee
        if self.debugee_pid.load(Ordering::Relaxed) != usize::MAX {
            signal(self.debugee_pid.load(Ordering::Relaxed), Signal::Kill);
        }
        if self.gdb_pid.load(Ordering::Relaxed) != usize::MAX {
            signal(self.gdb_pid.load(Ordering::Relaxed), Signal::Kill);
        }
    }
}

use crate::signal;
use sysinfo::Signal;

impl Drop for Debugger {
    fn drop(&mut self) {
        self.terminate();
    }
}
