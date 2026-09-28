use super::*;

const MAX_ARGUMENTS: usize = 64;
const MAX_ARGUMENT_BYTES: usize = 8192;
const MAX_COMMAND_BYTES: usize = 32768;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Placeholder {
    Target,
    Path,
    Url,
    Line,
    Column,
}

enum Part {
    Literal(String),
    Value(Placeholder),
}

/// Parse a template once. Substituted values are never parsed a second time.
fn parts(template: &str) -> Result<Vec<Part>, String> {
    let mut result = Vec::new();
    let mut literal = String::new();
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                literal.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                literal.push('}');
            }
            '{' => {
                if !literal.is_empty() {
                    result.push(Part::Literal(std::mem::take(&mut literal)));
                }
                let mut name = String::new();
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == '}' {
                        closed = true;
                        break;
                    }
                    name.push(next);
                }
                if !closed {
                    return Err("Unclosed argument placeholder".into());
                }
                let value = match name.as_str() {
                    "target" => Placeholder::Target,
                    "path" => Placeholder::Path,
                    "url" => Placeholder::Url,
                    "line" => Placeholder::Line,
                    "column" => Placeholder::Column,
                    _ => return Err(format!("Unknown argument placeholder: {{{name}}}")),
                };
                result.push(Part::Value(value));
            }
            '}' => return Err("Unmatched closing brace; use }} for a literal brace".into()),
            _ => literal.push(ch),
        }
    }
    if !literal.is_empty() {
        result.push(Part::Literal(literal));
    }
    Ok(result)
}

fn carries_target(parts: &[Part]) -> bool {
    parts.iter().any(|p| {
        matches!(
            p,
            Part::Value(Placeholder::Target | Placeholder::Path | Placeholder::Url)
        )
    })
}

/// A guardrail for direct substitutions into common code positions, not a
/// sandbox or a detector for arbitrary interpreters/user-provided wrappers.
fn code_position(program: &str, arguments: &[String], index: usize) -> bool {
    let program = std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    let arg = arguments[index].as_str();
    let previous = index.checked_sub(1).map(|i| arguments[i].as_str());
    if arguments[..index].iter().any(|arg| arg == "--") {
        return false;
    }
    let flag = |candidate: &str| -> bool {
        match program.as_str() {
            "sh" | "bash" | "dash" | "zsh" | "ksh" | "fish" => {
                candidate.starts_with('-')
                    && !candidate.starts_with("--")
                    && candidate[1..].contains('c')
            }
            "node" | "nodejs" => matches!(candidate, "-e" | "--eval" | "-p" | "--print"),
            "osascript" => candidate == "-e",
            "nvim" | "vim" | "vi" | "ex" => matches!(candidate, "-c" | "--cmd" | "+"),
            "pwsh" | "powershell" => {
                candidate.eq_ignore_ascii_case("-command") || candidate.eq_ignore_ascii_case("-c")
            }
            "cmd" | "cmd.exe" => {
                candidate.eq_ignore_ascii_case("/c") || candidate.eq_ignore_ascii_case("/k")
            }
            _ if program.starts_with("python") => candidate == "-c",
            _ => false,
        }
    };
    if previous.is_some_and(flag) {
        return true;
    }
    // Flag and code can share one argv element. Stop at -- for data operands.
    match program.as_str() {
        "node" | "nodejs" => {
            arg.starts_with("--eval=")
                || arg.starts_with("--print=")
                || (arg.len() > 2 && (arg.starts_with("-e") || arg.starts_with("-p")))
        }
        "nvim" | "vim" | "vi" | "ex" => {
            arg.starts_with('+')
                || arg.starts_with("--cmd=")
                || (arg.len() > 2 && arg.starts_with("-c"))
        }
        "osascript" => arg.len() > 2 && arg.starts_with("-e"),
        "sh" | "bash" | "dash" | "zsh" | "ksh" | "fish" => arg.len() > 2 && arg.starts_with("-c"),
        _ if program.starts_with("python") => arg.len() > 2 && arg.starts_with("-c"),
        _ => false,
    }
}

impl ActionProfiles {
    pub fn validate(&self) -> Result<(), String> {
        self.editor.validate().map_err(|e| format!("Editor: {e}"))?;
        self.system_opener
            .validate()
            .map_err(|e| format!("System opener: {e}"))
    }

