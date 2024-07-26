//! Extensions for [`std::process::Command`] that operate on concepts from cap-std.
//!
//! The key APIs here are:
//!
//! - File descriptor passing
//! - Changing to a file-descriptor relative directory

use cap_std::fs::Dir;
use cap_std::io_lifetimes;
use cap_tempfile::cap_std;
use io_lifetimes::OwnedFd;
use rustix::fd::{AsFd, FromRawFd, IntoRawFd};
use std::fs::File;
use std::os::unix::process::CommandExt;
use std::sync::Arc;

/// Extension trait for [`std::process::Command`].
///
/// [`cap_std::fs::Dir`]: https://docs.rs/cap-std/latest/cap_std/fs/struct.Dir.html
pub trait CapStdExtCommandExt {
    /// Pass a file descriptor into the target process, which will see it as
    /// the provided file descriptor number. You must
    /// to choose e.g. file descriptor 3 or above unless you're very specifically
    /// intending to replace one of the standard I/O streams.
    fn take_fd_n(&mut self, fd: Arc<OwnedFd>, target: i32) -> &mut Self;

    /// Pass a [`Dir`] to the child.
    fn take_dirfd_n(&mut self, fd: Dir, target: i32) -> &mut Self {
        self.take_fd_n(Arc::new(fd.into()), target)
    }

    /// Pass a [`File`] to the child.
    fn take_file_n(&mut self, fd: File, target: i32) -> &mut Self {
        self.take_fd_n(Arc::new(fd.into()), target)
    }

    /// Use the given directory as the current working directory for the process.
    fn cwd_dir(&mut self, dir: Dir) -> &mut Self;
}

#[allow(unsafe_code)]
impl CapStdExtCommandExt for std::process::Command {
    fn take_fd_n(&mut self, fd: Arc<OwnedFd>, target: i32) -> &mut Self {
        unsafe {
            self.pre_exec(move || {
                let mut target = OwnedFd::from_raw_fd(target);
                rustix::io::dup2(&*fd, &mut target)?;
                // Intentionally leak into the child.
                let _ = target.into_raw_fd();
                Ok(())
            });
        }
        self
    }

    fn cwd_dir(&mut self, dir: Dir) -> &mut Self {
        unsafe {
            self.pre_exec(move || {
                rustix::process::fchdir(dir.as_fd())?;
                Ok(())
            });
        }
        self
    }
}
