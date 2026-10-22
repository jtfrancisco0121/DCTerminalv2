//! One PTY process: spawn, write, resize, and kill the process tree.
//!
//! Output is delivered on a channel. The master stays on a control thread so
//! resize and write do not race the reader.

#[cfg(unix)]
use crate::process_tree::STOP_GRACE;
use crate::process_tree::{PidGuard, PtyGroup};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// ConPTY's cursor-position query. It sends this on startup and blocks
/// further output until a cursor position report comes back.
const DSR_QUERY: &[u8] = b"\x1b[6n";
const DSR_REPLY: &[u8] = b"\x1b[1;1R";

#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    /// Set on top of the inherited environment (e.g. `CLAUDE_CONFIG_DIR`).
    pub env: Vec<(String, String)>,
    /// Dropped from the inherited environment before `env` is set.
    pub env_remove: Vec<String>,
    pub cwd: std::path::PathBuf,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyOutput {
    Data(Vec<u8>),
    Exit { code: Option<i32> },
}

enum Ctrl {
    Write(Vec<u8>),
    /// A cursor-position report we generated. Does not mean the frontend is listening.
    Reply(Vec<u8>),
    /// Not acknowledged: the control thread may be blocked in `write_all`.
    Resize {
        cols: u16,
        rows: u16,
    },
    #[cfg(test)]
    Size(Sender<io::Result<(u16, u16)>>),
    Kill,
}

/// Sends to a session's control thread without the registry lock held.
#[derive(Clone)]
pub(crate) struct PtyHandle {
    ctrl: Sender<Ctrl>,
}

impl PtyHandle {
    /// Queues the resize and returns at once; the control thread applies it
    /// in order with writes and logs a failure.
    pub(crate) fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.ctrl
            .send(Ctrl::Resize {
                cols: cols.max(1),
                rows: rows.max(1),
            })
            .map_err(|err| io::Error::other(err.to_string()))
    }
}

/// Milliseconds since the first call in this process: a clock an atomic can hold.
pub struct PtySession {
    ctrl: Sender<Ctrl>,
    output: Option<Receiver<PtyOutput>>,
    pid: u32,
    group: PtyGroup,
}

impl PtySession {
    pub fn spawn(spec: SpawnSpec) -> io::Result<Self> {
        let cols = spec.cols.max(1);
        let rows = spec.rows.max(1);
        let system = native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| io::Error::other(err.to_string()))?;
        let mut command = CommandBuilder::new(&spec.program);
        for arg in &spec.args {
            command.arg(arg);
        }
        if spec.cwd.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pty cwd is empty",
            ));
        }
        command.cwd(spec.cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        for key in &spec.env_remove {
            command.env_remove(key);
        }
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|err| io::Error::other(err.to_string()))?;
        let pid = child.process_id().unwrap_or(0);
        let guard = PidGuard::adopt(pid);
        let group = guard.group();
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| io::Error::other(err.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|err| io::Error::other(err.to_string()))?;
        let master = pair.master;
        let (ctrl_tx, ctrl_rx) = mpsc::channel();
        let (out_tx, out_rx) = mpsc::channel();
        let read_out = out_tx.clone();
        let attached = Arc::new(AtomicBool::new(false));
        let read_ctrl = ctrl_tx.clone();
        let read_attached = Arc::clone(&attached);
        thread::Builder::new()
            .name("pty-read".into())
            .spawn(move || read_loop(reader, read_out, read_ctrl, read_attached))
            .map_err(|err| io::Error::other(err.to_string()))?;
        thread::Builder::new()
            .name("pty-ctrl".into())
            .spawn(move || control_loop(master, writer, child, guard, ctrl_rx, out_tx, attached))
            .map_err(|err| io::Error::other(err.to_string()))?;
        Ok(Self {
            ctrl: ctrl_tx,
            output: Some(out_rx),
            pid,
            group,
        })
    }

    pub fn take_output(&mut self) -> Option<Receiver<PtyOutput>> {
        self.output.take()
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The process group, for app exit. Never signalled once the control
    /// thread is done with the child.
    pub fn group(&self) -> PtyGroup {
        self.group.clone()
    }

    pub fn write(&self, data: &[u8]) -> io::Result<()> {
        self.ctrl
            .send(Ctrl::Write(data.to_vec()))
            .map_err(|err| io::Error::other(err.to_string()))
    }

    pub(crate) fn handle(&self) -> PtyHandle {
        PtyHandle {
            ctrl: self.ctrl.clone(),
        }
    }

    #[cfg(test)]
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.handle().resize(cols, rows)
    }

    #[cfg(test)]
    pub fn size(&self) -> io::Result<(u16, u16)> {
        let (ack_tx, ack_rx) = mpsc::channel();
        self.ctrl
            .send(Ctrl::Size(ack_tx))
            .map_err(|err| io::Error::other(err.to_string()))?;
        ack_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|err| io::Error::other(err.to_string()))?
    }

    pub fn kill(&self) {
        let _ = self.ctrl.send(Ctrl::Kill);
    }

    /// A session whose control thread never answers, like one blocked in
    /// `write_all` on a terminal that stopped reading. The receiver gets the
    /// size of each resize sent to it.
    #[cfg(test)]
    pub fn stalled_for_test() -> (Self, Receiver<(u16, u16)>) {
        let (ctrl_tx, ctrl_rx) = mpsc::channel();
        let (size_tx, size_rx) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(msg) = ctrl_rx.recv() {
                if let Ctrl::Resize { cols, rows } = msg {
                    let _ = size_tx.send((cols, rows));
                }
            }
        });
        let session = Self {
            ctrl: ctrl_tx,
            output: None,
            pid: 0,
            group: PidGuard::adopt(0).group(),
        };
        (session, size_rx)
    }

    #[cfg(test)]
    pub fn recv_timeout(&self, timeout: Duration) -> io::Result<Option<PtyOutput>> {
        let Some(output) = self.output.as_ref() else {
            return Ok(None);
        };
        match output.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(None),
        }
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        self.kill();
    }
}

