// Copyright 2020 Google LLC
//
// Use of this source code is governed by an MIT-style license that can be found
// in the LICENSE file or at https://opensource.org/licenses/MIT.

//! A handler and associated types for the timeline action.

use std::path::PathBuf;
use std::result::Result;

use rrg_proto::convert::FromLossy;

use crate::session::{self, Session};

/// Arguments of the `get_filesystem_timeline` action.
pub struct Args {
    root: PathBuf,
}

/// Result of the `get_filesystem_timeline` action.
pub struct Item {
    /// SHA-256 digest of the timeline batch sent to the blob sink.
    blob_sha256: [u8; 32],
    // TODO(@panhania): Add support for `entry_count`.
}

impl FromLossy<crate::fs::Entry> for rrg_proto::v2::get_filesystem_timeline::Entry {

    fn from_lossy(entry: crate::fs::Entry) -> Self {
        let mut proto = Self::default();
        proto.set_path(rrg_proto::path::into_bytes(entry.path));
        proto.set_size(entry.metadata.len());

        fn nanos(time: std::time::SystemTime) -> Option<i64> {
            i64::try_from(rrg_proto::nanos(time).ok()?).ok()
        }

        let atime_nanos = entry.metadata.accessed().ok().and_then(nanos);
        if let Some(atime_nanos) = atime_nanos {
            proto.set_atime_ns(atime_nanos);
        }

        let mtime_nanos = entry.metadata.modified().ok().and_then(nanos);
        if let Some(mtime_nanos) = mtime_nanos {
            proto.set_mtime_ns(mtime_nanos);
        }

        let btime_nanos = entry.metadata.created().ok().and_then(nanos);
        if let Some(btime_nanos) = btime_nanos {
            proto.set_btime_ns(btime_nanos);
        }

        #[cfg(target_family = "unix")]
        {
            use std::os::unix::fs::MetadataExt as _;

            proto.set_mode(i64::from(entry.metadata.mode()));
            proto.set_ino(entry.metadata.ino());
            if let Some(dev) = i64::try_from(entry.metadata.dev()).ok() {
                proto.set_dev(dev);
            }
            if let Some(uid) = i64::try_from(entry.metadata.uid()).ok() {
                proto.set_uid(uid);
            }
            if let Some(gid) = i64::try_from(entry.metadata.gid()).ok() {
                proto.set_gid(gid);
            }
            proto.set_ctime_ns(entry.metadata.ctime_nsec());
        }

        // TODO: Export file attributes on Windows.
        proto
    }
}

/// Handles requests for the timeline action.
pub fn handle<S>(session: &mut S, args: Args) -> session::Result<()>
where
    S: Session,
{
    use sha2::Digest as _;

    let entries = crate::fs::walk_dir(&args.root)
        .map_err(crate::session::Error::action)?
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(error) => {
                log::warn!("failed to obtain directory entry: {}", error);
                None
            }
        })
        .map(rrg_proto::v2::get_filesystem_timeline::Entry::from_lossy);

    for batch in crate::gzchunked::encode(entries) {
        let batch = batch
            .map_err(crate::session::Error::action)?;

        let blob = crate::blob::Blob::from(batch);
        let blob_sha256 = sha2::Sha256::digest(blob.as_bytes()).into();

        session.send(crate::Sink::Blob, blob)?;
        session.reply(Item {
            blob_sha256,
        })?;
    }

    Ok(())
}

impl crate::request::Args for Args {

    type Proto = rrg_proto::v2::get_filesystem_timeline::Args;

    fn from_proto(mut proto: Self::Proto) -> Result<Args, crate::request::ParseArgsError> {
        use crate::request::ParseArgsError;

        let root = PathBuf::try_from(proto.take_root())
            .map_err(|error| ParseArgsError::invalid_field("root", error))?;

        Ok(Args {
            root: root,
        })
    }
}

impl crate::response::Item for Item {

    type Proto = rrg_proto::v2::get_filesystem_timeline::Result;

