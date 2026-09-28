use super::responses::{encode_error, encode_success};
use crate::{
    api::schema::{
        EventData, EventEnvelope, EventKind, PaneFileEditParams, PaneFileKind,
        PaneFileResolveParams, ResponseResult,
    },
    app::App,
};

fn local_runtime(runtime: &crate::terminal::TerminalRuntime) -> Option<u32> {
    if !crate::platform::local_file_actions_supported() {
        return None;
    }
    let pid = runtime.child_pid()?;
    // Fail closed when foreground provenance cannot be inspected. This cannot
    // prove the origin of historical output or historical relative-path CWD.
    let job = crate::detect::foreground_job(pid)?;
    if job.processes.is_empty()
        || job
            .processes
            .iter()
            .any(|p| crate::file_paths::remote_program(&p.name))
    {
        return None;
    }
    Some(pid)
}

fn viewport_matches(
    runtime: &crate::terminal::TerminalRuntime,
    revision: u64,
    offset: u64,
) -> bool {
    viewport_version_matches(
        runtime.content_seq(),
        runtime
            .scroll_metrics()
            .map(|m| m.offset_from_bottom as u64)
            .unwrap_or(0),
        revision,
        offset,
    )
}

fn viewport_version_matches(
    current_revision: u64,
    current_offset: u64,
    revision: u64,
    offset: u64,
) -> bool {
    revision.is_multiple_of(2) && current_revision == revision && current_offset == offset
}

