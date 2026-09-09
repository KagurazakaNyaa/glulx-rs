//! Startup-only OS memory policy. Windows counts committed process memory;
//! Linux counts virtual address space (including mappings), not resident RAM.
use std::io;

pub fn apply_limit_mib(mib: u32) -> io::Result<()> {
    if mib == 0 {
        return Ok(());
    }
    let bytes = u64::from(mib) * 1024 * 1024;
    apply_limit_bytes(bytes)
}

pub fn apply_limit_bytes(bytes: u64) -> io::Result<()> {
    if bytes == 0 {
        return Ok(());
    }
    apply(bytes)
}

#[cfg(target_os = "linux")]
fn apply(bytes: u64) -> io::Result<()> {
    // Preserve any stricter inherited soft or hard limit. Lowering the hard
    // limit is irreversible in an unprivileged process; apply only at startup.
    let mut old = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: old and limit are valid rlimit values, and no pointers escape.
    unsafe {
        if libc::getrlimit(libc::RLIMIT_AS, &mut old) != 0 {
            return Err(io::Error::last_os_error());
        }
        let requested = libc::rlim_t::try_from(bytes).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "process memory limit is too large",
            )
        })?;
        let limit = libc::rlimit {
            rlim_cur: old.rlim_cur.min(requested),
            rlim_max: old.rlim_max.min(requested),
        };
        if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(windows)]
fn apply(bytes: u64) -> io::Result<()> {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::{JobObjects::*, Threading::GetCurrentProcess},
    };
    let maximum = usize::try_from(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "process memory limit is too large",
        )
    })?;
    // SAFETY: the job is unnamed, the information buffer has the documented
    // layout/size, and GetCurrentProcess supplies a valid pseudo-handle.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        info.ProcessMemoryLimit = maximum;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&info) as u32,
        ) == 0
            || AssignProcessToJobObject(job, GetCurrentProcess()) == 0
        {
            let error = io::Error::last_os_error();
            CloseHandle(job);
            return Err(error);
        }
        // A job remains alive while it has an associated process. No kill-on-
        // close flag is set, so closing our handle keeps the policy in force.
        CloseHandle(job);
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", windows)))]
fn apply(_bytes: u64) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "process hard memory limits are supported only on Linux and Windows",
    ))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn process_memory_limit_is_enforced_in_a_child() {
        const CHILD: &str = "GLULX_TEST_PROCESS_LIMIT_CHILD";
        if std::env::var_os(CHILD).is_some() {
            super::apply_limit_mib(512).unwrap();
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            // SAFETY: valid output pointer; the failed mapping is never used.
            unsafe {
                assert_eq!(libc::getrlimit(libc::RLIMIT_AS, &mut limit), 0);
                assert!(limit.rlim_cur <= 512 * 1024 * 1024);
                assert!(limit.rlim_max <= 512 * 1024 * 1024);
                let allocation = libc::mmap(
                    std::ptr::null_mut(),
                    1024 * 1024 * 1024,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                );
                assert_eq!(allocation, libc::MAP_FAILED);
            }
            return;
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "process_memory::tests::process_memory_limit_is_enforced_in_a_child",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
    }
}
