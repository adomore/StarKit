//! The single write path (INV-3).
//!
//! Temp file in the **same directory** as the target → write → fsync the file →
//! rename over the target → best-effort fsync of the directory.
//!
//! Every rule here exists because of a specific way this goes wrong:
//!
//! - **Same directory, not the system temp dir.** `rename` is only atomic within
//!   a filesystem; across one it degrades to copy-then-delete, which is exactly
//!   the torn write we are preventing.
//! - **fsync before rename.** Without it the rename can reach disk before the
//!   data, so a crash leaves a correctly-named file full of nothing. This is the
//!   classic ext4 truncate-on-crash story, and it is real on NTFS too.
//! - **Unique temp name.** Two concurrent batch writers to one directory (FR-8)
//!   must not collide on a fixed `.tmp` name and corrupt each other.
//! - **Temp file removed on failure.** A crash may leave one behind — the point
//!   of INV-3 is that the *target* is never partial, not that no debris exists —
//!   but an ordinary error must not litter.
//!
//! There is deliberately no in-place mode and no append: the input file is never
//! opened for writing at all (starkit-io/CLAUDE.md).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{IoError, Result};

/// Distinguishes concurrent writers within one process. Paired with the pid,
/// this is enough to keep two batch runs from colliding in a shared output dir.
/// Not a security measure — a hostile local user can still race us, and no
/// userspace scheme fixes that.
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_path_for(target: &Path) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let stem = target
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "out".to_string());
    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    dir.join(format!(
        ".{}.{}.{}.starkit-tmp",
        stem,
        std::process::id(),
        n
    ))
}

/// Write `bytes` to `path` atomically. The only write path in the workspace.
///
/// On success the target contains exactly `bytes`. On failure the target is
/// untouched — it keeps its previous contents, or stays absent if it did not
/// exist. It is never left partially written.
pub fn atomic_write(path: impl AsRef<Path>, bytes: &[u8]) -> Result<()> {
    let path = path.as_ref();
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    if !dir.as_os_str().is_empty() {
        std::fs::create_dir_all(dir).map_err(|e| IoError::io(dir, e))?;
    }

    let tmp = temp_path_for(path);
    let result = write_and_sync(&tmp, bytes).and_then(|()| {
        // Rename last: until this instant the target is whatever it was.
        std::fs::rename(&tmp, path).map_err(|e| IoError::io(path, e))
    });

    if result.is_err() {
        // Best-effort: the original error is what the caller needs to see, and a
        // failure to clean up must not mask it.
        let _ = std::fs::remove_file(&tmp);
    }
    if result.is_ok() {
        sync_dir(dir);
    }
    result
}

fn write_and_sync(tmp: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = std::fs::File::create(tmp).map_err(|e| IoError::io(tmp, e))?;
    f.write_all(bytes).map_err(|e| IoError::io(tmp, e))?;
    // Durability before visibility: the rename must not be able to land first.
    f.sync_all().map_err(|e| IoError::io(tmp, e))?;
    Ok(())
}

/// fsync the directory so the rename itself is durable.
///
/// Best-effort by design: it is a no-op on Windows (opening a directory as a
/// file is not permitted, and NTFS does not need it), and on network mounts it
/// can fail for reasons that say nothing about our data. The rename has already
/// succeeded at this point, so failing the whole write here would report a
/// non-problem.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

/// The temp path this module would use for `target`, exposed for tests that need
/// to assert on debris.
#[cfg(test)]
pub(crate) fn debris_in(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().ends_with(".starkit-tmp"))
                        .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_exact_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("out.bin");
        atomic_write(&p, b"hello").expect("write");
        assert_eq!(std::fs::read(&p).expect("read"), b"hello");
    }

    #[test]
    fn replaces_an_existing_file_completely() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("out.bin");
        atomic_write(&p, b"a-long-previous-content").expect("write");
        atomic_write(&p, b"short").expect("write");
        // Not "shortprevious-content": a rename replaces, it does not overwrite
        // in place.
        assert_eq!(std::fs::read(&p).expect("read"), b"short");
    }

    #[test]
    fn leaves_no_temp_debris_on_success() {
        let dir = tempfile::tempdir().expect("tempdir");
        atomic_write(dir.path().join("out.bin"), b"x").expect("write");
        assert!(
            debris_in(dir.path()).is_empty(),
            "temp file survived a successful write"
        );
    }

    #[test]
    fn the_temp_file_lives_next_to_the_target_not_in_the_system_temp_dir() {
        // A cross-filesystem rename is not atomic; keeping the temp file beside
        // the target is what makes the guarantee hold.
        let target = Path::new("/some/output/dir/image.tiff");
        let tmp = temp_path_for(target);
        assert_eq!(tmp.parent(), target.parent());
    }

    #[test]
    fn concurrent_writers_do_not_collide_on_one_temp_name() {
        let target = Path::new("out.tiff");
        let a = temp_path_for(target);
        let b = temp_path_for(target);
        assert_ne!(a, b, "two writers would race on the same temp file");
    }

    #[test]
    fn creates_missing_parent_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("nested/deeper/out.bin");
        atomic_write(&p, b"x").expect("write");
        assert!(p.exists());
    }

    /// INV-3, the guarantee itself: a failed write must leave the previous
    /// target byte-for-byte intact. Simulated by pointing the write at a path
    /// whose parent is a *file*, so the temp create fails after the target
    /// already exists.
    #[test]
    fn a_failed_write_leaves_the_previous_target_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("out.bin");
        atomic_write(&target, b"original").expect("seed");

        // `target/child` — the parent of `child` is a regular file, so both the
        // dir creation and the temp create must fail.
        let doomed = target.join("child");
        assert!(
            atomic_write(&doomed, b"new").is_err(),
            "write should have failed"
        );
        assert_eq!(
            std::fs::read(&target).expect("read"),
            b"original",
            "a failed write corrupted the existing file"
        );
    }

    #[test]
    fn a_failed_write_leaves_no_debris() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("out.bin");
        atomic_write(&target, b"original").expect("seed");
        let _ = atomic_write(target.join("child"), b"new");
        assert!(debris_in(dir.path()).is_empty(), "a failed write littered");
    }

    #[test]
    fn error_carries_the_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("out.bin");
        atomic_write(&target, b"x").expect("seed");
        let err = atomic_write(target.join("child"), b"y").expect_err("must fail");
        assert!(
            err.to_string().contains("child") || err.to_string().contains("out.bin"),
            "error must name the path: {err}"
        );
    }
}