    pub fn profile(&self, action: ActionId) -> &ActionProfile {
        match action {
            ActionId::Editor => &self.editor,
            ActionId::SystemOpener => &self.system_opener,
        }
    }
}

impl ActionProfile {
    pub fn validate(&self) -> Result<(), String> {
        if self.executable.is_empty()
            || self.executable.len() > 4096
            || self.executable.chars().any(char::is_control)
            || self.executable.contains(['{', '}'])
        {
            return Err(
                "Executable must be a fixed program name/path, without controls or placeholders"
                    .into(),
            );
        }
        if self.arguments.len() > MAX_ARGUMENTS {
            return Err("At most 64 arguments are supported".into());
        }
        let mut total = self.executable.len();
        let mut has_target = false;
        for (index, argument) in self.arguments.iter().enumerate() {
            if argument.len() > MAX_ARGUMENT_BYTES || argument.chars().any(char::is_control) {
                return Err(
                    "Arguments must be at most 8192 bytes and contain no control characters".into(),
                );
            }
            total += argument.len();
            let parsed = parts(argument)?;
            if carries_target(&parsed) {
                has_target = true;
                if code_position(&self.executable, &self.arguments, index) {
                    return Err("Target text cannot be substituted into an interpreter's code argument; pass it as positional data instead".into());
                }
            }
            for part in &parsed {
                match part {
                    Part::Value(Placeholder::Path | Placeholder::Line | Placeholder::Column)
                        if self.applies_to.urls =>
                    {
                        return Err(
                            "File-only placeholders cannot be used when URLs are enabled".into(),
                        );
                    }
                    Part::Value(Placeholder::Url)
                        if self.applies_to.text_files || self.applies_to.other_files =>
                    {
                        return Err("{url} cannot be used for file actions; use {target}".into());
                    }
                    _ => {}
                }
            }
        }
        if total > MAX_COMMAND_BYTES {
            return Err("Command exceeds the 32768-byte limit".into());
        }
        if !has_target {
            return Err("Arguments must include {target}, {path}, or {url}".into());
        }
        Ok(())
    }

    pub fn applies_to(&self, target: &TargetValue) -> bool {
        self.enabled
            && match target {
                TargetValue::File {
                    file_kind: TargetFileKind::Text,
                    ..
                } => self.applies_to.text_files,
                TargetValue::File { .. } => self.applies_to.other_files,
                TargetValue::Url { .. } => self.applies_to.urls,
            }
    }

