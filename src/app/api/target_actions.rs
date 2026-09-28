//! Scoped, optional target-action API. Legacy pane.file methods are independent.
use super::responses::{encode_error, encode_success};
use crate::{
    api::{
        schema::{
            FileUrlActionsSetParams, Method, PaneTargetActionParams, PaneTargetResolveParams,
            Request, ResponseResult,
        },
        ActionRequestContext,
    },
    app::App,
    file_url_actions::{
        ActionId, ActionPermit, ActionRevision, LaunchMode, ResolvedTarget, TargetFileKind,
        TargetValue,
    },
};
use std::{
    collections::VecDeque,
    path::PathBuf,
    time::{Duration, Instant},
};

mod highlight;

#[cfg(test)]
#[path = "target_actions_tests.rs"]
mod tests;

const TARGET_TTL: Duration = Duration::from_secs(60);
const TARGET_CAPACITY: usize = 64;

struct CachedActionTarget {
    token: String,
    client_id: u64,
    pane_id: String,
    terminal_id: crate::terminal::TerminalId,
    child_pid: u32,
    revision: ActionRevision,
    target: TargetValue,
    metadata: Option<std::fs::Metadata>,
    cwd: PathBuf,
    created: Instant,
}

pub(crate) struct ActionTargetCache {
    entries: VecDeque<CachedActionTarget>,
    next: u64,
    startup_cwd: Option<PathBuf>,
}

impl Default for ActionTargetCache {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            next: 0,
            startup_cwd: std::env::current_dir()
                .ok()
                .and_then(|p| p.canonicalize().ok()),
        }
    }
}

impl ActionTargetCache {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
    fn insert(&mut self, mut entry: CachedActionTarget) -> Option<String> {
        self.entries.retain(|e| e.created.elapsed() < TARGET_TTL);
        while self.entries.len() >= TARGET_CAPACITY {
            self.entries.pop_front();
        }
        self.next = self.next.checked_add(1)?;
        entry.token = format!("target-{}-{}", entry.revision.boot_id, self.next);
        let token = entry.token.clone();
        self.entries.push_back(entry);
        Some(token)
    }
    fn take(
        &mut self,
        scope: ActionRequestContext,
        pane: &str,
        token: &str,
    ) -> Option<CachedActionTarget> {
        self.entries.retain(|e| e.created.elapsed() < TARGET_TTL);
        let index = self.entries.iter().position(|e| {
            e.client_id == scope.client_id && e.pane_id == pane && e.token == token
        })?;
        self.entries.remove(index)
    }
}

fn local_file_context(runtime: &crate::terminal::TerminalRuntime) -> Option<u32> {
    if !crate::platform::local_file_actions_supported() {
        return None;
    }
    let pid = runtime.child_pid()?;
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
    params: &PaneTargetResolveParams,
) -> bool {
    params.content_revision.is_multiple_of(2)
        && runtime.content_seq() == params.content_revision
        && runtime
            .scroll_metrics()
            .map(|m| m.offset_from_bottom as u64)
            .unwrap_or(0)
            == params.offset_from_bottom
}

fn file_kind(kind: crate::api::schema::PaneFileKind) -> TargetFileKind {
    match kind {
        crate::api::schema::PaneFileKind::Text => TargetFileKind::Text,
        crate::api::schema::PaneFileKind::Binary => TargetFileKind::Binary,
        crate::api::schema::PaneFileKind::Unknown => TargetFileKind::Unknown,
    }
}

fn resolve_file(
    candidates: &[String],
    cwd: &std::path::Path,
) -> Option<(TargetValue, std::fs::Metadata, usize)> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let (file, index) = candidates
        .iter()
        .take(4)
        .enumerate()
        .find_map(|(index, candidate)| {
            crate::file_paths::resolve_candidate(candidate, cwd, home.as_deref())
                .map(|file| (file, index))
        })?;
    let target = TargetValue::File {
        path: file.path.to_str()?.to_owned(),
        line: file.line,
        column: file.column,
        file_kind: file_kind(file.kind),
    };
    target.validate().ok()?;
    Some((target, file.metadata, index))
}

