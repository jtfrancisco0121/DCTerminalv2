//! Kill a terminal's program and the children it spawns.
//!
//! On Windows the ConPTY child is assigned to a job; `KILL_ON_JOB_CLOSE` plus
//! `taskkill /T` covers the tree. On Unix the child leads its own session
//! and process group; killing the group reaps grandchildren.
//!
//! Stopping on Unix sends SIGTERM to the group first, so Claude can finish
//! its session log, and SIGKILL only after [`STOP_GRACE`]. A group is never
//! signalled blind after its leader was reaped: once the leader is gone its
//! pgid could belong to a new process, so it is only signalled while the
//! group still has members.
//!
//! A descendant that left the leader's session (`setsid`, Node's
//! `detached: true`) is missed by a group signal, so each Unix signal also
//! goes to the leader's [`Detached`] descendants, found by parent pid before
//! the group exits. A shell's jobs (own group, same session) are left alone.

use crate::pty::session::WriterMsg;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
#[cfg(unix)]
use std::sync::Mutex;
use std::time::Duration;

/// How long a stopped terminal gets to exit on SIGTERM (finishing its
/// session log) before the group is killed.
pub const STOP_GRACE: Duration = Duration::from_millis(1500);

#[cfg(windows)]
fn quiet_taskkill(pid: u32) {
    use std::process::Stdio;
    let _ = crate::platform::quiet_command("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Kills a PTY child and the processes it has already spawned.
///
/// Unix: the child is a session leader (`setsid` inside portable-pty), so a
/// negative-pid signal covers the group. Windows: a job object with
/// `KILL_ON_JOB_CLOSE`, plus `taskkill /T`.
pub struct PidGuard {
    pid: u32,
    /// The leader was reaped, so its pid (and pgid) may be reused.
    reaped: AtomicBool,
    /// Shared with [`PtyGroup`]: the group is no longer ours to signal.
    done: Arc<AtomicBool>,
    #[cfg(unix)]
    detached: Arc<Mutex<Detached>>,
    #[cfg(windows)]
    job: Option<Arc<winjob::Job>>,
    /// The session's writer, so [`PtyGroup::terminate`] can type the stop ask (Windows).
    #[cfg(windows)]
    writer: Option<Sender<WriterMsg>>,
}

impl PidGuard {
    pub fn adopt(pid: u32) -> Self {
        #[cfg(windows)]
        {
            Self {
                pid,
                reaped: AtomicBool::new(false),
                done: Arc::new(AtomicBool::new(false)),
                job: winjob::Job::create_and_assign_pid(pid).ok().map(Arc::new),
                writer: None,
            }
        }
        #[cfg(not(windows))]
        {
            Self {
                pid,
                reaped: AtomicBool::new(false),
                done: Arc::new(AtomicBool::new(false)),
                #[cfg(unix)]
                detached: Arc::default(),
            }
        }
    }

    /// The session's writer channel. Windows: [`PtyGroup::terminate`] types the
    /// stop ask through it. Unix signals the group, so it is not needed.
    pub fn set_writer(&mut self, writer: Sender<WriterMsg>) {
        #[cfg(windows)]
        {
            self.writer = Some(writer);
        }
        #[cfg(not(windows))]
        let _ = writer;
    }

    /// The group as seen from outside the control thread (app exit).
    pub fn group(&self) -> PtyGroup {
        PtyGroup {
            pid: self.pid,
            done: Arc::clone(&self.done),
            #[cfg(unix)]
            detached: Arc::clone(&self.detached),
            #[cfg(windows)]
            job: self.job.clone(),
            #[cfg(windows)]
            writer: self.writer.clone(),
        }
    }

    /// Called once the leader's exit has been collected. If the group is
    /// already empty it is done at once, so [`PtyGroup`] never relies on the
    /// pgid alone after this.
    pub fn mark_reaped(&self) {
        self.reaped.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        let _ = self.ours();
    }

    /// Whether anything of the group may still be running. Unix only.
    #[cfg(unix)]
    pub fn group_alive(&self) -> bool {
        self.ours() || lock(&self.detached).alive()
    }

    /// Whether the pgid is still ours: the leader is unreaped, or the group
    /// still has members. Once a reaped group is seen empty it stays done.
    #[cfg(unix)]
    fn ours(&self) -> bool {
        if self.done.load(Ordering::SeqCst) {
            return false;
        }
        if !self.reaped.load(Ordering::SeqCst) || group_alive(self.pid) {
            return true;
        }
        self.done.store(true, Ordering::SeqCst);
        false
    }

    /// Ask the group to exit (SIGTERM). Unix only.
    #[cfg(unix)]
    pub fn terminate(&self) {
        self.signal(SIGTERM);
    }

    #[cfg(unix)]
    fn signal(&self, sig: i32) {
        signal_pty_tree(self.ours(), self.pid, &self.detached, sig);
    }

    pub fn kill(&self) {
        if self.pid == 0 {
            return;
        }
        #[cfg(unix)]
        {
            self.signal(SIGKILL);
        }
        #[cfg(windows)]
        {
            if let Some(job) = &self.job {
                job.terminate();
            }
            // An already-dead pid makes taskkill print "process not found" on the console.
            quiet_taskkill(self.pid);
        }
    }
}

impl Drop for PidGuard {
    fn drop(&mut self) {
        self.kill();
        // The control thread is done with the child; whatever was left got
        // SIGKILL, and the pgid is no longer ours to signal.
        self.done.store(true, Ordering::SeqCst);
    }
}

#[cfg(unix)]
const SIGTERM: i32 = 15;
#[cfg(unix)]
const SIGKILL: i32 = 9;

/// Signal the process group `pid` leads. Returns whether it was delivered.
/// Never signals pid 0, 1 or -1 (our own group, init, everything).
#[cfg(unix)]
fn signal_group(pid: u32, sig: i32) -> bool {
    let Some(pid) = i32::try_from(pid).ok().filter(|pid| *pid > 1) else {
        return false;
    };
    // SAFETY: kill(2) with a negative pid signals that group only.
    unsafe { kill(-pid, sig) == 0 }
}

#[cfg(unix)]
extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn getpgrp() -> i32;
    #[cfg(target_os = "macos")]
    fn getsid(pid: i32) -> i32;
}

/// One process, as seen when it was found. `start` tells a reused pid apart.
#[cfg(unix)]
#[derive(Clone, Copy, PartialEq)]
struct ProcInfo {
    pid: u32,
    ppid: u32,
    pgid: u32,
    sid: u32,
    start: u64,
}

/// Descendants of a group leader that left its session. They are found by
/// parent pid before the group is signalled: once the group exits they are
/// reparented and the link is lost. A child that already double-forked away
/// to init is missed. Enumeration failures find nothing (group-only stop).
#[cfg(unix)]
#[derive(Default)]
struct Detached(Vec<ProcInfo>);

#[cfg(unix)]
impl Detached {
    /// Adds `leader`'s live descendants outside its session (they called
    /// `setsid`). Others, such as a shell's jobs in their own group, are
    /// walked through but kept out. Call only while `leader`'s pid is ours.
    fn remember(&mut self, leader: u32) {
        let Some(session) = proc_info(leader).map(|info| info.sid) else {
            return;
        };
        let children = children_lister();
        let (mut parents, mut walked) = (vec![leader], 0);
        while let Some(parent) = parents.pop() {
            for child in children(parent) {
                walked += 1;
                if walked > 4096 {
                    return; // only a pid-reuse cycle gets this deep
                }
                parents.push(child.pid);
                if child.sid != session && !self.0.contains(&child) {
                    self.0.push(child);
                }
            }
        }
    }

    /// The remembered processes still running as themselves, never our own
    /// process or group, init, or a pid reused since.
    fn live(&self) -> impl Iterator<Item = u32> + '_ {
        // SAFETY: getpgrp(2) cannot fail.
        let own_group = unsafe { getpgrp() } as u32;
        self.0
            .iter()
            .filter(move |seen| {
                proc_info(seen.pid).is_some_and(|now| {
                    now.start == seen.start
                        && now.pgid != own_group
                        && seen.pid > 1
                        && seen.pid != std::process::id()
                })
            })
            .map(|seen| seen.pid)
    }

    fn alive(&self) -> bool {
        self.live().next().is_some()
    }

    fn signal(&mut self, sig: i32) {
        let live: Vec<u32> = self.live().collect();
        self.0.retain(|seen| live.contains(&seen.pid));
        for pid in live {
            // SAFETY: kill(2) on one pid just confirmed to be the same process.
            unsafe { kill(pid as i32, sig) };
        }
    }
}

