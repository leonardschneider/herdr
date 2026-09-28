use super::*;
use crate::api::schema::{Method, ResponseResult};
use crate::file_url_actions::*;

fn settings() -> ActionSettings {
    ActionSettings {
        revision: ActionRevision {
            boot_id: "boot-1".into(),
            counter: 1,
        },
        profiles: ActionProfiles::default(),
        diagnostic: None,
    }
}

fn file_state() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(
        [
            "file_url_actions.get",
            "file_url_actions.set",
            "pane.target.resolve",
            "pane.target.execute",
            "pane.target.authorize",
            "pane.focus",
            "pane.split",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    ));
    state.compose(106, 20).expect("pane frame");
    state
}

fn right_click(state: &mut ClientShellState) -> ClientShellInput {
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
    ])
}

fn file_items(state: &ClientShellState) -> Vec<ClientContextMenuItem> {
    match state.overlay.as_ref() {
        Some(ClientShellOverlay::ContextMenu(menu)) => menu.items(),
        _ => Vec::new(),
    }
}

fn request(actions: &[ClientShellAction]) -> &crate::api::schema::Request {
    let [ClientShellAction::Endpoint { request, .. }] = actions else {
        panic!("expected one API request: {actions:?}");
    };
    request
}

// Observe every transition's state AND composed hit regions, not a final screenshot.
fn invisible(state: &mut ClientShellState) {
    assert!(file_items(state).is_empty());
    assert!(state.hits.context_menu_rows.is_empty());
    state.compose(106, 20).expect("frame");
    assert!(state.hits.context_menu_rows.is_empty());
    assert!(file_items(state).is_empty());
}

fn ordinary_pane_actions(state: &mut ClientShellState) {
    assert!(state.target_menu().is_none());
    let items = file_items(state);
    for action in [
        ClientContextMenuAction::RenamePane,
        ClientContextMenuAction::SplitRight,
        ClientContextMenuAction::Zoom,
        ClientContextMenuAction::ClosePane,
    ] {
        assert!(items.iter().any(|item| item.action == action));
    }
    assert!(!items.iter().any(|item| matches!(
        item.action,
        ClientContextMenuAction::OpenFile | ClientContextMenuAction::EditFile
    )));
    state.compose(106, 20).unwrap();
    assert!(!state.hits.context_menu_rows.is_empty());
}

fn finish_refresh(
    state: &mut ClientShellState,
    actions: &[ClientShellAction],
) -> Vec<ClientShellAction> {
    let req = request(actions);
    assert!(matches!(req.method, Method::FileUrlActionsGet(_)));
    state
        .handle_endpoint_result(
            "boot-1",
            &req.id,
            Ok(ResponseResult::FileUrlActionsSettings {
                settings: settings(),
            }),
        )
        .1
}

fn begin(state: &mut ClientShellState) -> String {
    let outcome = right_click(state);
    invisible(state);
    let actions = finish_refresh(state, &outcome.actions);
    invisible(state);
    let req = request(&actions);
    assert!(
        matches!(&req.method, Method::PaneTargetResolve(params) if params.pane_id == "pane_1" && params.col == 1)
    );
    req.id.clone()
}

fn target(kind: TargetFileKind) -> ResolvedTarget {
    ResolvedTarget {
        target_id: "opaque-target".into(),
        revision: settings().revision,
        target: TargetValue::File {
            path: "/tmp/actual-file.rs".into(),
            line: Some(12),
            column: Some(3),
            file_kind: kind,
        },
        actions: vec![ActionId::SystemOpener, ActionId::Editor],
        highlight: Vec::new(),
    }
}

fn resolve_highlight(state: &mut ClientShellState, spans: Vec<TargetCellSpan>) {
    let id = begin(state);
    let mut value = target(TargetFileKind::Text);
    value.highlight = spans;
    state.handle_endpoint_result(
        "boot-1",
        &id,
        Ok(ResponseResult::PaneTargetResolved {
            target: Some(value),
        }),
    );
}

