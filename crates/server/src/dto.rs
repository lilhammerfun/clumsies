//! Response shapes shared by multiple resources.

use serde::{Deserialize, Serialize};

/// Stable resource identity and confirmation of a completed deletion.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeleteResult {
    /// Whether the requested deletion completed successfully.
    pub deleted: bool,
    /// Stable identifier of the resource described by this result.
    pub id: String,
}
