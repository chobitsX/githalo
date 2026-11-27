//! 应用内更新交互逻辑、顶栏提示胶囊与更新弹窗 UI。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use githalo::i18n::{is_zh, tr};
use githalo::updater::{
    CheckUpdateOutcome, DownloadProgress, DownloadStage, InstallAction, PreparedUpdate,
    RELEASES_PAGE_URL, ReleaseInfo, apply_prepared_update_and_relaunch, check_latest_release,
    download_and_prepare_update, format_bytes, now_epoch_secs, should_auto_check,
};
use gpui::{AnyElement, Context, Div, FontWeight, Role, Window, div, prelude::*, px, rems, rgb};

use super::DesktopView;
use super::components::*;
use super::overlay::Dialog;

#[derive(Clone, Debug, Default)]
pub(super) enum AppUpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate {
        version: String,
    },
    Available {
        release: ReleaseInfo,
    },
    Downloading {
        release: ReleaseInfo,
        downloaded: u64,
        total: Option<u64>,
        percent: Option<u8>,
        stage: DownloadStage,
        cancel: Arc<AtomicBool>,
    },
    ReadyToRestart {
        prepared: PreparedUpdate,
    },
    Error {
        message: String,
        release: Option<ReleaseInfo>,
    },
}

impl DesktopView {
    /// 启动后延迟几秒在后台静默检查一次更新（不阻塞首屏，不重复骚扰）。
    pub(super) fn schedule_auto_update_check(&self, cx: &mut Context<Self>) {
        if !self.auto_check_updates {
            return;
        }
        let now = now_epoch_secs();
        if !should_auto_check(self.last_update_check_epoch, now) {
            return;
        }
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            let _ = view.update(cx, |view, cx| {
                if view.auto_check_updates && matches!(view.app_update, AppUpdateState::Idle) {
                    view.start_update_check(false, cx);
                }
            });
        })
        .detach();
    }

    /// 打开更新弹窗并触发检查（供菜单栏「检查更新…」、命令面板、设置页调用）。
    pub(super) fn open_update_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let should_check = matches!(
            self.app_update,
            AppUpdateState::Idle | AppUpdateState::UpToDate { .. } | AppUpdateState::Error { .. }
        );
        self.open_confirmation(Dialog::AppUpdate, window, cx);
        if should_check {
            self.start_update_check(true, cx);
        }
    }

    /// 后台发起 GitHub Releases 最新版本检查。
    pub(super) fn start_update_check(&mut self, manual: bool, cx: &mut Context<Self>) {
        if matches!(
            self.app_update,
            AppUpdateState::Checking | AppUpdateState::Downloading { .. }
        ) {
            return;
        }
        self.app_update = AppUpdateState::Checking;
        cx.notify();

        let current = env!("CARGO_PKG_VERSION").to_owned();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::Builder::new()
            .name("githalo-update-check".into())
            .spawn(move || {
                let result = check_latest_release(&current);
                let _ = tx.send_blocking(result);
            })
            .ok();

        cx.spawn(async move |view, cx| {
            let Ok(result) = rx.recv().await else {
                return;
            };
            let _ = view.update(cx, |view, cx| {
                // 若期间已转入下载或就绪状态则丢弃旧检查结果。
                if !matches!(view.app_update, AppUpdateState::Checking) {
                    return;
                }
                match result {
                    Ok(CheckUpdateOutcome::Available(release)) => {
                        view.last_update_check_epoch = now_epoch_secs();
                        // 如果用户曾手动跳过此版本且本次为后台自动检测，静默保持 Idle 不打扰。
                        if !manual && view.skipped_update_version == release.version {
                            view.app_update = AppUpdateState::Idle;
                        } else {
                            view.app_update = AppUpdateState::Available { release };
                        }
                        view.save_preferences();
                    }
                    Ok(CheckUpdateOutcome::UpToDate { latest, .. }) => {
                        view.last_update_check_epoch = now_epoch_secs();
                        view.app_update = if manual {
                            AppUpdateState::UpToDate { version: latest }
                        } else {
                            AppUpdateState::Idle
                        };
                        view.save_preferences();
                    }
                    Err(message) => {
                        view.app_update = if manual {
                            AppUpdateState::Error {
                                message,
                                release: None,
                            }
                        } else {
                            // 后台静默检查遇断网/离线时不打扰用户
                            AppUpdateState::Idle
                        };
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 开始后台下载并校验新版本安装包。
    pub(super) fn start_update_download(&mut self, release: ReleaseInfo, cx: &mut Context<Self>) {
        if matches!(self.app_update, AppUpdateState::Downloading { .. }) {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.app_update = AppUpdateState::Downloading {
            release: release.clone(),
            downloaded: 0,
            total: release.asset_size,
            percent: Some(0),
            stage: DownloadStage::Downloading,
            cancel: cancel.clone(),
        };
        cx.notify();

        enum DownloadMsg {
            Progress(DownloadProgress),
            Done(Box<Result<PreparedUpdate, String>>),
        }

        let (tx, rx) = async_channel::unbounded();
        let release_for_thread = release.clone();
        let cancel_for_thread = cancel.clone();
        std::thread::Builder::new()
            .name("githalo-update-download".into())
            .spawn(move || {
                let result = download_and_prepare_update(
                    &release_for_thread,
                    &cancel_for_thread,
                    |progress| {
                        let _ = tx.try_send(DownloadMsg::Progress(progress));
                    },
                );
                let _ = tx.send_blocking(DownloadMsg::Done(Box::new(result)));
            })
            .ok();

        cx.spawn(async move |view, cx| {
            while let Ok(msg) = rx.recv().await {
                let keep_going = view
                    .update(cx, |view, cx| match msg {
                        DownloadMsg::Progress(progress) => {
                            if let AppUpdateState::Downloading {
                                downloaded,
                                total,
                                percent,
                                stage,
                                ..
                            } = &mut view.app_update
                            {
                                *downloaded = progress.downloaded;
                                *total = progress.total;
                                *percent = progress.percent;
                                *stage = progress.stage;
                                cx.notify();
                            }
                            true
                        }
                        DownloadMsg::Done(result) => {
                            match *result {
                                Ok(prepared) => {
                                    let ver = prepared.release.version.clone();
                                    view.app_update = AppUpdateState::ReadyToRestart { prepared };
                                    view.notice = Some(if is_zh() {
                                        format!(
                                            "新版本 v{ver} 已下载并校验完成，点击顶栏即可重启更新"
                                        )
                                    } else {
                                        format!("GitHalo v{ver} is ready — click Restart to update")
                                    });
                                }
                                Err(err) => {
                                    if cancel.load(Ordering::Relaxed) {
                                        view.app_update = AppUpdateState::Available {
                                            release: release.clone(),
                                        };
                                    } else {
                                        view.app_update = AppUpdateState::Error {
                                            message: err,
                                            release: Some(release.clone()),
                                        };
                                    }
                                }
                            }
                            cx.notify();
                            false
                        }
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn cancel_update_download(&mut self, cx: &mut Context<Self>) {
        if let AppUpdateState::Downloading {
            cancel, release, ..
        } = &self.app_update
        {
            cancel.store(true, Ordering::Relaxed);
            self.app_update = AppUpdateState::Available {
                release: release.clone(),
            };
            cx.notify();
        }
    }

    pub(super) fn skip_update_version(
        &mut self,
        version: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.skipped_update_version = version;
        self.app_update = AppUpdateState::Idle;
        self.save_preferences();
        self.close_overlay(window, cx);
    }

    pub(super) fn install_prepared_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let AppUpdateState::ReadyToRestart { prepared } = &self.app_update else {
            return;
        };
        let prepared = prepared.clone();
        match apply_prepared_update_and_relaunch(&prepared) {
            Ok(InstallAction::RestartNow) => {
                // 守护脚本已开始等待当前进程退出并原子替换 .app 后拉起新版本
                cx.quit();
            }
            Ok(InstallAction::OpenedInstaller) => {
                self.close_overlay(window, cx);
                self.notice = Some(
                    tr(
                        "已打开新版本安装包（.dmg），拖入「应用程序」即可完成更新",
                        "Opened the downloaded .dmg installer — drag GitHalo into Applications to finish updating",
                    )
                    .to_owned(),
                );
                cx.notify();
            }
            Err(err) => {
                self.app_update = AppUpdateState::Error {
                    message: err,
                    release: Some(prepared.release),
                };
                cx.notify();
            }
        }
    }

    /// 顶栏右侧精致的非打扰更新状态微胶囊（无新版本时返回 `None`）。
    pub(super) fn topbar_update_pill(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (label, tip, bg, border, fg, dot) = match &self.app_update {
            AppUpdateState::Available { release } => (
                if is_zh() {
                    format!("新版本 v{}", release.version)
                } else {
                    format!("Update v{}", release.version)
                },
                if is_zh() {
                    format!("发现新版本 v{}，点击查看更新内容并下载", release.version)
                } else {
                    format!(
                        "GitHalo v{} is available — click to view release notes & download",
                        release.version
                    )
                },
                theme().brand_bg,
                theme().brand_border,
                theme().brand_text,
                theme().brand,
            ),
            AppUpdateState::Downloading { percent, stage, .. } => {
                let label = match stage {
                    DownloadStage::Downloading => match percent {
                        Some(p) if is_zh() => format!("下载更新 {p}%"),
                        Some(p) => format!("Updating {p}%"),
                        None => tr("下载更新…", "Downloading…").to_owned(),
                    },
                    DownloadStage::VerifyingChecksum => tr("校验更新…", "Verifying…").to_owned(),
                    DownloadStage::ExtractingApp => tr("解压更新…", "Extracting…").to_owned(),
                    DownloadStage::VerifyingSignature => tr("验签中…", "Checking sig…").to_owned(),
                };
                (
                    label,
                    tr(
                        "正在后台准备新版本，点击查看进度",
                        "Preparing update in background — click for details",
                    )
                    .to_owned(),
                    theme().accent_bg,
                    theme().accent_border,
                    theme().link,
                    theme().link,
                )
            }
            AppUpdateState::ReadyToRestart { prepared } => (
                if prepared.can_in_place_restart() {
                    if is_zh() {
                        format!("重启更新 v{}", prepared.release.version)
                    } else {
                        format!("Restart for v{}", prepared.release.version)
                    }
                } else if is_zh() {
                    format!("安装 v{}", prepared.release.version)
                } else {
                    format!("Install v{}", prepared.release.version)
                },
                if is_zh() {
                    format!(
                        "GitHalo v{} 已下载并校验完成，点击立即重启更新",
                        prepared.release.version
                    )
                } else {
                    format!(
                        "GitHalo v{} is ready — click to restart and apply",
                        prepared.release.version
                    )
                },
                theme().success_bg,
                theme().success_border,
                theme().success,
                theme().success,
            ),
            _ => return None,
        };

        Some(
            div()
                .id("topbar-update-pill")
                .role(Role::Button)
                .tab_index(0)
                .h(rems(1.6875))
                .px(px(8.5))
                .flex_shrink_0()
                .rounded(px(6.))
                .bg(rgb(bg))
                .border_1()
                .border_color(rgb(border))
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(rems(0.72))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(fg))
                .hover(|s| s.border_color(rgb(theme().brand)))
                .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                    view.set_hovered_hint(hovered.then(|| tip.clone()), cx);
                }))
                .on_click(cx.listener(|view, _, window, cx| {
                    view.open_confirmation(Dialog::AppUpdate, window, cx);
                }))
                .child(
                    div()
                        .size(px(5.5))
                        .flex_shrink_0()
                        .rounded(px(999.))
                        .bg(rgb(dot)),
                )
                .child(label)
                .into_any_element(),
        )
    }

    /// 内置「软件更新」模态对话框。
    pub(super) fn app_update_dialog(&self, cx: &mut Context<Self>) -> Div {
        let current_ver = env!("CARGO_PKG_VERSION");

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(icon(Icon::Refresh, theme().brand))
                    .child(
                        div()
                            .text_size(px(14.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme().text))
                            .child(tr("软件更新", "Software Update")),
                    ),
            )
            .child(
                div()
                    .px(px(7.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .bg(rgb(theme().card))
                    .border_1()
                    .border_color(rgb(theme().border))
                    .font_family("Menlo")
                    .text_size(px(11.))
                    .text_color(rgb(theme().subtle))
                    .child(format!("{} v{current_ver}", tr("当前", "Current"))),
            );

        let body: Div = match &self.app_update {
            AppUpdateState::Idle | AppUpdateState::Checking => div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(
                    div()
                        .px(px(14.))
                        .py(px(18.))
                        .rounded(px(8.))
                        .bg(rgb(theme().card))
                        .border_1()
                        .border_color(rgb(theme().border))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(div().size(px(8.)).rounded(px(999.)).bg(rgb(theme().brand)))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(rgb(theme().text))
                                .child(tr("正在检查最新版本…", "Checking for the latest version…")),
                        ),
                )
                .child(div().flex().justify_end().child(
                    text_button("update-close-checking", tr("关闭", "Close"), false).on_click(
                        cx.listener(|view, _, window, cx| {
                            view.close_overlay(window, cx);
                        }),
                    ),
                )),
            AppUpdateState::UpToDate { version } => {
                let display_ver = if version.is_empty() {
                    current_ver
                } else {
                    version.as_str()
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(
                        div()
                            .px(px(14.))
                            .py(px(16.))
                            .rounded(px(8.))
                            .bg(rgb(theme().success_bg))
                            .border_1()
                            .border_color(rgb(theme().success_border))
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme().success))
                                    .child(tr("当前已是最新版本", "GitHalo is up to date")),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(theme().muted))
                                    .child(if is_zh() {
                                        format!("当前使用的 GitHalo v{display_ver} 已是最新版本。")
                                    } else {
                                        format!(
                                            "GitHalo v{display_ver} is currently the newest version available."
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                text_button(
                                    "update-recheck",
                                    tr("重新检查", "Check Again"),
                                    false,
                                )
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.start_update_check(true, cx);
                                })),
                            )
                            .child(
                                text_button("update-close-latest", tr("完成", "Done"), true)
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.close_overlay(window, cx);
                                    })),
                            ),
                    )
            }
            AppUpdateState::Available { release } => {
                let release_clone = release.clone();
                let skip_ver = release.version.clone();
                let release_url = release.html_url.clone();
                let size_hint = release.asset_size.map(format_bytes);
                let notes_text = if release.notes.is_empty() {
                    tr("此版本未附带详细说明。", "No release notes provided.").to_owned()
                } else {
                    release.notes.clone()
                };
                let has_asset = release.asset_url.is_some();

                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div()
                            .px(px(12.))
                            .py(px(10.))
                            .rounded(px(8.))
                            .bg(rgb(theme().brand_bg))
                            .border_1()
                            .border_color(rgb(theme().brand_border))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(theme().brand_text))
                                            .child(if is_zh() {
                                                format!("发现新版本 v{}", release.version)
                                            } else {
                                                format!("GitHalo v{} is available", release.version)
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .text_color(rgb(theme().muted))
                                            .child(match size_hint {
                                                Some(sz) if is_zh() => {
                                                    format!("安装包大小约 {sz} · 支持后台下载与校验后一键重启更新")
                                                }
                                                Some(sz) => {
                                                    format!("Download size ~{sz} · verified in background before restart")
                                                }
                                                None => tr(
                                                    "支持后台下载与签名校验后一键重启更新",
                                                    "Downloads and verifies in background before restart",
                                                )
                                                .to_owned(),
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .id("update-open-release-notes")
                                    .role(Role::Button)
                                    .tab_index(0)
                                    .px(px(7.))
                                    .py(px(3.))
                                    .rounded(px(5.))
                                    .text_size(px(11.))
                                    .text_color(rgb(theme().link))
                                    .hover(|s| s.bg(rgb(theme().hover)))
                                    .child(tr("发行说明 ↗", "Release Notes ↗"))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.open_url(&release_url);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id("update-release-notes")
                            .max_h(px(210.))
                            .overflow_y_scroll()
                            .px(px(12.))
                            .py(px(10.))
                            .rounded(px(7.))
                            .bg(rgb(theme().card))
                            .border_1()
                            .border_color(rgb(theme().border))
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(rgb(theme().text))
                            .child(notes_text),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .child(
                                text_button(
                                    "update-skip-version",
                                    tr("跳过此版本", "Skip This Version"),
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |view, _, window, cx| {
                                        view.skip_update_version(skip_ver.clone(), window, cx);
                                    },
                                )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        text_button(
                                            "update-later",
                                            tr("稍后再说", "Later"),
                                            false,
                                        )
                                        .on_click(cx.listener(|view, _, window, cx| {
                                            view.close_overlay(window, cx);
                                        })),
                                    )
                                    .when(has_asset, |row| {
                                        row.child(
                                            text_button(
                                                "update-start-download",
                                                tr("下载更新", "Download Update"),
                                                true,
                                            )
                                            .on_click(cx.listener(
                                                move |view, _, _, cx| {
                                                    view.start_update_download(
                                                        release_clone.clone(),
                                                        cx,
                                                    );
                                                },
                                            )),
                                        )
                                    }),
                            ),
                    )
            }
            AppUpdateState::Downloading {
                release,
                downloaded,
                total,
                percent,
                stage,
                ..
            } => {
                let pct = percent.unwrap_or(0);
                let stage_label = match stage {
                    DownloadStage::Downloading => {
                        let bytes_text = match total {
                            Some(t) => {
                                format!("{} / {}", format_bytes(*downloaded), format_bytes(*t))
                            }
                            None => format_bytes(*downloaded),
                        };
                        if is_zh() {
                            format!(
                                "正在下载 GitHalo v{}（{bytes_text} · {pct}%）",
                                release.version
                            )
                        } else {
                            format!(
                                "Downloading GitHalo v{} ({bytes_text} · {pct}%)",
                                release.version
                            )
                        }
                    }
                    DownloadStage::VerifyingChecksum => {
                        tr("正在校验安装包完整性…", "Verifying package integrity…").to_owned()
                    }
                    DownloadStage::ExtractingApp => {
                        tr("正在准备新版本应用…", "Preparing application bundle…").to_owned()
                    }
                    DownloadStage::VerifyingSignature => {
                        tr("正在校验安全签名…", "Verifying application signature…").to_owned()
                    }
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(
                        div()
                            .px(px(14.))
                            .py(px(14.))
                            .rounded(px(8.))
                            .bg(rgb(theme().card))
                            .border_1()
                            .border_color(rgb(theme().border))
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(theme().text))
                                    .child(stage_label),
                            )
                            .child(
                                div()
                                    .h(px(7.))
                                    .w_full()
                                    .rounded(px(999.))
                                    .bg(rgb(theme().border))
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .h_full()
                                            .w(gpui::relative(pct as f32 / 100.0))
                                            .rounded(px(999.))
                                            .bg(rgb(theme().brand)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(theme().subtle))
                                    .child(tr(
                                        "可关闭此弹窗继续工作，下载完成后将在顶栏提示一键重启更新。",
                                        "You may close this dialog while downloading continues in the background.",
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                text_button(
                                    "update-cancel-download",
                                    tr("取消下载", "Cancel"),
                                    false,
                                )
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.cancel_update_download(cx);
                                })),
                            )
                            .child(
                                text_button(
                                    "update-hide-downloading",
                                    tr("后台下载", "Continue in Background"),
                                    true,
                                )
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.close_overlay(window, cx);
                                })),
                            ),
                    )
            }
            AppUpdateState::ReadyToRestart { prepared } => {
                let in_place = prepared.can_in_place_restart();
                let ver = &prepared.release.version;
                let desc = if in_place {
                    if is_zh() {
                        format!(
                            "GitHalo v{ver} 已下载并通过完整性与安全签名校验。点击下方按钮将自动完成更新并重新启动。"
                        )
                    } else {
                        format!(
                            "GitHalo v{ver} is downloaded and verified. Click below to finish updating and relaunch."
                        )
                    }
                } else if is_zh() {
                    format!(
                        "GitHalo v{ver} 已下载并校验完成。当前以调试模式或只读目录运行，点击下方按钮将打开 .dmg 安装包完成更新。"
                    )
                } else {
                    format!(
                        "GitHalo v{ver} is downloaded and verified. Click below to open the .dmg installer."
                    )
                };
                let primary_label = if in_place {
                    tr("立即重启并更新", "Restart & Update Now")
                } else {
                    tr("打开安装包 (.dmg)", "Open Installer (.dmg)")
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(
                        div()
                            .px(px(14.))
                            .py(px(14.))
                            .rounded(px(8.))
                            .bg(rgb(theme().success_bg))
                            .border_1()
                            .border_color(rgb(theme().success_border))
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme().success))
                                    .child(if is_zh() {
                                        format!("新版本 v{ver} 已准备就绪")
                                    } else {
                                        format!("GitHalo v{ver} is ready to install")
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(17.))
                                    .text_color(rgb(theme().text))
                                    .child(desc),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                text_button("update-restart-later", tr("稍后重启", "Later"), false)
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.close_overlay(window, cx);
                                    })),
                            )
                            .child(
                                text_button("update-install-now", primary_label, true).on_click(
                                    cx.listener(|view, _, window, cx| {
                                        view.install_prepared_update(window, cx);
                                    }),
                                ),
                            ),
                    )
            }
            AppUpdateState::Error { message, release } => {
                let retry_release = release.clone();
                let open_url = release
                    .as_ref()
                    .map(|r| r.html_url.clone())
                    .unwrap_or_else(|| RELEASES_PAGE_URL.to_owned());
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(
                        div()
                            .px(px(14.))
                            .py(px(12.))
                            .rounded(px(8.))
                            .bg(rgb(theme().danger_bg))
                            .border_1()
                            .border_color(rgb(theme().danger_border))
                            .text_size(px(12.))
                            .line_height(px(17.))
                            .text_color(rgb(theme().error))
                            .child(message.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                text_button(
                                    "update-open-releases-fallback",
                                    tr("手动下载", "Download Manually"),
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |_, _, _, cx| {
                                        cx.open_url(&open_url);
                                    },
                                )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        text_button(
                                            "update-close-error",
                                            tr("关闭", "Close"),
                                            false,
                                        )
                                        .on_click(
                                            cx.listener(|view, _, window, cx| {
                                                view.close_overlay(window, cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        text_button("update-retry", tr("重试", "Retry"), true)
                                            .on_click(cx.listener(move |view, _, _, cx| {
                                                if let Some(rel) = retry_release.clone() {
                                                    view.start_update_download(rel, cx);
                                                } else {
                                                    view.start_update_check(true, cx);
                                                }
                                            })),
                                    ),
                            ),
                    )
            }
        };

        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(header)
            .child(body)
    }
}