#[test]
fn target_highlight_missing_and_invalid_metadata_preserves_actions() {
    for spans in [
        vec![],
        vec![TargetCellSpan {
            row: u16::MAX,
            start_col: 0,
            end_col: 1,
        }],
        vec![TargetCellSpan {
            row: 0,
            start_col: 2,
            end_col: 2,
        }],
        vec![TargetCellSpan {
            row: 0,
            start_col: 0,
            end_col: u16::MAX,
        }],
        vec![
            TargetCellSpan {
                row: 1,
                start_col: 0,
                end_col: 2,
            },
            TargetCellSpan {
                row: 0,
                start_col: 0,
                end_col: 2,
            },
        ],
        vec![
            TargetCellSpan {
                row: 0,
                start_col: 0,
                end_col: 2,
            },
            TargetCellSpan {
                row: 0,
                start_col: 1,
                end_col: 3,
            },
        ],
    ] {
        let mut state = file_state();
        resolve_highlight(&mut state, spans);
        assert_eq!(file_items(&state).len(), 2);
        assert!(state.target_menu().unwrap().highlight.is_none());
    }
}

#[test]
fn target_highlight_menu_avoids_source_rows_and_keeps_anchor_after_output() {
    let mut state = file_state();
    resolve_highlight(
        &mut state,
        vec![
            TargetCellSpan {
                row: 0,
                start_col: 0,
                end_col: 4,
            },
            TargetCellSpan {
                row: 1,
                start_col: 0,
                end_col: 2,
            },
        ],
    );
    state.compose(106, 20).unwrap();
    let ClientShellOverlay::ContextMenu(menu) = state.overlay.as_ref().unwrap() else {
        panic!("menu")
    };
    let y = menu.y;
    assert!(y > state.target_menu().unwrap().context.inner_rect.y + 1);
    let mut changed = state.pane_surface.as_ref().unwrap().clone();
    changed.surface_revision += 1;
    changed.panes[0].content_revision += 1;
    state.set_pane_surface(changed);
    state.compose(106, 20).unwrap();
    assert!(state.target_menu().unwrap().highlight.is_none());
    let ClientShellOverlay::ContextMenu(menu) = state.overlay.as_ref().unwrap() else {
        panic!("ready target remains")
    };
    assert_eq!(menu.y, y);
}

#[test]
fn target_highlight_retained_patch_cleans_untouched_row_and_keeps_target() {
    let mut state = file_state();
    let baseline = state.compose(106, 20).unwrap();
    let inner = state.hits.panes[0].inner_rect;
    let row = inner.height - 1;
    let col = inner.width - 1;
    resolve_highlight(
        &mut state,
        vec![TargetCellSpan {
            row,
            start_col: col,
            end_col: col + 1,
        }],
    );
    // Keep the menu away from this tiny fixture's two terminal rows.
    if let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_mut() {
        menu.x = 70;
        menu.y = 10;
    }
    let highlighted = state.compose(106, 20).unwrap();
    let index =
        usize::from(inner.y + row) * usize::from(baseline.width) + usize::from(inner.x + col);
    assert_ne!(highlighted.cells[index], baseline.cells[index]);
    let current = state.pane_surface.as_ref().unwrap();
    let mut pane = current.panes[0].clone();
    pane.content_revision += 1;
    let patch = crate::protocol::PaneSurfacePatch {
        boot_id: current.boot_id.clone(),
        projection_revision: current.projection_revision,
        base_surface_revision: current.surface_revision,
        surface_revision: current.surface_revision + 1,
        rows: vec![crate::protocol::PaneSurfacePatchRow {
            x: pane.inner_rect.x,
            y: pane.inner_rect.y,
            cells: vec![current.frame.cells[0].clone()],
        }],
        panes: vec![pane],
        cursor: current.frame.cursor.clone(),
    };
    assert!(matches!(
        state.apply_pane_surface_patch(patch),
        ClientPaneSurfacePatchOutcome::Applied(None)
    ));
    assert!(state.target_menu().unwrap().highlight.is_none());
    let clean = state.compose(106, 20).unwrap();
    assert_eq!(clean.cells[index], baseline.cells[index]);
    assert_eq!(
        state
            .target_menu()
            .unwrap()
            .target
            .as_ref()
            .unwrap()
            .target_id,
        "opaque-target"
    );
    assert!(
        state.ready_file_context_is_current(&state.target_menu().unwrap().context) == Some(true)
    );
}

