//! Starting helper processes quietly and stopping a command with everything it started.

use std::path::PathBuf;

/// Windows: start console programs without opening a console window (a host
/// that runs Forge without a console, such as an IDE, would otherwise flash one
/// for every git, hook or shell call). No-op elsewhere.
pub fn no_window(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let _ = cmd;
}

/// A command that is stopped as a whole: its own process group on Unix (the
/// terminal's Ctrl-C doesn't reach it); on Windows a hidden console, started
/// suspended so it joins its job before it can start anything. The spawned
/// process MUST then go to [`ProcessTree::attach`], which resumes it.
pub fn isolate(cmd: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    }
}

/// A started command and every process it starts.
/// Unix: its process group. Windows: a job object the process is placed in
/// right after it starts; children it creates join the job.
#[derive(Debug)]
pub struct ProcessTree {
    pid: u32,
    #[cfg(windows)]
    job: Option<Job>,
}

impl ProcessTree {
    /// Call right after spawning `pid` with [`isolate`].
    pub fn attach(pid: u32) -> ProcessTree {
        #[cfg(windows)]
        {
            let job = Job::for_process(pid);
            resume_threads(pid);
            ProcessTree { pid, job }
        }
        #[cfg(not(windows))]
        {
            ProcessTree { pid }
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Stop the tree. Unix: SIGTERM to the group, SIGKILL 300 ms later.
    /// Windows: terminate every process in the job at once (there is no
    /// polite signal for a windowless console tree).
    pub fn kill(&self) {
        #[cfg(unix)]
        {
            let pgid = self.pid as i32;
            // SAFETY: plain syscalls; a negative pid addresses the process group.
            unsafe {
                libc::kill(-pgid, libc::SIGTERM);
            }
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
            });
        }
        #[cfg(windows)]
        {
            match &self.job {
                Some(j) => j.terminate(),
                None => terminate_pid(self.pid),
            }
        }
    }
}

#[cfg(windows)]
#[derive(Debug)]
struct Job(isize);

#[cfg(windows)]
impl Job {
    fn for_process(pid: u32) -> Option<Job> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};
        // SAFETY: handles are checked for null and closed exactly once.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let job = Job(job as isize);
            let proc = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if proc.is_null() {
                return None;
            }
            let ok = AssignProcessToJobObject(job.0 as _, proc) != 0;
            CloseHandle(proc);
            ok.then_some(job)
        }
    }

    fn terminate(&self) {
        // SAFETY: the handle is owned by self.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0 as _, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: owned handle, closed once. Without KILL_ON_JOB_CLOSE the
        // processes keep running, as an exited command's background children do on Unix.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0 as _);
        }
    }
}

/// Resume the threads of a process started with CREATE_SUSPENDED (its main
/// thread; std doesn't hand out the thread handle).
#[cfg(windows)]
fn resume_threads(pid: u32) {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    // SAFETY: snapshot and thread handles are checked and closed; the entry is sized.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return;
        }
        let mut e: THREADENTRY32 = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut more = Thread32First(snap, &mut e) != 0;
        while more {
            if e.th32OwnerProcessID == pid {
                let t = OpenThread(THREAD_SUSPEND_RESUME, 0, e.th32ThreadID);
                if !t.is_null() {
                    ResumeThread(t);
                    CloseHandle(t);
                }
            }
            more = Thread32Next(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
}

#[cfg(windows)]
fn terminate_pid(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    // SAFETY: handle checked and closed.
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !h.is_null() {
            TerminateProcess(h, 1);
            CloseHandle(h);
        }
    }
}

/// Where a bare program name resolves on PATH. On Windows each `PATHEXT`
/// extension is tried (`npx` -> `npx.cmd`), which `Command::new` doesn't do.
pub fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        let mut v: Vec<String> = pathext.split(';').filter(|e| !e.is_empty()).map(str::to_string).collect();
        if std::path::Path::new(name).extension().is_some() {
            v.insert(0, String::new());
        }
        v
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&path)
        .find_map(|dir| exts.iter().map(|e| dir.join(format!("{name}{e}"))).find(|p| p.is_file()))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Read;
    use std::process::Stdio;

    #[test]
    fn kill_stops_the_command_and_its_children() {
        let mut c = std::process::Command::new("/bin/sh");
        c.arg("-c").arg("sleep 30 & sleep 30").stdout(Stdio::piped());
        isolate(&mut c);
        let mut child = c.spawn().unwrap();
        let tree = ProcessTree::attach(child.id());
        let started = std::time::Instant::now();
        tree.kill();
        let _ = child.wait();
        // The background `sleep` held stdout too: EOF means it is gone.
        let mut s = String::new();
        child.stdout.take().unwrap().read_to_string(&mut s).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}