#[cfg(target_os = "macos")]
fn proc_info(pid: u32) -> Option<ProcInfo> {
    /// `struct proc_bsdinfo` from <sys/proc_info.h>.
    #[repr(C)]
    struct BsdInfo {
        head: [u32; 4],
        ppid: u32,
        ids: [u32; 7],
        names: [u8; 48],
        nfiles: u32,
        pgid: u32,
        tail: [u32; 4],
        start_sec: u64,
        start_usec: u64,
    }
    const _: () = assert!(std::mem::size_of::<BsdInfo>() == 136);
    extern "C" {
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buf: *mut BsdInfo, size: i32) -> i32;
    }
    const PROC_PIDTBSDINFO: i32 = 3;
    let size = std::mem::size_of::<BsdInfo>() as i32;
    // SAFETY: an all-zero proc_bsdinfo is valid; the kernel fills at most `size` bytes.
    let mut info: BsdInfo = unsafe { std::mem::zeroed() };
    let got = unsafe {
        proc_pidinfo(
            i32::try_from(pid).ok()?,
            PROC_PIDTBSDINFO,
            0,
            &mut info,
            size,
        )
    };
    // SAFETY: getsid(2) only reads; -1 (error) skips the pid.
    let sid = u32::try_from(unsafe { getsid(pid as i32) }).ok()?;
    (got == size).then_some(ProcInfo {
        pid,
        ppid: info.ppid,
        pgid: info.pgid,
        sid,
        start: info.start_sec * 1_000_000 + info.start_usec,
    })
}

