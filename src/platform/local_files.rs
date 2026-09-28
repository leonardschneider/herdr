//! Shared policy for explicit, local file actions. No shell evaluation or temporary-file cleanup.
use std::io;
use std::path::Path;

#[cfg(target_os = "macos")]
pub(crate) fn target_action_argv(
    target: &crate::file_url_actions::TargetValue,
    profile: &crate::file_url_actions::ActionProfile,
) -> io::Result<Vec<String>> {
    if profile.launch_mode != crate::file_url_actions::LaunchMode::External {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Action is not an external command",
        ));
    }
    let argv = profile
        .render_argv(target)
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
    if let crate::file_url_actions::TargetValue::File { path, .. } = target {
        validate_local_file_path(Path::new(path))?;
        // The runtime already checked the captured identity before issuing a
        // permit. Recheck immediately before spawn, without following a newly
        // substituted symlink. Arbitrary apps still open by pathname.
        if !std::fs::symlink_metadata(path)?.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Target is no longer a regular local file",
            ));
        }
    }
    Ok(argv)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn launch_target_action(
    target: &crate::file_url_actions::TargetValue,
    _profile: &crate::file_url_actions::ActionProfile,
) -> io::Result<Option<std::process::Child>> {
    if let crate::file_url_actions::TargetValue::File { path, .. } = target {
        return open_local_file(Path::new(path));
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Local target actions are only supported on macOS",
    ))
}

#[cfg(unix)]
pub(crate) const LOCAL_FILE_SAMPLE_BYTES: u64 = 8192;

pub(crate) fn local_file_actions_supported() -> bool {
    cfg!(target_os = "macos")
}

pub(crate) fn validate_local_file_path(path: &Path) -> io::Result<&str> {
    let text = path.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "file path is not valid UTF-8")
    })?;
    if !path.is_absolute() || text.chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file actions require an absolute path without control characters",
        ));
    }
    Ok(text)
}

pub(crate) fn local_file_editor_argv(
    path: &Path,
    line: Option<u32>,
    column: Option<u32>,
) -> io::Result<Vec<String>> {
    let path = validate_local_file_path(path)?;
    if line.is_some_and(|value| value == 0 || value > i32::MAX as u32)
        || column.is_some_and(|value| value == 0 || value > i32::MAX as u32)
        || (column.is_some() && line.is_none())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file cursor positions must be positive 1-based integers",
        ));
    }
    let mut argv = vec!["nvim".to_owned()];
    if let Some(line) = line {
        argv.extend([
            "-c".to_owned(),
            format!("call cursor({line},{})", column.unwrap_or(1)),
        ]);
    }
    argv.extend(["--".to_owned(), path.to_owned()]);
    Ok(argv)
}

#[cfg(not(unix))]
pub(crate) fn sample_local_regular_file(_path: &Path) -> io::Result<(std::fs::Metadata, Vec<u8>)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "local file actions are not supported on this platform",
    ))
}

#[cfg(not(unix))]
pub(crate) fn same_local_file(_a: &std::fs::Metadata, _b: &std::fs::Metadata) -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn open_local_file(_path: &Path) -> io::Result<Option<std::process::Child>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Open in macOS is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_file_editor_argv_treats_path_as_one_literal_argument() {
        let path = std::env::temp_dir().join("space ' quote $(touch should-not-exist).txt");
        let args = local_file_editor_argv(&path, Some(42), Some(7)).expect("valid path");
        assert_eq!(args.len(), 5);
        assert_eq!(&args[..4], ["nvim", "-c", "call cursor(42,7)", "--"]);
        assert_eq!(args[4], path.to_string_lossy());
        assert!(!args.iter().any(|arg| arg == "sh" || arg == "rm"));
    }

    #[test]
    fn local_file_editor_argv_validates_paths_and_cursor_positions() {
        let path = std::env::temp_dir().join("example.txt");
        assert_eq!(
            local_file_editor_argv(&path, None, None)
                .expect("path")
                .len(),
            3
        );
        assert!(local_file_editor_argv(Path::new("relative.txt"), None, None).is_err());
        assert!(local_file_editor_argv(&path, Some(0), None).is_err());
        assert!(local_file_editor_argv(&path, None, Some(2)).is_err());
        assert!(local_file_editor_argv(&path, Some(u32::MAX), None).is_err());
        assert!(local_file_editor_argv(&path.with_file_name("bad\nname"), None, None).is_err());
        assert_eq!(
            local_file_editor_argv(&path, Some(3), None).expect("line")[2],
            "call cursor(3,1)"
        );
    }
}
