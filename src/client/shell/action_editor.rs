use super::*;
use crate::file_url_actions::{
    ActionId, ActionProfile, ActionProfiles, ActionRevision, ActionSettings, LaunchMode,
};
use crossterm::event::KeyModifiers;

#[derive(Clone, Copy)]
pub(super) enum ActionEditorRow {
    Profile,
    Enabled,
    Executable,
    Mode,
    Text,
    Other,
    Urls,
    Argument(usize),
    RemoveArgument(usize),
    AddArgument,
    Defaults,
    GuiPreset,
    Reload,
}

impl ClientActionDraft {
    pub(super) fn new(settings: ActionSettings) -> Self {
        let error = settings.diagnostic.clone();
        Self {
            settings,
            profile: ActionId::Editor,
            editing: false,
            replace_on_type: false,
            saving: false,
            error,
        }
    }
    fn profile_mut(&mut self) -> &mut ActionProfile {
        match self.profile {
            ActionId::Editor => &mut self.settings.profiles.editor,
            ActionId::SystemOpener => &mut self.settings.profiles.system_opener,
        }
    }
    pub(super) fn rows(&self) -> Vec<(ActionEditorRow, String)> {
        let profile = self.settings.profiles.profile(self.profile);
        let yes = |value| if value { "on" } else { "off" };
        let mut rows = vec![
            (
                ActionEditorRow::Profile,
                format!(
                    "Profile: {} (click to switch)",
                    match self.profile {
                        ActionId::Editor => "Editor",
                        ActionId::SystemOpener => "System opener",
                    }
                ),
            ),
            (
                ActionEditorRow::Enabled,
                format!("Enabled: {}", yes(profile.enabled)),
            ),
            (
                ActionEditorRow::Executable,
                format!("Executable: {}", profile.executable),
            ),
            (
                ActionEditorRow::Mode,
                format!(
                    "Launch: {}",
                    match profile.launch_mode {
                        LaunchMode::Pane => "new terminal pane",
                        LaunchMode::External => "external process",
                    }
                ),
            ),
            (
                ActionEditorRow::Text,
                format!("Text files: {}", yes(profile.applies_to.text_files)),
            ),
            (
                ActionEditorRow::Other,
                format!(
                    "Other regular files: {}",
                    yes(profile.applies_to.other_files)
                ),
            ),
            (
                ActionEditorRow::Urls,
                format!("HTTP(S) URLs: {}", yes(profile.applies_to.urls)),
            ),
        ];
        for (index, argument) in profile.arguments.iter().enumerate() {
            rows.push((
                ActionEditorRow::Argument(index),
                format!("Argument {}: {argument}", index + 1),
            ));
            rows.push((
                ActionEditorRow::RemoveArgument(index),
                format!("  Remove argument {}", index + 1),
            ));
        }
        rows.extend([
            (ActionEditorRow::AddArgument, "Add argument".into()),
            (
                ActionEditorRow::Defaults,
                "Reset this profile to defaults".into(),
            ),
            (
                ActionEditorRow::GuiPreset,
                "Preset: VS Code (external)".into(),
            ),
            (
                ActionEditorRow::Reload,
                "Reload saved settings (discard draft)".into(),
            ),
        ]);
        rows
    }
    fn text_mut(&mut self, selected: usize) -> Option<&mut String> {
        let row = self.rows().get(selected)?.0;
        match row {
            ActionEditorRow::Executable => Some(&mut self.profile_mut().executable),
            ActionEditorRow::Argument(index) => self.profile_mut().arguments.get_mut(index),
            _ => None,
        }
    }
}

