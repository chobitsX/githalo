mod ui;

use githalo::i18n::{self, AppPreferences, tr};
use gpui::{
    AppContext as _, QuitMode, TitlebarOptions, WindowAppearance, WindowBounds, WindowOptions,
    point, px, size,
};

use ui::DesktopView;

fn main() {
    let prefs = AppPreferences::load(&AppPreferences::default_path());
    i18n::apply_preference(prefs.language);

    gpui_platform::application()
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(|cx| {
            cx.set_window_appearance(Some(WindowAppearance::Dark));
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
