use anyhow::Result;

use cap_std::fs::{Dir, File, Permissions, PermissionsExt};
use cap_std_ext::cmdext::CapStdExtCommandExt;
use cap_std_ext::dirext::{CapStdExtDirExt, WalkConfiguration};
use cap_std_ext::{cap_std, RootDir};
use rustix::path::DecInt;
use std::cmp::Ordering;
use std::ffi::OsStr;
use std::io::Write;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::{process::Command, sync::Arc};

#[test]
fn take_fd() -> Result<()> {
    let mut c = Command::new("/bin/bash");
    c.arg("-c");
    c.arg("wc -c <&5");
    let (r, w) = rustix::pipe::pipe()?;
    let r = Arc::new(r);
    let mut w: File = w.into();
    c.take_fd_n(r, 5);
    write!(w, "hello world")?;
    drop(w);
    c.stdout(std::process::Stdio::piped());
    let s = c.output()?;
    assert!(s.status.success());
    assert_eq!(s.stdout.as_slice(), b"11\n");
    Ok(())
}

#[test]
fn fchdir() -> Result<()> {
    static CONTENTS: &[u8] = b"hello world";

    fn new_cmd() -> Command {
        let mut c = Command::new("/usr/bin/cat");
        c.arg("somefile");
        c
    }

    fn test_cmd(mut c: Command) -> Result<()> {
        let st = c.output()?;
        if !st.status.success() {
            anyhow::bail!("Failed to exec cat");
        }
        assert_eq!(st.stdout.as_slice(), CONTENTS);
        Ok(())
    }

    let td = Arc::new(cap_tempfile::tempdir(cap_std::ambient_authority())?);

    td.write("somefile", CONTENTS)?;

    let mut c = new_cmd();
    let subdir = td.open_dir(".")?;
    c.cwd_dir(subdir.try_clone()?);
    test_cmd(c).unwrap();

    Ok(())
}

#[test]
fn optionals() -> Result<()> {
    let td = cap_tempfile::tempdir(cap_std::ambient_authority())?;

    // file
    assert!(td.open_optional("bar")?.is_none());
    assert!(td.metadata_optional("bar").unwrap().is_none());
    assert!(!(td.remove_file_optional("bar")?));
    td.write("bar", "testcontents")?;
    assert!(td.metadata_optional("bar").unwrap().is_some());
    assert!(td.symlink_metadata_optional("bar").unwrap().is_some());
    assert_eq!(td.read("bar")?.as_slice(), b"testcontents");
    assert!(td.remove_file_optional("bar")?);

    // directory
    assert!(td.open_dir_optional("somedir")?.is_none());
    td.create_dir("somedir")?;
    assert!(td.open_dir_optional("somedir")?.is_some());
    Ok(())
}

#[test]
fn ensuredir() -> Result<()> {
    let td = cap_tempfile::tempdir(cap_std::ambient_authority())?;

    let p = Path::new("somedir");
    let b = &cap_std::fs::DirBuilder::new();
    assert!(td.metadata_optional(p)?.is_none());
    assert!(td.symlink_metadata_optional(p)?.is_none());
    assert!(td.ensure_dir_with(p, b).unwrap());
    assert!(td.metadata_optional(p)?.is_some());
    assert!(td.symlink_metadata_optional(p)?.is_some());
    assert!(!td.ensure_dir_with(p, b).unwrap());

    let p = Path::new("somedir/without/existing-parent");
    // We should fail because the intermediate directory doesn't exist.
    assert!(td.ensure_dir_with(p, b).is_err());
    // Now create the parent
    assert!(td.ensure_dir_with(p.parent().unwrap(), b).unwrap());
    assert!(td.ensure_dir_with(p, b).unwrap());
    assert!(!td.ensure_dir_with(p, b).unwrap());

    // Verify we don't replace a file
    let p = Path::new("somefile");
    td.write(p, "some file contents")?;
    assert!(td.ensure_dir_with(p, b).is_err());

    // Broken symlinks aren't followed and are errors
    let p = Path::new("linksrc");
    td.symlink("linkdest", p)?;
    assert!(td.metadata(p).is_err());
    assert!(td
        .symlink_metadata_optional(p)
        .unwrap()
        .unwrap()
        .is_symlink());
    // Non-broken symlinks are also an error
    assert!(td.ensure_dir_with(p, b).is_err());
    td.create_dir("linkdest")?;
    assert!(td.ensure_dir_with(p, b).is_err());
    assert!(td.metadata_optional(p).unwrap().unwrap().is_dir());

    Ok(())
}

