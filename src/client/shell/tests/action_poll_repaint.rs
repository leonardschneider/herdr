use super::*;
use crate::api::schema::ResponseResult;
use crate::file_url_actions::{ActionProfiles, ActionRevision, ActionSettings};

fn settings() -> ActionSettings {
    ActionSettings {
        revision: ActionRevision {
            boot_id: "policy-boot".into(),
            counter: 1,
        },
        profiles: ActionProfiles::default(),
        diagnostic: None,
    }
}

fn state() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(vec![
        "file_url_actions.get".into(),
        "pane.target.resolve".into(),
    ]));
    state.compose(106, 20).expect("initial frame");
    state.install_action_settings(settings());
    state
}

fn complete(state: &mut ClientShellState, settings: ActionSettings) -> bool {
    state.action_cache.refreshing = true;
    let (repaint, actions) = state.complete_action_settings_get(
        state.action_cache.reload_epoch,
        Ok(ResponseResult::FileUrlActionsSettings { settings }),
    );
    assert!(actions.is_empty());
    assert!(!state.action_cache.refreshing);
    repaint
}

fn ready_menu(state: &mut ClientShellState) {
    let pane = state.hits.panes[0].clone();
    let down = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: pane.inner_rect.x + 1,
        row: pane.inner_rect.y,
        modifiers: KeyModifiers::empty(),
    };
    state.handle_raw_events(vec![
        RawInputEvent::Mouse(down),
        RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Right),
            ..down
        }),
    ]);
    // The settings completion only needs the captured policy and phase; target
    // resolution/authorization itself is covered by the file-context tests.
    let file = state.target_menu_mut().expect("target lookup");
    file.phase = TargetMenuPhase::Ready;
    file.settings = Some(settings());
}

#[test]
fn background_settings_poll_does_not_request_presentation() {
    let mut state = state();
    let composed_at = state.last_composed_at;
    for _ in 0..3 {
        assert!(!complete(&mut state, settings()));
        assert!(state.overlay.is_none());
        assert_eq!(state.last_composed_at, composed_at);
    }
    let mut changed = settings();
    changed.revision.counter += 1;
    assert!(!complete(&mut state, changed.clone()));
    assert_eq!(state.action_cache.settings.as_ref(), Some(&changed));
}

#[test]
fn unchanged_ready_menu_is_quiet_but_changed_policy_closes_it() {
    let mut state = state();
    ready_menu(&mut state);
    assert!(!complete(&mut state, settings()));
    assert_eq!(state.target_menu().unwrap().phase, TargetMenuPhase::Ready);
    let mut changed = settings();
    changed.revision.counter += 1;
    assert!(complete(&mut state, changed));
    assert!(state.target_menu().is_none());
    assert!(state.overlay.is_none());
}

#[test]
fn settings_draft_population_and_reload_completion_repaint_once() {
    let mut state = state();
    state.open_settings_overlay();
    state.select_settings_section(
        ClientSettingsSection::FileUrlActions,
        &mut ClientShellInput::default(),
    );
    let Some(ClientShellOverlay::Settings(overlay)) = state.overlay.as_mut() else {
        panic!("settings overlay");
    };
    overlay.action_draft = None;
    assert!(complete(&mut state, settings()));
    assert!(!complete(&mut state, settings()));
    let epoch = state.action_cache.reload_epoch;
    let Some(ClientShellOverlay::Settings(overlay)) = state.overlay.as_mut() else {
        panic!("settings overlay");
    };
    assert!(overlay.action_draft.is_some());
    overlay.action_reload = Some(epoch);
    assert!(complete(&mut state, settings()));
    assert!(!complete(&mut state, settings()));
}

#[test]
fn background_polls_preserve_interleaved_retained_patches() {
    let mut state = state();
    let composed_at = state.last_composed_at;
    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    for revision in 2..=101 {
        assert!(!complete(&mut state, settings()));
        let surface = state.pane_surface.as_ref().unwrap();
        let mut pane = surface.panes[0].clone();
        pane.content_revision = revision;
        let mut cell = surface.frame.cells[0].clone();
        let glyph = spinner[((revision - 2) as usize) % spinner.len()];
        cell.symbol = glyph.into();
        let patch = crate::protocol::PaneSurfacePatch {
            boot_id: surface.boot_id.clone(),
            projection_revision: surface.projection_revision,
            base_surface_revision: revision - 1,
            surface_revision: revision,
            rows: vec![crate::protocol::PaneSurfacePatchRow {
                x: pane.inner_rect.x,
                y: pane.inner_rect.y,
                cells: vec![cell],
            }],
            panes: vec![pane],
            cursor: None,
        };
        assert!(matches!(
            state.apply_pane_surface_patch(patch),
            ClientPaneSurfacePatchOutcome::Applied(Some(_))
        ));
        assert_eq!(
            state.pane_surface.as_ref().unwrap().surface_revision,
            revision
        );
        assert_eq!(
            state.pane_surface.as_ref().unwrap().frame.cells[0].symbol,
            glyph
        );
        assert!(!complete(&mut state, settings()));
        assert_eq!(state.last_composed_at, composed_at);
    }
}

#[test]
fn lookup_progression_and_diagnostics_still_repaint() {
    let mut state = state();
    ready_menu(&mut state);
    let file = state.target_menu_mut().unwrap();
    file.phase = TargetMenuPhase::RefreshResolve;
    file.settings = None;
    let (repaint, actions) = state.complete_action_settings_get(
        state.action_cache.reload_epoch,
        Ok(ResponseResult::FileUrlActionsSettings {
            settings: settings(),
        }),
    );
    assert!(repaint);
    assert!(matches!(
        actions.as_slice(),
        [ClientShellAction::Endpoint { .. }]
    ));
    assert_eq!(
        state.target_menu().unwrap().phase,
        TargetMenuPhase::Resolving
    );

    state.overlay = None;
    let mut invalid = settings();
    invalid.diagnostic = Some("invalid test policy".into());
    assert!(complete(&mut state, invalid));
}

#[test]
fn stale_or_reloading_poll_remains_ignored() {
    let mut state = state();
    state.action_cache.reload_epoch = 10;
    for (epoch, reloading) in [(9, false), (10, true)] {
        state.action_cache.reloading = reloading;
        let (repaint, actions) = state.complete_action_settings_get(
            epoch,
            Ok(ResponseResult::FileUrlActionsSettings {
                settings: settings(),
            }),
        );
        assert!(!repaint);
        assert!(actions.is_empty());
    }
}