#[test]
fn target_highlight_clears_on_execution_resize_and_dismissal() {
    for transition in 0..3 {
        let mut state = file_state();
        resolve_highlight(
            &mut state,
            vec![TargetCellSpan {
                row: 0,
                start_col: 0,
                end_col: 1,
            }],
        );
        assert!(state.target_menu().unwrap().highlight.is_some());
        match transition {
            0 => {
                activate(&mut state, ClientContextMenuAction::OpenFile);
            }
            1 => {
                state.compose(105, 20).unwrap();
            }
            _ => {
                state.close_target_menu();
            }
        }
        assert!(state
            .target_menu()
            .is_none_or(|file| file.highlight.is_none()));
    }
}

#[test]
fn target_highlight_pending_scroll_never_resurrects_or_changes_selection() {
    let mut state = file_state();
    state.selection = Some(crate::selection::Selection::absolute_range(
        "pane_1".into(),
        (0, 0),
        (0, 3),
    ));
    let before = format!("{:?}", state.selection);
    resolve_highlight(
        &mut state,
        vec![TargetCellSpan {
            row: 0,
            start_col: 0,
            end_col: 4,
        }],
    );
    state.compose(106, 20).unwrap();
    assert_eq!(format!("{:?}", state.selection), before);
    assert!(state.copy_mode.is_none());
    state.pane_scroll_targets.insert("pane_1".into(), 1);
    state.compose(106, 20).unwrap();
    assert!(state.target_menu().unwrap().highlight.is_none());
    state.pane_scroll_targets.clear();
    state.compose(106, 20).unwrap();
    assert!(state.target_menu().unwrap().highlight.is_none());
    assert_eq!(format!("{:?}", state.selection), before);
    state.close_target_menu();
    state.compose(106, 20).unwrap();
    assert_eq!(format!("{:?}", state.selection), before);
}

fn resolve(state: &mut ClientShellState, id: &str, kind: TargetFileKind) {
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        id,
        Ok(ResponseResult::PaneTargetResolved {
            target: Some(target(kind)),
        }),
    );
    assert!(actions.is_empty(), "resolution must not launch anything");
}

fn activate(
    state: &mut ClientShellState,
    action: ClientContextMenuAction,
) -> Vec<ClientShellAction> {
    let index = file_items(state)
        .iter()
        .position(|item| item.action == action)
        .unwrap();
    let mut outcome = ClientShellInput::default();
    state.activate_context_menu_item(index, &mut outcome);
    outcome.actions
}

fn authorize(state: &mut ClientShellState) -> (String, ActionPermit) {
    let actions = activate(state, ClientContextMenuAction::OpenFile);
    invisible(state);
    let actions = finish_refresh(state, &actions);
    invisible(state);
    let req = request(&actions);
    assert!(
        matches!(&req.method, Method::PaneTargetAuthorize(params) if params.pane_id == "pane_1" && params.target_id == "opaque-target")
    );
    let permit = ActionPermit {
        permit_id: "permit-1".into(),
        action_id: ActionId::SystemOpener,
        revision: settings().revision,
        target: target(TargetFileKind::Text).target,
        profile: settings().profiles.system_opener,
    };
    (req.id.clone(), permit)
}

#[test]
fn absent_file_capabilities_keep_ordinary_pane_actions_without_file_requests() {
    for methods in [
        None,
        Some(Vec::new()),
        Some(vec!["pane.file.resolve".into(), "pane.file.edit".into()]),
        Some(vec!["pane.target.resolve".into()]),
    ] {
        let mut state = file_state();
        state.set_endpoint_methods(methods);
        assert!(right_click(&mut state).actions.is_empty());
        ordinary_pane_actions(&mut state);
    }
}