#[test]
fn test_remove_all_optional() -> Result<()> {
    let td = cap_tempfile::tempdir(cap_std::ambient_authority())?;

    let p = Path::new("somedir");
    assert!(!td.remove_all_optional(p).unwrap());
    td.create_dir(p)?;
    assert!(td.remove_all_optional(p).unwrap());
    let subpath = p.join("foo/bar");
    td.create_dir_all(subpath)?;
    assert!(td.remove_all_optional(p).unwrap());

    // regular file
    td.write(p, "test")?;
    assert!(td.remove_all_optional(p).unwrap());

    // symlinks; broken and not
    let p = Path::new("linksrc");
    td.symlink("linkdest", p)?;
    assert!(td.remove_all_optional(p).unwrap());
    td.symlink("linkdest", p)?;
    assert!(td.remove_all_optional(p).unwrap());

    Ok(())
}

/// Hack to determine the default mode for a file; we could
/// on Linux actually parse /proc/self/umask as is done in cap_tempfile,
/// but eh this is just to cross check with that code.
fn default_mode(d: &Dir) -> Result<Permissions> {
    let f = cap_tempfile::TempFile::new(d)?;
    Ok(f.as_file().metadata()?.permissions())
}

#[test]
fn link_tempfile_with() -> Result<()> {
    let td = cap_tempfile::tempdir(cap_std::ambient_authority())?;
    let p = Path::new("foo");
    td.atomic_replace_with(p, |f| writeln!(f, "hello world"))
        .unwrap();
    assert_eq!(td.read_to_string(p).unwrap().as_str(), "hello world\n");
    let default_perms = default_mode(&td)?;
    assert_eq!(td.metadata(p)?.permissions(), default_perms);

    td.atomic_replace_with(p, |f| writeln!(f, "atomic replacement"))
        .unwrap();
    assert_eq!(
        td.read_to_string(p).unwrap().as_str(),
        "atomic replacement\n"
    );

    let e = td
        .atomic_replace_with(p, |f| {
            writeln!(f, "should not be written")?;
            Err::<(), _>(std::io::Error::new(std::io::ErrorKind::Other, "oops"))
        })
        .err()
        .unwrap();
    assert!(e.to_string().contains("oops"));
    // We should not have written to the file!
    assert_eq!(
        td.read_to_string(p).unwrap().as_str(),
        "atomic replacement\n"
    );

    td.atomic_write(p, "atomic replacement write\n").unwrap();
    assert_eq!(
        td.read_to_string(p).unwrap().as_str(),
        "atomic replacement write\n"
    );
    assert_eq!(td.metadata(p)?.permissions(), default_perms);

    td.atomic_write_with_perms(p, "atomic replacement 3\n", Permissions::from_mode(0o700))
        .unwrap();
    assert_eq!(
        td.read_to_string(p).unwrap().as_str(),
        "atomic replacement 3\n"
    );
    assert_eq!(td.metadata(p)?.permissions().mode() & 0o777, 0o700);

    // Ensure we preserve the executable bit on an existing file
    assert_eq!(td.metadata(p).unwrap().permissions().mode() & 0o700, 0o700);
    td.atomic_write(p, "atomic replacement 4\n").unwrap();
    assert_eq!(
        td.read_to_string(p).unwrap().as_str(),
        "atomic replacement 4\n"
    );
    assert_eq!(td.metadata(p)?.permissions().mode() & 0o777, 0o700);

    // But we should ignore permissions on a symlink (both existing and broken)
    td.remove_file(p)?;
    let p2 = Path::new("bar");
    td.atomic_write_with_perms(p2, "link target", Permissions::from_mode(0o755))
        .unwrap();
    td.symlink(p2, p)?;
    td.atomic_write(p, "atomic replacement symlink\n").unwrap();
    assert_eq!(td.metadata(p)?.permissions(), default_perms);
    // And break the link
    td.remove_file(p2)?;
    td.atomic_write(p, "atomic replacement symlink\n").unwrap();
    assert_eq!(td.metadata(p)?.permissions(), default_perms);

    // Also test with mode 0600
    td.atomic_write_with_perms(p, "self-only file", Permissions::from_mode(0o600))
        .unwrap();
    assert_eq!(td.metadata(p).unwrap().permissions().mode() & 0o777, 0o600);
    td.atomic_write(p, "self-only file v2").unwrap();
    assert_eq!(td.metadata(p).unwrap().permissions().mode() & 0o777, 0o600);
    // But we can override
    td.atomic_write_with_perms(p, "self-only file v3", Permissions::from_mode(0o640))
        .unwrap();
    assert_eq!(td.metadata(p).unwrap().permissions().mode() & 0o777, 0o640);
    Ok(())
}

