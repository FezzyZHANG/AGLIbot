//! Lifetime guard for delegates, including descendants of Windows .cmd shims.

use anyhow::Result;
use tokio::process::{Child, Command};

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    use windows_sys::Win32::System::JobObjects::*;
    use windows_sys::Win32::System::Threading::*;

    pub struct ProcessTree(OwnedHandle);

    fn owned(handle: HANDLE) -> Result<OwnedHandle> {
        anyhow::ensure!(!handle.is_null() && handle != INVALID_HANDLE_VALUE, "{}", std::io::Error::last_os_error());
        // The API returned a new, exclusively owned kernel handle.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    impl ProcessTree {
        pub fn prepare(cmd: &mut Command) -> Result<Self> {
            // No delegate code runs until attach() has installed the job. Attaching after a
            // normal spawn races a .cmd shim creating descendants (or exiting).
            cmd.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
            let job = owned(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = unsafe {
                SetInformationJobObject(
                    job.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                )
            };
            anyhow::ensure!(ok != 0, "setting delegate job limits: {}", std::io::Error::last_os_error());
            Ok(Self(job))
        }

        pub fn attach(&mut self, child: &Child) -> Result<()> {
            let process = child.raw_handle().ok_or_else(|| anyhow::anyhow!("delegate has no process handle"))?;
            let ok = unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), process) };
            anyhow::ensure!(ok != 0, "assigning delegate job: {}", std::io::Error::last_os_error());
            let pid = child.id().ok_or_else(|| anyhow::anyhow!("delegate has no pid"))?;
            let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
            let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
            entry.dwSize = std::mem::size_of_val(&entry) as u32;
            let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
            while found != 0 {
                if entry.th32OwnerProcessID == pid {
                    let thread = owned(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) })?;
                    anyhow::ensure!(
                        unsafe { ResumeThread(thread.as_raw_handle()) } != u32::MAX,
                        "resuming delegate: {}",
                        std::io::Error::last_os_error()
                    );
                    return Ok(());
                }
                found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
            }
            anyhow::bail!("could not find suspended delegate thread")
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::*;

    pub struct ProcessTree(Option<u32>);

    impl ProcessTree {
        pub fn prepare(cmd: &mut Command) -> Result<Self> {
            cmd.process_group(0);
            Ok(Self(None))
        }

        pub fn attach(&mut self, child: &Child) -> Result<()> {
            self.0 = child.id();
            Ok(())
        }
    }

    impl Drop for ProcessTree {
        fn drop(&mut self) {
            if let Some(pid) = self.0 {
                // The delegate has its own process group; never signal our caller's group.
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
        }
    }
}

pub use platform::ProcessTree;