#[test]
fn file_menu_remote_endpoint_never_resolves_files_or_urls() {
    let mut state = file_state();
    let remote = ClientEndpointId::Ssh(
        crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
    );
    state.endpoints[0].endpoint_id = remote.clone();
    state.active_endpoint_id = remote;
    assert!(right_click(&mut state).actions.is_empty());
    ordinary_pane_actions(&mut state);
}

#[test]
fn file_menu_capabilities_do_not_intercept_selection_or_application_right_click() {
    let mut state = file_state();
    let pane = state.hits.panes[0].clone();
    let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: pane.inner_rect.x,
        row: pane.inner_rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(state.selection.is_some());
    assert!(!outcome.actions.iter().any(|action| matches!(action, ClientShellAction::Endpoint { request, .. } if matches!(request.method, Method::PaneTargetResolve(_)))));
    // An unrelated selection cannot supply a target; the pointer still supplies col 1.
    let id = begin(&mut state);
    resolve(&mut state, &id, TargetFileKind::Text);
    let mut state = file_state();
    let mut projected = snapshot();
    projected.panes[0].right_click_passthrough = true;
    state.set_snapshot(Box::new(projected));
    let mut frame = surface();
    frame.panes[0].mouse_reporting = true;
    state.set_pane_surface(frame);
    state.compose(106, 20).unwrap();
    let outcome = right_click(&mut state);
    assert!(state.overlay.is_none());
    assert!(outcome
        .requests
        .iter()
        .any(|request| matches!(request, ClientMessage::ClientShellPaneInput { .. })));
    assert!(!outcome.actions.iter().any(|action| matches!(action, ClientShellAction::Endpoint { request, .. } if matches!(request.method, Method::FileUrlActionsGet(_)))));
}

fn apply_file_provenance_patch(state: &mut ClientShellState, kind: u8) {
    let mut pane = state.pane_surface.as_ref().unwrap().panes[0].clone();
    if kind == 0 {
        pane.content_revision = 7;
    }
    let patch = crate::protocol::PaneSurfacePatch {
        boot_id: "boot-1".into(),
        projection_revision: 1,
        base_surface_revision: 1,
        surface_revision: 2,
        rows: if kind == 0 {
            vec![crate::protocol::PaneSurfacePatchRow {
                x: 0,
                y: 0,
                cells: vec![crate::protocol::CellData {
                    symbol: "N".into(),
                    fg: 0,
                    bg: 0,
                    modifier: 0,
                    skip: false,
                    hyperlink: None,
                }],
            }]
        } else {
            Vec::new()
        },
        panes: if kind == 0 { vec![pane] } else { Vec::new() },
        cursor: (kind == 2).then_some(crate::protocol::CursorState {
            x: 2,
            y: 0,
            visible: true,
            shape: 2,
        }),
    };
    assert!(matches!(
        state.apply_pane_surface_patch(patch),
        ClientPaneSurfacePatchOutcome::Applied(Some(_))
    ));
    assert_eq!(
        state.hits.file_surface_revision,
        Some((1, 1)),
        "applying is not presentation"
    );
}

#[test]
fn file_menu_patch_provenance_content_noop_cursor_successful_presentation_resolves() {
    for kind in 0..3 {
        let mut state = file_state();
        apply_file_provenance_patch(&mut state, kind);
        state.acknowledge_presented_file_surface((1, 2));
        let outcome = right_click(&mut state);
        let actions = finish_refresh(&mut state, &outcome.actions);
        assert!(
            matches!(&request(&actions).method, Method::PaneTargetResolve(params) if params.content_revision == if kind == 0 { 7 } else { 0 })
        );
    }
}