impl App {
    pub(super) fn handle_pane_file_resolve(
        &mut self,
        id: String,
        params: PaneFileResolveParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return encode_error(id, "pane_not_found", "pane not found");
        };
        if !self.state.pane_visible_on_active_surface(ws_idx, pane_id) {
            return encode_error(id, "stale_target", "pane is not visible");
        }
        let Some(runtime) =
            self.state
                .runtime_for_pane_in_workspace(&self.terminal_runtimes, ws_idx, pane_id)
        else {
            return encode_error(id, "stale_target", "pane runtime disappeared");
        };
        if !viewport_matches(runtime, params.content_revision, params.offset_from_bottom) {
            return encode_error(id, "stale_content", "pane viewport changed");
        }
        let Some(child_pid) = local_runtime(runtime) else {
            return encode_error(
                id,
                "unsupported_context",
                "local file actions require a supported local foreground context",
            );
        };
        let Some(terminal_id) = self.state.workspaces[ws_idx].terminal_id(pane_id).cloned() else {
            return encode_error(id, "stale_target", "terminal disappeared");
        };
        let (height, width) = runtime.current_size();
        if params.viewport_row >= height || params.col >= width {
            return encode_error(id, "stale_content", "cell outside viewport");
        }
        let Some(cwd) = runtime.follow_cwd().filter(|p| p.is_absolute()) else {
            return encode_error(id, "unsupported_context", "pane CWD is unknown");
        };
        let area = ratatui::layout::Rect::new(0, 0, width, height);
        let link = runtime
            .visible_hyperlinks(area)
            .into_iter()
            .find(|((x, y), _, _)| *x == params.col && *y == params.viewport_row);
        let candidates = if let Some((_, _, uri)) = link {
            // An explicit non-file URI must never become a filename or run a plugin.
            if uri.starts_with("file://") {
                vec![uri]
            } else {
                Vec::new()
            }
        } else {
            let metrics = runtime.scroll_metrics();
            let selection = crate::selection::Selection::line_range(
                pane_id,
                crate::selection::absolute_row_for_viewport(0, metrics),
                crate::selection::absolute_row_for_viewport(height.saturating_sub(1), metrics),
                width.saturating_sub(1),
            );
            runtime
                .extract_selection(&selection)
                .and_then(|text| {
                    let cell = crate::app::actions::logical_cell_for_visible_cell(
                        &text,
                        width,
                        params.viewport_row,
                        params.col,
                    )?;
                    Some(crate::file_paths::candidates_at(&text, cell.byte_index))
                })
                .unwrap_or_default()
        };
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let file = candidates.iter().take(4).find_map(|candidate| {
            crate::file_paths::resolve_candidate(candidate, &cwd, home.as_deref())
        });
        if !viewport_matches(runtime, params.content_revision, params.offset_from_bottom)
            || local_runtime(runtime) != Some(child_pid)
        {
            return encode_error(id, "stale_content", "pane changed during file resolution");
        }
        let target = file.and_then(|file| {
            self.file_targets
                .insert(params.pane_id, terminal_id, child_pid, file)
        });
        encode_success(id, ResponseResult::PaneFileResolved { target })
    }

    pub(super) fn handle_pane_file_edit(
        &mut self,
        id: String,
        params: PaneFileEditParams,
    ) -> String {
        let Some(target) = self.file_targets.take(&params.pane_id, &params.target_id) else {
            return encode_error(
                id,
                "stale_target",
                "file target expired or does not belong to pane",
            );
        };
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return encode_error(id, "stale_target", "original pane disappeared");
        };
        if self.state.workspaces[ws_idx].terminal_id(pane_id) != Some(&target.terminal_id) {
            return encode_error(id, "stale_target", "original terminal changed");
        }
        let Some(runtime) =
            self.state
                .runtime_for_pane_in_workspace(&self.terminal_runtimes, ws_idx, pane_id)
        else {
            return encode_error(id, "stale_target", "original runtime disappeared");
        };
        if !local_runtime(runtime).is_some_and(|pid| {
            self.state.workspaces[ws_idx]
                .terminal_id(pane_id)
                .is_some_and(|terminal_id| target.matches_runtime(terminal_id, pid))
        }) {
            return encode_error(
                id,
                "unsupported_context",
                "original local foreground context changed",
            );
        }
        let file = target.file;
        let Ok((metadata, sample)) = crate::platform::sample_local_regular_file(&file.path) else {
            return encode_error(id, "stale_target", "file no longer readable");
        };
        if !crate::platform::same_local_file(&file.metadata, &metadata) {
            return encode_error(id, "stale_target", "file changed since resolution");
        }
        if file.kind != PaneFileKind::Text
            || crate::file_paths::classify(&sample, metadata.len()) != PaneFileKind::Text
        {
            return encode_error(id, "not_text", "editor requires a sampled UTF-8 text file");
        }
        let argv = match crate::platform::local_file_editor_argv(&file.path, file.line, file.column)
        {
            Ok(argv) => argv,
            Err(err) => return encode_error(id, "editor_unavailable", err.to_string()),
        };
        let (rows, cols) = self.state.estimate_pane_size();
        let previous_focus = self.state.current_pane_focus_target();
        let result = self.state.workspaces[ws_idx].split_pane_argv_command(
            pane_id,
            ratatui::layout::Direction::Horizontal,
            rows.max(4),
            cols.max(10),
            file.path.parent().map(ToOwned::to_owned),
            &argv,
            Vec::new(),
            self.state.pane_scrollback_limit_bytes,
            self.state.host_terminal_theme,
            self.state.host_terminal_appearance,
            true,
        );
        let (tab_idx, new_pane) = match result {
            Some(Ok(result)) => result,
            Some(Err(err)) => return encode_error(id, "pane_split_failed", err.to_string()),
            None => return encode_error(id, "stale_target", "original pane disappeared"),
        };
        let new_id = new_pane.pane_id;
        self.terminal_runtimes
            .insert(new_pane.terminal.id.clone(), new_pane.runtime);
        self.state
            .terminals
            .insert(new_pane.terminal.id.clone(), new_pane.terminal);
        self.state.remove_alias_shadowed_by_new_pane(new_id);
        self.state.switch_workspace_tab(ws_idx, tab_idx);
        self.state
            .record_pane_focus_change(previous_focus, ws_idx, new_id);
        self.state.mode = crate::app::Mode::Terminal;
        self.schedule_session_save();
        let Some(pane) = self.pane_info(ws_idx, new_id) else {
            self.emit_layout_updated_event(ws_idx, tab_idx);
            return encode_error(id, "pane_not_found", "new editor pane disappeared");
        };
        let new_id = pane.pane_id.clone();
        self.emit_event(EventEnvelope {
            event: EventKind::PaneCreated,
            data: EventData::PaneCreated { pane },
        });
        self.emit_layout_updated_event(ws_idx, tab_idx);
        encode_success(id, ResponseResult::PaneFileEdited { pane_id: new_id })
    }
}

#[cfg(test)]
mod tests {
    use super::viewport_version_matches;
    #[test]
    fn file_viewport_rejects_stale_and_in_progress_content() {
        assert!(viewport_version_matches(4, 2, 4, 2));
        assert!(!viewport_version_matches(6, 2, 4, 2));
        assert!(!viewport_version_matches(4, 3, 4, 2));
        assert!(!viewport_version_matches(5, 2, 5, 2));
    }
}