fn validate_file_identity(target: &CachedActionTarget) -> Result<(), String> {
    let TargetValue::File {
        path,
        file_kind: expected_kind,
        ..
    } = &target.target
    else {
        return Ok(());
    };
    let (metadata, sample) = crate::platform::sample_local_regular_file(std::path::Path::new(path))
        .map_err(|_| "file no longer readable".to_string())?;
    if !target
        .metadata
        .as_ref()
        .is_some_and(|old| crate::platform::same_local_file(old, &metadata))
        || file_kind(crate::file_paths::classify(&sample, metadata.len())) != *expected_kind
    {
        return Err("file identity or classification changed".into());
    }
    Ok(())
}

impl App {
    /// Called only with transport-derived context, never with JSON identity fields.
    pub(crate) fn handle_scoped_action_request(
        &mut self,
        request: Request,
        scope: Option<ActionRequestContext>,
    ) -> String {
        match request.method {
            Method::FileUrlActionsGet(_) => encode_success(
                request.id,
                ResponseResult::FileUrlActionsSettings {
                    settings: self.action_store.settings(),
                },
            ),
            Method::FileUrlActionsSet(params) => {
                self.handle_action_settings_set(request.id, params)
            }
            Method::PaneTargetResolve(params) => {
                let Some(scope) = scope else {
                    return encode_error(
                        request.id,
                        "client_scope_required",
                        "clicked targets require an attached client",
                    );
                };
                self.handle_target_resolve(request.id, params, scope)
            }
            Method::PaneTargetExecute(params) | Method::PaneTargetAuthorize(params)
                if scope.is_none() =>
            {
                let _ = params;
                encode_error(
                    request.id,
                    "client_scope_required",
                    "clicked targets require an attached client",
                )
            }
            Method::PaneTargetExecute(params) => {
                self.handle_target_action(request.id, params, scope, LaunchMode::Pane)
            }
            Method::PaneTargetAuthorize(params) => {
                self.handle_target_action(request.id, params, scope, LaunchMode::External)
            }
            _ => encode_error(
                request.id,
                "unsupported_method",
                "not a target action method",
            ),
        }
    }

    fn handle_action_settings_set(
        &mut self,
        id: String,
        params: FileUrlActionsSetParams,
    ) -> String {
        match self
            .action_store
            .save(&params.expected_revision, params.profiles)
        {
            Ok(settings) => {
                self.action_targets.clear();
                encode_success(id, ResponseResult::FileUrlActionsSettings { settings })
            }
            Err((code, message)) => encode_error(id, code, message),
        }
    }