    fn into_proto(self) -> Self::Proto {
        let mut proto = Self::Proto::default();
        proto.set_blob_sha256(self.blob_sha256.into());

        proto
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    use session::FakeSession as Session;

    #[test]
    fn test_non_existent_path() {
        let tempdir = tempfile::tempdir().unwrap();

        let request = Args {
            root: tempdir.path().join("foo")
        };

        let mut session = Session::new();
        assert!(handle(&mut session, request).is_err());
    }

    #[test]
    fn test_empty_dir() {
        let tempdir = tempfile::tempdir().unwrap();
        let tempdir_path = tempdir.path().to_path_buf();

        let request = Args {
            root: tempdir_path.clone(),
        };

        let mut session = Session::new();
        assert!(handle(&mut session, request).is_ok());

        let entries = entries(&session);
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_dir_with_files() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::File::create(tempdir.path().join("a")).unwrap();
        std::fs::File::create(tempdir.path().join("b")).unwrap();
        std::fs::File::create(tempdir.path().join("c")).unwrap();

        let request = Args {
            root: tempdir.path().to_path_buf(),
        };

        let mut session = Session::new();
        assert!(handle(&mut session, request).is_ok());

        let mut entries = entries(&session);
        entries.sort_by_key(|entry| entry.get_path().to_owned());

        assert_eq!(entries.len(), 3);
        assert_eq!(path(&entries[0]), Some(tempdir.path().join("a")));
        assert_eq!(path(&entries[1]), Some(tempdir.path().join("b")));
        assert_eq!(path(&entries[2]), Some(tempdir.path().join("c")));
    }

    #[test]
    fn test_dir_with_nested_dirs() {
        let tempdir = tempfile::tempdir().unwrap();
        let tempdir_path = tempdir.path().to_path_buf();

        std::fs::create_dir_all(tempdir_path.join("a").join("b")).unwrap();

        let request = Args {
            root: tempdir_path.clone(),
        };

        let mut session = Session::new();
        assert!(handle(&mut session, request).is_ok());

        let mut entries = entries(&session);
        entries.sort_by_key(|entry| entry.get_path().to_owned());

        assert_eq!(entries.len(), 2);
        assert_eq!(path(&entries[0]), Some(tempdir_path.join("a")));
        assert_eq!(path(&entries[1]), Some(tempdir_path.join("a").join("b")));
    }

    // Symlinking is supported only on Unix-like systems.
    #[cfg(target_family = "unix")]
    #[test]
    fn test_dir_with_circular_symlinks() {
        let tempdir = tempfile::tempdir().unwrap();

        let root_path = tempdir.path().to_path_buf();
        let dir_path = root_path.join("dir");
        let symlink_path = dir_path.join("symlink");

        std::fs::create_dir(&dir_path).unwrap();
        std::os::unix::fs::symlink(&dir_path, &symlink_path).unwrap();

        let request = Args {
            root: root_path.clone(),
        };

        let mut session = Session::new();
        assert!(handle(&mut session, request).is_ok());

        let mut entries = entries(&session);
        entries.sort_by_key(|entry| entry.get_path().to_owned());

        assert_eq!(entries.len(), 2);
        assert_eq!(path(&entries[0]), Some(dir_path));
        assert_eq!(path(&entries[1]), Some(symlink_path));
    }

    #[test]
    fn test_dir_with_unicode_files() {
        let tempdir = tempfile::tempdir().unwrap();

        let root_path = tempdir.path().to_path_buf();
        let file_path_1 = root_path.join("zażółć gęślą jaźń");
        let file_path_2 = root_path.join("што й па мору");

        std::fs::File::create(&file_path_1).unwrap();
        std::fs::File::create(&file_path_2).unwrap();

        let request = Args {
            root: root_path.clone(),
        };

        let mut session = Session::new();
        assert!(handle(&mut session, request).is_ok());

        let mut entries = entries(&session);
        entries.sort_by_key(|entry| entry.get_path().to_owned());

        assert_eq!(entries.len(), 2);

        // macOS mangles Unicode-specific characters in filenames.
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(path(&entries[0]), Some(file_path_1));
            assert_eq!(path(&entries[1]), Some(file_path_2));
        }
    }

    #[test]
    fn test_file_metadata() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("foo"), b"123456789").unwrap();

        let request = Args {
            root: tempdir.path().to_path_buf(),
        };

        let mut session = Session::new();
        assert!(handle(&mut session, request).is_ok());

        let mut entries = entries(&session);
        entries.sort_by_key(|entry| entry.get_path().to_owned());

        assert_eq!(entries.len(), 1);
        assert_eq!(path(&entries[0]), Some(tempdir.path().join("foo")));
        assert_eq!(entries[0].get_size(), 9);

        // Information about the file mode, user and group identifiers is
        // available only on UNIX systems.
        #[cfg(target_family = "unix")]
        {
            let mode = entries[0].get_mode() as libc::mode_t;
            assert_eq!(mode & libc::S_IFMT, libc::S_IFREG);

            let uid = unsafe { libc::getuid() };
            assert_eq!(entries[0].get_uid(), uid.into());

            let gid = unsafe { libc::getgid() };
            assert_eq!(entries[0].get_gid(), gid.into());
        }
    }

    #[test]
    fn test_hardlink_metadata() {
        let tempdir = tempfile::tempdir().unwrap();

        let root_path = tempdir.path().to_path_buf();
        let file_path = root_path.join("file");
        let hardlink_path = root_path.join("hardlink");

        std::fs::File::create(&file_path).unwrap();
        std::fs::hard_link(&file_path, &hardlink_path).unwrap();

        let request = Args {
            root: root_path.clone(),
        };

        let mut session = Session::new();
        assert!(handle(&mut session, request).is_ok());

        let mut entries = entries(&session);
        entries.sort_by_key(|entry| entry.get_path().to_owned());

        assert_eq!(entries.len(), 2);
        assert_eq!(path(&entries[0]), Some(file_path));
        assert_eq!(path(&entries[1]), Some(hardlink_path));

        // Information about inode is not available on Windows.
        #[cfg(not(target_os = "windows"))]
        assert_eq!(entries[0].get_ino(), entries[1].get_ino());
    }

    /// Retrieves timeline entries from the given session object.
    fn entries(session: &Session) -> Vec<rrg_proto::timeline::TimelineEntry> {
        let blob_count = session.parcel_count(crate::Sink::Blob);
        let reply_count = session.reply_count();
        assert_eq!(blob_count, reply_count);

        let blobs = session.parcels::<crate::blob::Blob>(crate::Sink::Blob);

        crate::gzchunked::decode(blobs.map(|blob| blob.as_bytes()))
            .map(Result::unwrap)
            .collect()
    }

    /// Constructs a path for the given timeline entry.
    fn path(entry: &rrg_proto::timeline::TimelineEntry) -> Option<PathBuf> {
        rrg_proto::path::from_bytes(entry.get_path().to_owned()).ok()
    }
}
