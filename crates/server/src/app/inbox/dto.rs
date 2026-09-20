//! Inbox wire types; receipts never change the underlying review or memory.
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Pagination inputs; an omitted limit uses one hundred subjects.
#[derive(Deserialize)]
pub(super) struct InboxQuery {
    /// Opaque last subject key from the preceding page.
    pub(super) cursor: Option<String>,
    /// Requested page size, limited to two hundred.
    pub(super) limit: Option<i64>,
}

/// A personal, aggregated notification and its latest acknowledged versions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InboxNotification {
    /// Stable subject key within the authenticated user's inbox.
    pub notification_id: String,
    /// Project whose current membership controls access.
    pub project_id: String,
    /// Display name resolved at read time.
    pub project_name: String,
    /// Semantic reason for the most recent notification.
    pub kind: String,
    /// Stable review or project identifier for navigation.
    pub target_id: String,
    /// Current review title, or the project name for shared updates.
    pub title: String,
    /// Display name of the latest actor, when still available.
    pub actor_name: Option<String>,
    /// Latest aggregated notification revision.
    pub version: i64,
    /// Read revision; marking the current notification unread clears it.
    pub read_version: i64,
    /// Highest revision explicitly archived by this user.
    pub archived_version: i64,
    /// Whether the source still requires this user's decision or revision.
    pub needs_action: bool,
    /// Current lifecycle of the referenced review, when applicable.
    pub review_status: Option<String>,
    /// Time of the most recent meaningful event.
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
}

/// A keyset page, including read and archived notifications.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InboxListResponse {
    /// Accessible notifications in stable subject-key order.
    pub items: Vec<InboxNotification>,
    /// Last subject key when another page exists.
    pub next_cursor: Option<String>,
}

/// Receipt mutation bound to the version actually displayed to the user.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateInboxRequest {
    /// Observed revision; a stale receipt must not acknowledge a newer event.
    pub version: i64,
    /// Read, unread, archive, or restore the displayed notification.
    pub action: InboxAction,
}

/// User-only notification actions, with no domain side effects.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxAction {
    /// Acknowledge this version.
    Read,
    /// Mark the current displayed version unread without changing its archive state.
    Unread,
    /// Remove this version from the inbox without changing its read state.
    Archive,
    /// Return the notification to the inbox without making it unread.
    Restore,
}
