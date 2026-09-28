use super::*;
use crate::file_url_actions::ActionProfiles;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
fn target_highlight_metadata_is_optional_for_older_json_responses() {
    let old = serde_json::json!({
        "target_id": "old-target", "target": {"kind":"url", "url":"https://example.com"},
        "revision": {"boot_id":"opaque-policy", "counter":1}, "actions":["system_opener"]
    });
    let target: ResolvedTarget = serde_json::from_value(old.clone()).unwrap();
    assert!(target.highlight.is_empty());
    assert_eq!(serde_json::to_value(&target).unwrap(), old);
}

#[test]
fn highlight_uses_the_winning_file_candidate_not_the_canonical_destination() {
    let f = Fixture::new();
    std::fs::write(f.0.join("found.rs"), b"text\n").unwrap();
    let text = "(found.rs),";
    let candidates = crate::file_url_actions::file_candidates_at(text, 2);
    let (target, _, index) = resolve_file(&candidates, &f.0).unwrap();
    assert!(target.display_target().ends_with("/found.rs"));
    assert_eq!(candidates[index], "found.rs");
    assert_eq!(
        highlight::source_spans(text, 2, &candidates[index], 80, 1),
        vec![crate::file_url_actions::TargetCellSpan {
            row: 0,
            start_col: 1,
            end_col: 9
        }]
    );
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "herdr-target-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn app(&self) -> App {
        let (_, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            rx,
            crate::api::EventHub::default(),
        );
        app.action_store = crate::config::ActionStore::new(self.0.join("config.toml"));
        app
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn entry(client_id: u64) -> CachedActionTarget {
    CachedActionTarget {
        token: String::new(),
        client_id,
        pane_id: "pane-1".into(),
        terminal_id: crate::terminal::TerminalId::alloc(),
        child_pid: 42,
        revision: ActionRevision {
            boot_id: "test-boot".into(),
            counter: 1,
        },
        target: TargetValue::Url {
            url: "https://example.com/path?q=1#anchor".into(),
        },
        metadata: None,
        cwd: std::env::temp_dir(),
        created: Instant::now(),
    }
}
fn scope(client_id: u64) -> ActionRequestContext {
    ActionRequestContext { client_id }
}
fn error(response: String) -> String {
    serde_json::from_str::<crate::api::schema::ErrorResponse>(&response)
        .unwrap()
        .error
        .code
}

#[test]
fn cached_target_is_client_and_pane_scoped_and_consumed_once() {
    let mut cache = ActionTargetCache::default();
    let token = cache.insert(entry(7)).unwrap();
    assert!(cache.take(scope(8), "pane-1", &token).is_none());
    assert!(cache.take(scope(7), "pane-2", &token).is_none());
    assert!(cache.take(scope(7), "pane-1", &token).is_some());
    assert!(cache.take(scope(7), "pane-1", &token).is_none());
}

#[test]
fn cache_is_bounded_expiring_and_never_wraps_token_sequence() {
    let mut cache = ActionTargetCache::default();
    let first = cache.insert(entry(7)).unwrap();
    for _ in 0..TARGET_CAPACITY {
        cache.insert(entry(7)).unwrap();
    }
    assert_eq!(cache.entries.len(), TARGET_CAPACITY);
    assert!(cache.take(scope(7), "pane-1", &first).is_none());
    let expired = cache.insert(entry(7)).unwrap();
    cache.entries.back_mut().unwrap().created =
        Instant::now() - TARGET_TTL - Duration::from_secs(1);
    assert!(cache.take(scope(7), "pane-1", &expired).is_none());
    cache.next = u64::MAX;
    assert!(cache.insert(entry(7)).is_none());
}

#[test]
fn public_request_id_cannot_forge_attached_client_scope() {
    let f = Fixture::new();
    let mut app = f.app();
    let params = PaneTargetActionParams {
        pane_id: "pane-1".into(),
        target_id: "token".into(),
        action_id: ActionId::SystemOpener,
        revision: entry(7).revision,
    };
    for method in [
        Method::PaneTargetAuthorize(params.clone()),
        Method::PaneTargetExecute(params),
    ] {
        assert_eq!(
            error(app.handle_api_request(Request {
                id: "endpoint:boot:7:forged".into(),
                method
            })),
            "client_scope_required"
        );
    }
    assert!(!f.0.join("config.toml").exists());
}

#[test]
fn stale_revision_consumes_token_and_duplicate_is_rejected() {
    let f = Fixture::new();
    let mut app = f.app();
    let settings = app.action_store.settings();
    let token = app.action_targets.insert(entry(7)).unwrap();
    let params = PaneTargetActionParams {
        pane_id: "pane-1".into(),
        target_id: token,
        action_id: ActionId::SystemOpener,
        revision: entry(7).revision,
    };
    let request = Request {
        id: "attempt".into(),
        method: Method::PaneTargetAuthorize(params),
    };
    assert_ne!(settings.revision, entry(7).revision);
    assert_eq!(
        error(app.handle_scoped_action_request(request.clone(), Some(scope(7)))),
        "settings_conflict"
    );
    assert_eq!(
        error(app.handle_scoped_action_request(request, Some(scope(7)))),
        "stale_target"
    );
}

#[test]
fn wrong_client_cannot_consume_and_wrong_launch_mode_is_not_a_fallback() {
    let f = Fixture::new();
    let mut app = f.app();
    let settings = app.action_store.settings();
    let mut cached = entry(7);
    cached.revision = settings.revision.clone();
    let token = app.action_targets.insert(cached).unwrap();
    let request = Request {
        id: "request".into(),
        method: Method::PaneTargetExecute(PaneTargetActionParams {
            pane_id: "pane-1".into(),
            target_id: token,
            action_id: ActionId::SystemOpener,
            revision: settings.revision,
        }),
    };
    assert_eq!(
        error(app.handle_scoped_action_request(request.clone(), Some(scope(8)))),
        "stale_target"
    );
    assert_eq!(app.action_targets.entries.len(), 1);
    assert_eq!(
        error(app.handle_scoped_action_request(request.clone(), Some(scope(7)))),
        "action_not_applicable"
    );
    assert_eq!(
        error(app.handle_scoped_action_request(request, Some(scope(7)))),
        "stale_target"
    );
}

#[test]
fn two_clients_save_with_cas_and_success_invalidates_pending_targets() {
    let f = Fixture::new();
    let mut app = f.app();
    let first = app.action_store.settings();
    app.action_targets.insert(entry(7)).unwrap();
    let mut profiles = first.profiles.clone();
    profiles.editor.enabled = false;
    let request = Request {
        id: "save-a".into(),
        method: Method::FileUrlActionsSet(FileUrlActionsSetParams {
            expected_revision: first.revision.clone(),
            profiles,
        }),
    };
    let response = app.handle_scoped_action_request(request, Some(scope(7)));
    assert!(serde_json::from_str::<crate::api::schema::SuccessResponse>(&response).is_ok());
    assert!(app.action_targets.entries.is_empty());
    let request = Request {
        id: "save-b".into(),
        method: Method::FileUrlActionsSet(FileUrlActionsSetParams {
            expected_revision: first.revision,
            profiles: first.profiles,
        }),
    };
    assert_eq!(
        error(app.handle_scoped_action_request(request, Some(scope(8)))),
        "settings_conflict"
    );
    assert!(!app.action_store.settings().profiles.editor.enabled);
}

#[test]
fn captured_permit_policy_does_not_change_after_save() {
    let f = Fixture::new();
    let mut app = f.app();
    let settings = app.action_store.settings();
    let permit = ActionPermit {
        permit_id: "captured-permit".into(),
        action_id: ActionId::SystemOpener,
        revision: settings.revision.clone(),
        target: entry(7).target,
        profile: settings.profiles.system_opener.clone(),
    };
    let mut changed = settings.profiles;
    changed.system_opener.executable = "other-program".into();
    let saved = app.action_store.save(&permit.revision, changed).unwrap();
    assert_ne!(saved.revision, permit.revision);
    assert_eq!(
        permit.profile.executable,
        ActionProfiles::default().system_opener.executable
    );
}

#[cfg(unix)]
#[test]
fn cached_file_rejects_replacement_and_directories() {
    let f = Fixture::new();
    let path = f.0.join("owned.txt");
    std::fs::write(&path, b"hello\n").unwrap();
    let mut cached = entry(7);
    cached.target = TargetValue::File {
        path: path.to_str().unwrap().into(),
        line: Some(1),
        column: Some(1),
        file_kind: TargetFileKind::Text,
    };
    cached.metadata = Some(std::fs::metadata(&path).unwrap());
    assert!(validate_file_identity(&cached).is_ok());
    let replacement = f.0.join("replacement");
    std::fs::write(&replacement, b"hello\n").unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    assert!(validate_file_identity(&cached).is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(validate_file_identity(&cached).is_err());
}
