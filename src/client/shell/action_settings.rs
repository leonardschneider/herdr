use super::*;
use crate::file_url_actions::{ActionId, ActionPermit, ActionSettings, LaunchMode};

impl ClientShellState {
    pub(super) fn target_menu(&self) -> Option<&ClientFileMenu> {
        match self.overlay.as_ref()? {
            ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
                target:
                    ClientContextMenuTarget::Pane {
                        file: Some(file), ..
                    },
                ..
            }) => Some(file),
            _ => None,
        }
    }

    pub(super) fn target_menu_mut(&mut self) -> Option<&mut ClientFileMenu> {
        match self.overlay.as_mut()? {
            ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
                target:
                    ClientContextMenuTarget::Pane {
                        file: Some(file), ..
                    },
                ..
            }) => Some(file),
            _ => None,
        }
    }

    pub(super) fn close_target_menu(&mut self) {
        if self.target_menu().is_some() {
            self.overlay = None;
            self.hits.context_menu_rows.clear();
        }
        self.action_cache.authorized = None;
    }

    pub(super) fn refresh_action_settings(&mut self, outcome: &mut ClientShellInput) -> bool {
        if !self.active_endpoint_id.is_local()
            || !self.endpoint_is_online(&self.active_endpoint_id)
            || !self.advertises_endpoint_method("file_url_actions.get")
        {
            return false;
        }
        let generation = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
            .and_then(|endpoint| endpoint.snapshot_generation);
        if self.action_cache.generation != generation {
            self.action_cache = ClientActionCache {
                generation,
                ..ClientActionCache::default()
            };
        }
        if self.action_cache.refreshing || self.action_cache.reloading {
            return true;
        }
        self.action_cache.refreshing = self.push_endpoint_method_with_kind(
            crate::api::schema::Method::FileUrlActionsGet(
                crate::api::schema::EmptyParams::default(),
            ),
            PendingEndpointKind::ActionSettingsGet {
                epoch: self.action_cache.reload_epoch,
            },
            outcome,
        );
        self.action_cache.next_refresh =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
        self.action_cache.refreshing
    }

    pub(crate) fn tick_action_settings(
        &mut self,
        now: std::time::Instant,
        outcome: &mut ClientShellInput,
    ) {
        let generation = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
            .and_then(|endpoint| endpoint.snapshot_generation);
        if !self.active_endpoint_id.is_local() || self.action_cache.generation != generation {
            outcome.repaint |= self.target_menu().is_some();
            self.close_target_menu();
            self.action_cache = ClientActionCache {
                generation,
                ..ClientActionCache::default()
            };
        }
        if self
            .action_cache
            .next_refresh
            .is_none_or(|deadline| now >= deadline)
        {
            self.refresh_action_settings(outcome);
        }
    }

    pub(super) fn reload_action_settings(&mut self, outcome: &mut ClientShellInput) {
        if !self.active_endpoint_id.is_local() {
            return;
        }
        let epoch = self.next_request_id;
        self.action_cache.reload_epoch = epoch;
        self.action_cache.reloading = true;
        // Older get responses remain transport-correlated but cannot install old policy.
        self.action_cache.refreshing = false;
        if let Some(ClientShellOverlay::Settings(overlay)) = self.overlay.as_mut() {
            overlay.action_reload = Some(epoch);
            overlay.action_draft = None;
        }
        if !self.push_endpoint_method_with_kind(
            crate::api::schema::Method::ServerReloadConfig(
                crate::api::schema::EmptyParams::default(),
            ),
            PendingEndpointKind::ActionSettingsReload { epoch },
            outcome,
        ) {
            self.finish_action_reload_failure(epoch, "Could not reload saved settings".into());
        }
        outcome.repaint = true;
    }

    fn finish_action_reload_failure(&mut self, epoch: u64, error: String) {
        self.close_target_menu();
        self.action_cache.reloading = false;
        if let Some(ClientShellOverlay::Settings(overlay)) = self.overlay.as_mut() {
            if overlay.action_reload == Some(epoch) {
                overlay.action_reload = None;
                overlay.action_draft = self.action_cache.settings.clone().map(|settings| {
                    let mut draft = ClientActionDraft::new(settings);
                    draft.error = Some(error.clone());
                    Box::new(draft)
                });
            }
        }
        self.report_target_error(error);
    }

    pub(super) fn complete_action_settings_reload(
        &mut self,
        epoch: u64,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        if epoch != self.action_cache.reload_epoch {
            return (false, Vec::new());
        }
        match result {
            Ok(crate::api::schema::ResponseResult::ConfigReload {
                status: crate::config::ConfigReloadStatus::Failed,
                diagnostics,
            }) => {
                self.finish_action_reload_failure(
                    epoch,
                    format!("Config reload failed: {}", diagnostics.join("; ")),
                );
                (true, Vec::new())
            }
            Ok(crate::api::schema::ResponseResult::ConfigReload { diagnostics, .. }) => {
                if !diagnostics.is_empty() {
                    self.report_target_error(format!("Config reload: {}", diagnostics.join("; ")));
                }
                self.action_cache.reloading = false;
                let mut outcome = ClientShellInput::default();
                if !self.refresh_action_settings(&mut outcome) {
                    self.finish_action_reload_failure(
                        epoch,
                        "Reload completed but action settings are unavailable".into(),
                    );
                }
                (true, outcome.actions)
            }
            result => {
                let error = match result {
                    Err(error) => error.message,
                    _ => "Unexpected config reload response".into(),
                };
                self.finish_action_reload_failure(epoch, error);
                (true, Vec::new())
            }
        }
    }

    pub(super) fn install_action_settings(&mut self, settings: ActionSettings) {
        // A resolve waiting for its initial settings has no displayed policy to invalidate.
        let changed = self
            .action_cache
            .settings
            .as_ref()
            .is_some_and(|old| old != &settings);
        if changed
            && self
                .target_menu()
                .is_some_and(|file| file.settings.is_some())
        {
            self.close_target_menu();
        }
        self.action_cache.generation = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
            .and_then(|endpoint| endpoint.snapshot_generation);
        if let Some(ClientShellOverlay::Settings(overlay)) = self.overlay.as_mut() {
            if overlay.action_draft.is_none() && overlay.action_reload.is_none() {
                overlay.action_draft = Some(Box::new(ClientActionDraft::new(settings.clone())));
            }
        }
        self.action_cache.settings = Some(settings);
    }

    pub(super) fn complete_action_settings_get(
        &mut self,
        epoch: u64,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        if epoch != self.action_cache.reload_epoch || self.action_cache.reloading {
            return (false, Vec::new());
        }
        self.action_cache.refreshing = false;
        let mut settings = match result {
            Ok(crate::api::schema::ResponseResult::FileUrlActionsSettings { settings }) => settings,
            result => {
                self.finish_target_lookup_without_actions();
                let error = match result {
                    Err(error) => error.message,
                    _ => "Unexpected action settings response".into(),
                };
                self.finish_action_reload_failure(epoch, error);
                return (true, Vec::new());
            }
        };
        let mut repaint = false;
        if let Some(ClientShellOverlay::Settings(overlay)) = self.overlay.as_mut() {
            if overlay.action_reload == Some(epoch) {
                overlay.action_reload = None;
                repaint = true;
            }
            // Installing the first draft replaces the visible loading state.
            repaint |= overlay.action_draft.is_none() && overlay.action_reload.is_none();
        }
        // The outer endpoint response already matched the pending request and
        // transport boot in handle_endpoint_result. Policy revisions have their
        // own runtime incarnation; they are not transport snapshot boot IDs.
        if !self.active_endpoint_id.is_local() || self.snapshot.is_none() {
            self.close_target_menu();
            self.report_target_error("Settings came from a different server generation".into());
            return (true, Vec::new());
        }
        if settings.diagnostic.is_none() {
            settings.diagnostic = settings.profiles.validate().err();
        }
        let had_target_menu = self.target_menu().is_some();
        self.install_action_settings(settings.clone());
        repaint |= had_target_menu && self.target_menu().is_none();
        if let Some(error) = settings.diagnostic.clone() {
            self.finish_target_lookup_without_actions();
            self.report_target_error(error);
            return (true, Vec::new());
        }
        let mut outcome = ClientShellInput::default();
        let Some(file) = self.target_menu().cloned() else {
            return (repaint, Vec::new());
        };
        // A background refresh of an already-ready (or in-flight) menu changes
        // only cached policy. Resolve/execute transitions still need presentation.
        repaint |= matches!(
            file.phase,
            TargetMenuPhase::RefreshResolve | TargetMenuPhase::RefreshExecute(_)
        );
        match file.phase {
            TargetMenuPhase::RefreshResolve => {
                self.invalidate_stale_file_menu();
                if self.target_menu().is_none() {
                    return (true, Vec::new());
                }
                let context = file.context.clone();
                let method = crate::api::schema::Method::PaneTargetResolve(
                    crate::api::schema::PaneTargetResolveParams {
                        pane_id: context.pane_id.clone(),
                        viewport_row: context.viewport_row,
                        col: context.col,
                        content_revision: context.content_revision,
                        offset_from_bottom: context.offset_from_bottom,
                        expected_revision: settings.revision.clone(),
                    },
                );
                if self.push_endpoint_method_with_kind(
                    method,
                    PendingEndpointKind::PaneFileResolve { context },
                    &mut outcome,
                ) {
                    if let Some(file) = self.target_menu_mut() {
                        file.phase = TargetMenuPhase::Resolving;
                        file.settings = Some(settings);
                    }
                } else {
                    self.finish_target_lookup_without_actions();
                }
            }
            TargetMenuPhase::RefreshExecute(action) => {
                if self.ready_file_context_is_current(&file.context) != Some(true)
                    || file.settings.as_ref() != Some(&settings)
                {
                    self.close_target_menu();
                    return (true, Vec::new());
                }
                let Some(target) = file.target.as_ref() else {
                    self.close_target_menu();
                    return (true, Vec::new());
                };
                let params = crate::api::schema::PaneTargetActionParams {
                    pane_id: file.context.pane_id.clone(),
                    target_id: target.target_id.clone(),
                    action_id: action,
                    revision: settings.revision.clone(),
                };
                let method = match settings.profiles.profile(action).launch_mode {
                    LaunchMode::Pane => crate::api::schema::Method::PaneTargetExecute(params),
                    LaunchMode::External => crate::api::schema::Method::PaneTargetAuthorize(params),
                };
                if self.push_endpoint_method_with_kind(
                    method,
                    PendingEndpointKind::TargetAction {
                        file: Box::new(file),
                        action,
                    },
                    &mut outcome,
                ) {
                    if let Some(file) = self.target_menu_mut() {
                        file.phase = TargetMenuPhase::Executing(action);
                    }
                } else {
                    self.close_target_menu();
                }
            }
            _ => {}
        }
        (repaint, outcome.actions)
    }

    pub(super) fn complete_target_action(
        &mut self,
        file: ClientFileMenu,
        action: ActionId,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        if !self.target_menu().is_some_and(|current| {
            current.context.generation == file.context.generation
                && current.phase == TargetMenuPhase::Executing(action)
        }) {
            return (false, Vec::new());
        }
        let current = self.ready_file_context_is_current(&file.context) == Some(true);
        self.close_target_menu();
        if !current {
            return (true, Vec::new());
        }
        match result {
            Ok(crate::api::schema::ResponseResult::PaneTargetAuthorized { permit }) => {
                let valid = file
                    .target
                    .as_ref()
                    .zip(file.settings.as_ref())
                    .is_some_and(|(target, settings)| {
                        !permit.permit_id.is_empty()
                            && permit.action_id == action
                            && permit.target == target.target
                            && permit.revision == target.revision
                            && permit.revision == settings.revision
                            && permit.profile == *settings.profiles.profile(action)
                            && permit.profile.launch_mode == LaunchMode::External
                            && permit.profile.enabled
                            && permit.profile.applies_to(&permit.target)
                    });
                if !valid {
                    self.report_target_error("Server returned a mismatched action permit".into());
                    return (true, Vec::new());
                }
                self.action_cache.authorized =
                    Some((file.context.generation, permit.permit_id.clone()));
                (
                    true,
                    vec![ClientShellAction::LaunchTargetAction {
                        context: file.context,
                        permit: Box::new(permit),
                    }],
                )
            }
            Ok(crate::api::schema::ResponseResult::PaneTargetExecuted { .. }) => (true, Vec::new()),
            Err(_) => (true, Vec::new()),
            _ => {
                self.report_target_error("Server returned an unexpected action result".into());
                (true, Vec::new())
            }
        }
    }

    pub(crate) fn consume_target_permit(
        &mut self,
        context: &ClientFileContext,
        permit: &ActionPermit,
    ) -> bool {
        // Consume even rejected attempts; an ambiguous outcome is never retried.
        let expected = self.action_cache.authorized.take();
        expected == Some((context.generation, permit.permit_id.clone()))
            && self.local_file_action_is_current(context)
    }

    pub(crate) fn report_target_error(&mut self, message: String) -> bool {
        self.push_endpoint_notice(
            ClientEndpointNoticeKind::Rejected,
            "file_url_action",
            "File / URL action failed",
            message,
        )
    }
}
