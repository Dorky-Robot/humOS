//! Tiny shell-out abstraction used by `mbsync` and `himalaya` wrappers.
//!
//! Production code uses [`SystemRunner`], which inherits stdout/stderr so the
//! user sees progress from the wrapped tool live, and optionally feeds a byte
//! buffer on stdin (for `himalaya message send`). Tests inject [`FakeRunner`]
//! to capture `(program, args, stdin)` tuples and assert on them without
//! requiring mbsync/himalaya on PATH.

use anyhow::{Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

/// One shell invocation.
#[derive(Debug, Clone)]
pub struct ShellCmd {
    pub program: String,
    pub args: Vec<String>,
    /// If `Some`, feed these bytes on the child's stdin. If `None`, the child
    /// inherits the parent's stdin (or gets null if the parent has none).
    pub stdin: Option<Vec<u8>>,
}

impl ShellCmd {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        ShellCmd {
            program: program.into(),
            args,
            stdin: None,
        }
    }

    pub fn with_stdin(mut self, bytes: Vec<u8>) -> Self {
        self.stdin = Some(bytes);
        self
    }
}

pub trait Runner {
    /// Run the command, blocking until it exits. Returns the exit code
    /// (0 on success). stderr/stdout are inherited so the user sees them
    /// live.
    fn run(&self, cmd: &ShellCmd) -> Result<i32>;
}

/// Real process runner. Uses `std::process::Command`.
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, cmd: &ShellCmd) -> Result<i32> {
        let mut c = Command::new(&cmd.program);
        c.args(&cmd.args);
        if cmd.stdin.is_some() {
            c.stdin(Stdio::piped());
        }
        let mut child = c.spawn().with_context(|| {
            format!(
                "spawning `{}` (is it on $PATH?)",
                cmd.program
            )
        })?;
        if let Some(bytes) = &cmd.stdin {
            let mut sink = child
                .stdin
                .take()
                .context("child process has no stdin pipe")?;
            sink.write_all(bytes).context("writing to child stdin")?;
            drop(sink);
        }
        let status = child.wait().context("waiting for child")?;
        Ok(status.code().unwrap_or(-1))
    }
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use std::cell::RefCell;

    /// Records every call it receives. Returns the configured exit code.
    pub struct FakeRunner {
        pub calls: RefCell<Vec<ShellCmd>>,
        pub exit_code: i32,
    }

    impl FakeRunner {
        pub fn new() -> Self {
            FakeRunner {
                calls: RefCell::new(Vec::new()),
                exit_code: 0,
            }
        }

        pub fn with_exit(code: i32) -> Self {
            FakeRunner {
                calls: RefCell::new(Vec::new()),
                exit_code: code,
            }
        }

        pub fn last(&self) -> ShellCmd {
            self.calls
                .borrow()
                .last()
                .cloned()
                .expect("no calls recorded")
        }

        pub fn call_count(&self) -> usize {
            self.calls.borrow().len()
        }
    }

    impl Runner for FakeRunner {
        fn run(&self, cmd: &ShellCmd) -> Result<i32> {
            self.calls.borrow_mut().push(cmd.clone());
            Ok(self.exit_code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::FakeRunner;
    use super::*;

    #[test]
    fn fake_runner_records_calls() {
        let r = FakeRunner::new();
        let cmd = ShellCmd::new("echo", vec!["hi".into()]);
        r.run(&cmd).unwrap();
        assert_eq!(r.call_count(), 1);
        assert_eq!(r.last().program, "echo");
        assert_eq!(r.last().args, vec!["hi"]);
    }

    #[test]
    fn fake_runner_returns_configured_exit() {
        let r = FakeRunner::with_exit(7);
        let code = r.run(&ShellCmd::new("x", vec![])).unwrap();
        assert_eq!(code, 7);
    }

    #[test]
    fn shell_cmd_with_stdin_stores_bytes() {
        let cmd = ShellCmd::new("x", vec![]).with_stdin(b"body\n".to_vec());
        assert_eq!(cmd.stdin.as_deref(), Some(b"body\n".as_slice()));
    }
}