    fn handle_target_resolve(
        &mut self,
        id: String,
        params: PaneTargetResolveParams,
        scope: ActionRequestContext,
    ) -> String {
        let settings = match self.action_store.check(&params.expected_revision) {
            Ok(settings) => settings,
            Err((code, message)) => return encode_error(id, code, message),
        };
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
        if !viewport_matches(runtime, &params) {
            return encode_error(id, "stale_content", "pane viewport changed");
        }
        let Some(child_pid) = runtime.child_pid() else {
            return encode_error(id, "unsupported_context", "pane has no local runtime");
        };
        let Some(terminal_id) = self.state.workspaces[ws_idx].terminal_id(pane_id).cloned() else {
            return encode_error(id, "stale_target", "terminal disappeared");
        };
        let (height, width) = runtime.current_size();
        if params.viewport_row >= height || params.col >= width {
            return encode_error(id, "stale_content", "cell outside viewport");
        }
        let file_context = local_file_context(runtime) == Some(child_pid);
        // Never use an SSH/container-reported path for a local command pane.
        let local_cwd = file_context
            .then(|| runtime.follow_cwd())
            .flatten()
            .filter(|p| p.is_absolute() && p.is_dir())
            .and_then(|p| p.canonicalize().ok());
        let launch_cwd = local_cwd
            .clone()
            .or_else(|| self.action_targets.startup_cwd.clone());
        let links = runtime.visible_hyperlinks(ratatui::layout::Rect::new(0, 0, width, height));
        let link = links
            .iter()
            .position(|((x, y), _, _)| *x == params.col && *y == params.viewport_row);
        let mut metadata = None;
        let mut highlight = Vec::new();
        let target = if let Some(index) = link {
            let uri = links[index].2.clone();
            highlight = highlight::hyperlink_spans(&links, index, width, height);
            if crate::file_url_actions::valid_web_url(&uri) {
                Some(TargetValue::Url { url: uri })
            } else if uri.starts_with("file://") && file_context {
                local_cwd
                    .as_deref()
                    .and_then(|cwd| resolve_file(&[uri], cwd))
                    .map(|(target, meta, _)| {
                        metadata = Some(meta);
                        target
                    })
            } else {
                None
            } // Explicit unsupported links never fall back to their labels.
        } else {
            let metrics = runtime.scroll_metrics();
            let selection = crate::selection::Selection::line_range(
                pane_id,
                crate::selection::absolute_row_for_viewport(0, metrics),
                crate::selection::absolute_row_for_viewport(height.saturating_sub(1), metrics),
                width.saturating_sub(1),
            );
            runtime.extract_selection(&selection).and_then(|text| {
                let cell = crate::app::actions::logical_cell_for_visible_cell(
                    &text,
                    width,
                    params.viewport_row,
                    params.col,
                )?;
                if let Some(url) = crate::file_url_actions::web_url_at(&text, cell.byte_index) {
                    highlight =
                        highlight::source_spans(&text, cell.byte_index, &url, width, height);
                    return Some(TargetValue::Url { url });
                }
                if !file_context {
                    return None;
                }
                let candidates =
                    crate::file_url_actions::file_candidates_at(&text, cell.byte_index);
                let (target, meta, index) = resolve_file(&candidates, local_cwd.as_deref()?)?;
                highlight = highlight::source_spans(
                    &text,
                    cell.byte_index,
                    &candidates[index],
                    width,
                    height,
                );
                metadata = Some(meta);
                Some(target)
            })
        };
        if !viewport_matches(runtime, &params)
            || runtime.child_pid() != Some(child_pid)
            || (metadata.is_some() && local_file_context(runtime) != Some(child_pid))
        {
            return encode_error(id, "stale_content", "pane changed during target resolution");
        }
        let Some(target) = target else {
            return encode_success(id, ResponseResult::PaneTargetResolved { target: None });
        };
        let actions: Vec<_> = [ActionId::Editor, ActionId::SystemOpener]
            .into_iter()
            .filter(|action| settings.profiles.profile(*action).applies_to(&target))
            .collect();
        if actions.is_empty() {
            return encode_success(id, ResponseResult::PaneTargetResolved { target: None });
        }
        let cwd = match &target {
            TargetValue::File { path, .. } => PathBuf::from(path).parent().map(ToOwned::to_owned),
            TargetValue::Url { .. } => launch_cwd,
        };
        let Some(cwd) = cwd else {
            return encode_error(
                id,
                "unsupported_context",
                "no validated local working directory",
            );
        };
        let entry = CachedActionTarget {
            token: String::new(),
            client_id: scope.client_id,
            pane_id: params.pane_id,
            terminal_id,
            child_pid,
            revision: settings.revision.clone(),
            target: target.clone(),
            metadata,
            cwd,
            created: Instant::now(),
        };
        let Some(target_id) = self.action_targets.insert(entry) else {
            return encode_error(id, "target_capacity", "target sequence exhausted");
        };
        encode_success(
            id,
            ResponseResult::PaneTargetResolved {
                target: Some(ResolvedTarget {
                    target_id,
                    target,
                    revision: settings.revision,
                    actions,
                    highlight,
                }),
            },
        )
    }