#[test]
fn test_timestamps() -> Result<()> {
    let td = cap_tempfile::tempdir(cap_std::ambient_authority())?;
    let p = Path::new("foo");
    td.atomic_replace_with(p, |f| writeln!(f, "hello world"))
        .unwrap();
    let ts0 = td.metadata(p)?.modified()?;
    // This test assumes at least second granularity on filesystem timestamps, and
    // that the system clock is not rolled back during the test.
    std::thread::sleep(std::time::Duration::from_secs(1));
    let ts1 = td.metadata(p)?.modified()?;
    assert_eq!(ts0, ts1);
    td.update_timestamps(p).unwrap();
    let ts2 = td.metadata(p)?.modified()?;
    assert_ne!(ts1, ts2);
    assert!(ts2 > ts1);

    Ok(())
}

// For now just this test is copy-pasted to verify utf8
#[test]
#[cfg(feature = "fs_utf8")]
fn ensuredir_utf8() -> Result<()> {
    use cap_std::fs_utf8::camino::Utf8Path;
    use cap_std_ext::dirext::CapStdExtDirExtUtf8;
    let td = cap_tempfile::tempdir(cap_std::ambient_authority())?;
    let td = &cap_std::fs_utf8::Dir::from_cap_std((&*td).try_clone()?);

    let p = Utf8Path::new("somedir");
    let b = &cap_std::fs::DirBuilder::new();
    assert!(td.metadata_optional(p)?.is_none());
    assert!(td.symlink_metadata_optional(p)?.is_none());
    assert!(td.ensure_dir_with(p, b).unwrap());
    assert!(td.metadata_optional(p)?.is_some());
    assert!(td.symlink_metadata_optional(p)?.is_some());
    assert!(!td.ensure_dir_with(p, b).unwrap());

    let p = Utf8Path::new("somedir/without/existing-parent");
    // We should fail because the intermediate directory doesn't exist.
    assert!(td.ensure_dir_with(p, b).is_err());
    // Now create the parent
    assert!(td.ensure_dir_with(p.parent().unwrap(), b).unwrap());
    assert!(td.ensure_dir_with(p, b).unwrap());
    assert!(!td.ensure_dir_with(p, b).unwrap());

    // Verify we don't replace a file
    let p = Utf8Path::new("somefile");
    td.write(p, "some file contents")?;
    assert!(td.ensure_dir_with(p, b).is_err());

    // Broken symlinks aren't followed and are errors
    let p = Utf8Path::new("linksrc");
    td.symlink("linkdest", p)?;
    assert!(td.metadata(p).is_err());
    assert!(td
        .symlink_metadata_optional(p)
        .unwrap()
        .unwrap()
        .is_symlink());
    // Non-broken symlinks are also an error
    assert!(td.ensure_dir_with(p, b).is_err());
    td.create_dir("linkdest")?;
    assert!(td.ensure_dir_with(p, b).is_err());
    assert!(td.metadata_optional(p).unwrap().unwrap().is_dir());

    Ok(())
}

