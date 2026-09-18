//! HTTP extraction and response construction for audit event resources.

use super::{dto, service};
use crate::app::auth::AuthPrincipal;
use crate::http::{HttpError, require_org_admin};
use crate::pagination::{AdminSearchQuery, parse_admin_page};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Extension, Query, State};

pub(super) async fn list_admin_audit_events(
    State(state): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<AdminSearchQuery>,
) -> Result<Json<dto::AuditEventListResponse>, HttpError> {
    require_org_admin(&principal)?;
    let page = parse_admin_page(query.page)?;
    Ok(Json(
        service::list_admin_audit_events(
            &state.pool,
            &principal.org_id,
            page.offset,
            page.limit,
            query.q.as_deref(),
        )
        .await?,
    ))
}
