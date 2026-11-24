//! Recognises harness-generated "user" records (task notifications, slash
//! command echoes, shell escapes) so they are not shown as the user's words.

/// How a user-role text record should be presented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserText {
    /// Something the user typed, possibly rewritten (`/cmd args`, `! cmd`).
    Prompt(String),
    /// Injected by the harness; shown as a dim event line.
    Event(String),
}

/// Returns `None` for ordinary prompts that should be kept as they are.
pub fn classify(text: &str) -> Option<UserText> {
    let t = text.trim_start();
    if t.starts_with("<task-notification>") {
        let summary = tag(t, "summary").map(str::trim).filter(|s| !s.is_empty());
        let status = tag(t, "status").map(str::trim).unwrap_or("update");
        return Some(UserText::Event(match summary {
            Some(s) => format!("task {status}: {s}"),
            None => format!("task {status}"),
        }));
    }
    if let Some(name) = tag(t, "command-name").filter(|_| t.starts_with("<command-")) {
        let args = tag(t, "command-args").map(str::trim).unwrap_or("");
        return Some(UserText::Prompt(
            format!("{} {args}", name.trim()).trim_end().to_string(),
        ));
    }
    if let Some(cmd) = tag(t, "bash-input").filter(|_| t.starts_with("<bash-input>")) {
        return Some(UserText::Prompt(format!("! {}", cmd.trim())));
    }
    if t.starts_with("<bash-stdout>") || t.starts_with("<bash-stderr>") {
        let out = [tag(t, "bash-stdout"), tag(t, "bash-stderr")]
            .into_iter()
            .flatten()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        return Some(UserText::Event(labelled("shell output", &out)));
    }
    if let Some(out) = tag(t, "local-command-stdout").filter(|_| t.starts_with("<local-command-")) {
        return Some(UserText::Event(labelled("command output", out.trim())));
    }
    if t.starts_with("[Request interrupted by user") {
        return Some(UserText::Event("interrupted by user".into()));
    }
    if t.starts_with("<system-reminder>") && strip_tag(t, "system-reminder").trim().is_empty() {
        return Some(UserText::Event("system reminder".into()));
    }
    None
}

fn labelled(label: &str, body: &str) -> String {
    if body.is_empty() {
        label.to_string()
    } else {
        format!("{label}: {body}")
    }
}

/// Inner text of the first `<name>…</name>`.
fn tag<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = s.find(&open)? + open.len();
    let end = s[start..].find(&close).map_or(s.len(), |e| start + e);
    Some(&s[start..end])
}

/// `s` with every `<name>…</name>` block removed.
fn strip_tag(s: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find(&open) {
        out.push_str(&rest[..i]);
        match rest[i..].find(&close) {
            Some(j) => rest = &rest[i + j + close.len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_notification_becomes_event() {
        let t = "<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>\n\
                 <summary>Background command \"Wait for CI\" completed (exit code 0)</summary>\n</task-notification>";
        assert_eq!(
            classify(t),
            Some(UserText::Event(
                "task completed: Background command \"Wait for CI\" completed (exit code 0)".into()
            ))
        );
    }

    #[test]
    fn slash_and_bang_commands_stay_prompts() {
        let t = "<command-name>/compact</command-name>\n  <command-message>compact</command-message>\n  <command-args></command-args>";
        assert_eq!(classify(t), Some(UserText::Prompt("/compact".into())));
        let t = "<command-message>review</command-message>\n<command-name>/review-pr</command-name>\n<command-args>12</command-args>";
        assert_eq!(classify(t), Some(UserText::Prompt("/review-pr 12".into())));
        assert_eq!(
            classify("<bash-input>ls -la</bash-input>"),
            Some(UserText::Prompt("! ls -la".into()))
        );
    }

    #[test]
    fn outputs_and_interruptions_become_events() {
        assert_eq!(
            classify("<bash-stdout>ok</bash-stdout><bash-stderr></bash-stderr>"),
            Some(UserText::Event("shell output: ok".into()))
        );
        assert_eq!(
            classify("<local-command-stdout>Compacted</local-command-stdout>"),
            Some(UserText::Event("command output: Compacted".into()))
        );
        assert_eq!(
            classify("[Request interrupted by user for tool use]"),
            Some(UserText::Event("interrupted by user".into()))
        );
        assert_eq!(
            classify("<system-reminder>x</system-reminder>\n"),
            Some(UserText::Event("system reminder".into()))
        );
    }

    #[test]
    fn ordinary_prompts_are_untouched() {
        assert_eq!(classify("Fix the <div> layout"), None);
        assert_eq!(
            classify("<system-reminder>x</system-reminder> real question"),
            None
        );
        assert_eq!(classify("[Image #1] what is this?"), None);
    }
}
