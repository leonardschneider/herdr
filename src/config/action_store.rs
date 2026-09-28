//! Runtime-authoritative, JSON-only executable policy. No client/wire state.
use crate::file_url_actions::{ActionProfiles, ActionRevision, ActionSettings};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;
static INCARNATION: AtomicU64 = AtomicU64::new(0);

pub(crate) struct ActionStore {
    path: PathBuf,
    settings: ActionSettings,
    // Exact bytes distinguish absence from an empty file and detect external edits.
    disk: Option<Vec<u8>>,
    initialized: bool,
}

impl ActionStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self {
            path,
            settings: ActionSettings {
                revision: ActionRevision {
                    boot_id: format!(
                        "actions-{}-{nanos}-{}",
                        std::process::id(),
                        INCARNATION.fetch_add(1, Ordering::Relaxed)
                    ),
                    counter: 0,
                },
                profiles: ActionProfiles::default(),
                diagnostic: None,
            },
            disk: None,
            initialized: false,
        }
    }

    pub(crate) fn settings(&mut self) -> ActionSettings {
        if !self.initialized {
            self.reload();
        }
        self.settings.clone()
    }

    pub(crate) fn reload(&mut self) {
        self.initialized = true;
        let result = read_disk(&self.path).and_then(|disk| {
            self.disk = disk;
            parse_profiles(self.disk.as_deref())
        });
        match result {
            Ok(profiles) => {
                self.settings.profiles = profiles;
                self.settings.diagnostic = None;
            }
            Err(error) => self.settings.diagnostic = Some(error),
        }
        self.advance();
    }

    fn advance(&mut self) {
        // No wrapping: a new runtime incarnation is required if exhausted.
        if let Some(counter) = self.settings.revision.counter.checked_add(1) {
            self.settings.revision.counter = counter;
        } else {
            self.settings.diagnostic = Some("action revision exhausted; restart runtime".into());
        }
    }

    pub(crate) fn check(
        &mut self,
        expected: &ActionRevision,
    ) -> Result<ActionSettings, (&'static str, String)> {
        let settings = self.settings();
        if &settings.revision != expected {
            return Err((
                "settings_conflict",
                "action settings changed; reload and click again".into(),
            ));
        }
        if let Some(error) = &settings.diagnostic {
            return Err(("invalid_action_settings", error.clone()));
        }
        Ok(settings)
    }

    pub(crate) fn save(
        &mut self,
        expected: &ActionRevision,
        profiles: ActionProfiles,
    ) -> Result<ActionSettings, (&'static str, String)> {
        // Invalid loaded policy can be repaired, but never executed.
        let current = self.settings();
        if &current.revision != expected {
            return Err((
                "settings_conflict",
                "action settings changed; reload before saving".into(),
            ));
        }
        profiles
            .validate()
            .map_err(|e| ("invalid_action_settings", e))?;
        let disk = read_disk(&self.path).map_err(|e| ("config_write_failed", e))?;
        if disk != self.disk {
            return Err((
                "settings_conflict",
                "configuration changed on disk; reload before saving".into(),
            ));
        }
        let content = std::str::from_utf8(disk.as_deref().unwrap_or_default())
            .map_err(|e| ("config_write_failed", e.to_string()))?;
        let updated =
            replace_section(content, &profiles).map_err(|e| ("config_write_failed", e))?;
        atomic_replace(&self.path, &disk, updated.as_bytes())
            .map_err(|e| ("config_write_failed", e))?;
        self.disk = Some(updated.into_bytes());
        self.settings.profiles = profiles;
        self.settings.diagnostic = None;
        self.advance();
        Ok(self.settings.clone())
    }
}