#[test]
#[cfg(feature = "fs_utf8")]
fn filenames_utf8() -> Result<()> {
    use std::collections::BTreeSet;

    use cap_std_ext::dirext::CapStdExtDirExtUtf8;
    let td = &cap_tempfile::utf8::TempDir::new(cap_std::ambient_authority())?;

    let mut expected = BTreeSet::new();
    const N: usize = 20;
    (0..N).try_for_each(|_| -> Result<()> {
        let fname = uuid::Uuid::new_v4().to_string();

        td.write(&fname, &fname)?;
        expected.insert(fname);
        Ok(())
    })?;
    let names = td.filenames_sorted().unwrap();
    for (a, b) in expected.iter().zip(names.iter()) {
        assert_eq!(a, b);
    }

    td.create_dir(".foo").unwrap();

    let names = td
        .filenames_filtered_sorted(|_ent, name| !name.starts_with('.'))
        .unwrap();
    assert_eq!(names.len(), N);
    for name in names.iter() {
        assert!(!name.starts_with('.'));
    }
    Ok(())
}

#[test]
fn test_rootdir_open() -> Result<()> {
    let td = &cap_tempfile::TempDir::new(cap_std::ambient_authority())?;
    let root = RootDir::new(td, ".").unwrap();

    assert!(root.open_optional("foo").unwrap().is_none());

    td.create_dir("etc")?;
    td.create_dir_all("usr/lib")?;

    let authjson = "usr/lib/auth.json";
    assert!(root.open(authjson).is_err());
    assert!(root.open_optional(authjson).unwrap().is_none());
    td.write(authjson, "auth contents")?;
    assert!(root.open_optional(authjson).unwrap().is_some());
    let contents = root.read_to_string(authjson).unwrap();
    assert_eq!(&contents, "auth contents");

    td.symlink_contents("/usr/lib/auth.json", "etc/auth.json")?;

    let contents = root.read_to_string("/etc/auth.json").unwrap();
    assert_eq!(&contents, "auth contents");

    // But this should fail due to an escape
    assert!(td.read_to_string("etc/auth.json").is_err());
    Ok(())
}

#[test]
fn test_rootdir_entries() -> Result<()> {
    let td = &cap_tempfile::TempDir::new(cap_std::ambient_authority())?;
    let root = RootDir::new(td, ".").unwrap();

    td.create_dir("etc")?;
    td.create_dir_all("usr/lib")?;

    let ents = root
        .entries()
        .unwrap()
        .collect::<std::io::Result<Vec<_>>>()?;
    assert_eq!(ents.len(), 2);
    Ok(())
}

#[test]
fn test_mountpoint() -> Result<()> {
    let root = &Dir::open_ambient_dir("/", cap_std::ambient_authority())?;
    assert_eq!(root.is_mountpoint(".").unwrap(), Some(true));
    let td = &cap_tempfile::TempDir::new(cap_std::ambient_authority())?;
    assert_eq!(td.is_mountpoint(".").unwrap(), Some(false));
    Ok(())
}