#[test]
fn file_menu_unpresented_patches_reject_file_actions_but_keep_pane_controls() {
    for kind in 0..3 {
        let mut state = file_state();
        apply_file_provenance_patch(&mut state, kind);
        assert!(right_click(&mut state).actions.is_empty());
        ordinary_pane_actions(&mut state);
    }
    let mut state = file_state();
    apply_file_provenance_patch(&mut state, 0);
    let mut newer = surface();
    newer.surface_revision = 3;
    newer.panes[0].content_revision = 8;
    state.set_pane_surface(newer);
    state.acknowledge_presented_file_surface((1, 2));
    assert_eq!(state.hits.file_surface_revision, Some((1, 1)));
    assert!(right_click(&mut state).actions.is_empty());
    ordinary_pane_actions(&mut state);
}

#[test]
fn file_menu_unpainted_full_surface_does_not_reinterpret_click() {
    let mut state = file_state();
    let mut next = surface();
    next.surface_revision += 1;
    next.panes[0].content_revision += 1;
    state.set_pane_surface(next);
    assert!(right_click(&mut state).actions.is_empty());
    ordinary_pane_actions(&mut state);
}

#[test]
fn file_menu_publishes_only_applicable_actions_and_actual_target() {
    let mut state = file_state();
    let id = begin(&mut state);
    resolve(&mut state, &id, TargetFileKind::Text);
    assert!(state.hits.context_menu_rows.is_empty());
    let items = file_items(&state);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].label, "System opener");
    assert_eq!(items[1].label, "Editor");
    state.compose(106, 20).unwrap();
    assert_eq!(state.hits.context_menu_rows.len(), 2);
    for kind in [TargetFileKind::Binary, TargetFileKind::Unknown] {
        let mut state = file_state();
        let id = begin(&mut state);
        resolve(&mut state, &id, kind);
        assert_eq!(file_items(&state).len(), 1);
        assert_eq!(
            file_items(&state)[0].action,
            ClientContextMenuAction::OpenFile
        );
    }
}

#[test]
fn file_menu_misses_and_failures_keep_only_pane_actions_cancellation_stays_closed() {
    for case in 0..10 {
        let mut state = file_state();
        let id = begin(&mut state);
        let mut value = target(TargetFileKind::Text);
        let result = match case {
            0 => Ok(ResponseResult::PaneTargetResolved { target: None }),
            1 => {
                value.actions.clear();
                Ok(ResponseResult::PaneTargetResolved {
                    target: Some(value),
                })
            }
            2 => {
                value.target_id.clear();
                Ok(ResponseResult::PaneTargetResolved {
                    target: Some(value),
                })
            }
            3 => {
                value.target = TargetValue::Url {
                    url: "javascript:evil()".into(),
                };
                Ok(ResponseResult::PaneTargetResolved {
                    target: Some(value),
                })
            }
            4 => {
                value.revision.counter += 1;
                Ok(ResponseResult::PaneTargetResolved {
                    target: Some(value),
                })
            }
            _ => Err(ClientShellEndpointError {
                code: Some(
                    [
                        "endpoint_timeout",
                        "endpoint_cancelled",
                        "stale_content",
                        "stale_target",
                        "unsupported",
                    ][case - 5]
                        .into(),
                ),
                message: "unavailable".into(),
            }),
        };
        let (_, actions) = state.handle_endpoint_result("boot-1", &id, result);
        assert!(actions.is_empty());
        if case == 6 {
            assert!(state.overlay.is_none());
            invisible(&mut state);
        } else {
            ordinary_pane_actions(&mut state);
        }
    }
}

#[test]
fn normal_pane_actions_survive_animated_output_and_ignore_late_file_results() {
    let mut state = file_state();
    let id = begin(&mut state);
    let mut changed = surface();
    changed.surface_revision += 1;
    changed.panes[0].content_revision += 1;
    state.set_pane_surface(changed);
    ordinary_pane_actions(&mut state);
    let labels: Vec<_> = file_items(&state)
        .into_iter()
        .map(|item| item.label)
        .collect();
    resolve(&mut state, &id, TargetFileKind::Text);
    assert_eq!(
        labels,
        file_items(&state)
            .into_iter()
            .map(|item| item.label)
            .collect::<Vec<_>>()
    );
    ordinary_pane_actions(&mut state);
    let actions = activate(&mut state, ClientContextMenuAction::SplitRight);
    assert!(matches!(&request(&actions).method,
        Method::PaneSplit(params) if params.target_pane_id.as_deref() == Some("pane_1")));
}

