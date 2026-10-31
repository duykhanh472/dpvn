//! Errors that can occur during package inspection, extraction, and validation.

use std::path::PathBuf;

/// Ingest pipeline errors.
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    /// The uploaded file is not a valid zip archive.
    #[error("tập tin tải lên không phải là tập tin zip hợp lệ: {0}")]
    NotAZip(String),

    /// Total entries in the zip archive exceeds the safety limit of 5,000.
    #[error("số lượng tập tin trong zip vượt quá giới hạn 5.000 (tìm thấy {0})")]
    TooManyEntries(usize),

    /// Total uncompressed size exceeds the zip-bomb safety limit of 4 GB.
    #[error("tổng dung lượng giải nén vượt quá 4 GB (tổng: {0} byte)")]
    ZipBomb(u64),

    /// Entry path attempts directory traversal.
    #[error("phát hiện đường dẫn không an toàn (path traversal): {0}")]
    PathTraversal(String),

    /// File has an unsupported extension.
    #[error("loại tập tin không được hỗ trợ trong gói: {0}")]
    UnsupportedFileType(String),

    /// Corrupt image or invalid magic bytes.
    #[error("tập tin ảnh '{file}' bị lỗi hoặc không đúng định dạng ({reason})")]
    InvalidImage {
        /// Name of the corrupted or invalid file.
        file: String,
        /// Description of the error.
        reason: String,
    },

    /// Image dimensions violate the allowed range (200x200 to 10000x10000).
    #[error(
        "kích thước ảnh '{file}' không hợp lệ: {width}x{height} (cho phép 200x200 đến 10.000x10.000)"
    )]
    InvalidDimensions {
        /// Offending image filename.
        file: String,
        /// Detected width.
        width: u32,
        /// Detected height.
        height: u32,
    },

    /// Text file contains invalid UTF-8 codepoints.
    #[error("tập tin văn bản '{file}' không phải là UTF-8 hợp lệ: {reason}")]
    InvalidUtf8 {
        /// Offending text filename.
        file: String,
        /// Description of UTF-8 decoding error.
        reason: String,
    },

    /// Project is not in 'draft' phase.
    #[error(
        "dự án đang ở trạng thái '{0}', chỉ dự án ở trạng thái 'draft' mới có thể nhập dữ liệu"
    )]
    ProjectNotInDraft(String),

    /// Project already has pages and --replace was not specified.
    #[error("dự án đã có dữ liệu trang. Sử dụng tùy chọn thay thế (--replace) nếu muốn ghi đè")]
    ProjectAlreadyHasPages,

    /// No valid pages could be parsed from the package.
    #[error("không tìm thấy trang hợp lệ nào trong gói tải lên")]
    NoPagesFound,

    /// General I/O error during file operations.
    #[error("lỗi I/O khi xử lý tập tin {path:?}: {source}")]
    Io {
        /// Path where the I/O error occurred.
        path: PathBuf,
        /// Underlying std::io::Error.
        #[source]
        source: std::io::Error,
    },

    /// Database failure during transactional page insertion.
    #[error("lỗi cơ sở dữ liệu khi nhập trang: {0}")]
    Db(#[from] dp_db::DbError),
}