#[test]
#[cfg(unix)]
fn test_reopen_as_ownedfd() -> Result<()> {
    let td = &cap_tempfile::TempDir::new(cap_std::ambient_authority())?;
    let fd = td.reopen_as_ownedfd()?;
    rustix::fs::fsync(fd)?;
    Ok(())
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn test_open_noxdev() -> Result<()> {
    let root = Dir::open_ambient_dir("/", cap_std::ambient_authority())?;
    // This hard requires the host setup to have /usr/bin on the same filesystem as /
    let usr = Dir::open_ambient_dir("/usr", cap_std::ambient_authority())?;
    assert!(usr.open_dir_noxdev("bin").unwrap().is_some());
    // Requires a mounted /proc, but that also seems ane.
    assert!(root.open_dir_noxdev("proc").unwrap().is_none());
    Ok(())
}

#[test]
fn test_walk() -> std::io::Result<()> {
    let td = cap_tempfile::TempDir::new(cap_std::ambient_authority())?;

    td.create_dir_all("usr/bin")?;
    td.create_dir_all("usr/lib/foo")?;
    td.create_dir_all("usr/share")?;
    td.create_dir_all("etc")?;
    td.write("usr/bin/bash", b"bash")?;
    td.write("usr/bin/true", b"true")?;
    td.write("usr/lib/foo/libfoo.so", b"libfoo")?;
    td.write("usr/lib/libbar.so", b"libbar")?;
    // Broken link
    td.symlink("usr/share/timezone", "usr/share/EST")?;
    // Symlink to self
    td.symlink(".", "usr/bin/selflink")?;
    td.write("etc/foo.conf", b"fooconf")?;
    td.write("etc/blah.conf", b"blahconf")?;

    // A successful walk, but skipping /etc
    {
        let mut n_dirs = 0;
        let mut n_symlinks = 0;
        let mut n_regfiles = 0;
        td.walk::<_, std::io::Error>(&WalkConfiguration::default(), |e| {
            if e.path.strip_prefix("usr/lib").is_ok() {
                return Ok(ControlFlow::Break(()));
            }
            if e.ty.is_dir() {
                n_dirs += 1;
            } else if e.ty.is_symlink() {
                n_symlinks += 1;
            } else if e.ty.is_file() {
                n_regfiles += 1;
                // Verify we can open
                let _ = e.entry.open().unwrap();
            } else {
                unreachable!();
            }
            Ok(ControlFlow::Continue(()))
        })
        .unwrap();
        assert_eq!(n_dirs, 4);
        assert_eq!(n_symlinks, 2);
        assert_eq!(n_regfiles, 4);
    }

    // An error case
    {
        let r: std::io::Result<_> = td.walk(&WalkConfiguration::default(), |e| {
            if e.path.strip_prefix("usr/share").is_ok() {
                return Err(std::io::Error::other("oops"));
            }
            return Ok(ControlFlow::Continue(()));
        });
        match r {
            Err(e) if e.kind() == std::io::ErrorKind::Other => {}
            _ => unreachable!(),
        }
    };

    Ok(())
}

#[test]
fn test_walk_sorted() -> Result<()> {
    let td = cap_tempfile::TempDir::new(cap_std::ambient_authority())?;

    for _ in 0..5 {
        let d = rand::random_range(0..1000);
        let d = format!("dir{d}");
        if td.try_exists(&d)? {
            continue;
        }
        td.create_dir(&d)?;
        for _ in 0..25 {
            let j = rand::random_range(0..10);
            let path = format!("{d}/{j}");
            td.write(&path, DecInt::new(j).as_bytes())?;
        }
    }

    // A successful walk in sorted order
    {
        let mut depth = 0;
        let mut previous: Option<PathBuf> = None;
        td.walk(
            &WalkConfiguration::default().sort_by_file_name(),
            |e| -> std::io::Result<_> {
                let curdepth = e.path.components().count();
                if curdepth != depth {
                    depth = curdepth;
                    previous = None;
                } else if let Some(previous) = previous.as_deref() {
                    assert_eq!(previous.cmp(e.path), Ordering::Less);
                } else {
                    previous = Some(e.path.to_owned());
                }
                Ok(ControlFlow::Continue(()))
            },
        )
        .unwrap();
    }

    Ok(())
}

#[test]
fn test_walk_noxdev() -> Result<()> {
    let rootfs = Dir::open_ambient_dir("/", cap_std::ambient_authority())?;

    let n_root_entries = std::fs::read_dir("/")?.count();
    let mut i = 0;
    rootfs
        .walk(
            &WalkConfiguration::default().noxdev(),
            |e| -> std::io::Result<_> {
                // Don't traverse, unless it's proc...except we shouldn't traverse that either because
                // of noxdev.
                let proc = "proc";
                let first_component = e.path.components().next().unwrap();
                let is_proc = first_component.as_os_str() == OsStr::new(proc);
                let r = if e.ty.is_dir() {
                    if is_proc {
                        ControlFlow::Continue(())
                    } else {
                        ControlFlow::Break(())
                    }
                } else {
                    ControlFlow::Continue(())
                };
                i += 1;
                Ok(r)
            },
        )
        .unwrap();
    assert_eq!(n_root_entries, i);

    Ok(())
}
