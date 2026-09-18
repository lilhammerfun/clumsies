//! Internal values and invariants for bundle resources.

pub(crate) struct PersonalBundleSnapshot {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) revision: i64,
}
