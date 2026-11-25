//! Application appearance theme state (Dark / Light / Follow System).
//!
//! By default GitHalo follows the host OS appearance (`ThemePreference::System`).
//! The resolved [`ThemeMode`] is stored in an atomic global so UI components,
//! canvas painters, and the `syntect` highlighter can query it lock-free.

#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};

/// Active resolved appearance mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ThemeMode {
    Dark = 0,
    Light = 1,
}

/// User-selected theme preference persisted in `preferences.ini`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ThemePreference {
    #[default]
    System,
    Dark,
    Light,
}

impl ThemePreference {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    pub fn from_key(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "dark" => Self::Dark,
            "light" => Self::Light,
            _ => Self::System,
        }
    }

    pub fn resolve(self) -> ThemeMode {
        match self {
            Self::System => detect_system_theme(),
            Self::Dark => ThemeMode::Dark,
            Self::Light => ThemeMode::Light,
        }
    }
}

static ACTIVE_THEME: AtomicU8 = AtomicU8::new(ThemeMode::Dark as u8);

/// Return the currently active resolved theme mode.
#[inline]
pub fn current_theme() -> ThemeMode {
    match ACTIVE_THEME.load(Ordering::Relaxed) {
        1 => ThemeMode::Light,
        _ => ThemeMode::Dark,
    }
}

/// Return `true` when the active appearance is light mode.
#[inline]
pub fn is_light() -> bool {
    current_theme() == ThemeMode::Light
}

/// Set the active resolved theme mode immediately.
#[inline]
pub fn set_theme(mode: ThemeMode) {
    ACTIVE_THEME.store(mode as u8, Ordering::Relaxed);
}

/// Resolve and activate the theme mode for a given user preference.
pub fn apply_preference(preference: ThemePreference) -> ThemeMode {
    let mode = preference.resolve();
    set_theme(mode);
    mode
}

/// Detect the operating system appearance as a synchronous baseline.
///
/// Honors `GITHALO_THEME=dark|light` when set (useful for testing and screenshots).
/// On macOS, queries `defaults read -g AppleInterfaceStyle` (`Dark` -> dark, absent -> light).
pub fn detect_system_theme() -> ThemeMode {
    if let Ok(explicit) = std::env::var("GITHALO_THEME")
        && let Some(mode) = parse_theme_hint(&explicit)
    {
        return mode;
    }

    #[cfg(target_os = "macos")]
    {
        detect_macos_interface_style()
    }

    #[cfg(not(target_os = "macos"))]
    {
        ThemeMode::Dark
    }
}

/// Parse an explicit theme hint (`dark` / `light`).
pub fn parse_theme_hint(raw: &str) -> Option<ThemeMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "dark" => Some(ThemeMode::Dark),
        "light" => Some(ThemeMode::Light),
        _ => None,
    }
}

/// Parse the stdout of `defaults read -g AppleInterfaceStyle` (with exit success flag).
///
/// On macOS, `AppleInterfaceStyle` is `"Dark"` in Dark Mode and absent (non-zero exit) in Light Mode.
pub fn parse_macos_interface_style(succeeded: bool, stdout: &str) -> ThemeMode {
    if succeeded && stdout.trim().eq_ignore_ascii_case("dark") {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    }
}

#[cfg(target_os = "macos")]
fn detect_macos_interface_style() -> ThemeMode {
    match Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            parse_macos_interface_style(output.status.success(), &stdout)
        }
        Err(_) => ThemeMode::Dark,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_preference_keys_round_trip() {
        assert_eq!(ThemePreference::default(), ThemePreference::System);
        assert_eq!(ThemePreference::from_key("system"), ThemePreference::System);
        assert_eq!(ThemePreference::from_key("dark"), ThemePreference::Dark);
        assert_eq!(ThemePreference::from_key("LIGHT"), ThemePreference::Light);
        assert_eq!(
            ThemePreference::from_key("unknown"),
            ThemePreference::System
        );
        assert_eq!(ThemePreference::System.as_key(), "system");
        assert_eq!(ThemePreference::Dark.as_key(), "dark");
        assert_eq!(ThemePreference::Light.as_key(), "light");
    }

    #[test]
    fn macos_interface_style_parses_dark_and_light() {
        assert_eq!(parse_macos_interface_style(true, "Dark\n"), ThemeMode::Dark);
        assert_eq!(parse_macos_interface_style(true, "dark"), ThemeMode::Dark);
        assert_eq!(parse_macos_interface_style(false, ""), ThemeMode::Light);
        assert_eq!(parse_macos_interface_style(true, ""), ThemeMode::Light);
    }

    #[test]
    fn theme_hint_parses_explicit_values() {
        assert_eq!(parse_theme_hint("dark"), Some(ThemeMode::Dark));
        assert_eq!(parse_theme_hint("Light"), Some(ThemeMode::Light));
        assert_eq!(parse_theme_hint("auto"), None);
    }
}
