//! Lightweight internationalization (Simplified Chinese & English) and UI preference persistence.
//!
//! By default the application follows the host OS language: Chinese locales resolve to
//! [`Locale::ZhCn`], and all other locales resolve to [`Locale::EnUs`].

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::branch_ops::PullStrategy;
use crate::theme::ThemePreference;

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

/// Behavior when clicking a branch in the Log Branches sidebar.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BranchClickMode {
    #[default]
    Filter,
    Navigate,
}

impl BranchClickMode {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::Filter => "filter",
            Self::Navigate => "navigate",
        }
    }

    pub fn from_key(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "navigate" | "jump" => Self::Navigate,
            _ => Self::Filter,
        }
    }
}

/// Persisted lightweight desktop preferences stored next to the workspace database.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppPreferences {
    pub language: LanguagePreference,
    pub theme: ThemePreference,
    pub ui_scale: u16,
    pub pull_strategy: PullStrategy,
    pub show_commit_details: bool,
    pub diff_side_by_side: bool,
    pub show_branches_panel: bool,
    pub show_branches_drawer: bool,
    pub branches_group_by_dir: bool,
    pub branches_show_tags: bool,
    pub branches_only_favorites: bool,
    pub branches_click_mode: BranchClickMode,
    pub favorite_branches: BTreeSet<String>,
    pub commit_group_by_dir: bool,
    pub commit_staging_view: bool,
    pub commit_review_before: bool,
    pub commit_sign_off: bool,
    pub commit_no_verify: bool,
    pub commit_show_conventional: bool,
}

/// Allowed UI zoom percentage presets (90% .. 130%).
pub const UI_SCALE_PRESETS: [u16; 5] = [90, 100, 110, 120, 130];

/// Clamp and snap a persisted or computed UI scale percentage into `[90, 130]`.
pub fn normalize_ui_scale(raw: u16) -> u16 {
    let clamped = raw.clamp(90, 130);
    // Round to nearest 5% so keyboard/menu zoom remains clean.
    ((clamped + 2) / 5) * 5
}

impl Default for AppPreferences {
    fn default() -> Self {
        let mut favorite_branches = BTreeSet::new();
        favorite_branches.insert("refs/heads/main".to_owned());
        favorite_branches.insert("refs/heads/master".to_owned());
        favorite_branches.insert("refs/remotes/origin/main".to_owned());
        favorite_branches.insert("refs/remotes/origin/master".to_owned());
        Self {
            language: LanguagePreference::System,
            theme: ThemePreference::System,
            ui_scale: 100,
            pull_strategy: PullStrategy::FastForwardOnly,
            show_commit_details: true,
            diff_side_by_side: true,
            show_branches_panel: false,
            show_branches_drawer: true,
            branches_group_by_dir: true,
            branches_show_tags: false,
            branches_only_favorites: false,
            branches_click_mode: BranchClickMode::Filter,
            favorite_branches,
            commit_group_by_dir: true,
            commit_staging_view: false,
            commit_review_before: false,
            commit_sign_off: false,
            commit_no_verify: false,
            commit_show_conventional: false,
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
        let mut has_branches_drawer_key = false;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let parse_bool =
                |v: &str| !matches!(v.trim().to_ascii_lowercase().as_str(), "false" | "0");
            match key.trim() {
                "language" => prefs.language = LanguagePreference::from_key(value),
                "theme" => prefs.theme = ThemePreference::from_key(value),
                "ui_scale" => {
                    if let Ok(parsed) = value.trim().parse::<u16>() {
                        prefs.ui_scale = normalize_ui_scale(parsed);
                    }
                }
                "pull_strategy" => {
                    prefs.pull_strategy = match value.trim().to_ascii_lowercase().as_str() {
                        "merge" => PullStrategy::Merge,
                        "rebase" => PullStrategy::Rebase,
                        _ => PullStrategy::FastForwardOnly,
                    };
                }
                "show_commit_details" => prefs.show_commit_details = parse_bool(value),
                "diff_side_by_side" => prefs.diff_side_by_side = parse_bool(value),
                "show_branches_panel" => prefs.show_branches_panel = parse_bool(value),
                "show_branches_drawer" => {
                    has_branches_drawer_key = true;
                    prefs.show_branches_drawer = parse_bool(value);
                }
                "branches_group_by_dir" => prefs.branches_group_by_dir = parse_bool(value),
                "branches_show_tags" => prefs.branches_show_tags = parse_bool(value),
                "branches_only_favorites" => prefs.branches_only_favorites = parse_bool(value),
                "branches_click_mode" => {
                    prefs.branches_click_mode = BranchClickMode::from_key(value);
                }
                "favorite_branches" => {
                    prefs.favorite_branches = value
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(ToOwned::to_owned)
                        .collect();
                }
                "commit_group_by_dir" => prefs.commit_group_by_dir = parse_bool(value),
                "commit_staging_view" => prefs.commit_staging_view = parse_bool(value),
                "commit_review_before" => prefs.commit_review_before = parse_bool(value),
                "commit_sign_off" => prefs.commit_sign_off = parse_bool(value),
                "commit_no_verify" => prefs.commit_no_verify = parse_bool(value),
                "commit_show_conventional" => prefs.commit_show_conventional = parse_bool(value),
                _ => {}
            }
        }
        if !has_branches_drawer_key {
            prefs.show_branches_drawer = true;
            prefs.show_branches_panel = false;
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
        let favorites = self
            .favorite_branches
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        let content = format!(
            "language={}\ntheme={}\nui_scale={}\npull_strategy={}\nshow_commit_details={}\ndiff_side_by_side={}\nshow_branches_panel={}\nshow_branches_drawer={}\nbranches_group_by_dir={}\nbranches_show_tags={}\nbranches_only_favorites={}\nbranches_click_mode={}\nfavorite_branches={}\ncommit_group_by_dir={}\ncommit_staging_view={}\ncommit_review_before={}\ncommit_sign_off={}\ncommit_no_verify={}\ncommit_show_conventional={}\n",
            self.language.as_key(),
            self.theme.as_key(),
            normalize_ui_scale(self.ui_scale),
            pull_key,
            self.show_commit_details,
            self.diff_side_by_side,
            self.show_branches_panel,
            self.show_branches_drawer,
            self.branches_group_by_dir,
            self.branches_show_tags,
            self.branches_only_favorites,
            self.branches_click_mode.as_key(),
            favorites,
            self.commit_group_by_dir,
            self.commit_staging_view,
            self.commit_review_before,
            self.commit_sign_off,
            self.commit_no_verify,
            self.commit_show_conventional,
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

        let mut favorites = BTreeSet::new();
        favorites.insert("refs/heads/main".to_owned());
        favorites.insert("refs/heads/release/v1".to_owned());
        let saved = AppPreferences {
            language: LanguagePreference::EnUs,
            theme: ThemePreference::Light,
            ui_scale: 110,
            pull_strategy: PullStrategy::Rebase,
            show_commit_details: false,
            diff_side_by_side: false,
            show_branches_panel: true,
            show_branches_drawer: false,
            branches_group_by_dir: false,
            branches_show_tags: true,
            branches_only_favorites: true,
            branches_click_mode: BranchClickMode::Navigate,
            favorite_branches: favorites,
            commit_group_by_dir: false,
            commit_staging_view: true,
            commit_review_before: true,
            commit_sign_off: true,
            commit_no_verify: true,
            commit_show_conventional: true,
        };
        saved.save(&path).unwrap();
        assert_eq!(AppPreferences::load(&path), saved);
    }
}
