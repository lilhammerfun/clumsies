//! Request and response data for audit event resources.

use crate::pagination::PageInfo;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Actor, action, and target recorded for a security-relevant operation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEvent {
    /// Stable opaque event identity; ordering uses the separate synchronization cursor.
    pub event_id: String,
    /// Identity responsible for the operation or audit event.
    pub actor_user_id: Option<String>,
    /// Display name of the user responsible for the event, when available.
    pub actor_display_name: Option<String>,
    /// Email of the user responsible for the event, when available.
    pub actor_email: Option<String>,
    /// Mutation to apply to the referenced resource.
    pub action: String,
    /// Resource category affected by the audit event.
    pub target_type: String,
    /// Stable identity of the resource affected by the operation.
    pub target_id: Option<String>,
    /// Current human-readable label of the audited resource, when resolvable.
    pub target_display_name: Option<String>,
    /// UTC timestamp at which the record was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Administrative audit history page with continuation metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEventListResponse {
    /// Ordered results included in the current response page.
    pub items: Vec<AuditEvent>,
    /// Continuation metadata for the returned result page.
    pub page_info: PageInfo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_timestamps_use_rfc3339() {
        let event = AuditEvent {
            event_id: "evt_test".to_owned(),
            actor_user_id: None,
            actor_display_name: None,
            actor_email: None,
            action: "test.created".to_owned(),
            target_type: "test".to_owned(),
            target_display_name: None,
            target_id: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
        };

        let json = serde_json::to_value(&event).expect("serialize audit event");
        assert_eq!(json["created_at"], "1970-01-01T00:00:00Z");

        let decoded: AuditEvent = serde_json::from_value(json).expect("deserialize audit event");
        assert_eq!(decoded.created_at, OffsetDateTime::UNIX_EPOCH);
    }
}