impl ClientShellState {
    pub(super) fn action_editor_active(&self) -> bool {
        matches!(
            self.overlay,
            Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
                section: ClientSettingsSection::FileUrlActions,
                ..
            }))
        )
    }

    pub(super) fn activate_action_editor_row(&mut self, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::Settings(overlay)) = self.overlay.as_mut() else {
            return;
        };
        let Some(draft) = overlay.action_draft.as_mut().filter(|draft| !draft.saving) else {
            return;
        };
        let Some((row, _)) = draft.rows().get(overlay.selected).cloned() else {
            return;
        };
        draft.editing = false;
        match row {
            ActionEditorRow::Profile => {
                draft.profile = match draft.profile {
                    ActionId::Editor => ActionId::SystemOpener,
                    ActionId::SystemOpener => ActionId::Editor,
                };
            }
            ActionEditorRow::Enabled => {
                let p = draft.profile_mut();
                p.enabled = !p.enabled;
            }
            ActionEditorRow::Mode => {
                let p = draft.profile_mut();
                p.launch_mode = match p.launch_mode {
                    LaunchMode::Pane => LaunchMode::External,
                    LaunchMode::External => LaunchMode::Pane,
                };
            }
            ActionEditorRow::Text => {
                let p = draft.profile_mut();
                p.applies_to.text_files = !p.applies_to.text_files;
            }
            ActionEditorRow::Other => {
                let p = draft.profile_mut();
                p.applies_to.other_files = !p.applies_to.other_files;
            }
            ActionEditorRow::Urls => {
                let p = draft.profile_mut();
                p.applies_to.urls = !p.applies_to.urls;
            }
            ActionEditorRow::Executable | ActionEditorRow::Argument(_) => {
                draft.editing = true;
                draft.replace_on_type = true;
            }
            ActionEditorRow::RemoveArgument(index) => {
                draft.profile_mut().arguments.remove(index);
            }
            ActionEditorRow::AddArgument => {
                let index = draft.profile_mut().arguments.len();
                draft.profile_mut().arguments.push(String::new());
                overlay.selected = 7 + index * 2;
                draft.editing = true;
                draft.replace_on_type = false;
            }
            ActionEditorRow::Defaults => {
                *draft.profile_mut() = ActionProfiles::default().profile(draft.profile).clone();
            }
            ActionEditorRow::GuiPreset => {
                let p = draft.profile_mut();
                p.enabled = true;
                p.executable = "code".into();
                p.arguments = vec!["--".into(), "{target}".into()];
                p.launch_mode = LaunchMode::External;
                p.applies_to.text_files = true;
                p.applies_to.other_files = false;
                p.applies_to.urls = false;
            }
            ActionEditorRow::Reload => {
                self.reload_action_settings(outcome);
                outcome.repaint = true;
                return;
            }
        }
        overlay.selected = overlay.selected.min(draft.rows().len().saturating_sub(1));
        outcome.repaint = true;
    }

    pub(super) fn insert_action_editor_text(&mut self, text: &str) -> bool {
        if !self.action_editor_active() {
            return false;
        }
        let Some(ClientShellOverlay::Settings(overlay)) = self.overlay.as_mut() else {
            return true;
        };
        let Some(draft) = overlay
            .action_draft
            .as_mut()
            .filter(|draft| draft.editing && !draft.saving)
        else {
            return true;
        };
        // Preserve literal argument data, but never accept control characters from a paste.
        let replace = std::mem::take(&mut draft.replace_on_type);
        if let Some(input) = draft.text_mut(overlay.selected) {
            if replace {
                input.clear();
            }
            input.extend(text.chars().filter(|character| !character.is_control()));
        }
        true
    }

    pub(super) fn route_action_editor_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !self.action_editor_active() {
            return false;
        }
        let Some(ClientShellOverlay::Settings(overlay)) = self.overlay.as_mut() else {
            return false;
        };
        let Some(draft) = overlay.action_draft.as_mut() else {
            return false;
        };
        if draft.saving {
            return true;
        }
        if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.save_action_settings(outcome);
            return true;
        }
        if !draft.editing {
            return false;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Tab => {
                draft.editing = false;
            }
            KeyCode::Backspace => {
                let replace = std::mem::take(&mut draft.replace_on_type);
                if let Some(input) = draft.text_mut(overlay.selected) {
                    if replace {
                        input.clear();
                    } else {
                        input.pop();
                    }
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(input) = draft.text_mut(overlay.selected) {
                    input.clear();
                }
                draft.replace_on_type = false;
            }
            KeyCode::Char(character)
                if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() =>
            {
                let text = key
                    .generated_text
                    .clone()
                    .unwrap_or_else(|| character.to_string());
                self.insert_action_editor_text(&text);
            }
            _ => {}
        }
        outcome.repaint = true;
        true
    }

    pub(super) fn save_action_settings(&mut self, outcome: &mut ClientShellInput) {
        if !self.active_endpoint_id.is_local()
            || !self.advertises_endpoint_method("file_url_actions.set")
        {
            self.report_target_error(
                "This local server does not support editable File & URL actions".into(),
            );
            outcome.repaint = true;
            return;
        }
        let Some(ClientShellOverlay::Settings(overlay)) = self.overlay.as_mut() else {
            return;
        };
        let Some(draft) = overlay.action_draft.as_mut().filter(|draft| !draft.saving) else {
            return;
        };
        if let Err(error) = draft.settings.profiles.validate() {
            draft.error = Some(error);
            outcome.repaint = true;
            return;
        }
        let revision = draft.settings.revision.clone();
        let profiles = draft.settings.profiles.clone();
        draft.editing = false;
        draft.saving = true;
        draft.error = None;
        let queued = self.push_endpoint_method_with_kind(
            crate::api::schema::Method::FileUrlActionsSet(
                crate::api::schema::FileUrlActionsSetParams {
                    expected_revision: revision.clone(),
                    profiles,
                },
            ),
            PendingEndpointKind::ActionSettingsSet { revision },
            outcome,
        );
        if !queued {
            if let Some(ClientShellOverlay::Settings(overlay)) = self.overlay.as_mut() {
                if let Some(draft) = overlay.action_draft.as_mut() {
                    draft.saving = false;
                    draft.error = Some("Save unavailable; no changes saved".into());
                }
            }
        }
        self.action_cache.authorized = None;
        outcome.repaint = true;
    }

    pub(super) fn complete_action_settings_set(
        &mut self,
        revision: ActionRevision,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        let matches = matches!(self.overlay.as_ref(), Some(ClientShellOverlay::Settings(overlay))
            if overlay.action_draft.as_ref().is_some_and(|draft| draft.saving && draft.settings.revision == revision));
        match result {
            Ok(crate::api::schema::ResponseResult::FileUrlActionsSettings { settings }) => {
                self.install_action_settings(settings.clone());
                if matches {
                    if let Some(ClientShellOverlay::Settings(overlay)) = self.overlay.as_mut() {
                        overlay.action_draft = Some(Box::new(ClientActionDraft::new(settings)));
                    }
                }
            }
            result => {
                if matches {
                    if let Some(ClientShellOverlay::Settings(overlay)) = self.overlay.as_mut() {
                        if let Some(draft) = overlay.action_draft.as_mut() {
                            draft.saving = false;
                            draft.error = Some(match result {
                                Err(error) => error.message,
                                _ => "Unexpected save response; reload before retrying".into(),
                            });
                        }
                    }
                }
            }
        }
        (true, Vec::new())
    }
}
