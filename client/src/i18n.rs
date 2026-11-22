//! Lightweight internationalization (Simplified Chinese & English) and UI preference persistence.
//!
//! By default the application follows the host OS language: Chinese locales resolve to
//! [`Locale::ZhCn`], and all other locales resolve to [`Locale::EnUs`].

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::branch_ops::PullStrategy;

/// Active display locale.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Locale {
    ZhCn = 0,
    EnUs = 1,
}

/// User-selected language preference.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LanguagePreference {
    #[default]
    System,
    ZhCn,
    EnUs,
}

impl LanguagePreference {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::ZhCn => "zh",
            Self::EnUs => "en",
        }
    }

    pub fn from_key(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "zh" | "zh-cn" | "zh_cn" | "chinese" => Self::ZhCn,
            "en" | "en-us" | "en_us" | "english" => Self::EnUs,
            _ => Self::System,
        }
    }

    pub fn resolve(self) -> Locale {
        match self {
            Self::System => detect_system_locale(),
            Self::ZhCn => Locale::ZhCn,
            Self::EnUs => Locale::EnUs,
        }
    }
}

static ACTIVE_LOCALE: AtomicU8 = AtomicU8::new(Locale::ZhCn as u8);

/// Return the currently active locale.
pub fn current_locale() -> Locale {
    match ACTIVE_LOCALE.load(Ordering::Relaxed) {
        0 => Locale::ZhCn,
        _ => Locale::EnUs,
    }
}

/// Return `true` when the active locale is Simplified Chinese.
#[inline]
pub fn is_zh() -> bool {
    current_locale() == Locale::ZhCn
}

/// Set the active locale immediately.
pub fn set_locale(locale: Locale) {
    ACTIVE_LOCALE.store(locale as u8, Ordering::Relaxed);
}

/// Resolve and activate the locale for a given user preference.
pub fn apply_preference(preference: LanguagePreference) -> Locale {
    let locale = preference.resolve();
    set_locale(locale);
    locale
}

/// Choose between a Chinese literal and an English literal according to the active locale.
#[inline]
pub fn tr(zh: &'static str, en: &'static str) -> &'static str {
    if is_zh() { zh } else { en }
}

/// Detect the operating system locale: Chinese -> [`Locale::ZhCn`], anything else -> [`Locale::EnUs`].
pub fn detect_system_locale() -> Locale {
    if let Ok(explicit) = std::env::var("GITHALO_LANG")
        && let Some(locale) = parse_locale_hint(&explicit)
    {
        return locale;
    }

    // On macOS, GUI apps launched from Finder/Dock often lack LANG/LC_ALL, so consult
    // macOS system preferences first unless the user explicitly set LC_ALL/LC_MESSAGES/LANG.
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(key)
            && let Some(locale) = parse_locale_hint(&value)
        {
            return locale;
        }
    }

    #[cfg(target_os = "macos")]
    if let Some(locale) = detect_macos_apple_languages() {
        return locale;
    }

    Locale::EnUs
}

#[cfg(target_os = "macos")]
fn detect_macos_apple_languages() -> Option<Locale> {
    let output = Command::new("defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_apple_languages_plist(&text)
}

/// Parse the text output of `defaults read -g AppleLanguages` and resolve the primary language.
pub fn parse_apple_languages_plist(output: &str) -> Option<Locale> {
    for line in output.lines() {
        let trimmed = line
            .trim()
            .trim_matches(|c| matches!(c, '(' | ')' | ',' | '"' | '\'' | ';'));
        if trimmed.is_empty() {
            continue;
        }
        if let Some(locale) = parse_locale_hint(trimmed) {
            return Some(locale);
        }
    }
    None
}

/// Parse a POSIX or BCP-47 locale tag (`zh_CN.UTF-8`, `zh-Hans-CN`, `en_US`, `ja-JP`, etc.).
/// Returns `None` for neutral tags (`C`, `POSIX`, empty) so callers can fall back cleanly.
pub fn parse_locale_hint(raw: &str) -> Option<Locale> {
    let cleaned = raw
        .trim()
        .trim_matches(|c| matches!(c, '"' | '\'' | ',' | ';'));
    if cleaned.is_empty() {
        return None;
    }
    // Strip encoding / modifier (`UTF-8`, `@euro`) and region subtags (`-CN`, `_US`).
    let primary = cleaned
        .split(['.', '@', '-', '_', ':'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if primary.is_empty() || primary == "c" || primary == "posix" {
        return None;
    }
    if primary == "zh" || primary == "chinese" {
        Some(Locale::ZhCn)
    } else {
        Some(Locale::EnUs)
    }
}

/// Persisted lightweight desktop preferences stored next to the workspace database.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppPreferences {
    pub language: LanguagePreference,
    pub pull_strategy: PullStrategy,
    pub show_commit_details: bool,
    pub diff_side_by_side: bool,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            language: LanguagePreference::System,
            pull_strategy: PullStrategy::FastForwardOnly,
            show_commit_details: true,
            diff_side_by_side: true,
        }
    }
}

