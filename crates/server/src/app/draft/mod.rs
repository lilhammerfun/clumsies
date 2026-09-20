//! Draft editing, ordered operations, and reconciliation.

pub mod dto;
mod handler;
mod repository;
pub(crate) mod routes;
mod service;

pub(crate) mod model;

pub(crate) use repository::{
    ensure_drafts_authored_by, insert_draft_event, invalidate_draft_candidates,
    load_draft_operations, user_ref,
};

pub use service::{
    append_draft_operation, create_draft, create_draft_operation_batch, create_draft_rebase,
    create_draft_reconciliation_candidate, discard_draft, get_draft,
    get_draft_reconciliation_candidate, list_draft_events, list_drafts, update_draft,
};

pub(crate) use service::{
    apply_draft_rebase_in_tx, apply_operation, auto_rebase_draft_in_tx, create_draft_in_tx,
    create_reconciliation_candidate_in_tx, draft_result_hash, draft_result_state,
    load_draft_detail, target_ref_for_draft, validate_org_draft_operation_inputs_are_selected,
    validate_stored_org_draft_operations_are_selected,
};
