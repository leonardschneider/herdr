use crate::api::schema::{
    IntegrationInfo, IntegrationInfoV2, IntegrationInstallResult, IntegrationState,
    IntegrationUninstallResult, ResponseResult,
};
use crate::app::App;

use super::responses::{encode_error, encode_success};

fn integration_info_v2(
    recommendation: crate::integration::IntegrationRecommendation,
) -> IntegrationInfoV2 {
    IntegrationInfoV2 {
        target: recommendation.target.as_str().to_owned(),
        label: recommendation.label.to_owned(),
        command: recommendation.command.to_owned(),
        available: recommendation.available,
        state: match recommendation.state {
            crate::integration::IntegrationStatusKind::NotInstalled => {
                IntegrationState::NotInstalled
            }
            crate::integration::IntegrationStatusKind::Current => IntegrationState::Current,
            crate::integration::IntegrationStatusKind::Outdated => IntegrationState::Outdated,
        },
    }
}

fn legacy_integration_info(
    recommendation: crate::integration::IntegrationRecommendation,
) -> Option<IntegrationInfo> {
    let target = recommendation.target.legacy()?;
    let info = integration_info_v2(recommendation);
    Some(IntegrationInfo {
        target,
        label: info.label,
        command: info.command,
        available: info.available,
        state: info.state,
    })
}

impl App {
    pub(super) fn handle_integration_list(&self, id: String) -> String {
        let integrations = crate::integration::integration_recommendations()
            .into_iter()
            .filter_map(legacy_integration_info)
            .collect();
        encode_success(id, ResponseResult::IntegrationList { integrations })
    }

    pub(super) fn handle_integration_list_v2(&self, id: String) -> String {
        let integrations = crate::integration::integration_recommendations()
            .into_iter()
            .map(integration_info_v2)
            .collect();
        encode_success(id, ResponseResult::IntegrationListV2 { integrations })
    }

    pub(super) fn handle_integration_install_v2(
        &mut self,
        id: String,
        params: crate::api::schema::IntegrationInstallParamsV2,
    ) -> String {
        // Validate before invoking installers or refreshing filesystem-backed recommendations.
        let target = match params
            .target
            .parse::<crate::integration::IntegrationTarget>()
        {
            Ok(target) => target,
            Err(err) => return encode_error(id, "invalid_params", err),
        };
        let messages = match crate::integration::install_target(target) {
            Ok(messages) => messages,
            Err(err) => return encode_error(id, "integration_install_failed", err.to_string()),
        };
        self.state.integration_recommendations = crate::integration::integration_recommendations();
        encode_success(
            id,
            ResponseResult::IntegrationInstallV2 {
                target: target.as_str().to_owned(),
                details: IntegrationInstallResult { messages },
            },
        )
    }

    pub(super) fn handle_integration_install(
        &mut self,
        id: String,
        params: crate::api::schema::IntegrationInstallParams,
    ) -> String {
        let target = params.target;
        let messages = match crate::integration::install_target(target.into()) {
            Ok(messages) => messages,
            Err(err) => return encode_error(id, "integration_install_failed", err.to_string()),
        };
        self.state.integration_recommendations = crate::integration::integration_recommendations();
        encode_success(
            id,
            ResponseResult::IntegrationInstall {
                target,
                details: IntegrationInstallResult { messages },
            },
        )
    }

    pub(super) fn handle_integration_uninstall(
        &mut self,
        id: String,
        params: crate::api::schema::IntegrationUninstallParams,
    ) -> String {
        let target = params.target;
        let messages = match crate::integration::uninstall_target(target.into()) {
            Ok(messages) => messages,
            Err(err) => return encode_error(id, "integration_uninstall_failed", err.to_string()),
        };
        self.state.integration_recommendations = crate::integration::integration_recommendations();
        encode_success(
            id,
            ResponseResult::IntegrationUninstall {
                target,
                details: IntegrationUninstallResult { messages },
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integration_v2_unknown_target_rejected_without_writes() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        let dir = std::env::temp_dir().join(format!(
            "herdr-unknown-target-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir(&dir).expect("isolated target directory");
        let _lock = crate::integration::integration_env_lock();
        let previous_home = std::env::var_os("HOME");
        let previous_userprofile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", &dir);
        std::env::set_var("USERPROFILE", &dir);
        let recommendations = app.state.integration_recommendations.clone();
        for target in [
            "unknown-agent".to_owned(),
            "Letta".to_owned(),
            dir.as_path().join("letta").to_string_lossy().into_owned(),
            "../letta".to_owned(),
        ] {
            let response = app.handle_integration_install_v2(
                "test".into(),
                crate::api::schema::IntegrationInstallParamsV2 { target },
            );
            let response: serde_json::Value = serde_json::from_str(&response).expect("response");
            assert_eq!(response["error"]["code"], "invalid_params");
            assert_eq!(app.state.integration_recommendations, recommendations);
            assert_eq!(
                std::fs::read_dir(dir.as_path()).expect("directory").count(),
                0
            );
        }
        for (key, value) in [
            ("HOME", previous_home),
            ("USERPROFILE", previous_userprofile),
        ] {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
        std::fs::remove_dir(dir).expect("remove empty directory");
    }

    #[test]
    fn integration_lists_filter_only_v1_unknown_targets() {
        for target in crate::integration::IntegrationTarget::ALL {
            let recommendation = crate::integration::IntegrationRecommendation {
                target,
                label: "test",
                command: "test",
                available: true,
                path: std::path::PathBuf::new(),
                state: crate::integration::IntegrationStatusKind::NotInstalled,
            };
            let v2 = integration_info_v2(recommendation.clone());
            assert_eq!(v2.target, target.as_str());
            let v1 = legacy_integration_info(recommendation);
            assert_eq!(
                v1.is_none(),
                target == crate::integration::IntegrationTarget::Letta
            );
            if let Some(info) = v1 {
                let json = serde_json::to_value(&info).expect("legacy info");
                assert_eq!(
                    serde_json::from_value::<IntegrationInfo>(json).expect("legacy decode"),
                    info
                );
            }
        }
    }
}
