use super::*;
use crate::api::schema::{Method, ResponseResult};
use crate::file_url_actions::*;

fn saved() -> ActionSettings {
    ActionSettings {
        revision: ActionRevision {
            boot_id: "actions-policy-incarnation-1".into(),
            counter: 1,
        },
        profiles: ActionProfiles::default(),
        diagnostic: None,
    }
}

fn editor() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(vec![
        "file_url_actions.get".into(),
        "file_url_actions.set".into(),
    ]));
    state.install_action_settings(saved());
    state.open_settings_overlay();
    state.select_settings_section(
        ClientSettingsSection::FileUrlActions,
        &mut ClientShellInput::default(),
    );
    state
}

fn draft(state: &ClientShellState) -> &ClientActionDraft {
    let Some(ClientShellOverlay::Settings(overlay)) = state.overlay.as_ref() else {
        panic!("settings");
    };
    overlay.action_draft.as_ref().unwrap()
}

fn press(state: &mut ClientShellState, code: KeyCode) -> ClientShellInput {
    let mut outcome = ClientShellInput::default();
    assert!(state.route_settings_key(
        &crate::input::TerminalKey::new(code, KeyModifiers::empty()),
        &mut outcome
    ));
    outcome
}

fn click_row(state: &mut ClientShellState, index: usize) {
    state.select_settings_choice(index);
    state.compose(76, 32).unwrap();
    let rect = state
        .hits
        .settings_choices
        .iter()
        .find(|(_, row)| *row == index)
        .unwrap()
        .0;
    let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(outcome.actions.is_empty(), "editing must not send commands");
}

#[test]
fn action_editor_every_profile_control_is_staged_and_cancel_is_nonmutating() {
    let mut state = editor();
    for id in [ActionId::Editor, ActionId::SystemOpener] {
        assert_eq!(draft(&state).profile, id);
        let original = draft(&state).settings.profiles.profile(id).clone();
        click_row(&mut state, 1); // enabled
        click_row(&mut state, 3); // launch mode
        click_row(&mut state, 4);
        click_row(&mut state, 5);
        click_row(&mut state, 6);
        click_row(&mut state, 2); // executable, with focused hjkl/space typing
        for ch in "hjkl program".chars() {
            assert!(press(&mut state, KeyCode::Char(ch)).actions.is_empty());
        }
        assert_eq!(
            draft(&state).settings.profiles.profile(id).executable,
            "hjkl program"
        );
        press(&mut state, KeyCode::Enter);
        let count = draft(&state).settings.profiles.profile(id).arguments.len();
        click_row(&mut state, 7 + count * 2); // add
        let paste = state.handle_raw_events(vec![RawInputEvent::Paste(
            "--location={target};$() snow 雪".into(),
        )]);
        assert!(paste.actions.is_empty());
        assert_eq!(
            draft(&state)
                .settings
                .profiles
                .profile(id)
                .arguments
                .last()
                .unwrap(),
            "--location={target};$() snow 雪"
        );
        press(&mut state, KeyCode::Enter);
        click_row(&mut state, 8 + count * 2); // remove added row
        let edited = draft(&state).settings.profiles.profile(id);
        assert_eq!(edited.arguments.len(), count);
        assert_eq!(edited.enabled, !original.enabled);
        assert_ne!(edited.launch_mode, original.launch_mode);
        assert_eq!(
            edited.applies_to.text_files,
            !original.applies_to.text_files
        );
        assert_eq!(
            edited.applies_to.other_files,
            !original.applies_to.other_files
        );
        assert_eq!(edited.applies_to.urls, !original.applies_to.urls);
        click_row(&mut state, 0);
    }
    assert_eq!(state.action_cache.settings.as_ref(), Some(&saved()));
    state.cancel_settings_overlay();
    assert!(state.overlay.is_none());
    state.open_settings_overlay();
    assert_eq!(draft(&state).settings, saved());
}

