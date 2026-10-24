//! Database entity representations mapping directly to SQL tables.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// A registered user in the system.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct User {
    /// Unique user ID.
    pub id: u64,
    /// Unique username.
    pub username: String,
    /// User email address.
    pub email: String,
    /// Argon2id password hash.
    pub password_hash: String,
    /// Real or display name.
    pub real_name: String,
    /// Account status ('pending', 'active', 'disabled').
    pub status: String,
    /// Activation token for pending accounts.
    pub activate_token: Option<String>,
    /// Account creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last successful login timestamp.
    pub last_login_at: Option<DateTime<Utc>>,
}

/// A role assigned to a user ('proofer', 'pm', 'pp', 'admin').
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct UserRole {
    /// Target user ID.
    pub user_id: u64,
    /// Role name.
    pub role: String,
    /// Timestamp when role was granted.
    pub granted_at: DateTime<Utc>,
}

/// Active user session.
#[derive(Debug, Clone, FromRow)]
pub struct Session {
    /// SHA256 hash of the 32-byte secret session token.
    pub token_hash: Vec<u8>,
    /// Associated user ID.
    pub user_id: u64,
    /// Per-session CSRF token.
    pub csrf_token: String,
    /// Session issuance timestamp.
    pub created_at: DateTime<Utc>,
    /// Last seen activity timestamp.
    pub last_seen_at: DateTime<Utc>,
    /// Absolute expiration timestamp.
    pub expires_at: DateTime<Utc>,
    /// Client IP address in binary form (IPv4/IPv6).
    pub ip: Option<Vec<u8>>,
    /// Client User-Agent header string.
    pub user_agent: String,
}

/// A book project.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Project {
    /// Unique project ID.
    pub id: u64,
    /// Project title / book name.
    pub name: String,
    /// Book author.
    pub author: String,
    /// Current lifecycle phase.
    pub phase: String,
    /// Current active round if in 'in_round' phase.
    pub current_round: Option<String>,
    /// Whether project is paused.
    pub on_hold: i8,
    /// Tone mark style convention.
    pub tone_style: String,
    /// Permitted character set identifier.
    pub charset: String,
    /// User ID of project manager.
    pub manager_id: u64,
    /// Attribution/source of page scans.
    pub image_source: String,
    /// Project manager instructions / comments.
    pub comments: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// A page within a project.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Page {
    /// Unique page ID.
    pub id: u64,
    /// Parent project ID.
    pub project_id: u64,
    /// 1-based page sequence number.
    pub page_number: i32,
    /// Relative path to original image scan.
    pub image_path: String,
    /// Image width in pixels.
    pub image_width: u32,
    /// Image height in pixels.
    pub image_height: u32,
    /// Immutable OCR text baseline.
    pub ocr_text: Option<String>,
}

/// State and text for a page in a specific round.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PageRound {
    /// Target page ID.
    pub page_id: u64,
    /// Round ID ('P1', 'P2', etc.).
    pub round: String,
    /// State ('available', 'in_progress', 'done').
    pub state: String,
    /// Current text for this round.
    pub text: Option<String>,
    /// ID of proofreader holding or having completed this page.
    pub proofer_id: Option<u64>,
    /// Timestamp when page was claimed.
    pub claimed_at: Option<DateTime<Utc>>,
    /// Timestamp when page was saved as done.
    pub done_at: Option<DateTime<Utc>>,
}

/// Historical version snapshot of a page.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PageVersion {
    /// Unique version record ID.
    pub id: u64,
    /// Page ID.
    pub page_id: u64,
    /// Round during which this version was saved.
    pub round: String,
    /// Saved text content.
    pub text: String,
    /// Proofreader who saved this version.
    pub proofer_id: Option<u64>,
    /// Save timestamp.
    pub created_at: DateTime<Utc>,
}

/// Audit event tracking project lifecycle transitions.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProjectEvent {
    /// Unique event ID.
    pub id: u64,
    /// Target project ID.
    pub project_id: u64,
    /// Previous phase.
    pub from_phase: Option<String>,
    /// New phase.
    pub to_phase: Option<String>,
    /// Previous round.
    pub from_round: Option<String>,
    /// New round.
    pub to_round: Option<String>,
    /// User who triggered transition (None if automatic).
    pub actor_id: Option<u64>,
    /// Contextual note.
    pub note: String,
    /// Event timestamp.
    pub created_at: DateTime<Utc>,
}

/// Page tally record for daily proofing limits.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PageTally {
    /// User ID.
    pub user_id: u64,
    /// Round ID.
    pub round: String,
    /// Date (Asia/Ho_Chi_Minh).
    pub day: NaiveDate,
    /// Completed page count.
    pub count: u32,
}

/// System settings key-value pair.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Setting {
    /// Setting key.
    pub k: String,
    /// Setting value.
    pub v: String,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}
