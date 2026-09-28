//! Optional JSON-only file/URL action extension. Never embed in a core codec.
use crate::file_url_actions::{ActionId, ActionProfiles, ActionRevision};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileUrlActionsSetParams {
    pub expected_revision: ActionRevision,
    pub profiles: ActionProfiles,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PaneTargetResolveParams {
    pub pane_id: String,
    pub viewport_row: u16,
    pub col: u16,
    pub content_revision: u64,
    pub offset_from_bottom: u64,
    pub expected_revision: ActionRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PaneTargetActionParams {
    pub pane_id: String,
    pub target_id: String,
    pub action_id: ActionId,
    pub revision: ActionRevision,
}
