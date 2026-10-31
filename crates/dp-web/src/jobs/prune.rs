//! Tác vụ dọn dẹp định kỳ: xóa tệp mồ côi, tmp cũ, phiên hết hạn, tài khoản chưa kích hoạt.
//!
//! Tất cả các tác vụ đều idempotent và an toàn khi chạy đồng thời.
//! Mỗi tác vụ trả về số mục đã xóa để ghi log.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sqlx::MySqlPool;
use tokio::fs;
use tracing::{info, warn};

// ───────────────────────────────────────────────────────────────────────────────
// Kết quả dọn dẹp
// ───────────────────────────────────────────────────────────────────────────────

/// Kết quả tổng hợp của một lần chạy `dp prune`.
#[derive(Debug, Default)]
pub struct PruneResult {
    /// Số phiên hết hạn đã xóa.
    pub sessions_removed: u64,
    /// Số tài khoản chờ xác nhận đã xóa.
    pub pending_users_removed: u64,
    /// Số lượt nhận trang hết hạn đã tự động giải phóng.
    pub stale_claims_released: u64,
    /// Số tệp tmp cũ đã xóa.
    pub tmp_files_removed: u64,
    /// Số tệp ảnh mồ côi đã xóa.
    pub orphan_images_removed: u64,
    /// Số tệp hiển thị mồ côi đã xóa.
    pub orphan_display_removed: u64,
}

// ───────────────────────────────────────────────────────────────────────────────
// Dọn dẹp toàn bộ
// ───────────────────────────────────────────────────────────────────────────────

/// Chạy tất cả tác vụ dọn dẹp và trả về thống kê.
///
/// - `data_dir`: thư mục gốc lưu trữ tệp (cấu hình `storage.data_dir`)
/// - `orphan_age_hours`: số giờ tệp mồ côi phải tồn tại trước khi bị xóa (mặc định 24)
/// - `pending_user_days`: số ngày tài khoản chờ xác nhận được giữ lại (mặc định 7)
/// - `claim_timeout_hours`: số giờ trang được giữ trước khi tự động giải phóng (mặc định 72)
pub async fn run_all(
    pool: &MySqlPool,
    data_dir: &str,
    orphan_age_hours: u64,
    pending_user_days: u64,
    claim_timeout_hours: u32,
) -> PruneResult {
    let mut result = PruneResult::default();

    // 1. Xóa phiên hết hạn
    match dp_db::queries::sessions::prune_expired_sessions(pool).await {
        Ok(n) => {
            result.sessions_removed = n;
            info!(removed = n, "pruned expired sessions");
        }
        Err(e) => warn!(error = %e, "failed to prune expired sessions"),
    }

    // 2. Xóa tài khoản chờ xác nhận cũ
    match dp_db::queries::users::prune_pending_users(pool, pending_user_days as u32).await {
        Ok(n) => {
            result.pending_users_removed = n;
            info!(removed = n, "pruned unactivated pending users");
        }
        Err(e) => warn!(error = %e, "failed to prune pending users"),
    }

    // 3. Tự động giải phóng các lượt nhận trang đã quá hạn
    match dp_db::queries::page_rounds::release_stale_claims(pool, claim_timeout_hours).await {
        Ok(n) => {
            result.stale_claims_released = n;
            info!(released = n, "released stale page claims");
        }
        Err(e) => warn!(error = %e, "failed to release stale page claims"),
    }

    // 4. Xóa tệp tmp cũ
    let tmp_dir = PathBuf::from(data_dir).join("tmp");
    result.tmp_files_removed =
        prune_tmp_dir(&tmp_dir, Duration::from_secs(orphan_age_hours * 3600)).await;

    // 5. Xóa tệp ảnh và hiển thị mồ côi
    let age = Duration::from_secs(orphan_age_hours * 3600);
    let (orphan_img, orphan_disp) = prune_orphan_files(pool, data_dir, age).await;
    result.orphan_images_removed = orphan_img;
    result.orphan_display_removed = orphan_disp;

    result
}

// ───────────────────────────────────────────────────────────────────────────────
// Dọn dẹp thư mục tmp
// ───────────────────────────────────────────────────────────────────────────────

/// Xóa tất cả các mục (tệp và thư mục) trong `tmp_dir` cũ hơn `max_age`.
///
/// Trả về số mục đã xóa.
async fn prune_tmp_dir(tmp_dir: &Path, max_age: Duration) -> u64 {
    if !tmp_dir.exists() {
        return 0;
    }

    let mut removed = 0u64;

    let mut rd = match fs::read_dir(tmp_dir).await {
        Ok(r) => r,
        Err(e) => {
            warn!(path = %tmp_dir.display(), error = %e, "không thể đọc thư mục tmp");
            return 0;
        }
    };

    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path();
        if !is_older_than(&path, max_age).await {
            continue;
        }

        let result = if path.is_dir() {
            fs::remove_dir_all(&path).await
        } else {
            fs::remove_file(&path).await
        };

        match result {
            Ok(()) => {
                removed += 1;
                info!(path = %path.display(), "xóa mục tmp cũ");
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e, "không thể xóa mục tmp");
            }
        }
    }

    removed
}

// ───────────────────────────────────────────────────────────────────────────────
// Dọn dẹp tệp mồ côi
// ───────────────────────────────────────────────────────────────────────────────