#[test]
fn dismissed_lookup_never_reopens_a_generic_menu() {
    for settings_pending in [false, true] {
        let mut state = file_state();
        let outcome = right_click(&mut state);
        let settings_id = request(&outcome.actions).id.clone();
        let resolve_id = if settings_pending {
            None
        } else {
            let actions = finish_refresh(&mut state, &outcome.actions);
            Some(request(&actions).id.clone())
        };
        state.close_target_menu();
        let result = if let Some(id) = resolve_id {
            state.handle_endpoint_result(
                "boot-1",
                &id,
                Ok(ResponseResult::PaneTargetResolved { target: None }),
            )
        } else {
            state.handle_endpoint_result(
                "boot-1",
                &settings_id,
                Ok(ResponseResult::FileUrlActionsSettings {
                    settings: settings(),
                }),
            )
        };
        assert!(result.1.is_empty());
        assert!(state.overlay.is_none());
        invisible(&mut state);
    }
}

#[test]
fn file_menu_border_preserves_ordinary_pane_menu() {
    let mut state = file_state();
    let mut frame = surface();
    frame.panes[0].inner_rect.x = 1;
    frame.panes[0].inner_rect.width = 3;
    state.set_pane_surface(frame);
    state.compose(106, 20).unwrap();
    let pane = state.hits.panes[0].clone();
    let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: pane.rect.x,
        row: pane.rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(outcome.actions.is_empty());
    assert_eq!(
        file_items(&state)[0].action,
        ClientContextMenuAction::RenamePane
    );
}

fn changed_output() -> PaneSurfaceFrame {
    let mut changed = surface();
    changed.surface_revision += 1;
    changed.panes[0].content_revision += 1;
    changed.panes[0].scroll = Some(crate::protocol::PaneSurfaceScrollMetrics {
        offset_from_bottom: 1,
        max_offset_from_bottom: 10,
        viewport_rows: 2,
    });
    changed
}

#[test]
fn file_menu_ready_target_survives_output_scroll_without_reresolving() {
    let mut state = file_state();
    let id = begin(&mut state);
    resolve(&mut state, &id, TargetFileKind::Text);
    state.set_pane_surface(changed_output());
    state.compose(106, 20).unwrap();
    let (id, permit) = authorize(&mut state);
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &id,
        Ok(ResponseResult::PaneTargetAuthorized {
            permit: permit.clone(),
        }),
    );
    let [ClientShellAction::LaunchTargetAction {
        context,
        permit: captured,
    }] = &actions[..]
    else {
        panic!("expected immutable launch");
    };
    assert_eq!(captured.as_ref(), &permit);
    assert!(state.consume_target_permit(context, captured));
    assert!(
        !state.consume_target_permit(context, captured),
        "single attempt"
    );
}

#[test]
fn file_menu_pending_output_change_discards_late_response() {
    let mut state = file_state();
    let id = begin(&mut state);
    state.set_pane_surface(changed_output());
    ordinary_pane_actions(&mut state);
    resolve(&mut state, &id, TargetFileKind::Text);
    ordinary_pane_actions(&mut state);
}

#[test]
fn file_menu_second_click_wins_and_dismissed_reply_cannot_resurrect_menu() {
    let mut state = file_state();
    let old = begin(&mut state);
    let new = begin(&mut state);
    assert_ne!(old, new);
    resolve(&mut state, &old, TargetFileKind::Text);
    invisible(&mut state);
    resolve(&mut state, &new, TargetFileKind::Binary);
    assert_eq!(file_items(&state).len(), 1);
    state.close_target_menu();
    let id = begin(&mut state);
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 105,
        row: 19,
        modifiers: KeyModifiers::empty(),
    })]);
    resolve(&mut state, &id, TargetFileKind::Text);
    invisible(&mut state);
}

