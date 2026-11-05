//! User settings, persisted as JSON in the local database (`kv['settings']`).
//! Every field here has a real effect somewhere in the app.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // General
    pub notifications: String,
    pub in_app_toasts: bool,
    pub working_section: bool,
    pub sidebar_density: String,
    pub project_order: String,
    pub archive_confirm: bool,
    pub delete_confirm: bool,
    pub time_format: String,
    // Appearance
    pub theme: String,
    pub theme_name: String,
    pub contrast: u32,
    pub chat_width: String,
    pub effort_display: String,
    pub ui_font: String,
    pub ui_font_size: u32,
    pub mono_font: String,
    pub mono_font_size: u32,
    pub word_wrap: bool,
    // Chat
    pub send_shortcut: String,
    pub follow_up: String,
    // Diffs & pull requests
    pub diff_layout: String,
    pub hide_whitespace: bool,
    pub diff_file_state: String,
    pub diff_colors: String,
    pub require_viewed: bool,
    pub proactive_changes: bool,
    pub pr_draft_default: bool,
    // Keybindings: action id → chord ("Ctrl+K")
    pub keybindings: BTreeMap<String, String>,
    // Checkpoints
    pub checkpoints: bool,
    pub checkpoint_keep_days: u32,
    // Providers
    pub claude_path: String,
    pub codex_path: String,
    pub claude_enabled: bool,
    pub codex_enabled: bool,
    pub claude_model: String,
    pub claude_effort: String,
    pub codex_model: String,
    pub codex_effort: String,
    pub health_interval_secs: u32,
    // Approvals (applied by Kiln when answering provider permission requests)
    pub risk_read: String,
    pub risk_write: String,
    pub risk_network: String,
    pub risk_outside: String,
    // Threads
    pub add_project_start: String,
    pub default_provider: String,
    pub permissions: String,
    pub workspace: String,
    pub parallel_agents: u32,
    pub auto_settle: bool,
    pub auto_settle_days: u32,
    // Source control
    pub merge_method: String,
    pub commit_template: String,
    // Worktrees
    pub worktree_location: String,
    pub worktree_from_remote: bool,
    pub branch_prefix: String,
    // Maintenance
    pub log_retention_days: u32,
    // Demo mode is explicit and labeled; it never replaces live data.
    pub demo_mode: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            notifications: "Notifications with sound".into(),
            in_app_toasts: true,
            working_section: true,
            sidebar_density: "Comfortable".into(),
            project_order: "Last user message".into(),
            archive_confirm: false,
            delete_confirm: true,
            time_format: "System default".into(),
            theme: "Dark".into(),
            theme_name: "Kiln".into(),
            contrast: 0,
            chat_width: "Comfortable".into(),
            effort_display: "Word".into(),
            ui_font: String::new(),
            ui_font_size: 14,
            mono_font: String::new(),
            mono_font_size: 13,
            word_wrap: false,
            send_shortcut: "Enter".into(),
            follow_up: "Queue".into(),
            diff_layout: "Stacked".into(),
            hide_whitespace: false,
            diff_file_state: "Expanded".into(),
            diff_colors: "Red & green".into(),
            require_viewed: false,
            proactive_changes: true,
            pr_draft_default: true,
            keybindings: default_keybindings(),
            checkpoints: true,
            checkpoint_keep_days: 30,
            claude_path: String::new(),
            codex_path: String::new(),
            claude_enabled: true,
            codex_enabled: true,
            claude_model: "default".into(),
            claude_effort: "high".into(),
            codex_model: String::new(),
            codex_effort: String::new(),
            health_interval_secs: 300,
            risk_read: "Always allow".into(),
            risk_write: "Ask".into(),
            risk_network: "Ask".into(),
            risk_outside: "Ask".into(),
            add_project_start: String::new(),
            default_provider: "claude".into(),
            permissions: "Supervised".into(),
            workspace: "New worktree".into(),
            parallel_agents: 4,
            auto_settle: true,
            auto_settle_days: 7,
            merge_method: "Squash and merge".into(),
            commit_template: String::new(),
            worktree_location: String::new(),
            worktree_from_remote: false,
            branch_prefix: "kiln".into(),
            log_retention_days: 14,
            demo_mode: false,
        }
    }
}