    fn handle_target_action(
        &mut self,
        id: String,
        params: PaneTargetActionParams,
        scope: Option<ActionRequestContext>,
        mode: LaunchMode,
    ) -> String {
        let Some(scope) = scope else {
            return encode_error(
                id,
                "client_scope_required",
                "clicked targets require an attached client",
            );
        };
        // Consume first: failed validation/launch is not automatically retried.
        let Some(target) = self
            .action_targets
            .take(scope, &params.pane_id, &params.target_id)
        else {
            return encode_error(
                id,
                "stale_target",
                "target expired, consumed, or belongs to another client/pane",
            );
        };
        let settings = match self.action_store.check(&params.revision) {
            Ok(settings) => settings,
            Err((code, message)) => return encode_error(id, code, message),
        };
        if target.revision != params.revision {
            return encode_error(
                id,
                "settings_conflict",
                "target was resolved with different settings",
            );
        }
        let profile = settings.profiles.profile(params.action_id).clone();
        if profile.launch_mode != mode || !profile.applies_to(&target.target) {
            return encode_error(
                id,
                "action_not_applicable",
                "action is disabled, incompatible, or uses another launch mode",
            );
        }
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
        if runtime.child_pid() != Some(target.child_pid) {
            return encode_error(id, "stale_target", "original runtime changed");
        }
        if let Err(error) = target.target.validate() {
            return encode_error(id, "invalid_target", error);
        }
        if matches!(&target.target, TargetValue::File { .. }) {
            if local_file_context(runtime) != Some(target.child_pid) {
                return encode_error(
                    id,
                    "unsupported_context",
                    "original local foreground context changed",
                );
            }
            if let Err(error) = validate_file_identity(&target) {
                return encode_error(id, "stale_target", error);
            }
        }
        if mode == LaunchMode::External {
            return encode_success(
                id,
                ResponseResult::PaneTargetAuthorized {
                    permit: ActionPermit {
                        permit_id: format!("permit-{}", target.token),
                        action_id: params.action_id,
                        revision: settings.revision,
                        target: target.target,
                        profile,
                    },
                },
            );
        }
        // Pane argv is expanded here; external argv is expanded only by the
        // client from the captured permit, never reparsed as shell syntax.
        let argv = match profile.render_argv(&target.target) {
            Ok(argv) => argv,
            Err(error) => return encode_error(id, "invalid_action_settings", error),
        };
        if !target.cwd.is_absolute() || !target.cwd.is_dir() {
            return encode_error(id, "stale_target", "local working directory disappeared");
        }
        let (rows, cols) = self.state.estimate_pane_size();
        let previous_focus = self.state.current_pane_focus_target();
        let result = self.state.workspaces[ws_idx].split_pane_argv_command(
            pane_id,
            ratatui::layout::Direction::Horizontal,
            rows.max(4),
            cols.max(10),
            Some(target.cwd),
            &argv,
            Vec::new(),
            self.state.pane_scrollback_limit_bytes,
            self.state.host_terminal_theme,
            self.state.host_terminal_appearance,
            true,
        );
        let (tab_idx, new_pane) = match result {
            Some(Ok(result)) => result,
            Some(Err(error)) => return encode_error(id, "action_launch_failed", error.to_string()),
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
            return encode_error(id, "pane_not_found", "new action pane disappeared");
        };
        let pane_id = pane.pane_id.clone();
        self.emit_event(crate::api::schema::EventEnvelope {
            event: crate::api::schema::EventKind::PaneCreated,
            data: crate::api::schema::EventData::PaneCreated { pane },
        });
        self.emit_layout_updated_event(ws_idx, tab_idx);
        encode_success(id, ResponseResult::PaneTargetExecuted { pane_id })
    }
}