#[test]
fn file_menu_ready_target_waits_through_metadata_projection_handoff() {
    for surface_first in [false, true] {
        let mut state = file_state();
        let id = begin(&mut state);
        resolve(&mut state, &id, TargetFileKind::Text);
        state.compose(106, 20).unwrap();
        let mut next_snapshot = snapshot();
        next_snapshot.revision = 2;
        let mut next_surface = surface();
        next_surface.projection_revision = 2;
        next_surface.surface_revision = 2;
        if surface_first {
            state.set_pane_surface(next_surface.clone());
        } else {
            state.set_snapshot(Box::new(next_snapshot.clone()));
        }
        assert!(state.compose(106, 20).is_none());
        assert!(activate(&mut state, ClientContextMenuAction::EditFile).is_empty());
        assert!(state.overlay.is_some());
        if surface_first {
            state.set_snapshot(Box::new(next_snapshot));
        } else {
            state.set_pane_surface(next_surface);
        }
        state.compose(106, 20).unwrap();
        let actions = activate(&mut state, ClientContextMenuAction::EditFile);
        let actions = finish_refresh(&mut state, &actions);
        assert!(
            matches!(&request(&actions).method, Method::PaneTargetExecute(params) if params.pane_id == "pane_1" && params.target_id == "opaque-target")
        );
    }
}

#[test]
fn file_menu_stale_geometry_endpoint_scroll_and_pane_results_are_ignored() {
    for mutation in 0..6 {
        let mut state = file_state();
        let id = begin(&mut state);
        match mutation {
            0 => state.endpoints[0].snapshot_generation = Some(99),
            1 => state.pane_surface.as_mut().unwrap().panes[0].content_revision += 1,
            2 => {
                state.pane_surface.as_mut().unwrap().panes[0]
                    .inner_rect
                    .width += 1
            }
            3 => state.hits.panes[0].inner_rect.x += 1,
            4 => {
                state.pane_scroll_targets.insert("pane_1".into(), 1);
            }
            5 => state.snapshot.as_mut().unwrap().panes.clear(),
            _ => unreachable!(),
        }
        resolve(&mut state, &id, TargetFileKind::Text);
        if matches!(mutation, 1 | 4) {
            ordinary_pane_actions(&mut state);
        } else {
            assert!(state.overlay.is_none());
        }
    }
}

#[test]
fn file_menu_settings_change_invalidates_ready_and_pending_target() {
    for ready in [false, true] {
        let mut state = file_state();
        let id = begin(&mut state);
        if ready {
            resolve(&mut state, &id, TargetFileKind::Text);
        }
        let mut updated = settings();
        updated.revision.counter += 1;
        state.install_action_settings(updated);
        assert!(state.overlay.is_none());
        resolve(&mut state, &id, TargetFileKind::Text);
        invisible(&mut state);
    }
}

#[test]
fn target_permit_rejects_wrong_profile_target_revision_action_and_late_duplicates() {
    for mutation in 0..7 {
        let mut state = file_state();
        let id = begin(&mut state);
        resolve(&mut state, &id, TargetFileKind::Text);
        let (id, mut permit) = authorize(&mut state);
        match mutation {
            0 => permit.profile.executable = "untrusted".into(),
            1 => permit.revision.counter += 1,
            2 => {
                permit.target = TargetValue::Url {
                    url: "https://example.com".into(),
                }
            }
            3 => permit.action_id = ActionId::Editor,
            4 => permit.permit_id.clear(),
            5 => state.close_target_menu(),
            6 => {
                state.endpoints[0].snapshot_generation = Some(99);
            }
            _ => unreachable!(),
        }
        let (_, actions) = state.handle_endpoint_result(
            "boot-1",
            &id,
            Ok(ResponseResult::PaneTargetAuthorized { permit }),
        );
        assert!(actions.is_empty(), "mutation {mutation}");
        assert!(state.action_cache.authorized.is_none());
    }
    let mut state = file_state();
    let id = begin(&mut state);
    resolve(&mut state, &id, TargetFileKind::Text);
    let (id, permit) = authorize(&mut state);
    assert_eq!(
        state
            .handle_endpoint_result(
                "boot-1",
                &id,
                Ok(ResponseResult::PaneTargetAuthorized {
                    permit: permit.clone()
                })
            )
            .1
            .len(),
        1
    );
    assert!(state
        .handle_endpoint_result(
            "boot-1",
            &id,
            Ok(ResponseResult::PaneTargetAuthorized { permit })
        )
        .1
        .is_empty());
}

