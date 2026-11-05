//! Child-process ownership. Every provider process (and everything it spawns) lives in a
//! Windows Job Object with KILL_ON_JOB_CLOSE, so stopping a session or Kiln exiting — even
//! by crashing — never leaves agent processes behind. On Unix each child gets its own
//! process group, which is signalled as a whole.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Environment variables that mark a *parent* Claude Code session. Inheriting them would make
/// a child `claude` think it is nested inside another session; they are not user configuration.
const NESTING_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_HOST_SESSION_ID",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_AGENT_SDK_VERSION",
];

pub fn command(exe: &Path) -> Command {
    let mut c = Command::new(exe);
    for v in NESTING_VARS {
        c.env_remove(v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    c
}

/// Runs a short command, killing it if it exceeds `timeout`.
pub fn output_with_timeout(mut cmd: Command, timeout: Duration) -> Result<Output, String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("could not start: {e}"))?;
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        b
    });
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("timed out after {}s", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    Ok(Output { status, stdout: t_out.join().unwrap_or_default(), stderr: t_err.join().unwrap_or_default() })
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::*;

    pub struct Job(HANDLE);
    // SAFETY: a job handle is a kernel object handle usable from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn new() -> Option<Self> {
            // SAFETY: plain Win32 calls with valid, zero-initialised structures.
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                Some(Job(h))
            }
        }
        pub fn assign(&self, child: &std::process::Child) -> bool {
            // SAFETY: the child handle is valid while `child` is alive.
            unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) != 0 }
        }
        pub fn terminate(&self) {
            // SAFETY: valid job handle.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }
    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: valid handle, closed once. KILL_ON_JOB_CLOSE ends any remaining members.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// A provider child process with a shared stdin writer.
pub struct Managed {
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    #[cfg(windows)]
    job: Option<job::Job>,
}

impl Managed {
    pub fn spawn(mut cmd: Command) -> Result<(Arc<Self>, std::process::ChildStdout, std::process::ChildStderr), String> {
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| format!("could not start provider: {e}"))?;
        #[cfg(windows)]
        let job = job::Job::new().filter(|j| j.assign(&child));
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take().ok_or("no stderr")?;
        Ok((
            Arc::new(Self {
                child: Mutex::new(child),
                stdin: Mutex::new(stdin),
                #[cfg(windows)]
                job,
            }),
            stdout,
            stderr,
        ))
    }

    /// Writes one newline-terminated JSON record.
    pub fn write_line(&self, line: &str) -> Result<(), String> {
        let mut g = self.stdin.lock().map_err(|_| "stdin poisoned")?;
        let w = g.as_mut().ok_or("provider stdin is closed")?;
        w.write_all(line.as_bytes())
            .and_then(|_| w.write_all(b"\n"))
            .and_then(|_| w.flush())
            .map_err(|e| format!("write to provider failed: {e}"))
    }

    #[cfg(unix)]
    pub fn pid(&self) -> Option<u32> {
        self.child.lock().ok().map(|c| c.id())
    }

    /// Graceful stop: close stdin, give the provider `grace` to exit, then kill the whole tree.
    pub fn shutdown(&self, grace: Duration) -> Option<i32> {
        if let Ok(mut g) = self.stdin.lock() {
            g.take();
        }
        let start = std::time::Instant::now();
        loop {
            // Take the exit status and release the lock before kill_tree(), which locks again.
            let exited = self.child.lock().ok().and_then(|mut c| c.try_wait().ok().flatten());
            if let Some(s) = exited {
                // The provider exited; still end any grandchildren it left in the job.
                self.kill_tree();
                return s.code();
            }
            if start.elapsed() > grace {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        self.kill_tree();
        self.child.lock().ok().and_then(|mut c| c.wait().ok()).and_then(|s| s.code())
    }

    pub fn kill_tree(&self) {
        #[cfg(windows)]
        if let Some(j) = &self.job {
            j.terminate();
        }
        #[cfg(unix)]
        if let Some(pid) = self.pid() {
            let _ = Command::new("kill").arg("-TERM").arg(format!("-{pid}")).status();
        }
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
        }
    }

    pub fn try_exit_code(&self) -> Option<Option<i32>> {
        self.child.lock().ok().and_then(|mut c| c.try_wait().ok().flatten()).map(|s| s.code())
    }
}

impl Drop for Managed {
    fn drop(&mut self) {
        self.kill_tree();
    }
}

/// Reads newline-delimited records. A trailing record without a newline at EOF is still
/// delivered (flagged `complete = false`) so the caller can report a truncated message.
pub fn read_lines(r: impl Read, mut f: impl FnMut(&str, bool)) {
    let mut br = BufReader::with_capacity(64 * 1024, r);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match br.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {
                let complete = buf.last() == Some(&b'\n');
                let s = String::from_utf8_lossy(&buf);
                let s = s.trim_end_matches(['\n', '\r']);
                if !s.trim().is_empty() {
                    f(s, complete);
                }
                if !complete {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

/// Collects the last ~4 KB of stderr for error reports.
pub fn stderr_collector(r: impl Read + Send + 'static, sink: super::Sink) -> Arc<Mutex<String>> {
    let tail = Arc::new(Mutex::new(String::new()));
    let t2 = tail.clone();
    std::thread::spawn(move || {
        read_lines(r, |line, _| {
            if let Ok(mut t) = t2.lock() {
                t.push_str(line);
                t.push('\n');
                if t.len() > 4096 {
                    let cut = t.len() - 4096;
                    let cut = (cut..t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(cut);
                    t.drain(..cut);
                }
            }
            sink.emit(super::Ev::Log(crate::log::redact(line)));
        });
    });
    tail
}

#[cfg(test)]
mod tests {
    #[test]
    fn partial_final_record_is_flagged() {
        let data = b"{\"a\":1}\n{\"b\":2}\n{\"c\":".to_vec();
        let mut got = Vec::new();
        super::read_lines(&data[..], |l, complete| got.push((l.to_string(), complete)));
        assert_eq!(got.len(), 3);
        assert!(got[0].1 && got[1].1);
        assert_eq!(got[2], ("{\"c\":".to_string(), false));
    }

    #[test]
    fn crlf_records_are_trimmed() {
        let mut got = Vec::new();
        super::read_lines(&b"{\"a\":1}\r\n"[..], |l, _| got.push(l.to_string()));
        assert_eq!(got, vec!["{\"a\":1}"]);
    }
}

#[cfg(test)]
mod shutdown_tests {
    /// Regression: a provider that exits as soon as stdin closes used to deadlock shutdown().
    #[test]
    fn shutdown_returns_when_child_exits_on_stdin_close() {
        let mut cmd = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "more"]);
            c
        } else {
            std::process::Command::new("cat")
        };
        cmd.stdin(std::process::Stdio::piped());
        let (m, _out, _err) = super::Managed::spawn(cmd).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(m.shutdown(std::time::Duration::from_secs(5)));
        });
        assert!(rx.recv_timeout(std::time::Duration::from_secs(10)).is_ok(), "shutdown hung");
    }
}