#[test]
fn action_editor_save_validates_whole_draft_and_uses_revision_cas_without_launch() {
    let mut state = editor();
    click_row(&mut state, 2);
    state.insert_overlay_text("custom-editor");
    press(&mut state, KeyCode::Enter);
    let mut outcome = ClientShellInput::default();
    state.save_action_settings(&mut outcome);
    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("settings request");
    };
    let Method::FileUrlActionsSet(params) = &request.method else {
        panic!("save not launch");
    };
    assert_eq!(params.expected_revision, saved().revision);
    assert_eq!(params.profiles.editor.executable, "custom-editor");
    assert_eq!(
        params.profiles.system_opener,
        saved().profiles.system_opener
    );
    assert!(draft(&state).saving);
    assert_eq!(state.action_cache.settings.as_ref(), Some(&saved()));
    let mut acknowledged = saved();
    acknowledged.revision.counter = 2;
    acknowledged.profiles = params.profiles.clone();
    let id = request.id.clone();
    assert!(state
        .handle_endpoint_result(
            "boot-1",
            &id,
            Ok(ResponseResult::FileUrlActionsSettings {
                settings: acknowledged.clone()
            })
        )
        .1
        .is_empty());
    assert_eq!(draft(&state).settings, acknowledged);
    assert!(!draft(&state).saving);
    state.cancel_settings_overlay();
    state.open_settings_overlay();
    assert_eq!(draft(&state).settings, acknowledged);
}

#[test]
fn action_editor_invalid_placeholders_and_failed_save_keep_prior_settings() {
    let mut state = editor();
    click_row(&mut state, 7);
    state.insert_overlay_text("{unknown}");
    press(&mut state, KeyCode::Enter);
    let mut outcome = ClientShellInput::default();
    state.save_action_settings(&mut outcome);
    assert!(outcome.actions.is_empty());
    assert!(draft(&state).error.is_some());
    assert_eq!(state.action_cache.settings.as_ref(), Some(&saved()));
    // Defaults are a draft operation, not a disk write.
    let count = draft(&state).settings.profiles.editor.arguments.len();
    click_row(&mut state, 8 + count * 2);
    state.save_action_settings(&mut outcome);
    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("valid save");
    };
    let id = request.id.clone();
    state.handle_endpoint_result(
        "boot-1",
        &id,
        Err(ClientShellEndpointError {
            code: Some("conflict".into()),
            message: "Settings changed; reload before saving".into(),
        }),
    );
    assert!(!draft(&state).saving);
    assert!(draft(&state).error.as_ref().unwrap().contains("reload"));
    assert_eq!(state.action_cache.settings.as_ref(), Some(&saved()));
}

#[test]
fn action_editor_gui_preset_and_individual_arguments_need_no_installed_program() {
    let mut state = editor();
    let count = draft(&state).settings.profiles.editor.arguments.len();
    click_row(&mut state, 9 + count * 2);
    let profile = &draft(&state).settings.profiles.editor;
    assert_eq!(profile.executable, "code");
    assert_eq!(profile.arguments, ["--", "{target}"]);
    assert_eq!(profile.launch_mode, LaunchMode::External);
    assert!(profile.validate().is_ok());
}

#[test]
fn action_editor_section_tab_is_mouse_accessible_at_76_and_narrow_width() {
    for width in [76, 48, 32] {
        let mut state = editor();
        state.compose(width, 32).unwrap();
        let rect = state
            .hits
            .settings_tabs
            .iter()
            .find(|(_, section)| *section == ClientSettingsSection::FileUrlActions)
            .expect("tab not truncated")
            .0;
        assert!(rect.width > 0);
        assert!(rect.right() <= width);
        let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::empty(),
        })]);
        assert!(state.action_editor_active());
        assert!(outcome.repaint);
    }
}