#[cfg(target_os = "macos")]
fn children_lister() -> impl Fn(u32) -> Vec<ProcInfo> {
    extern "C" {
        fn proc_listpids(kind: u32, typeinfo: u32, buf: *mut i32, size: i32) -> i32;
    }
    const PROC_PPID_ONLY: u32 = 6;
    |ppid| {
        let mut pids = vec![0i32; 1024];
        let size = (pids.len() * 4) as i32;
        // SAFETY: the kernel writes at most `size` bytes of pids.
        let bytes = unsafe { proc_listpids(PROC_PPID_ONLY, ppid, pids.as_mut_ptr(), size) };
        pids.truncate(usize::try_from(bytes).unwrap_or(0) / 4);
        pids.into_iter()
            .filter_map(|pid| proc_info(u32::try_from(pid).ok().filter(|pid| *pid > 1)?))
            .filter(|info| info.ppid == ppid)
            .collect()
    }
}

#[cfg(target_os = "linux")]
fn proc_info(pid: u32) -> Option<ProcInfo> {
    parse_stat(
        pid,
        &std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?,
    )
}

/// One `/proc/<pid>/stat` line. The comm may hold spaces and ')', so fields
/// count from the last ')': state ppid pgrp session ... starttime (22nd).
#[cfg(any(target_os = "linux", all(test, unix)))]
fn parse_stat(pid: u32, stat: &str) -> Option<ProcInfo> {
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some(ProcInfo {
        pid,
        ppid: fields.get(1)?.parse().ok()?,
        pgid: fields.get(2)?.parse().ok()?,
        sid: fields.get(3)?.parse().ok()?,
        start: fields.get(19)?.parse().ok()?,
    })
}

