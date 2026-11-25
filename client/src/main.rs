mod ui;

use githalo::i18n::{self, AppPreferences, tr};
use githalo::theme::{self, ThemePreference};
use gpui::{
    AppContext as _, QuitMode, TitlebarOptions, WindowAppearance, WindowBounds, WindowOptions,
    point, px, size,
};

use ui::DesktopView;

fn main() {
    let prefs = AppPreferences::load(&AppPreferences::default_path());
    i18n::apply_preference(prefs.language);
    theme::apply_preference(prefs.theme);

    let initial_appearance = match prefs.theme {
        ThemePreference::System => std::env::var("GITHALO_THEME")
            .ok()
            .and_then(|raw| theme::parse_theme_hint(&raw))
            .map(|mode| match mode {
                theme::ThemeMode::Dark => WindowAppearance::Dark,
                theme::ThemeMode::Light => WindowAppearance::Light,
            }),
        ThemePreference::Dark => Some(WindowAppearance::Dark),
        ThemePreference::Light => Some(WindowAppearance::Light),
    };

    gpui_platform::application()
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(move |cx| {
            cx.set_window_appearance(initial_appearance);
            ui::bind_keys(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1320.), px(810.)), cx)),
                window_min_size: Some(size(px(740.), px(460.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("GitHalo".into()),
                    appears_transparent: cfg!(target_os = "macos"),
                    traffic_light_position: cfg!(target_os = "macos")
                        .then(|| point(px(12.), px(13.))),
                }),
                app_owns_titlebar_drag: cfg!(target_os = "macos"),
                ..Default::default()
            };

            if let Err(error) = cx.open_window(options, |window, cx| {
                cx.new(|cx| DesktopView::new(window, cx))
            }) {
                eprintln!(
                    "{}: {error:#}",
                    tr("无法打开 GitHalo 窗口", "Failed to open GitHalo window")
                );
                std::process::exit(1);
            }

            cx.activate(true);
        });
}
