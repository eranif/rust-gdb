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
use crate::parser;
use std::convert::From;
use std::fmt;
use std::process::Stdio;
use std::result;
use std::str;
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
    /// We read from gdb an already processed records
    stdout: Receiver<msg::Record>,
    /// We write to gdb raw string commands
    stdin: Sender<String>,
}

fn escape_command(cmd: &str) -> String {
    let cmd = cmd.replace("\r", "\\r");
    let cmd = cmd.replace("\n", "\\n");
    cmd
}

impl Debugger {
    /// start new gdb process
    pub async fn start() -> Result<Self> {
        tracing::debug!("launching debugger");
        let name = ::std::env::var("GDB_BINARY").unwrap_or("gdb".to_string());
        let mut child = Command::new(name)
            .args(&["--interpreter=mi"])
            .stdout(Stdio::piped())
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // =======================
        // Handling stdout
        // =======================
        let stdout = child
            .stdout
            .take()
            .expect("child did not have a handle to stdout");

        // start a tasks here that always listens to gdb, parses the output and put it inside a channel
        let (sender, stdout_receiver) = channel::<msg::Record>(100);
        let mut reader = BufReader::new(stdout).lines();
        tracing::debug!("launching gdb reader task");
        tokio::spawn(async move {
            while let Ok(line) = reader.next_line().await {
                if let Some(line) = line {
                    // skip gdb prompt line
                    if line.starts_with("(gdb)") {
                        continue;
                    }
                    tracing::trace!("{}", escape_command(&line));
                    Self::process_line(line, &sender).await;
                }
            }
        });

        // =======================
        // Handling stdin
        // =======================

        let stdin = child
            .stdin
            .take()
            .expect("child did not have a handle to stdin");
        let (sender, mut stdin_receiver) = channel::<String>(100);
        let mut writer = BufWriter::new(stdin);
        tracing::debug!("launching gdb writer task");
        tokio::spawn(async move {
            while let Some(line) = stdin_receiver.recv().await {
                tracing::debug!("will send command to gdb: {}", escape_command(&line));
                let buf = line.as_bytes();
                let _ = writer.write(buf).await;
                let _ = writer.flush().await;
                tracing::debug!("command sent!");
            }
        });

        tracing::debug!("gdb is up and running");
        Ok(Debugger {
            stdout: stdout_receiver,
            stdin: sender,
        })
    }

    /// Process gdb output line
    async fn process_line(mut line: String, sender: &Sender<msg::Record>) {
        if !line.ends_with("\n") {
            line.push('\n');
        }
        match parser::parse_line(line.as_str()) {
            Ok(resp) => {
                tracing::trace!("pushing response to queue");
                let _ = sender.send(resp).await;
            }
            Err(_) => {
                tracing::trace!("error parsing line: `{}`", line.as_str());
                return;
            }
        };
    }

    /// Read a single gdb `ResultRecord` from gdb
    pub async fn read_result_record(&mut self) -> msg::MessageRecord<msg::ResultClass> {
        loop {
            if let Some(record) = self.stdout.recv().await {
                tracing::trace!("got {:?}", record);
                match record {
                    msg::Record::Result(msg) => return msg,
                    msg::Record::Stream(rec) => match rec {
                        msg::StreamRecord::Console(msg)
                        | msg::StreamRecord::Target(msg)
                        | msg::StreamRecord::Log(msg) => {
                            tracing::trace!("read stream: {}", escape_command(&msg))
                        }
                    },
                    _ => {}
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
}