/// One `/proc` walk per snapshot, not one per tree level.
#[cfg(target_os = "linux")]
fn children_lister() -> impl Fn(u32) -> Vec<ProcInfo> {
    let table: Vec<ProcInfo> = std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .filter(|pid| *pid > 1)
        .filter_map(proc_info)
        .collect();
    move |ppid| {
        table
            .iter()
            .filter(|info| info.ppid == ppid)
            .copied()
            .collect()
    }
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn proc_info(_pid: u32) -> Option<ProcInfo> {
    None
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn children_lister() -> impl Fn(u32) -> Vec<ProcInfo> {
    |_| Vec::new()
}

/// Signal 0: whether the group still has a member (a zombie counts).
#[cfg(unix)]
fn group_alive(pid: u32) -> bool {
    signal_group(pid, 0)
}

/// A PTY's process group (it leads its own session), for app exit. The
/// PTY's control thread still owns and reaps the child; once it is done with
/// the group (`done`), the pgid may be reused and is never signalled again.
/// Otherwise the group is signalled only while it has members.
#[derive(Clone)]
#[cfg_attr(not(unix), allow(dead_code))]
pub struct PtyGroup {
    pid: u32,
    done: Arc<AtomicBool>,
    #[cfg(unix)]
    detached: Arc<Mutex<Detached>>,
    /// Windows: the session's job (every process of the tab), and its writer.
    #[cfg(windows)]
    job: Option<Arc<winjob::Job>>,
    #[cfg(windows)]
    writer: Option<Sender<WriterMsg>>,
}

impl PtyGroup {
    /// Whether the group may still be ours and has a member. Unix: the group
    /// or a detached descendant. Windows: the job has an active process.
    pub fn running(&self) -> bool {
        #[cfg(unix)]
        {
            self.group_running() || lock(&self.detached).alive()
        }
        #[cfg(windows)]
        {
            !self.done.load(Ordering::SeqCst)
                && self
                    .job
                    .as_ref()
                    .is_some_and(|job| job.active_processes() > 0)
        }
        #[cfg(not(any(unix, windows)))]
        {
            false
        }
    }

    #[cfg(unix)]
    fn group_running(&self) -> bool {
        !self.done.load(Ordering::SeqCst) && group_alive(self.pid)
    }

    /// SIGTERM to the group. Windows: types the stop ask (ETX) into the
    /// terminal through its writer, so Claude can finish its session log.
    pub fn terminate(&self) {
        #[cfg(unix)]
        signal_pty_tree(self.group_running(), self.pid, &self.detached, SIGTERM);
        #[cfg(windows)]
        if !self.done.load(Ordering::SeqCst) {
            if let Some(writer) = &self.writer {
                let _ = writer.send(WriterMsg::Ask(
                    crate::pty::session::WINDOWS_STOP_ASK.to_vec(),
                ));
            }
        }
    }

    /// SIGKILL to the group if it is still running. Windows: terminates the job.
    pub fn kill(&self) {
        #[cfg(unix)]
        signal_pty_tree(self.group_running(), self.pid, &self.detached, SIGKILL);
        #[cfg(windows)]
        if !self.done.load(Ordering::SeqCst) {
            if let Some(job) = &self.job {
                job.terminate();
            } else if self.pid != 0 {
                quiet_taskkill(self.pid);
            }
        }
    }
}

/// Signals a PTY's group while it is `ours`, after noting its detached
/// descendants, then the detached ones still alive.
#[cfg(unix)]
fn signal_pty_tree(ours: bool, pid: u32, detached: &Mutex<Detached>, sig: i32) {
    let mut detached = lock(detached);
    if ours {
        detached.remember(pid);
        signal_group(pid, sig);
    }
    detached.signal(sig);
}

#[cfg(unix)]
fn lock(detached: &Mutex<Detached>) -> std::sync::MutexGuard<'_, Detached> {
    detached
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(windows)]
mod winjob {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};

    #[repr(C)]
    struct BasicLimit {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        _pad: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        _pad2: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    struct ExtendedLimit {
        basic: BasicLimit,
        io: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    const KILL_ON_JOB_CLOSE: u32 = 0x2000;
    const EXTENDED_LIMIT_CLASS: i32 = 9;

    #[cfg(target_pointer_width = "64")]
    const _: () = {
        assert!(std::mem::size_of::<ExtendedLimit>() == 144);
        assert!(std::mem::align_of::<ExtendedLimit>() == 8);
    };

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attrs: *mut c_void, name: *const u16) -> RawHandle;
        fn SetInformationJobObject(
            job: RawHandle,
            class: i32,
            info: *const c_void,
            len: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: RawHandle, process: RawHandle) -> i32;
        fn TerminateJobObject(job: RawHandle, exit_code: u32) -> i32;
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> RawHandle;
        fn QueryInformationJobObject(
            job: RawHandle,
            class: i32,
            info: *mut c_void,
            len: u32,
            returned: *mut u32,
        ) -> i32;
    }

    /// `JOBOBJECT_BASIC_ACCOUNTING_INFORMATION`.
    #[repr(C)]
    struct BasicAccounting {
        total_user_time: i64,
        total_kernel_time: i64,
        this_period_total_user_time: i64,
        this_period_total_kernel_time: i64,
        total_page_fault_count: u32,
        total_processes: u32,
        active_processes: u32,
        total_terminated_processes: u32,
    }

    const _: () = assert!(std::mem::size_of::<BasicAccounting>() == 48);
    const BASIC_ACCOUNTING_CLASS: i32 = 1;

    /// Owns one Win32 job-object handle.
    ///
    /// `OwnedHandle` is `Send + Sync` because a kernel HANDLE is not bound to
    /// the thread that created it: `AssignProcessToJobObject`,
    /// `TerminateJobObject`, and `CloseHandle` are safe from any thread.
    /// The handle is closed when this value drops.
    pub struct Job(OwnedHandle);

    fn assert_job_is_threadsafe() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Job>();
    }

    impl Job {
        /// Assign an already-running process. Used for ConPTY children, which
        /// portable-pty starts itself so they cannot be created suspended.
        pub fn create_and_assign_pid(pid: u32) -> io::Result<Self> {
            const PROCESS_SET_QUOTA: u32 = 0x0100;
            const PROCESS_TERMINATE: u32 = 0x0001;
            assert_job_is_threadsafe();
            unsafe {
                let raw = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if raw.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let job = OwnedHandle::from_raw_handle(raw);
                let mut info: ExtendedLimit = std::mem::zeroed();
                info.basic.limit_flags = KILL_ON_JOB_CLOSE;
                let configured = SetInformationJobObject(
                    job.as_raw_handle(),
                    EXTENDED_LIMIT_CLASS,
                    &info as *const ExtendedLimit as *const c_void,
                    std::mem::size_of::<ExtendedLimit>() as u32,
                );
                if configured == 0 {
                    return Err(io::Error::last_os_error());
                }
                let proc_raw = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if proc_raw.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let process = OwnedHandle::from_raw_handle(proc_raw);
                let assigned =
                    AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle());
                if assigned == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Job(job))
            }
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0.as_raw_handle(), 1);
            }
        }

        /// How many processes of the job are still running (0 when the query fails).
        pub fn active_processes(&self) -> u32 {
            // SAFETY: an all-zero accounting struct is valid; the kernel fills at most `len` bytes.
            unsafe {
                let mut info: BasicAccounting = std::mem::zeroed();
                let ok = QueryInformationJobObject(
                    self.0.as_raw_handle(),
                    BASIC_ACCOUNTING_CLASS,
                    &mut info as *mut BasicAccounting as *mut c_void,
                    std::mem::size_of::<BasicAccounting>() as u32,
                    std::ptr::null_mut(),
                );
                if ok == 0 {
                    0
                } else {
                    info.active_processes
                }
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            self.terminate();
        }
    }
}

