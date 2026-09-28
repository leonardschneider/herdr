use super::*;
use crate::file_url_actions::{ActionId, LaunchMode, TargetCellSpan};

fn validated_file_highlight(spans: &[TargetCellSpan], inner: Rect) -> Option<Vec<TargetCellSpan>> {
    if spans.is_empty()
        || spans.len() > usize::from(inner.width) * usize::from(inner.height)
        || spans.iter().any(|span| {
            span.row >= inner.height || span.start_col >= span.end_col || span.end_col > inner.width
        })
        || spans.windows(2).any(|pair| {
            pair[1].row < pair[0].row
                || (pair[1].row == pair[0].row && pair[1].start_col < pair[0].end_col)
        })
    {
        return None;
    }
    Some(spans.to_vec())
}

impl ClientShellState {
    /// Retained patches bypass composition. Advance click provenance only after
    /// the host presentation succeeds, never merely when applying incoming cells.
    pub(crate) fn acknowledge_presented_file_surface(&mut self, revision: (u64, u64)) {
        if self.pane_surface.as_ref().is_some_and(|surface| {
            (surface.projection_revision, surface.surface_revision) == revision
        }) {
            self.hits.file_surface_revision = Some(revision);
        }
    }

    pub(super) fn local_file_methods_available(&self) -> bool {
        self.active_endpoint_id.is_local()
            && self.endpoint_is_online(&self.active_endpoint_id)
            && ["file_url_actions.get", "pane.target.resolve"]
                .iter()
                .all(|method| self.advertises_endpoint_method(method))
    }