#[test]
fn action_settings_timer_coalesces_and_reconnect_invalidates_cache() {
    let mut state = file_state();
    let now = std::time::Instant::now();
    let mut outcome = ClientShellInput::default();
    state.tick_action_settings(now, &mut outcome);
    assert_eq!(outcome.actions.len(), 1);
    state.tick_action_settings(now + std::time::Duration::from_secs(10), &mut outcome);
    assert_eq!(outcome.actions.len(), 1);
    finish_refresh(&mut state, &outcome.actions);
    let mut next = ClientShellInput::default();
    state.tick_action_settings(now, &mut next);
    assert!(next.actions.is_empty());
    state.endpoints[0].snapshot_generation = Some(99);
    state.tick_action_settings(now, &mut next);
    assert!(state.action_cache.settings.is_none());
    assert_eq!(next.actions.len(), 1);
}

#[test]
fn target_escape_and_authorization_timeout_have_no_menu_or_retry() {
    for cancel in [false, true] {
        let mut state = file_state();
        let id = begin(&mut state);
        resolve(&mut state, &id, TargetFileKind::Text);
        let (id, permit) = authorize(&mut state);
        if cancel {
            let mut outcome = ClientShellInput::default();
            state.route_overlay_key(
                &crate::input::TerminalKey::new(KeyCode::Esc, KeyModifiers::empty()),
                &mut outcome,
            );
            assert!(outcome.actions.is_empty());
            invisible(&mut state);
        }
        let response = if cancel {
            Ok(ResponseResult::PaneTargetAuthorized { permit })
        } else {
            Err(ClientShellEndpointError {
                code: Some("endpoint_timeout".into()),
                message: "Timed out; do not retry automatically".into(),
            })
        };
        assert!(state
            .handle_endpoint_result("boot-1", &id, response)
            .1
            .is_empty());
        assert!(state.action_cache.authorized.is_none());
        invisible(&mut state);
    }
}

#[test]
fn target_authorization_cutoff_preserves_captured_profile_after_settings_save() {
    let mut state = file_state();
    let id = begin(&mut state);
    resolve(&mut state, &id, TargetFileKind::Text);
    let (id, permit) = authorize(&mut state);
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &id,
        Ok(ResponseResult::PaneTargetAuthorized {
            permit: permit.clone(),
        }),
    );
    let mut updated = settings();
    updated.revision.counter += 1;
    updated.profiles.system_opener.executable = "different-program".into();
    state.install_action_settings(updated);
    let [ClientShellAction::LaunchTargetAction {
        context,
        permit: captured,
    }] = &actions[..]
    else {
        panic!("authorized attempt");
    };
    assert_eq!(captured.as_ref(), &permit);
    assert!(state.consume_target_permit(context, captured));
    assert!(!state.consume_target_permit(context, captured));
}

#[test]
fn target_settings_error_and_disabled_policy_leave_pane_actions_available() {
    for error in [false, true] {
        let mut state = file_state();
        let outcome = right_click(&mut state);
        invisible(&mut state);
        let id = request(&outcome.actions).id.clone();
        let response = if error {
            Err(ClientShellEndpointError {
                code: Some("endpoint_timeout".into()),
                message: "settings unavailable".into(),
            })
        } else {
            let mut value = settings();
            value.diagnostic = Some("Invalid action templates".into());
            Ok(ResponseResult::FileUrlActionsSettings { settings: value })
        };
        assert!(state
            .handle_endpoint_result("boot-1", &id, response)
            .1
            .is_empty());
        ordinary_pane_actions(&mut state);
    }
}
