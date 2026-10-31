//! Zip archive inspection, entry enumeration, and safe extraction.
//!
//! This module enforces zip-bomb guards (entry count ≤ 5,000; uncompressed size ≤ 4 GB)
//! and rejects path traversal attacks (`..` or absolute paths).

use std::{
    io::Read,
    path::{Component, Path, PathBuf},
};

use zip::ZipArchive;

use super::error::IngestError;

/// The absolute maximum number of zip entries allowed (including directories).
const MAX_ENTRIES: usize = 5_000;
/// 4 GiB uncompressed size guard against zip bombs.
const MAX_UNCOMPRESSED: u64 = 4 * 1024 * 1024 * 1024;

/// Summary of a zip archive's valid, file-typed entries.
#[derive(Debug, Clone)]
pub struct ZipEntry {
    /// Sanitized, relative entry name.
    pub name: String,
    /// Uncompressed byte size.
    pub size: u64,
    /// True when the original entry is a directory.
    pub is_dir: bool,
}

/// Inspect a zip archive opened from bytes, returning the list of entries.
///
/// # Errors
/// Returns [`IngestError`] if zip is malformed, has too many entries, total
/// uncompressed size exceeds the guard, or any entry has a dangerous path.
pub fn inspect_zip(data: &[u8]) -> Result<Vec<ZipEntry>, IngestError> {
    let cursor = std::io::Cursor::new(data);
    let mut archive = ZipArchive::new(cursor).map_err(|e| IngestError::NotAZip(e.to_string()))?;

    let count = archive.len();
    if count > MAX_ENTRIES {
        return Err(IngestError::TooManyEntries(count));
    }

    let mut entries = Vec::with_capacity(count);
    let mut total_uncompressed: u64 = 0;

    for i in 0..count {
        let entry = archive
            .by_index(i)
            .map_err(|e| IngestError::NotAZip(e.to_string()))?;

        let raw_name = entry.name().to_owned();
        validate_entry_path(&raw_name)?;

        let size = entry.size();
        total_uncompressed = total_uncompressed.saturating_add(size);
        if total_uncompressed > MAX_UNCOMPRESSED {
            return Err(IngestError::ZipBomb(total_uncompressed));
        }

        let is_dir = entry.is_dir();
        entries.push(ZipEntry {
            name: raw_name,
            size,
            is_dir,
        });
    }

    Ok(entries)
}

/// Validate that a zip entry path component does not escape the target directory.
///
/// # Errors
/// Returns [`IngestError::PathTraversal`] if the path contains `..` or is absolute.
pub fn validate_entry_path(name: &str) -> Result<(), IngestError> {
    let path = Path::new(name);

    // Reject absolute paths
    if path.is_absolute() {
        return Err(IngestError::PathTraversal(name.to_owned()));
    }

    // Reject any `..` component
    for component in path.components() {
        if component == Component::ParentDir {
            return Err(IngestError::PathTraversal(name.to_owned()));
        }
    }

    Ok(())
}

/// Read the raw bytes of a specific entry from the zip archive.
///
/// # Errors
/// Returns [`IngestError::NotAZip`] if reading the entry fails.
pub fn read_entry(data: &[u8], entry_name: &str) -> Result<Vec<u8>, IngestError> {
    let cursor = std::io::Cursor::new(data);
    let mut archive = ZipArchive::new(cursor).map_err(|e| IngestError::NotAZip(e.to_string()))?;

    let mut entry = archive
        .by_name(entry_name)
        .map_err(|e| IngestError::NotAZip(format!("entry '{entry_name}': {e}")))?;

    let mut buf = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut buf).map_err(|e| IngestError::Io {
        path: PathBuf::from(entry_name),
        source: e,
    })?;

    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_safe_paths() {
        let cases = ["file.png", "subdir/file.txt", "a/b/c/d.jpg", "page 01.png"];
        for c in &cases {
            assert!(validate_entry_path(c).is_ok(), "should accept: {c}");
        }
    }

    #[test]
    fn test_reject_traversal() {
        let cases = ["../etc/passwd", "foo/../../../etc/passwd", "foo/../bar"];
        for c in &cases {
            assert!(
                validate_entry_path(c).is_err(),
                "should reject traversal: {c}"
            );
        }
    }

    #[test]
    fn test_reject_absolute() {
        // /etc/passwd on Unix is absolute
        let path = "/etc/passwd";
        assert!(validate_entry_path(path).is_err());
    }
}