fn read_loop(
    mut reader: Box<dyn Read + Send>,
    output: Sender<PtyOutput>,
    ctrl: Sender<Ctrl>,
    attached: Arc<AtomicBool>,
) {
    let mut gate = DsrGate::default();
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => {
                let tail = gate.finish();
                if !tail.is_empty() {
                    let _ = output.send(PtyOutput::Data(tail));
                }
                break;
            }
            Ok(n) => {
                let (forward, reply) = gate.push(&buf[..n], attached.load(Ordering::Relaxed));
                if !reply.is_empty() && ctrl.send(Ctrl::Reply(reply)).is_err() {
                    break;
                }
                if !forward.is_empty() && output.send(PtyOutput::Data(forward)).is_err() {
                    break;
                }
            }
        }
    }
}

/// Holds a split `ESC [ 6 n` and answers it until xterm is attached.
#[derive(Default)]
struct DsrGate {
    held: Vec<u8>,
}

impl DsrGate {
    fn push(&mut self, input: &[u8], frontend_attached: bool) -> (Vec<u8>, Vec<u8>) {
        self.held.extend_from_slice(input);
        let bytes = std::mem::take(&mut self.held);
        let mut forward = Vec::new();
        let mut reply = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] != 0x1b {
                forward.push(bytes[index]);
                index += 1;
                continue;
            }
            let rest = &bytes[index..];
            if rest.starts_with(DSR_QUERY) {
                if frontend_attached {
                    forward.extend_from_slice(DSR_QUERY);
                } else {
                    reply.extend_from_slice(DSR_REPLY);
                }
                index += DSR_QUERY.len();
                continue;
            }
            if DSR_QUERY.starts_with(rest) {
                self.held.extend_from_slice(rest);
                break;
            }
            forward.push(bytes[index]);
            index += 1;
        }
        (forward, reply)
    }

    fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.held)
    }
}

