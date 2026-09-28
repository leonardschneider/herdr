use super::*;

#[tokio::test]
async fn action_settings_from_inactive_client_do_not_claim_foreground_or_geometry() {
    let mut server = test_headless_server();
    let config_path = server.client_socket_path.with_extension("actions.toml");
    server.app.action_store = crate::config::ActionStore::new(config_path.clone());
    server.app.state.workspaces = vec![crate::workspace::Workspace::test_new("settings")];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    let (active_writer, active_rx, _active_render) = test_client_writer();
    server.handle_server_event(ServerEvent::ClientShellConnected {
        client_id: 71,
        surface_cols: 80,
        surface_rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        pixel_mouse: false,
        direct_graphics: false,
        endpoint_keybindings: false,
        mouse_capture: false,
        surface_active: true,
        writer: active_writer,
    });
    let _ = active_rx.recv().unwrap();
    let (writer, control_rx, _render_rx) = test_client_writer();
    server.handle_server_event(ServerEvent::ClientShellConnected {
        client_id: 72,
        surface_cols: 140,
        surface_rows: 50,
        cell_width_px: 0,
        cell_height_px: 0,
        pixel_mouse: false,
        direct_graphics: false,
        endpoint_keybindings: false,
        mouse_capture: false,
        surface_active: false,
        writer,
    });
    let _ = control_rx.recv().unwrap();
    let foreground = server.foreground_client_id;
    let geometry = server.effective_size;
    let focus = server.app.session_snapshot().focused_pane_id;
    let initial = server.app.action_store.settings();
    for (id, method) in [
        (
            "settings-get",
            api::schema::Method::FileUrlActionsGet(api::schema::EmptyParams::default()),
        ),
        (
            "settings-set",
            api::schema::Method::FileUrlActionsSet(api::schema::FileUrlActionsSetParams {
                expected_revision: initial.revision,
                profiles: initial.profiles,
            }),
        ),
    ] {
        assert!(
            !server.handle_server_event(ServerEvent::ClientShellEndpointRequest {
                client_id: 72,
                boot_id: server.client_shell_boot_id.clone(),
                request: Box::new(api::schema::Request {
                    id: id.into(),
                    method
                }),
            })
        );
        let event = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            server.server_event_rx.recv(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!server.handle_server_event(event));
        let ServerMessage::ClientShellEndpointResponseChunk {
            data, request_id, ..
        } = read_server_message(control_rx.recv().unwrap())
        else {
            panic!("expected settings response");
        };
        assert_eq!(request_id, id);
        let response: api::schema::SuccessResponse = serde_json::from_slice(&data).unwrap();
        assert!(matches!(
            response.result,
            api::schema::ResponseResult::FileUrlActionsSettings { .. }
        ));
        assert_eq!(server.foreground_client_id, foreground);
        assert_eq!(server.effective_size, geometry);
        assert_eq!(server.app.session_snapshot().focused_pane_id, focus);
        assert!(!server.clients[&72].shell_surface_active);
    }
    std::fs::remove_file(config_path).unwrap();
    shutdown_test_runtimes(&mut server);
}