#[cfg(all(test, unix))]
pub(crate) mod tests {
    use super::*;
    use std::fs;
    use std::process::{Child, Command};
    use std::thread;
    use std::time::Duration;

    /// `sh -c script` as its own process group leader.
    fn spawn_group(script: &str) -> Child {
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script).process_group(0);
        cmd.spawn().expect("spawn sh")
    }

    // Review: once the control thread is done with a terminal (`done`), its
    // pgid may lead an unrelated group, so app exit must not wait on or
    // signal it even though that group is alive.
    #[test]
    fn a_done_pty_group_is_never_signalled_even_while_its_pgid_is_alive() {
        let mut child = spawn_group("sleep 30");
        let group = PtyGroup {
            pid: child.id(),
            done: Arc::new(AtomicBool::new(true)),
            detached: Arc::default(),
        };
        assert!(!group.running(), "a done group is not running");
        group.terminate();
        group.kill();
        thread::sleep(Duration::from_millis(200));
        let untouched = matches!(child.try_wait(), Ok(None));
        let _ = child.kill();
        let _ = child.wait();
        assert!(untouched, "the unrelated group got no signal");
    }

    // Review: between "leader reaped and group empty" and the control thread
    // setting `done`, app exit relied on the pgid alone, which may already
    // lead an unrelated group.
    #[test]
    fn marking_an_empty_group_reaped_makes_it_done_at_once() {
        let mut child = spawn_group("exit 0");
        let guard = PidGuard::adopt(child.id());
        child.wait().expect("reap sh");
        guard.mark_reaped();
        assert!(guard.done.load(Ordering::SeqCst), "done as soon as reaped");
        assert!(!guard.group().running());
    }

    /// The killed grandchild is reparented and reaped by init; poll for it.
    pub(crate) fn wait_until_gone(pid: u32) -> bool {
        for _ in 0..100 {
            if !process_alive(pid) {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    pub(crate) fn wait_for_pid(path: &std::path::Path) -> u32 {
        for _ in 0..100 {
            if let Ok(text) = fs::read_to_string(path) {
                if let Ok(pid) = text.trim().parse::<u32>() {
                    return pid;
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("grandchild pid file was not written");
    }

    /// A `sh -c` line whose program forks a child that leaves the session
    /// (`setsid`, like Node's `detached: true`) and writes its pid to
    /// `pid_file`, and a job-control-style child (own group, same session,
    /// like a shell's `&` or `nohup` job) that writes `<pid_file>.job`. The
    /// parent stays. None when there is no `perl`.
    pub(crate) fn detached_child_script(
        pid_file: &std::path::Path,
        ignore_term: bool,
    ) -> Option<String> {
        let perl = Command::new("perl").arg("-e1").status();
        if !perl.is_ok_and(|status| status.success()) {
            eprintln!("skipped: no perl to start a detached child");
            return None;
        }
        Some(format!(
            "perl -MPOSIX -e 'unless (fork) {{ setpgid(0, 0); \
             open my $f, \">\", \"$ARGV[0].job\" or die; print $f $$; close $f; sleep 60; exit }} \
             if (fork) {{ sleep 60; exit }} POSIX::setsid(); \
             $SIG{{TERM}} = \"IGNORE\" if {}; open my $f, \">\", $ARGV[0] or die; \
             print $f $$; close $f; sleep 60' '{}'",
            u8::from(ignore_term),
            pid_file.display()
        ))
    }

    /// The (detached, job) pids from [`detached_child_script`].
    pub(crate) fn wait_for_children(pid_file: &std::path::Path) -> (u32, u32) {
        let job = wait_for_pid(std::path::Path::new(&format!("{}.job", pid_file.display())));
        (wait_for_pid(pid_file), job)
    }

    /// After a stop: the detached child is gone, the same-session job is
    /// not (closing a tab must not kill a shell's `nohup` jobs).
    pub(crate) fn assert_only_detached_stopped((detached, job): (u32, u32), what: &str) {
        let gone = wait_until_gone(detached);
        let job_kept = crate::test_support::pid_alive(job) == Some(true);
        reap_leftover(detached);
        reap_leftover(job);
        assert!(gone, "detached child {detached} survived {what}");
        assert!(job_kept, "same-session job {job} was killed by {what}");
    }

    /// Kills a leftover test process so a failed run does not leak it.
    pub(crate) fn reap_leftover(pid: u32) {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        if pid > 1 && crate::test_support::pid_alive(pid) == Some(true) {
            // SAFETY: kill(2) on one pid the test itself started.
            unsafe { kill(pid as i32, SIGKILL) };
        }
    }

    // Review: the guards were untested. A remembered pid that now has
    // another start time (reused), sits in our own group, is our own pid,
    // or is init is never signalled.
    #[test]
    fn remembered_pids_that_fail_a_guard_are_never_signalled() {
        let mut reused = spawn_group("sleep 30");
        let mut same_group = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let mut seen = proc_info(reused.id()).expect("info for a live child");
        seen.start += 1;
        let mut detached = Detached(vec![seen]);
        let own = [same_group.id(), std::process::id(), 1];
        detached.0.extend(own.into_iter().filter_map(proc_info));
        assert!(!detached.alive(), "no remembered pid passes the guards");
        detached.signal(SIGKILL);
        thread::sleep(Duration::from_millis(200));
        let untouched = [&mut reused, &mut same_group].map(|child| {
            let alive = matches!(child.try_wait(), Ok(None));
            let _ = child.kill();
            let _ = child.wait();
            alive
        });
        assert_eq!(untouched, [true, true], "a guarded pid got SIGKILL");
    }

    // Review: the Linux `/proc/<pid>/stat` parser was never compiled on
    // macOS. A comm may hold ") " and spaces; fields count from the last ')'.
    #[test]
    fn parses_a_proc_stat_line_whose_comm_holds_parens() {
        let mut line = String::from("4242 ((a) b) (c d)) S 77 88 99 34816 -1");
        for field in 9..=21 {
            line.push_str(&format!(" {field}"));
        }
        line.push_str(" 123456 0 0\n");
        let info = parse_stat(4242, &line).expect("parsed");
        assert_eq!((info.pid, info.ppid, info.pgid), (4242, 77, 88));
        assert_eq!((info.sid, info.start), (99, 123456));
        assert!(parse_stat(1, "1 (short) S 0").is_none());
        assert!(parse_stat(1, "no parens at all").is_none());
    }

    /// `kill(pid, 0)`: works on macOS too (there is no `/proc` there).
    fn process_alive(pid: u32) -> bool {
        crate::test_support::pid_alive(pid) != Some(false)
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn spawn_ping() -> std::process::Child {
        crate::platform::quiet_command("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 > NUL"])
            .spawn()
            .expect("spawn cmd")
    }

    // The job reports its active processes, and a kill empties it.
    #[test]
    fn running_is_real_and_turns_false_after_kill() {
        let mut child = spawn_ping();
        let guard = PidGuard::adopt(child.id());
        let group = guard.group();
        assert!(group.running(), "the job holds the child");
        group.kill();
        let _ = child.wait();
        let deadline = Instant::now() + Duration::from_secs(5);
        while group.running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!group.running(), "no active process after kill");
    }

    // terminate() types the ask through the writer channel.
    #[test]
    fn terminate_sends_the_stop_ask_to_the_writer() {
        let mut child = spawn_ping();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut guard = PidGuard::adopt(child.id());
        guard.set_writer(tx);
        guard.group().terminate();
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(WriterMsg::Ask(bytes)) => assert_eq!(bytes, crate::pty::session::WINDOWS_STOP_ASK),
            _ => panic!("no stop ask was sent"),
        }
        guard.kill();
        let _ = child.wait();
    }
}
