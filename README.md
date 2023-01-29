# Description

*rust-gdb* is a WIP library for controlling GDB from Rust programs using the [tokio runtime](https://tokio.rs). At the
moment, it can launch a GDB process, pass commands, and parse gdb's responses.
*rust-gdb* uses GDB's
[Machine Interface](https://sourceware.org/gdb/onlinedocs/gdb/GDB_002fMI.html).

Missing features:

* Handle asynchronous output from GDB (currently it's ignored)
* Better interface for executing commands
* Proper documentation

# Usage example


```rust
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
```
