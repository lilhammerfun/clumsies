//! Request and response data for audit event resources.

use crate::pagination::PageInfo;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEvent {
    pub event_id: String,
    pub actor_user_id: Option<String>,
    pub actor_display_name: Option<String>,
    pub actor_email: Option<String>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub target_display_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEventListResponse {
    pub items: Vec<AuditEvent>,
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