fn control_loop(
    master: Box<dyn portable_pty::MasterPty + Send>,
    mut writer: Box<dyn Write + Send>,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    guard: PidGuard,
    ctrl: Receiver<Ctrl>,
    output: Sender<PtyOutput>,
    attached: Arc<AtomicBool>,
) {
    let mut exited = false;
    loop {
        match ctrl.recv_timeout(Duration::from_millis(200)) {
            Ok(Ctrl::Write(data)) => {
                // A frontend write means xterm is listening and will answer later queries.
                attached.store(true, Ordering::Relaxed);
                let _ = writer.write_all(&data);
                let _ = writer.flush();
            }
            Ok(Ctrl::Reply(data)) => {
                let _ = writer.write_all(&data);
                let _ = writer.flush();
            }
            Ok(Ctrl::Resize { cols, rows }) => {
                attached.store(true, Ordering::Relaxed);
                if let Err(err) = master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                }) {
                    eprintln!("DCTerminalv2: terminal resize to {cols}x{rows} failed: {err}");
                }
            }
            #[cfg(test)]
            Ok(Ctrl::Size(ack)) => {
                let result = master
                    .get_size()
                    .map(|size| (size.cols, size.rows))
                    .map_err(|err| io::Error::other(err.to_string()));
                let _ = ack.send(result);
            }
            Ok(Ctrl::Kill) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                finish(&guard, child.as_mut(), &output, &mut exited);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(Some(status)) = child.try_wait() {
                    guard.mark_reaped();
                    if !exited {
                        let _ = output.send(PtyOutput::Exit {
                            code: Some(status.exit_code() as i32),
                        });
                    }
                    break;
                }
            }
        }
    }
    drop(writer);
    drop(master);
}