pub const ACTIONS: &[(&str, &str, &str, &str)] = &[
    // (id, group, title, when)
    ("palette", "Navigation", "Command palette", "always"),
    ("new_thread", "Navigation", "New thread", "always"),
    ("toggle_sidebar", "Navigation", "Toggle main sidebar", "always"),
    ("settings", "Navigation", "Open settings", "always"),
    ("board", "Navigation", "Open agents board", "always"),
    ("next_needs_you", "Threads", "Next thread that needs you", "always"),
    ("prev_thread", "Threads", "Previous thread", "always"),
    ("next_thread", "Threads", "Next thread", "always"),
    ("archive_thread", "Threads", "Archive thread", "a thread is open"),
    ("interrupt", "Composer", "Interrupt", "an agent is running"),
    ("approve", "Composer", "Approve request", "an approval is pending"),
    ("toggle_right", "Preview & diff", "Toggle right panel", "always"),
    ("toggle_terminal", "Preview & diff", "Toggle terminal drawer", "always"),
    ("toggle_details", "Preview & diff", "Toggle thread details", "a thread is open"),
];

pub fn default_keybindings() -> BTreeMap<String, String> {
    [
        ("palette", "Ctrl+K"),
        ("new_thread", "Ctrl+N"),
        ("toggle_sidebar", "Ctrl+B"),
        ("settings", "Ctrl+,"),
        ("board", "Ctrl+Shift+B"),
        ("next_needs_you", "Alt+Down"),
        ("prev_thread", "Alt+Up"),
        ("next_thread", "Ctrl+Tab"),
        ("archive_thread", "Ctrl+Shift+A"),
        ("interrupt", "Escape"),
        ("approve", "Ctrl+Enter"),
        ("toggle_right", "Ctrl+Shift+D"),
        ("toggle_terminal", "Ctrl+`"),
        ("toggle_details", "Ctrl+Shift+I"),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect()
}

/// Parsed chord: modifiers + key, matched against egui input.
#[derive(Clone, Debug, PartialEq)]
pub struct Chord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: egui::Key,
}

pub fn parse_chord(s: &str) -> Option<Chord> {
    let mut c = Chord { ctrl: false, shift: false, alt: false, key: egui::Key::Escape };
    let mut key = None;
    for part in s.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "cmd" | "command" => c.ctrl = true,
            "shift" => c.shift = true,
            "alt" | "option" => c.alt = true,
            "" => key = egui::Key::from_name("+"),
            other => key = egui::Key::from_name(part).or_else(|| egui::Key::from_name(&other.to_uppercase())),
        }
    }
    c.key = key?;
    Some(c)
}

pub fn chord_label(ev_mods: egui::Modifiers, key: egui::Key) -> String {
    let mut s = String::new();
    if ev_mods.command || ev_mods.ctrl {
        s.push_str("Ctrl+");
    }
    if ev_mods.shift {
        s.push_str("Shift+");
    }
    if ev_mods.alt {
        s.push_str("Alt+");
    }
    s.push_str(key.name());
    s
}

impl Settings {
    pub fn chat_max_width(&self) -> f32 {
        match self.chat_width.as_str() {
            "Wide" => 960.0,
            "Full" => f32::INFINITY,
            _ => crate::theme::LANE_MAX,
        }
    }
    pub fn is_dark(&self, system_dark: bool) -> bool {
        match self.theme.as_str() {
            "Light" => false,
            "Dark" => true,
            _ => system_dark,
        }
    }
    pub fn notify_popup(&self) -> bool {
        matches!(self.notifications.as_str(), "Notifications only" | "Notifications with sound")
    }
    pub fn notify_sound(&self) -> bool {
        matches!(self.notifications.as_str(), "Sound only" | "Notifications with sound")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_tolerate_missing_fields() {
        let s = Settings { theme: "Light".into(), parallel_agents: 2, ..Default::default() };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), s);
        // An older settings blob without newer fields still loads with defaults.
        let old: Settings = serde_json::from_str(r#"{"theme":"Light"}"#).unwrap();
        assert_eq!(old.theme, "Light");
        assert_eq!(old.branch_prefix, "kiln");
    }

    #[test]
    fn chords_parse() {
        let c = parse_chord("Ctrl+Shift+D").unwrap();
        assert!(c.ctrl && c.shift && !c.alt);
        assert_eq!(c.key, egui::Key::D);
        assert_eq!(parse_chord("Alt+Down").unwrap().key, egui::Key::ArrowDown);
        assert_eq!(parse_chord("Ctrl+,").unwrap().key, egui::Key::Comma);
        for (_, v) in default_keybindings() {
            assert!(parse_chord(&v).is_some(), "{v}");
        }
    }
}