fn read_disk(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::metadata(path) {
        Ok(meta) if !meta.is_file() || meta.len() > MAX_CONFIG_BYTES => {
            return Err("configuration must be a regular file smaller than 4 MiB".into())
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot inspect configuration: {e}")),
    }
    fs::read(path)
        .map(Some)
        .map_err(|e| format!("cannot read configuration: {e}"))
}

fn parse_profiles(disk: Option<&[u8]>) -> Result<ActionProfiles, String> {
    let text = std::str::from_utf8(disk.unwrap_or_default()).map_err(|e| e.to_string())?;
    let value: toml::Value = text
        .trim_start_matches('\u{feff}')
        .parse()
        .map_err(|e| format!("invalid configuration: {e}"))?;
    let profiles: ActionProfiles = match value.get("file_url_actions") {
        Some(value) => value
            .clone()
            .try_into()
            .map_err(|e| format!("invalid file/URL actions: {e}"))?,
        None => ActionProfiles::default(),
    };
    profiles.validate()?;
    Ok(profiles)
}

fn replace_section(content: &str, profiles: &ActionProfiles) -> Result<String, String> {
    let content = content.trim_start_matches('\u{feff}');
    let mut expected: toml::Value = content
        .parse()
        .map_err(|e| format!("repair malformed TOML before saving: {e}"))?;
    let value = toml::Value::try_from(profiles).map_err(|e| e.to_string())?;
    expected
        .as_table_mut()
        .ok_or("configuration is not a table")?
        .insert("file_url_actions".into(), value.clone());
    let mut output = String::new();
    let mut in_actions = false;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if let Some(end) = trimmed.find(']') {
                let header = trimmed[1..end].trim();
                in_actions =
                    header == "file_url_actions" || header.starts_with("file_url_actions.");
            }
        }
        if !in_actions {
            output.push_str(line);
        }
    }
    if !output.ends_with('\n') {
        output.push('\n');
    }
    let mut section = toml::map::Map::new();
    section.insert("file_url_actions".into(), value);
    output.push_str(&toml::to_string(&toml::Value::Table(section)).map_err(|e| e.to_string())?);
    // Fail closed for exotic quoted/dotted/inline layouts or multiline strings
    // that our conservative section splicer cannot preserve exactly.
    let actual: toml::Value = output.parse().map_err(|_| {
        "cannot safely update action section; use ordinary [file_url_actions.editor] tables"
            .to_string()
    })?;
    if actual != expected {
        return Err("cannot update action section without changing unrelated configuration".into());
    }
    Ok(output)
}

fn atomic_replace(path: &Path, expected: &Option<Vec<u8>>, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("refusing to replace a symlink configuration; use its actual path".into());
    }
    let temp = parent.join(format!(
        ".herdr-actions-{}-{}.tmp",
        std::process::id(),
        INCARNATION.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        if let Ok(metadata) = fs::metadata(path) {
            fs::set_permissions(&temp, metadata.permissions()).map_err(|e| e.to_string())?;
        }
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        if &read_disk(path)? != expected {
            return Err("configuration changed during save; reload before retrying".into());
        }
        // Atomic replacement; not a lock against arbitrary noncooperating editors.
        fs::rename(&temp, path).map_err(|e| format!("cannot atomically replace configuration: {e}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "herdr-actions-test-{}-{}",
                std::process::id(),
                INCARNATION.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn store(&self) -> ActionStore {
            ActionStore::new(self.0.join("config.toml"))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn save_cas_preserves_unrelated_bytes_and_rejects_external_changes() {
        let f = Fixture::new();
        let mut store = f.store();
        fs::write(&store.path, "# retained comment\n[theme]\nname = 'dark'\n").unwrap();
        let initial = store.settings();
        let saved = store
            .save(&initial.revision, initial.profiles.clone())
            .unwrap();
        assert!(fs::read_to_string(&store.path)
            .unwrap()
            .starts_with("# retained comment\n[theme]\nname = 'dark'\n"));
        assert_ne!(saved.revision, initial.revision);
        assert_eq!(
            store
                .save(&initial.revision, initial.profiles.clone())
                .unwrap_err()
                .0,
            "settings_conflict"
        );
        fs::write(&store.path, "[theme]\nname = 'light'\n").unwrap();
        assert_eq!(
            store.save(&saved.revision, initial.profiles).unwrap_err().0,
            "settings_conflict"
        );
        store.reload();
        assert_ne!(store.settings().revision, saved.revision);
    }
    #[test]
    fn malformed_policy_disables_actions_and_valid_save_repairs_section() {
        let f = Fixture::new();
        let mut store = f.store();
        fs::write(&store.path, "[file_url_actions.editor]\nexecutable = 42\n").unwrap();
        let initial = store.settings();
        assert!(initial.diagnostic.is_some());
        assert_eq!(
            store.check(&initial.revision).unwrap_err().0,
            "invalid_action_settings"
        );
        let fixed = store
            .save(&initial.revision, ActionProfiles::default())
            .unwrap();
        assert!(fixed.diagnostic.is_none());
    }
    #[test]
    fn write_failure_does_not_acknowledge_or_change_revision() {
        let f = Fixture::new();
        let mut store = f.store();
        let initial = store.settings();
        fs::create_dir(&store.path).unwrap();
        assert!(store.save(&initial.revision, initial.profiles).is_err());
        assert_eq!(store.settings().revision, initial.revision);
    }
    #[test]
    fn malformed_toml_never_gets_overwritten() {
        let f = Fixture::new();
        let mut store = f.store();
        fs::write(&store.path, "[[").unwrap();
        let initial = store.settings();
        assert!(initial.diagnostic.is_some());
        assert!(store
            .save(&initial.revision, ActionProfiles::default())
            .is_err());
        assert_eq!(fs::read_to_string(&store.path).unwrap(), "[[");
    }
}
