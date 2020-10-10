// Copyright 2020 Google LLC
//
// Use of this source code is governed by an MIT-style license that can be found
// in the LICENSE file or at https://opensource.org/licenses/MIT.

use std::path::Path;

pub fn flags<P>(path: P) -> std::io::Result<u32> where
    P: AsRef<Path>
{
    let file = std::fs::File::open(path)?;

    let mut flags = 0;
    let code = unsafe {
        use std::os::unix::io::AsRawFd as _;
        ioctls::fs_ioc_getflags(file.as_raw_fd(), &mut flags)
    };

    if code == 0 {
        Ok(flags as u32)
    } else {
        Err(std::io::Error::from_raw_os_error(code))
    }
}

#[cfg(test)]
mod tests {

    use std::fs::File;

    use super::*;

    #[test]
    fn test_flags_non_existing() {
        let tempdir = tempfile::tempdir().unwrap();

        assert!(flags(tempdir.path().join("foo")).is_err());
    }

    #[test]
    fn test_flags_noatime() {
        // https://elixir.bootlin.com/linux/v5.8.14/source/include/uapi/linux/fs.h#L245
        const FS_NOATIME_FL: std::os::raw::c_long = 0x00000080;

        let tempdir = tempfile::tempdir().unwrap();
        let tempfile = File::create(tempdir.path().join("foo")).unwrap();

        unsafe {
            use std::os::unix::io::AsRawFd as _;
            let fd = tempfile.as_raw_fd();

            assert_eq!(ioctls::fs_ioc_setflags(fd, &FS_NOATIME_FL), 0);
        }

        let flags = flags(tempdir.path().join("foo")).unwrap();
        assert_eq!(flags & FS_NOATIME_FL as u32, FS_NOATIME_FL as u32);
    }
}
