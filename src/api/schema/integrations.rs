use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationState {
    NotInstalled,
    Current,
    Outdated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IntegrationInfo {
    pub target: IntegrationTarget,
    pub label: String,
    pub command: String,
    pub available: bool,
    pub state: IntegrationState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IntegrationInstallParams {
    pub target: IntegrationTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IntegrationInfoV2 {
    pub target: String,
    pub label: String,
    pub command: String,
    pub available: bool,
    pub state: IntegrationState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IntegrationInstallParamsV2 {
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IntegrationUninstallParams {
    pub target: IntegrationTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationTarget {
    Pi,
    Omp,
    Claude,
    Codex,
    Copilot,
    Devin,
    Droid,
    Kimi,
    Opencode,
    Kilo,
    Hermes,
    Qodercli,
    Qwen,
    Cursor,
    Mastracode,
    AntigravityCli,
    Grok,
}

impl IntegrationTarget {
    pub(crate) const ALL: [Self; 17] = [
        Self::Pi,
        Self::Omp,
        Self::Claude,
        Self::Codex,
        Self::Copilot,
        Self::Devin,
        Self::Droid,
        Self::Kimi,
        Self::Opencode,
        Self::Kilo,
        Self::Hermes,
        Self::Qodercli,
        Self::Qwen,
        Self::Cursor,
        Self::Mastracode,
        Self::AntigravityCli,
        Self::Grok,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IntegrationInstallResult {
    pub messages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IntegrationUninstallResult {
    pub messages: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_integration_responses_and_target_enum_stay_closed() {
        // Requests alone cannot protect the list response against appended variants.
        let names = [
            "pi",
            "omp",
            "claude",
            "codex",
            "copilot",
            "devin",
            "droid",
            "kimi",
            "opencode",
            "kilo",
            "hermes",
            "qodercli",
            "qwen",
            "cursor",
            "mastracode",
            "antigravity_cli",
            "grok",
        ];
        let schema =
            serde_json::to_value(schemars::schema_for!(IntegrationTarget)).expect("schema");
        assert_eq!(schema["enum"], serde_json::json!(names));
        for (target, name) in IntegrationTarget::ALL.into_iter().zip(names) {
            assert_eq!(serde_json::to_value(target).expect("target"), name);
            assert_eq!(
                serde_json::from_value::<IntegrationTarget>(serde_json::json!(name))
                    .expect("target"),
                target
            );
            let runtime = crate::integration::IntegrationTarget::from(target);
            assert_eq!(runtime.as_str(), name);
            assert_eq!(runtime.legacy(), Some(target));
            assert_eq!(
                name.parse::<crate::integration::IntegrationTarget>()
                    .expect("runtime target"),
                runtime
            );
            let list = crate::api::schema::ResponseResult::IntegrationList {
                integrations: vec![IntegrationInfo {
                    target,
                    label: name.into(),
                    command: name.into(),
                    available: true,
                    state: IntegrationState::Current,
                }],
            };
            assert_eq!(
                serde_json::to_value(list).expect("list"),
                serde_json::json!({
                    "type": "integration_list", "integrations": [{"target": name, "label": name, "command": name,
                        "available": true, "state": "current"}]
                })
            );
            for (result, kind) in [
                (
                    crate::api::schema::ResponseResult::IntegrationInstall {
                        target,
                        details: IntegrationInstallResult {
                            messages: vec!["ok".into()],
                        },
                    },
                    "integration_install",
                ),
                (
                    crate::api::schema::ResponseResult::IntegrationUninstall {
                        target,
                        details: IntegrationUninstallResult {
                            messages: vec!["ok".into()],
                        },
                    },
                    "integration_uninstall",
                ),
            ] {
                assert_eq!(
                    serde_json::to_value(result).expect("result"),
                    serde_json::json!({"type": kind, "target": name, "details": {"messages": ["ok"]}})
                );
            }
        }
        assert!(serde_json::from_value::<IntegrationTarget>(serde_json::json!("letta")).is_err());
        assert_eq!(
            "letta"
                .parse::<crate::integration::IntegrationTarget>()
                .expect("letta"),
            crate::integration::IntegrationTarget::Letta
        );
    }
}