    pub fn render_argv(&self, target: &TargetValue) -> Result<Vec<String>, String> {
        self.validate()?;
        target.validate()?;
        if !self.applies_to(target) {
            return Err("This action is not enabled for the target".into());
        }
        let mut argv = vec![self.executable.clone()];
        let mut total = self.executable.len();
        for argument in &self.arguments {
            let mut rendered = String::new();
            for part in parts(argument)? {
                match part {
                    Part::Literal(value) => rendered.push_str(&value),
                    Part::Value(Placeholder::Target) => rendered.push_str(target.display_target()),
                    Part::Value(value) => match (value, target) {
                        (Placeholder::Path, TargetValue::File { path, .. }) => {
                            rendered.push_str(path)
                        }
                        (Placeholder::Url, TargetValue::Url { url }) => rendered.push_str(url),
                        (Placeholder::Line, TargetValue::File { line, .. }) => {
                            rendered.push_str(&line.unwrap_or(1).to_string())
                        }
                        (Placeholder::Column, TargetValue::File { column, .. }) => {
                            rendered.push_str(&column.unwrap_or(1).to_string())
                        }
                        _ => return Err("Placeholder is not available for this target".into()),
                    },
                }
            }
            total += rendered.len();
            if rendered.len() > MAX_ARGUMENT_BYTES || total > MAX_COMMAND_BYTES {
                return Err("Expanded command exceeds the size limit".into());
            }
            argv.push(rendered);
        }
        Ok(argv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> TargetValue {
        TargetValue::File {
            path: std::env::temp_dir()
                .join("space ' $(touch BAD);{url}.txt")
                .to_string_lossy()
                .into_owned(),
            line: Some(42),
            column: Some(7),
            file_kind: TargetFileKind::Text,
        }
    }

    #[test]
    fn defaults_and_literal_expansion_preserve_original_target() {
        let profiles = ActionProfiles::default();
        profiles.validate().unwrap();
        let target = target();
        assert_eq!(
            profiles.editor.render_argv(&target).unwrap(),
            vec![
                "nvim",
                "-c",
                "call cursor(42,7)",
                "--",
                target.display_target()
            ]
        );
        let url = TargetValue::Url {
            url: "https://example.com/a?q=$(touch%20BAD)&b=x;y#part".into(),
        };
        assert_eq!(
            profiles.system_opener.render_argv(&url).unwrap(),
            vec!["open", "--", url.display_target()]
        );
        assert!(profiles.editor.render_argv(&url).is_err());
        assert_eq!(
            serde_json::from_str::<ActionProfiles>("{}").unwrap(),
            profiles
        );
    }

    #[test]
    fn direct_interpreter_code_substitution_is_rejected_but_positional_data_is_valid() {
        for (program, flag) in [
            ("sh", "-c"),
            ("bash", "-lc"),
            ("python3.14", "-c"),
            ("node", "--eval"),
            ("osascript", "-e"),
            ("nvim", "--cmd"),
        ] {
            let mut profile = ActionProfiles::default().system_opener;
            profile.executable = program.into();
            profile.arguments = vec![flag.into(), "prefix {target}".into()];
            assert!(profile.validate().is_err(), "{program}");
        }
        for (program, argument) in [
            ("node", "--eval={target}"),
            ("python3", "-c{target}"),
            ("nvim", "+{target}"),
        ] {
            let mut profile = ActionProfiles::default().system_opener;
            profile.executable = program.into();
            profile.arguments = vec![argument.into()];
            assert!(profile.validate().is_err(), "{program}");
        }
        let mut profile = ActionProfiles::default().system_opener;
        profile.executable = "sh".into();
        profile.arguments = vec![
            "-c".into(),
            "printf '%s' \"$1\"".into(),
            "fixture".into(),
            "{target}".into(),
        ];
        assert!(profile.validate().is_ok());
        assert_eq!(
            profile.render_argv(&target()).unwrap().last().unwrap(),
            target().display_target()
        );
    }

    #[test]
    fn incompatible_unknown_and_oversized_templates_are_rejected() {
        let mut profile = ActionProfiles::default().system_opener;
        for template in [
            "{path}",
            "{line}",
            "{column}",
            "{unknown}",
            "{target",
            "{target}}",
            "{target}\n",
        ] {
            profile.arguments = vec![template.into()];
            assert!(profile.validate().is_err(), "{template}");
        }
        profile.arguments = vec!["{url}".into()];
        assert!(profile.validate().is_err());
        profile.arguments = vec!["{target}".into(); 65];
        assert!(profile.validate().is_err());
        profile.arguments = vec!["{target}".repeat(4000)];
        assert!(profile.validate().is_err());
    }

    #[test]
    fn editor_embedded_location_and_braces_are_literal() {
        let mut profile = ActionProfiles::default().editor;
        profile.executable = "code".into();
        profile.arguments = vec![
            "--goto".into(),
            "{path}:{line}:{column}".into(),
            "{{literal}}".into(),
        ];
        let value = target();
        assert_eq!(
            profile.render_argv(&value).unwrap(),
            vec![
                "code".to_owned(),
                "--goto".into(),
                format!("{}:42:7", value.display_target()),
                "{literal}".into()
            ]
        );
        profile.enabled = false;
        assert!(profile.render_argv(&value).is_err());
    }

    #[test]
    fn invalid_target_positions_and_controls_fail_closed() {
        let profile = ActionProfiles::default().editor;
        for (line, column) in [
            (Some(0), None),
            (Some(u32::MAX), None),
            (None, Some(2)),
            (Some(1), Some(0)),
        ] {
            let TargetValue::File {
                path, file_kind, ..
            } = target()
            else {
                panic!()
            };
            assert!(profile
                .render_argv(&TargetValue::File {
                    path,
                    file_kind,
                    line,
                    column
                })
                .is_err());
        }
        let mut value = target();
        if let TargetValue::File { path, .. } = &mut value {
            path.push('\n');
        }
        assert!(profile.render_argv(&value).is_err());
    }
}