/// Runs on the control thread, never the UI thread. Unix: SIGTERM to the
/// group, then up to [`STOP_GRACE`] for it to exit (Claude finishes its
/// session log), then SIGKILL. An interactive shell ignores SIGTERM and
/// waits the full grace; the tab is already gone by then.
fn finish(
    guard: &PidGuard,
    child: &mut dyn portable_pty::Child,
    output: &Sender<PtyOutput>,
    exited: &mut bool,
) {
    #[allow(unused_mut)]
    let mut status = None;
    #[cfg(unix)]
    {
        guard.terminate();
        let deadline = std::time::Instant::now() + STOP_GRACE;
        loop {
            if status.is_none() {
                if let Ok(Some(done)) = child.try_wait() {
                    guard.mark_reaped();
                    status = Some(done);
                }
            }
            if (status.is_some() && !guard.group_alive()) || std::time::Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    guard.kill();
    if status.is_none() {
        let _ = child.kill();
        status = child.try_wait().ok().flatten();
    }
    let code = status.map(|status| status.exit_code() as i32);
    if !*exited {
        *exited = true;
        let _ = output.send(PtyOutput::Exit { code });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_shell() -> Option<(String, Vec<String>)> {
        if cfg!(windows) {
            let cmd = PathBuf::from(r"C:\Windows\System32\cmd.exe");
            if cmd.is_file() {
                // /Q turns command echo off. /D skips AutoRun. /K stays open.
                return Some((
                    cmd.display().to_string(),
                    vec!["/Q".into(), "/D".into(), "/K".into()],
                ));
            }
            let powershell =
                PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe");
            if powershell.is_file() {
                return Some((
                    powershell.display().to_string(),
                    vec!["-NoProfile".into(), "-NoLogo".into(), "-NoExit".into()],
                ));
            }
            None
        } else if std::path::Path::new("/bin/sh").is_file() {
            Some(("/bin/sh".to_string(), Vec::new()))
        } else {
            None
        }
    }

    /// ConPTY inserts cursor sequences between echoed characters. The marker
    /// is the command's output after those sequences are removed.
    fn strip_vt(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '\u{1b}' {
                out.push(ch);
                continue;
            }
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        }
        out
    }

    fn wait_for_output(session: &PtySession, marker: &str) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let mut raw = Vec::new();
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let slice = remaining.min(Duration::from_millis(200));
            match session.recv_timeout(slice) {
                Ok(Some(PtyOutput::Data(bytes))) => {
                    raw.extend_from_slice(&bytes);
                    let text = strip_vt(&String::from_utf8_lossy(&raw));
                    if text.contains(marker) {
                        return true;
                    }
                }
                Ok(Some(PtyOutput::Exit { .. })) => {
                    let text = strip_vt(&String::from_utf8_lossy(&raw));
                    return text.contains(marker);
                }
                _ => {}
            }
        }
        false
    }

    #[test]
    fn an_unattached_terminal_answers_conpty_dsr() {
        let mut gate = DsrGate::default();
        let (forward, reply) = gate.push(b"\x1b[6nC:\\>", false);
        assert_eq!(reply, b"\x1b[1;1R");
        assert_eq!(forward, b"C:\\>");

        let (again, no_reply) = gate.push(b"\x1b[6nready", true);
        assert!(no_reply.is_empty());
        assert_eq!(again, b"\x1b[6nready");
    }

    #[test]
    fn a_split_dsr_is_answered_once_the_query_is_complete() {
        let mut gate = DsrGate::default();
        let (forward, reply) = gate.push(b"\x1b[6", false);
        assert!(forward.is_empty());
        assert!(reply.is_empty());
        let (forward, reply) = gate.push(b"nprompt", false);
        assert_eq!(reply, b"\x1b[1;1R");
        assert_eq!(forward, b"prompt");
    }

    /// The read loop, not the test, writes the cursor report. No frontend is attached.
    #[cfg(unix)]
    #[test]
    fn the_backend_answers_dsr_before_a_frontend_is_attached() {
        if std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("ignored: node is not on PATH");
            return;
        }
        let dir =
            crate::test_support::test_root().join(format!("dctv2-dsr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dsr temp dir");
        let script = dir.join("dsr.mjs");
        std::fs::write(
            &script,
            "import fs from \"node:fs\";\n\
             fs.writeSync(1, Buffer.from(\"\\u001b[6n\"));\n\
             try { process.stdin.setRawMode(true); } catch { /* not a tty */ }\n\
             process.stdin.resume();\n\
             let pending = Buffer.alloc(0);\n\
             process.stdin.on(\"data\", (chunk) => {\n\
             pending = Buffer.concat([pending, chunk]);\n\
             if (pending.includes(Buffer.from(\"\\u001b[1;1R\"))) {\n\
             fs.writeSync(1, \"pty_marker\\n\");\n\
             }\n\
             });\n\
             setInterval(() => {}, 1000);\n",
        )
        .expect("write dsr script");
        let session = PtySession::spawn(SpawnSpec {
            program: "node".into(),
            args: vec![script.display().to_string()],
            env: Vec::new(),
            env_remove: Vec::new(),
            cwd: dir.clone(),
            cols: 80,
            rows: 24,
        })
        .expect("spawn node");
        let answered = wait_for_output(&session, "pty_marker");
        session.kill();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(answered, "backend did not answer the cursor query");
    }

    #[test]
    fn ansi_sequences_do_not_hide_the_marker() {
        let wrapped = "\u{1b}[0mp\u{1b}[32mty_marker\u{1b}[0m";
        assert!(strip_vt(wrapped).contains("pty_marker"));
    }

    #[test]
    fn pty_spawn_write_resize_and_kill() {
        let Some((program, args)) = test_shell() else {
            eprintln!("ignored: this platform has no cmd.exe, powershell, or /bin/sh");
            return;
        };
        let cwd = crate::test_support::test_root().to_path_buf();
        let session = PtySession::spawn(SpawnSpec {
            program,
            args,
            env: Vec::new(),
            env_remove: Vec::new(),
            cwd,
            cols: 80,
            rows: 24,
        })
        .expect("spawn shell");
        let pid = session.pid();
        assert!(pid > 0, "shell pid");
        // cmd /Q does not echo the typed line. Wait for the command's output.
        let line: &[u8] = if cfg!(windows) {
            b"echo pty_marker\r"
        } else {
            b"printf 'pty_marker\\n'\n"
        };
        if cfg!(windows) {
            // ConPTY emits CSI 6 n at startup and will not run the command
            // until something answers with a cursor position report.
            session
                .write(b"\x1b[1;1R")
                .expect("answer device status report");
        }
        session.write(line).expect("write");
        assert!(
            wait_for_output(&session, "pty_marker"),
            "shell did not print the marker"
        );
        session.resize(40, 12).expect("resize");
        let (cols, rows) = session.size().expect("size");
        assert_eq!((cols, rows), (40, 12));
        session.kill();
        // kill() waits up to STOP_GRACE for SIGTERM before SIGKILL, so a
        // loaded machine needs a deadline well past it (not 30 x 50 ms).
        let deadline = std::time::Instant::now() + STOP_GRACE + Duration::from_secs(10);
        let mut saw_exit = false;
        while !saw_exit && std::time::Instant::now() < deadline {
            saw_exit = matches!(
                session.recv_timeout(Duration::from_millis(100)),
                Ok(Some(PtyOutput::Exit { .. }))
            );
        }
        assert!(saw_exit, "kill did not report an exit");
        #[cfg(unix)]
        {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while process_alive(pid) && std::time::Instant::now() < deadline {
                thread::sleep(Duration::from_millis(50));
            }
            assert!(
                !process_alive(pid),
                "shell {pid} was still alive after kill"
            );
        }
    }

    // Regression: closing a terminal tab sent SIGKILL only, so a Claude
    // running in it lost the end of its session log.
    #[cfg(unix)]
    #[test]
    fn kill_gives_the_program_a_chance_to_handle_sigterm() {
        let dir = crate::test_support::TempDir::new("pty_term");
        let handled = dir.join("handled");
        let script = format!(
            "trap \"touch '{}'; exit 0\" TERM\nprintf 'pty_%s\\n' ready\nwhile :; do sleep 0.05; done",
            handled.display()
        );
        let session = PtySession::spawn(SpawnSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script],
            env: Vec::new(),
            env_remove: Vec::new(),
            cwd: dir.path().to_path_buf(),
            cols: 80,
            rows: 24,
        })
        .expect("spawn sh");
        assert!(wait_for_output(&session, "pty_ready"), "script ready");
        session.kill();
        let mut saw_exit = false;
        for _ in 0..80 {
            if matches!(
                session.recv_timeout(Duration::from_millis(50)),
                Ok(Some(PtyOutput::Exit { .. }))
            ) {
                saw_exit = true;
                break;
            }
        }
        assert!(handled.exists(), "the TERM trap ran");
        assert!(saw_exit, "kill reported an exit");
    }

    // Ticket 14yq6g2z34b: closing a terminal left a child that had left
    // the terminal's group (setsid) running.
    #[cfg(unix)]
    #[test]
    fn kill_stops_a_detached_child() {
        use crate::process_tree::tests::{
            assert_only_detached_stopped, detached_child_script, wait_for_children,
        };
        let dir = crate::test_support::TempDir::new("pty_detached");
        let pid_file = dir.join("detached.pid");
        let Some(script) = detached_child_script(&pid_file, false) else {
            return;
        };
        let session = PtySession::spawn(SpawnSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script],
            env: Vec::new(),
            env_remove: Vec::new(),
            cwd: dir.path().to_path_buf(),
            cols: 80,
            rows: 24,
        })
        .expect("spawn sh");
        let children = wait_for_children(&pid_file);
        session.kill();
        assert_only_detached_stopped(children, "closing the terminal");
    }

    #[cfg(unix)]
    fn process_alive(pid: u32) -> bool {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }

    /// The app never passes DCV2_TEST_MODE on, even when it inherited one:
    /// not into a dcv2 Claude tab's terminal, not into its own check runs.
    /// The only test that sets the variable (process-wide).
    #[cfg(unix)]
    #[test]
    fn an_inherited_test_mode_reaches_neither_a_dcv2_tab_nor_an_app_check() {
        std::env::set_var("DCV2_TEST_MODE", "auto-approve");
        let launch = crate::store::TabLaunch {
            role: "eagle-eye".into(),
            prompt: "x".into(),
            env: vec![("DCV2_TEST_MODE".into(), "halt-after=PR1".into())],
            ..Default::default()
        };
        let mut command = crate::provider::ProgramArgs::new("/bin/sh", vec![]);
        let env_remove = crate::run::launch::apply_launch(&mut command, Some(&launch), true);
        let session = PtySession::spawn(SpawnSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo \"TM=[${DCV2_TEST_MODE-unset}]\"".into()],
            env: command.env.clone(),
            env_remove,
            cwd: std::env::temp_dir(),
            cols: 80,
            rows: 24,
        })
        .unwrap();
        let seen = wait_for_output(&session, "TM=[unset]");
        let dir = crate::test_support::TempDir::new("check_env");
        let log = dir.join("log.txt");
        let (result, _, _) = crate::run::check::run_one(
            "echo \"TM=[${DCV2_TEST_MODE-unset}]\"",
            dir.path(),
            &log,
            Duration::from_secs(20),
        );
        std::env::remove_var("DCV2_TEST_MODE");
        assert!(seen, "the PTY saw DCV2_TEST_MODE");
        assert_eq!(result, "pass");
        assert!(std::fs::read_to_string(&log)
            .unwrap()
            .contains("TM=[unset]"));
    }
}