#[test]
fn action_editor_background_refresh_preserves_dirty_draft_and_save_base_revision() {
    let mut state = editor();
    click_row(&mut state, 2);
    state.insert_overlay_text("my-editor");
    press(&mut state, KeyCode::Enter);
    let mut updated = saved();
    updated.revision.counter = 2;
    updated.profiles.editor.executable = "other-client-editor".into();
    state.install_action_settings(updated);
    assert_eq!(draft(&state).settings.revision, saved().revision);
    assert_eq!(
        draft(&state).settings.profiles.editor.executable,
        "my-editor"
    );
    let mut outcome = ClientShellInput::default();
    state.save_action_settings(&mut outcome);
    assert!(
        matches!(&outcome.actions[..], [ClientShellAction::Endpoint { request, .. }] if matches!(&request.method, Method::FileUrlActionsSet(params) if params.expected_revision.counter == 1))
    );
}

fn reload_from_row(state: &mut ClientShellState) -> String {
    state.set_endpoint_methods(Some(vec![
        "file_url_actions.get".into(),
        "file_url_actions.set".into(),
        "server.reload_config".into(),
    ]));
    let index = draft(state).rows().len() - 1;
    state.select_settings_choice(index);
    let mut outcome = ClientShellInput::default();
    state.activate_action_editor_row(&mut outcome);
    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("one reload request");
    };
    assert!(matches!(request.method, Method::ServerReloadConfig(_)));
    request.id.clone()
}

#[test]
fn explicit_reload_waits_for_server_reload_then_fresh_get_and_ignores_older_get() {
    let mut state = editor();
    let old_get = state
        .pending_requests
        .iter()
        .find(|(_, pending)| matches!(pending.kind, PendingEndpointKind::ActionSettingsGet { .. }))
        .unwrap()
        .0
        .clone();
    let reload = reload_from_row(&mut state);
    assert!(state.action_cache.reloading);
    assert!(state
        .handle_endpoint_result(
            "boot-1",
            &old_get,
            Ok(ResponseResult::FileUrlActionsSettings { settings: saved() })
        )
        .1
        .is_empty());
    assert!(
        matches!(state.overlay.as_ref(), Some(ClientShellOverlay::Settings(overlay)) if overlay.action_draft.is_none())
    );
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &reload,
        Ok(ResponseResult::ConfigReload {
            status: crate::config::ConfigReloadStatus::Applied,
            diagnostics: Vec::new(),
        }),
    );
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("fresh get after reload ack");
    };
    assert!(matches!(request.method, Method::FileUrlActionsGet(_)));
    let mut disk = saved();
    disk.revision.counter += 1;
    disk.profiles.editor.executable = "disk-editor".into();
    state.handle_endpoint_result(
        "boot-1",
        &request.id,
        Ok(ResponseResult::FileUrlActionsSettings {
            settings: disk.clone(),
        }),
    );
    assert_eq!(draft(&state).settings, disk);
}

#[test]
fn explicit_reload_failure_is_visible_and_dismissal_is_not_resurrected() {
    for failed_report in [false, true] {
        let mut state = editor();
        let id = reload_from_row(&mut state);
        let result = if failed_report {
            Ok(ResponseResult::ConfigReload {
                status: crate::config::ConfigReloadStatus::Failed,
                diagnostics: vec!["disk read failed".into()],
            })
        } else {
            Err(ClientShellEndpointError {
                code: Some("io_error".into()),
                message: "disk read failed".into(),
            })
        };
        assert!(state
            .handle_endpoint_result("boot-1", &id, result)
            .1
            .is_empty());
        assert!(!state.action_cache.reloading);
        assert!(draft(&state)
            .error
            .as_ref()
            .unwrap()
            .contains("disk read failed"));
        assert!(state.visible_endpoint_notice.is_some());
    }
    let mut state = editor();
    let id = reload_from_row(&mut state);
    state.cancel_settings_overlay();
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &id,
        Ok(ResponseResult::ConfigReload {
            status: crate::config::ConfigReloadStatus::Applied,
            diagnostics: Vec::new(),
        }),
    );
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("refresh cache");
    };
    state.handle_endpoint_result(
        "boot-1",
        &request.id,
        Ok(ResponseResult::FileUrlActionsSettings { settings: saved() }),
    );
    assert!(state.overlay.is_none());
}
