//! Explicit, user-configured file/URL actions. These types are JSON-only and
//! must not be added to a published binary protocol codec.
mod profiles;
mod recognition;

pub(crate) use recognition::{file_candidates_at, valid_web_url, web_url_at};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionRevision {
    pub boot_id: String,
    pub counter: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionId {
    Editor,
    SystemOpener,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMode {
    Pane,
    External,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Applicability {
    pub text_files: bool,
    pub other_files: bool,
    pub urls: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionProfile {
    pub enabled: bool,
    pub executable: String,
    pub arguments: Vec<String>,
    pub launch_mode: LaunchMode,
    pub applies_to: Applicability,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ActionProfiles {
    pub editor: ActionProfile,
    pub system_opener: ActionProfile,
}

impl Default for ActionProfiles {
    fn default() -> Self {
        Self {
            editor: ActionProfile {
                enabled: true,
                executable: "nvim".into(),
                arguments: vec![
                    "-c".into(),
                    "call cursor({line},{column})".into(),
                    "--".into(),
                    "{path}".into(),
                ],
                launch_mode: LaunchMode::Pane,
                applies_to: Applicability {
                    text_files: true,
                    other_files: false,
                    urls: false,
                },
            },
            system_opener: ActionProfile {
                enabled: true,
                executable: "open".into(),
                arguments: vec!["--".into(), "{target}".into()],
                launch_mode: LaunchMode::External,
                applies_to: Applicability {
                    text_files: true,
                    other_files: true,
                    urls: true,
                },
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActionSettings {
    pub revision: ActionRevision,
    pub profiles: ActionProfiles,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetFileKind {
    Text,
    Binary,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TargetValue {
    File {
        path: String,
        line: Option<u32>,
        column: Option<u32>,
        file_kind: TargetFileKind,
    },
    Url {
        url: String,
    },
}

impl TargetValue {
    pub fn display_target(&self) -> &str {
        match self {
            Self::File { path, .. } => path,
            Self::Url { url } => url,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::File {
                path, line, column, ..
            } => {
                crate::platform::validate_local_file_path(std::path::Path::new(path))
                    .map_err(|e| e.to_string())?;
                if path.len() > 4096
                    || line.is_some_and(|v| v == 0 || v > i32::MAX as u32)
                    || column.is_some_and(|v| v == 0 || v > i32::MAX as u32)
                    || (column.is_some() && line.is_none())
                {
                    return Err("Invalid file path or cursor position".into());
                }
                Ok(())
            }
            Self::Url { url } if valid_web_url(url) => Ok(()),
            Self::Url { .. } => Err(
                "Only valid HTTP(S) URLs without whitespace/control characters are supported"
                    .into(),
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TargetCellSpan {
    /// Row in the resolved viewport, not an absolute scrollback coordinate.
    pub row: u16,
    pub start_col: u16,
    /// Exclusive end column, measured in terminal cells rather than UTF-8 bytes.
    pub end_col: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedTarget {
    pub target_id: String,
    pub target: TargetValue,
    pub revision: ActionRevision,
    pub actions: Vec<ActionId>,
    /// Optional presentation metadata for the exact clicked source revision.
    /// It is never an execution identity and is absent on older endpoints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub highlight: Vec<TargetCellSpan>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActionPermit {
    pub permit_id: String,
    pub action_id: ActionId,
    pub revision: ActionRevision,
    pub target: TargetValue,
    pub profile: ActionProfile,
}
