//! Internal values and invariants for bundle resources.

/// Locked collection metadata used for a revision-checked edit.
pub(crate) struct PersonalBundleSnapshot {
    /// Human-readable resource name; normalization occurs at the use-case boundary.
    pub(crate) name: String,
    /// Human-readable explanation associated with the resource.
    pub(crate) description: String,
    /// Monotonic revision used for optimistic concurrency and cache validation.
    pub(crate) revision: i64,
}
