use serde::{Deserialize, Serialize};

pub use crate::auth::api::*;
pub use crate::changes::api::*;
pub use crate::episode::api::*;
pub use crate::installation::api::*;
pub use crate::memory::api::*;
pub use crate::organization::api::*;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PageInfo {
    pub next_cursor: Option<String>,
    pub has_more: bool,
}