impl AppPreferences {
    pub fn default_path() -> PathBuf {
        let mut path = crate::workspace::database_path();
        path.set_file_name("preferences.ini");
        path
    }

    pub fn load(path: &Path) -> Self {
        let Ok(content) = fs::read_to_string(path) else {
            return Self::default();
        };
        let mut prefs = Self::default();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "language" => prefs.language = LanguagePreference::from_key(value),
                "pull_strategy" => {
                    prefs.pull_strategy = match value.trim().to_ascii_lowercase().as_str() {
                        "merge" => PullStrategy::Merge,
                        "rebase" => PullStrategy::Rebase,
                        _ => PullStrategy::FastForwardOnly,
                    };
                }
                "show_commit_details" => {
                    prefs.show_commit_details =
                        !matches!(value.trim().to_ascii_lowercase().as_str(), "false" | "0");
                }
                "diff_side_by_side" => {
                    prefs.diff_side_by_side =
                        !matches!(value.trim().to_ascii_lowercase().as_str(), "false" | "0");
                }
                _ => {}
            }
        }
        prefs
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let pull_key = match self.pull_strategy {
            PullStrategy::FastForwardOnly => "ff",
            PullStrategy::Merge => "merge",
            PullStrategy::Rebase => "rebase",
        };
        let content = format!(
            "language={}\npull_strategy={}\nshow_commit_details={}\ndiff_side_by_side={}\n",
            self.language.as_key(),
            pull_key,
            self.show_commit_details,
            self.diff_side_by_side,
        );
        fs::write(path, content).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_hint_maps_chinese_to_zh_and_others_to_en() {
        assert_eq!(parse_locale_hint("zh_CN.UTF-8"), Some(Locale::ZhCn));
        assert_eq!(parse_locale_hint("zh-Hans-CN"), Some(Locale::ZhCn));
        assert_eq!(parse_locale_hint("zh-Hant-TW"), Some(Locale::ZhCn));
        assert_eq!(parse_locale_hint("zh"), Some(Locale::ZhCn));
        assert_eq!(parse_locale_hint("en_US.UTF-8"), Some(Locale::EnUs));
        assert_eq!(parse_locale_hint("ja-JP"), Some(Locale::EnUs));
        assert_eq!(parse_locale_hint("fr_FR"), Some(Locale::EnUs));
        assert_eq!(parse_locale_hint("C"), None);
        assert_eq!(parse_locale_hint("POSIX"), None);
        assert_eq!(parse_locale_hint(""), None);
    }

    #[test]
    fn apple_languages_plist_uses_primary_entry() {
        let zh_first = "(\n    \"zh-Hans-CN\",\n    \"en-US\"\n)\n";
        assert_eq!(parse_apple_languages_plist(zh_first), Some(Locale::ZhCn));

        let en_first = "(\n    \"en-US\",\n    \"zh-Hans-CN\"\n)\n";
        assert_eq!(parse_apple_languages_plist(en_first), Some(Locale::EnUs));

        let ja_first = "(\n    \"ja-JP\",\n    \"zh-Hans-CN\"\n)\n";
        assert_eq!(parse_apple_languages_plist(ja_first), Some(Locale::EnUs));
    }

    #[test]
    fn preferences_round_trip_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.ini");
        assert_eq!(AppPreferences::load(&path), AppPreferences::default());

        let saved = AppPreferences {
            language: LanguagePreference::EnUs,
            pull_strategy: PullStrategy::Rebase,
            show_commit_details: false,
            diff_side_by_side: false,
        };
        saved.save(&path).unwrap();
        assert_eq!(AppPreferences::load(&path), saved);
    }
}
