//! Writing files safely: through a temporary file and a rename, so a crash or
//! a power cut mid-write leaves either the old file or the new one, never a
//! truncated one. Secrets (identity, server key, files with passwords) are
//! also made readable by the current user only.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Replaces `path` with `data` atomically. The file gets the default
/// permissions of its folder.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    replace_with(path, data, |p| File::options().write(true).create_new(true).open(p))
}

/// Replaces `path` with `data` atomically; only the current user can read or
/// change the file: mode 0600 on Unix, on Windows an access list with only
/// the current user and the system, not inherited from the folder (portable
/// mode, a server folder or any other folder others can read).
pub fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    replace_with(path, data, create_private)
}

/// `path` with ".tmp" appended: "identity.txt" -> "identity.txt.tmp".
fn tmp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

fn replace_with(path: &Path, data: &[u8], create: impl FnOnce(&Path) -> io::Result<File>) -> io::Result<()> {
    let tmp = tmp_path(path);
    // A leftover from a crash. Creating anew (never opening an existing file)
    // matters on Windows: the access list is only applied to a new file.
    let _ = std::fs::remove_file(&tmp);
    let written = create(&tmp).and_then(|mut f| {
        f.write_all(data)?;
        f.sync_all()
    });
    // The rename keeps the temporary file's permissions.
    let res = written.and_then(|()| std::fs::rename(&tmp, path));
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

#[cfg(unix)]
fn create_private(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    File::options().write(true).create_new(true).mode(0o600).open(path)
}

#[cfg(windows)]
fn create_private(path: &Path) -> io::Result<File> {
    win::create_private(path)
}

#[cfg(windows)]
mod win {
    use std::ffi::{OsStr, OsString};
    use std::fs::File;
    use std::io;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::io::{FromRawHandle, RawHandle};
    use std::path::Path;
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// Creates a new file whose access list allows only the current user and
    /// the system ("P": protected, nothing inherited from the folder).
    pub(super) fn create_private(path: &Path) -> io::Result<File> {
        let sddl = format!("D:P(A;;FA;;;{})(A;;FA;;;SY)", current_user_sid()?);
        let sd = SecurityDescriptor::from_sddl(&sddl)?;
        let sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: 0,
        };
        let name = wide(path.as_os_str());
        // SAFETY: `name` is NUL-terminated, `sa` and the descriptor it points
        // to outlive the call.
        let handle =
            unsafe { CreateFileW(name.as_ptr(), GENERIC_WRITE, 0, &sa, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, null_mut()) };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a valid handle that nothing else owns.
        Ok(unsafe { File::from_raw_handle(handle as RawHandle) })
    }

    /// The current user's SID as a string ("S-1-5-21-...").
    pub(super) fn current_user_sid() -> io::Result<String> {
        let mut token: HANDLE = null_mut();
        // SAFETY: the pseudo handle of the current process needs no closing.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Handle(token);
        let mut len = 0u32;
        // SAFETY: asks only for the size.
        unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut len) };
        // u64 for the alignment of TOKEN_USER.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: the buffer is at least `len` bytes.
        if unsafe { GetTokenInformation(token.0, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: filled by the call above, aligned.
        let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut text = null_mut();
        // SAFETY: `sid` points into `buf`, which is alive.
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a NUL-terminated string allocated by the system.
        let s = unsafe { from_wide_ptr(text) };
        unsafe { LocalFree(text.cast()) };
        Ok(s)
    }

    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: an open handle owned by this value.
            unsafe { CloseHandle(self.0) };
        }
    }

    struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

    impl SecurityDescriptor {
        fn from_sddl(sddl: &str) -> io::Result<Self> {
            let text = wide(OsStr::new(sddl));
            let mut sd = null_mut();
            // SAFETY: `text` is NUL-terminated.
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    text.as_ptr(),
                    SDDL_REVISION_1,
                    &mut sd,
                    null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(sd))
        }
    }

    impl Drop for SecurityDescriptor {
        fn drop(&mut self) {
            // SAFETY: allocated by the system with LocalAlloc.
            unsafe { LocalFree(self.0) };
        }
    }

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }

    /// # Safety
    /// `p` must point to a NUL-terminated UTF-16 string.
    unsafe fn from_wide_ptr(p: *const u16) -> String {
        let mut len = 0;
        // SAFETY: the caller promises a terminating NUL.
        while unsafe { *p.add(len) } != 0 {
            len += 1;
        }
        // SAFETY: `len` characters before the NUL.
        let units = unsafe { std::slice::from_raw_parts(p, len) };
        OsString::from_wide(units).to_string_lossy().into_owned()
    }

    /// The file's access list in SDDL, for the tests.
    #[cfg(test)]
    pub(super) fn dacl_sddl(path: &Path) -> io::Result<String> {
        use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
        use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

        let name = wide(path.as_os_str());
        let mut sd = null_mut();
        // SAFETY: `name` is NUL-terminated; only the descriptor is asked for.
        let err = unsafe {
            GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut sd,
            )
        };
        if err != 0 {
            return Err(io::Error::from_raw_os_error(err as i32));
        }
        sddl_of(&SecurityDescriptor(sd))
    }

    /// `sddl` as Windows writes it back: well-known SIDs abbreviated (the
    /// built-in Administrator, which CI runners use, becomes "LA"), so it can
    /// be compared with `dacl_sddl`. For the tests.
    #[cfg(test)]
    pub(super) fn canonical_sddl(sddl: &str) -> io::Result<String> {
        sddl_of(&SecurityDescriptor::from_sddl(sddl)?)
    }

    /// The access list of `sd` in SDDL. For the tests.
    #[cfg(test)]
    fn sddl_of(sd: &SecurityDescriptor) -> io::Result<String> {
        use windows_sys::Win32::Security::Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW;
        use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

        let mut text = null_mut();
        // SAFETY: a valid descriptor.
        let ok = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                sd.0,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a NUL-terminated string allocated by the system.
        let s = unsafe { from_wide_ptr(text) };
        unsafe { LocalFree(text.cast()) };
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_atomically_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        // A leftover temporary file from a crash does not get in the way.
        std::fs::write(tmp_path(&path), b"junk").unwrap();
        write_private(&path, b"three").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"three");
        assert!(!tmp_path(&path).exists());
    }

    #[test]
    fn failed_write_keeps_the_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        write_atomic(&path, b"old").unwrap();
        let res = replace_with(&path, b"new", |_| Err(io::Error::other("disk full")));
        assert!(res.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert!(!tmp_path(&path).exists());
    }

    #[cfg(unix)]
    #[test]
    fn private_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        std::fs::write(&path, b"old, readable by all").unwrap();
        write_private(&path, b"secret").unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[cfg(windows)]
    #[test]
    fn private_file_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        // An existing file with the folder's (inherited) permissions.
        std::fs::write(&path, b"old").unwrap();
        write_private(&path, b"secret").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"secret");
        let sid = win::current_user_sid().unwrap();
        let dacl = win::dacl_sddl(&path).unwrap();
        // Protected (nothing inherited), exactly the user and the system.
        assert_eq!(dacl, win::canonical_sddl(&format!("D:P(A;;FA;;;{sid})(A;;FA;;;SY)")).unwrap());
    }
}