/// Duyệt `data/images/{project_id}/{page_id}.{ext}` và `data/display/{project_id}/{page_id}.jpg`.
///
/// Xóa bất kỳ tệp nào không có hàng DB tương ứng và cũ hơn `max_age`.
/// Trả về `(orphan_images_removed, orphan_display_removed)`.
async fn prune_orphan_files(pool: &MySqlPool, data_dir: &str, max_age: Duration) -> (u64, u64) {
    let images_root = PathBuf::from(data_dir).join("images");
    let display_root = PathBuf::from(data_dir).join("display");

    let orphan_img = prune_orphan_dir(pool, &images_root, max_age).await;
    let orphan_disp = prune_orphan_dir(pool, &display_root, max_age).await;

    (orphan_img, orphan_disp)
}

/// Duyệt `root/{project_id}/{page_id}.{ext}`, kiểm tra DB và xóa nếu mồ côi.
async fn prune_orphan_dir(pool: &MySqlPool, root: &Path, max_age: Duration) -> u64 {
    if !root.exists() {
        return 0;
    }

    let mut removed = 0u64;

    // Duyệt từng thư mục project_id
    let mut proj_rd = match fs::read_dir(root).await {
        Ok(r) => r,
        Err(e) => {
            warn!(path = %root.display(), error = %e, "không thể đọc thư mục gốc ảnh");
            return 0;
        }
    };

    while let Ok(Some(proj_entry)) = proj_rd.next_entry().await {
        let proj_path = proj_entry.path();
        if !proj_path.is_dir() {
            continue;
        }

        let Some(proj_id_str) = proj_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(project_id) = proj_id_str.parse::<u64>() else {
            continue;
        };

        // Duyệt tệp trong thư mục project
        let mut page_rd = match fs::read_dir(&proj_path).await {
            Ok(r) => r,
            Err(e) => {
                warn!(path = %proj_path.display(), error = %e, "không thể đọc thư mục project ảnh");
                continue;
            }
        };

        while let Ok(Some(page_entry)) = page_rd.next_entry().await {
            let page_path = page_entry.path();
            if !page_path.is_file() {
                continue;
            }

            // Lấy page_id từ stem tên tệp
            let Some(stem) = page_path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(page_id) = stem.parse::<u64>() else {
                continue;
            };

            // Chỉ xóa nếu tệp đủ cũ
            if !is_older_than(&page_path, max_age).await {
                continue;
            }

            // Kiểm tra xem page_id có tồn tại trong DB không
            match page_exists_in_db(pool, project_id, page_id).await {
                Ok(true) => {
                    // Tệp hợp lệ, bỏ qua
                }
                Ok(false) => {
                    // Tệp mồ côi — xóa
                    match fs::remove_file(&page_path).await {
                        Ok(()) => {
                            removed += 1;
                            info!(
                                path = %page_path.display(),
                                project_id,
                                page_id,
                                "xóa tệp ảnh mồ côi"
                            );
                        }
                        Err(e) => {
                            warn!(
                                path = %page_path.display(),
                                error = %e,
                                "không thể xóa tệp ảnh mồ côi"
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        page_id,
                        error = %e,
                        "lỗi DB khi kiểm tra trang mồ côi — bỏ qua"
                    );
                }
            }
        }
    }

    removed
}

// ───────────────────────────────────────────────────────────────────────────────
// Trợ năng
// ───────────────────────────────────────────────────────────────────────────────

/// Trả về `true` nếu tệp tồn tại trong DB tại bảng `pages`.
async fn page_exists_in_db(
    pool: &MySqlPool,
    project_id: u64,
    page_id: u64,
) -> Result<bool, sqlx::Error> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM pages WHERE id = ? AND project_id = ? LIMIT 1")
            .bind(page_id)
            .bind(project_id)
            .fetch_optional(pool)
            .await?;

    Ok(row.is_some())
}

/// Trả về `true` nếu thời gian sửa đổi cuối của đường dẫn vượt quá `max_age`.
async fn is_older_than(path: &Path, max_age: Duration) -> bool {
    let meta = match fs::metadata(path).await {
        Ok(m) => m,
        Err(_) => return false,
    };
    let modified = match meta.modified() {
        Ok(t) => t,
        Err(_) => return false,
    };
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO);
    age > max_age
}

// ───────────────────────────────────────────────────────────────────────────────
// Tests
// ───────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::fs;

    #[tokio::test]
    async fn test_prune_tmp_removes_old_files() {
        let tmp = TempDir::new().unwrap();
        let old_file = tmp.path().join("old.tmp");
        fs::write(&old_file, b"test").await.unwrap();

        // Xóa với max_age = 0 (tức thì)
        let removed = prune_tmp_dir(tmp.path(), Duration::ZERO).await;
        assert_eq!(removed, 1);
        assert!(!old_file.exists());
    }

    #[tokio::test]
    async fn test_prune_tmp_skips_new_files() {
        let tmp = TempDir::new().unwrap();
        let new_file = tmp.path().join("new.tmp");
        fs::write(&new_file, b"test").await.unwrap();

        // max_age = 24 giờ — tệp vừa tạo không bị xóa
        let removed = prune_tmp_dir(tmp.path(), Duration::from_secs(86400)).await;
        assert_eq!(removed, 0);
        assert!(new_file.exists());
    }

    #[tokio::test]
    async fn test_prune_tmp_nonexistent_dir() {
        let removed = prune_tmp_dir(Path::new("/nonexistent/path/tmp"), Duration::ZERO).await;
        assert_eq!(removed, 0);
    }
}