    pub(super) fn begin_file_context_menu(
        &mut self,
        x: u16,
        y: u16,
        outcome: &mut ClientShellInput,
    ) {
        // Keep the menu hidden during lookup; publish either applicable target
        // actions or the ordinary pane menu, never an interim wrong-action menu.
        self.overlay = None;
        self.action_cache.authorized = None;
        self.hits.context_menu_rows.clear();
        if !self.local_file_methods_available() {
            return;
        }
        let Some(hit) = self
            .hits
            .panes
            .iter()
            .find(|hit| super::contains(hit.inner_rect, (x, y)))
        else {
            return;
        };
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let Some(surface) = self
            .pane_surface
            .as_ref()
            .filter(|surface| surface.projection_revision == snapshot.revision)
        else {
            return;
        };
        // An incoming surface may be newer than the frame the user clicked. Do not
        // interpret the old screen coordinates against that unpainted content.
        if self.hits.file_surface_revision
            != Some((surface.projection_revision, surface.surface_revision))
        {
            return;
        }
        let Some(pane) = surface
            .panes
            .iter()
            .find(|pane| pane.pane_id == hit.pane_id)
        else {
            return;
        };
        let context = ClientFileContext {
            endpoint_id: self.active_endpoint_id.clone(),
            connection_generation: self
                .endpoints
                .iter()
                .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
                .and_then(|endpoint| endpoint.snapshot_generation),
            boot_id: snapshot.boot_id.clone(),
            pane_id: hit.pane_id.clone(),
            generation: self.next_request_id,
            content_revision: pane.content_revision,
            offset_from_bottom: hit
                .scroll
                .map_or(0, |scroll| scroll.offset_from_bottom as u64),
            inner_rect: hit.inner_rect,
            pane_rect: hit.rect,
            surface_pane_rect: pane.rect,
            surface_inner_rect: pane.inner_rect,
            alternate_screen_active: pane.alternate_screen_active,
            composed_size: self.last_composed_size,
            viewport_row: y - hit.inner_rect.y,
            col: x - hit.inner_rect.x,
        };
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::Pane {
                pane_id: context.pane_id.clone(),
                workspace_id: String::new(),
                source_pane_id: None,
                has_manual_label: false,
                right_click_passthrough: false,
                file: Some(Box::new(ClientFileMenu {
                    context,
                    highlight: None,
                    phase: TargetMenuPhase::RefreshResolve,
                    target: None,
                    settings: None,
                })),
            },
            x,
            y,
            highlighted: 0,
        }));
        if !self.refresh_action_settings(outcome) {
            self.finish_target_lookup_without_actions();
        }
    }

    pub(super) fn finish_target_lookup_without_actions(&mut self) {
        let fallback = match self.overlay.as_ref() {
            Some(ClientShellOverlay::ContextMenu(menu)) => match &menu.target {
                ClientContextMenuTarget::Pane {
                    file: Some(file), ..
                } if matches!(
                    file.phase,
                    TargetMenuPhase::RefreshResolve | TargetMenuPhase::Resolving
                ) && self.ready_file_context_is_current(&file.context) == Some(true) =>
                {
                    Some((file.context.pane_id.clone(), menu.x, menu.y))
                }
                _ => None,
            },
            _ => None,
        };
        // Pane actions depend on pane identity/geometry, not the clicked text.
        // Output or scrolling must not make ordinary pane controls disappear.
        // A cancelled/dismissed/replaced menu or an executing file action never
        // creates a new generic menu here.
        self.close_target_menu();
        if let Some((pane_id, x, y)) = fallback {
            self.open_pane_context_menu(pane_id, x, y);
        }
    }

    fn pending_file_context_is_current(&self, context: &ClientFileContext) -> bool {
        self.local_file_methods_available()
            && context.endpoint_id == self.active_endpoint_id
            && self.endpoints.iter().any(|endpoint| {
                endpoint.endpoint_id == context.endpoint_id
                    && endpoint.snapshot_generation == context.connection_generation
            })
            && self.mode == ClientShellMode::Terminal
            && self.last_composed_size == context.composed_size
            && self.popup_terminal_id.is_none()
            && !self.popup_pending
            && self.snapshot.as_deref().is_some_and(|snapshot| {
                snapshot.boot_id == context.boot_id
                    && snapshot
                        .panes
                        .iter()
                        .any(|pane| pane.pane_id == context.pane_id)
                    && self.pane_surface.as_ref().is_some_and(|surface| {
                        surface.projection_revision == snapshot.revision
                            && surface.panes.iter().any(|pane| {
                                pane.pane_id == context.pane_id
                                    && pane.content_revision == context.content_revision
                                    && pane.rect == context.surface_pane_rect
                                    && pane.inner_rect == context.surface_inner_rect
                                    && pane.alternate_screen_active
                                        == context.alternate_screen_active
                                    && pane.scroll.map_or(0, |scroll| scroll.offset_from_bottom)
                                        == context.offset_from_bottom
                            })
                    })
            })
            && self.hits.panes.iter().any(|hit| {
                hit.pane_id == context.pane_id
                    && hit.rect == context.pane_rect
                    && hit.inner_rect == context.inner_rect
                    && hit
                        .scroll
                        .map_or(0, |scroll| scroll.offset_from_bottom as u64)
                        == context.offset_from_bottom
            })
            && self
                .pane_scroll_targets
                .get(&context.pane_id)
                .is_none_or(|offset| *offset as u64 == context.offset_from_bottom)
    }

    /// A resolved target is immutable: subsequent output and scrolling cannot
    /// change its meaning. None means a projection/frame handoff is incomplete;
    /// retain the menu, but do not dispatch an action until geometry is known.
    pub(super) fn ready_file_context_is_current(
        &self,
        context: &ClientFileContext,
    ) -> Option<bool> {
        if !self.local_file_methods_available()
            || context.endpoint_id != self.active_endpoint_id
            || !self.endpoints.iter().any(|endpoint| {
                endpoint.endpoint_id == context.endpoint_id
                    && endpoint.snapshot_generation == context.connection_generation
            })
            || self.mode != ClientShellMode::Terminal
            || self.last_composed_size != context.composed_size
            || self.popup_terminal_id.is_some()
            || self.popup_pending
        {
            return Some(false);
        }
        let Some(snapshot) = self.snapshot.as_deref() else {
            return Some(false);
        };
        if snapshot.boot_id != context.boot_id
            || !snapshot
                .panes
                .iter()
                .any(|pane| pane.pane_id == context.pane_id)
        {
            return Some(false);
        }
        let surface = self.pane_surface.as_ref()?;
        if surface.boot_id != context.boot_id {
            return Some(false);
        }
        if self.pending_pane_surface.is_some() || surface.projection_revision != snapshot.revision {
            return None;
        }
        let Some(pane) = surface
            .panes
            .iter()
            .find(|pane| pane.pane_id == context.pane_id)
        else {
            return Some(false);
        };
        if pane.rect != context.surface_pane_rect
            || pane.inner_rect != context.surface_inner_rect
            || pane.alternate_screen_active != context.alternate_screen_active
        {
            return Some(false);
        }
        // Wait for hit geometry from this projection, not for its latest text
        // frame: content-only surface updates cannot change the frozen target.
        if self
            .hits
            .file_surface_revision
            .is_none_or(|(projection, _)| projection != surface.projection_revision)
        {
            return None;
        }
        Some(self.hits.panes.iter().any(|hit| {
            hit.pane_id == context.pane_id
                && hit.rect == context.pane_rect
                && hit.inner_rect == context.inner_rect
        }))
    }

    pub(super) fn file_highlight_is_current(&self, file: &ClientFileMenu) -> bool {
        file.phase == TargetMenuPhase::Ready
            && self.pending_pane_surface.is_none()
            && self.pending_file_context_is_current(&file.context)
    }

    pub(super) fn invalidate_stale_file_menu(&mut self) {
        // Decoration follows the captured cells, unlike the immutable launch target.
        // Clearing is permanent: scrolling back must not resurrect a prior highlight.
        let clear_highlight = self
            .target_menu()
            .is_some_and(|file| file.highlight.is_some() && !self.file_highlight_is_current(file));
        if clear_highlight {
            if let Some(file) = self.target_menu_mut() {
                file.highlight = None;
            }
        }
        let stale = match self.overlay.as_ref() {
            Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
                target:
                    ClientContextMenuTarget::Pane {
                        file: Some(file), ..
                    },
                ..
            })) => {
                if file.target.is_none() {
                    !self.pending_file_context_is_current(&file.context)
                } else {
                    self.ready_file_context_is_current(&file.context) == Some(false)
                }
            }
            _ => false,
        };
        if stale {
            self.finish_target_lookup_without_actions();
        }
    }

    /// Rechecked immediately before the client-host side effect, after menu activation.
    pub(crate) fn local_file_action_is_current(&self, context: &ClientFileContext) -> bool {
        self.overlay.is_none() && self.ready_file_context_is_current(context) == Some(true)
    }

    pub(super) fn complete_file_context_menu(
        &mut self,
        context: ClientFileContext,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        let matches_menu = self.target_menu().is_some_and(|file| {
            file.phase == TargetMenuPhase::Resolving
                && file.context.generation == context.generation
                && file.context.endpoint_id == context.endpoint_id
                && file.context.boot_id == context.boot_id
        });
        if !matches_menu {
            return (false, Vec::new());
        }
        if matches!(&result, Err(error) if error.code.as_deref() == Some("endpoint_cancelled")) {
            self.overlay = None;
            self.hits.context_menu_rows.clear();
            return (true, Vec::new());
        }
        if !self.pending_file_context_is_current(&context) {
            self.finish_target_lookup_without_actions();
            return (true, Vec::new());
        }
        let target = match result {
            Ok(crate::api::schema::ResponseResult::PaneTargetResolved { target }) => target,
            _ => None,
        };
        let Some(mut target) = target else {
            self.finish_target_lookup_without_actions();
            return (true, Vec::new());
        };
        let Some(settings) = self.action_cache.settings.clone().filter(|settings| {
            settings.diagnostic.is_none()
                && settings.revision == target.revision
                && !target.target_id.is_empty()
                && target.target.validate().is_ok()
        }) else {
            self.finish_target_lookup_without_actions();
            return (true, Vec::new());
        };
        target.actions.retain(|id| {
            let profile = settings.profiles.profile(*id);
            profile.enabled
                && profile.applies_to(&target.target)
                && self.advertises_endpoint_method(match profile.launch_mode {
                    LaunchMode::Pane => "pane.target.execute",
                    LaunchMode::External => "pane.target.authorize",
                })
        });
        if target.actions.is_empty() {
            self.finish_target_lookup_without_actions();
        } else if let Some(file) = self.target_menu_mut() {
            file.phase = TargetMenuPhase::Ready;
            file.highlight = validated_file_highlight(&target.highlight, context.inner_rect);
            file.target = Some(target);
            file.settings = Some(settings);
        }
        // Choose the popup anchor once, before its first visible frame. Keep the
        // marked spelling visible where space permits, without making a ready
        // menu jump when later output clears its decoration.
        if let Some(ClientShellOverlay::ContextMenu(menu)) = self.overlay.as_mut() {
            if let ClientContextMenuTarget::Pane {
                file: Some(file), ..
            } = &menu.target
            {
                if let Some((_, screen_height)) = file.context.composed_size {
                    let first = file
                        .highlight
                        .as_ref()
                        .and_then(|spans| spans.first())
                        .map_or(file.context.viewport_row, |span| span.row);
                    let last = file
                        .highlight
                        .as_ref()
                        .and_then(|spans| spans.last())
                        .map_or(file.context.viewport_row, |span| span.row);
                    let top = file.context.inner_rect.y.saturating_add(first);
                    let below = file
                        .context
                        .inner_rect
                        .y
                        .saturating_add(last)
                        .saturating_add(1);
                    let height = file
                        .target
                        .as_ref()
                        .map_or(0, |target| target.actions.len())
                        .saturating_add(2)
                        .min(usize::from(screen_height)) as u16;
                    if below.saturating_add(height) <= screen_height {
                        menu.y = below;
                    } else if top >= height {
                        menu.y = top - height;
                    }
                }
            }
        }
        self.hits.context_menu_rows.clear();
        (true, Vec::new())
    }

    pub(super) fn activate_file_context_action(
        &mut self,
        file: ClientFileMenu,
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        if file.phase != TargetMenuPhase::Ready
            || self.ready_file_context_is_current(&file.context) != Some(true)
        {
            return;
        }
        let id = match action {
            ClientContextMenuAction::OpenFile => ActionId::SystemOpener,
            ClientContextMenuAction::EditFile => ActionId::Editor,
            _ => return,
        };
        if !file
            .target
            .as_ref()
            .is_some_and(|target| target.actions.contains(&id))
        {
            return;
        }
        let mut file = file;
        file.highlight = None;
        file.phase = TargetMenuPhase::RefreshExecute(id);
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            x: file.context.inner_rect.x,
            y: file.context.inner_rect.y,
            highlighted: 0,
            target: ClientContextMenuTarget::Pane {
                pane_id: file.context.pane_id.clone(),
                workspace_id: String::new(),
                source_pane_id: None,
                has_manual_label: false,
                right_click_passthrough: false,
                file: Some(Box::new(file)),
            },
        }));
        self.hits.context_menu_rows.clear();
        if !self.refresh_action_settings(outcome) {
            self.close_target_menu();
        }
    }
}
