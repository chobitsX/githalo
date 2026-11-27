use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use githalo::branch_ops::PullStrategy;
use githalo::history::ChangedFile;
use githalo::i18n::{self, LanguagePreference, Locale, UI_SCALE_PRESETS, is_zh, tr};
use githalo::syntax::{self, SyntaxSpan};
use githalo::theme::{self as app_theme, ThemeMode, ThemePreference};
use githalo::workspace::{Action, FileInspectData, GroupId, RepoOpAction};
use githalo::worktree::WorktreeTarget;
use gpui::{
    AnyElement, ClipboardItem, Context, Div, Entity, Focusable, FontStyle, FontWeight,
    HighlightStyle, MouseButton, Pixels, Role, ScrollStrategy, SharedString, StyledText, Toggled,
    UniformListScrollHandle, Window, div, prelude::*, px, rgb, uniform_list,
};

use super::components::*;
use super::daily::check_indicator;
use super::file_icons::file_type_icon;
use super::history::mono_font;
use super::operations::review_detail;
use super::overlay::{Dialog, Overlay, dialog_heading, menu_item, menu_rule};
use super::{Confirm, DesktopView, TextField, text_field};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum SettingsSection {
    #[default]
    Appearance,
    General,
    Workspace,
    Shortcuts,
}

pub(super) struct CloneRepoForm {
    pub(super) group: Option<GroupId>,
    pub(super) url_input: Entity<TextField>,
    pub(super) dest_input: Entity<TextField>,
    pub(super) branch_input: Entity<TextField>,
    pub(super) error: Option<SharedString>,
}

pub(super) struct InitRepoForm {
    pub(super) group: Option<GroupId>,
    pub(super) dir_input: Entity<TextField>,
    pub(super) branch_input: Entity<TextField>,
    pub(super) error: Option<SharedString>,
}

pub(super) struct RemotesDialog {
    pub(super) target: WorktreeTarget,
    pub(super) name_input: Entity<TextField>,
    pub(super) url_input: Entity<TextField>,
    pub(super) error: Option<SharedString>,
}

pub(super) struct WorktreesDialog {
    pub(super) target: WorktreeTarget,
    pub(super) path_input: Entity<TextField>,
    pub(super) new_branch_input: Entity<TextField>,
    pub(super) start_ref_input: Entity<TextField>,
    pub(super) register_in_board: bool,
    pub(super) error: Option<SharedString>,
}

pub(super) struct PrecomputedBlameRow {
    oid: String,
    short_oid: SharedString,
    author: SharedString,
    date: SharedString,
    line_no: usize,
    line_no_str: SharedString,
    display_content: SharedString,
    same_as_prev: bool,
    age_bucket: usize,
}

pub(super) struct BlameCommitSummary {
    short_oid: SharedString,
    author: SharedString,
    date: SharedString,
    summary: SharedString,
    line_count: usize,
}

#[derive(Default)]
struct FileInspectFilterCache {
    query: String,
    blame_indices: Vec<usize>,
    history_indices: Vec<usize>,
    jumped_line: Option<usize>,
}

pub(super) struct FileInspectViewCache {
    data_ptr: usize,
    light: bool,
    rows: Vec<PrecomputedBlameRow>,
    commits: HashMap<String, BlameCommitSummary>,
    syntax: Rc<OnceCell<Vec<Vec<SyntaxSpan>>>>,
    filter_cache: RefCell<FileInspectFilterCache>,
}

const MAX_BLAME_DISPLAY_BYTES: usize = 4_096;

fn format_blame_display_line(raw: &str) -> String {
    let expanded = raw.replace('\t', "    ");
    if expanded.len() <= MAX_BLAME_DISPLAY_BYTES {
        return expanded;
    }
    let mut end = MAX_BLAME_DISPLAY_BYTES;
    while end > 0 && !expanded.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = expanded[..end].to_owned();
    out.push_str(" …");
    out
}

fn blame_author_initials(name: &str) -> String {
    let parts: Vec<&str> = name.split_whitespace().filter(|s| !s.is_empty()).collect();
    if parts.len() >= 2 {
        let mut s = String::new();
        if let Some(c) = parts[0].chars().next() {
            s.extend(c.to_uppercase());
        }
        if let Some(c) = parts[parts.len() - 1].chars().next() {
            s.extend(c.to_uppercase());
        }
        s
    } else {
        name.chars()
            .take(2)
            .flat_map(|c| c.to_uppercase())
            .collect()
    }
}

fn blame_avatar_color(name: &str) -> u32 {
    let palette = theme().avatar_palette;
    let hash = name.bytes().fold(5381usize, |acc, b| {
        acc.wrapping_mul(33).wrapping_add(b as usize)
    });
    palette[hash % palette.len()]
}

impl FileInspectViewCache {
    fn build(data: &Arc<FileInspectData>, light: bool) -> (Self, Vec<String>) {
        // Compute age buckets (0 = newest -> 5 = oldest) from sorted distinct dates.
        let distinct_dates: BTreeSet<&str> = data
            .blame
            .iter()
            .map(|l| l.date.as_str())
            .filter(|d| !d.is_empty())
            .collect();
        let date_rank: HashMap<&str, usize> = distinct_dates
            .iter()
            .rev()
            .enumerate()
            .map(|(idx, &date)| {
                let bucket = if distinct_dates.len() <= 1 {
                    0
                } else {
                    (idx * 5) / (distinct_dates.len() - 1)
                };
                (date, bucket.min(5))
            })
            .collect();

        // Seed commit subjects from `file_history_at` as fallback alongside porcelain summaries.
        let history_subjects: HashMap<&str, &str> = data
            .commits
            .iter()
            .map(|c| (c.oid.as_str(), c.subject.as_str()))
            .collect();

        let mut rows = Vec::with_capacity(data.blame.len());
        let mut highlight_lines = Vec::with_capacity(data.blame.len());
        let mut commits: HashMap<String, BlameCommitSummary> = HashMap::new();
        let mut prev_oid: Option<&str> = None;

        for line in &data.blame {
            let short = line.oid.get(..8).unwrap_or(&line.oid);
            let same_as_prev = prev_oid == Some(line.oid.as_str());
            prev_oid = Some(line.oid.as_str());
            let age_bucket = date_rank.get(line.date.as_str()).copied().unwrap_or(5);
            let display = format_blame_display_line(&line.content);
            highlight_lines.push(display.clone());

            commits
                .entry(line.oid.clone())
                .and_modify(|c| c.line_count += 1)
                .or_insert_with(|| {
                    let summary_text = if !line.summary.is_empty() {
                        line.summary.as_str()
                    } else {
                        history_subjects
                            .get(line.oid.as_str())
                            .copied()
                            .unwrap_or("")
                    };
                    BlameCommitSummary {
                        short_oid: SharedString::from(short.to_owned()),
                        author: SharedString::from(line.author.clone()),
                        date: SharedString::from(line.date.clone()),
                        summary: SharedString::from(summary_text.to_owned()),
                        line_count: 1,
                    }
                });

            rows.push(PrecomputedBlameRow {
                oid: line.oid.clone(),
                short_oid: SharedString::from(short.to_owned()),
                author: SharedString::from(line.author.clone()),
                date: SharedString::from(line.date.clone()),
                line_no: line.line_number,
                line_no_str: SharedString::from(line.line_number.to_string()),
                display_content: SharedString::from(display),
                same_as_prev,
                age_bucket,
            });
        }

        (
            Self {
                data_ptr: Arc::as_ptr(data) as usize,
                light,
                rows,
                commits,
                syntax: Rc::new(OnceCell::new()),
                filter_cache: RefCell::new(FileInspectFilterCache::default()),
            },
            highlight_lines,
        )
    }

    fn ensure_filter(
        &self,
        raw_query: &str,
        data: &FileInspectData,
        blame_scroll: &UniformListScrollHandle,
    ) -> (Option<usize>, Option<Vec<usize>>, Option<Vec<usize>>) {
        let trimmed = raw_query.trim();
        if trimmed.is_empty() {
            return (None, None, None);
        }
        // `:123` jumps directly to line 123 without filtering out surrounding context.
        if let Some(num_str) = trimmed.strip_prefix(':')
            && let Ok(target_line) = num_str.trim().parse::<usize>()
        {
            let target_idx = self
                .rows
                .binary_search_by_key(&target_line, |r| r.line_no)
                .unwrap_or_else(|i| i.min(self.rows.len().saturating_sub(1)));
            let mut fc = self.filter_cache.borrow_mut();
            if fc.query != trimmed || fc.jumped_line != Some(target_idx) {
                fc.query = trimmed.to_owned();
                fc.blame_indices.clear();
                fc.history_indices.clear();
                fc.jumped_line = Some(target_idx);
                if !self.rows.is_empty() {
                    blame_scroll.scroll_to_item(target_idx, ScrollStrategy::Center);
                }
            }
            return (Some(target_idx), None, None);
        }

        let mut fc = self.filter_cache.borrow_mut();
        if fc.query != trimmed {
            let needle = trimmed.to_lowercase();
            let blame_indices: Vec<usize> = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    r.display_content.to_lowercase().contains(&needle)
                        || r.author.to_lowercase().contains(&needle)
                        || r.short_oid.to_lowercase().contains(&needle)
                        || r.date.as_ref().contains(&needle)
                })
                .map(|(i, _)| i)
                .collect();
            let history_indices: Vec<usize> = data
                .commits
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.subject.to_lowercase().contains(&needle)
                        || c.author.to_lowercase().contains(&needle)
                        || c.oid.to_lowercase().contains(&needle)
                        || c.date.contains(&needle)
                })
                .map(|(i, _)| i)
                .collect();
            fc.query = trimmed.to_owned();
            fc.blame_indices = blame_indices;
            fc.history_indices = history_indices;
            fc.jumped_line = None;
        }
        (
            None,
            Some(fc.blame_indices.clone()),
            Some(fc.history_indices.clone()),
        )
    }
}

pub(super) struct FileInspectDialog {
    pub(super) target: WorktreeTarget,
    pub(super) revision: String,
    pub(super) path: PathBuf,
    pub(super) show_blame: bool,
    pub(super) filter_input: Entity<TextField>,
    pub(super) selected_oid: Option<String>,
    pub(super) blame_scroll: UniformListScrollHandle,
    pub(super) history_scroll: UniformListScrollHandle,
    pub(super) cache: Rc<RefCell<Option<Rc<FileInspectViewCache>>>>,
}

pub(super) struct CompareRevisionsDialog {
    pub(super) target: WorktreeTarget,
    pub(super) from_oid: String,
    pub(super) to_oid: String,
    pub(super) from_label: Option<String>,
    pub(super) to_label: Option<String>,
    pub(super) to_working_tree: bool,
}

pub(super) struct TagsDialog {
    pub(super) target: WorktreeTarget,
    pub(super) highlight_tag: Option<String>,
    pub(super) selected_remote: Option<String>,
}

impl DesktopView {
    pub(super) fn repo_admin_menu_items(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(target) = self.active_worktree().cloned() else {
            return vec![menu_item(
                "open-settings-dialog",
                tr("视图与偏好设置…", "Preferences & Language…"),
                Some(
                    tr(
                        "界面语言、默认 Pull 策略、布局与快捷键",
                        "Language, default Pull strategy, layout & shortcuts",
                    )
                    .into(),
                ),
                cx,
                |view, _, window, cx| {
                    view.open_settings_dialog(window, cx);
                },
            )];
        };
        let t1 = target.clone();
        let t2 = target.clone();
        let t3 = target;
        vec![
            menu_item(
                "open-remotes-dialog",
                tr("管理远端仓库（Remotes）…", "Manage Remotes…"),
                Some(
                    tr(
                        "查看、新增或删除远端地址",
                        "Inspect, add, update, or remove remotes",
                    )
                    .into(),
                ),
                cx,
                move |view, _, window, cx| {
                    view.open_remotes_dialog(t1.clone(), window, cx);
                },
            ),
            menu_item(
                "open-tags-dialog",
                tr("管理标签与推送（Tags）…", "Manage & Push Tags…"),
                Some(
                    tr(
                        "查看本地标签、推送到远端或删除",
                        "Browse local tags, push to remote, or delete",
                    )
                    .into(),
                ),
                cx,
                move |view, _, window, cx| {
                    view.open_tags_dialog(t2.clone(), None, window, cx);
                },
            ),
            menu_item(
                "open-worktrees-dialog",
                tr("管理关联工作树（Worktrees）…", "Manage Worktrees…"),
                Some(
                    tr(
                        "创建、锁定、移除或清理失效工作树",
                        "Create, lock, remove, or prune linked worktrees",
                    )
                    .into(),
                ),
                cx,
                move |view, _, window, cx| {
                    view.open_worktrees_dialog(t3.clone(), window, cx);
                },
            ),
            menu_rule(),
            menu_item(
                "open-settings-dialog",
                tr("视图与偏好设置…", "Preferences & Language…"),
                Some(
                    tr(
                        "界面语言、默认 Pull 策略、布局与快捷键",
                        "Language, default Pull strategy, layout & shortcuts",
                    )
                    .into(),
                ),
                cx,
                |view, _, window, cx| {
                    view.open_settings_dialog(window, cx);
                },
            ),
        ]
    }

    pub(super) fn open_clone_dialog(
        &mut self,
        group: Option<GroupId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let url_input = cx.new(|cx| {
            TextField::new(
                "仓库地址（HTTPS / SSH / 本地路径）",
                String::new(),
                false,
                cx,
            )
        });
        let dest_input =
            cx.new(|cx| TextField::new("目标空目录路径（绝对路径）", String::new(), false, cx));
        let branch_input = cx.new(|cx| {
            TextField::new(
                "可选：指定检出分支（留空使用默认分支）",
                String::new(),
                false,
                cx,
            )
        });
        self.open_confirmation(
            Dialog::CloneRepo(CloneRepoForm {
                group,
                url_input: url_input.clone(),
                dest_input,
                branch_input,
                error: None,
            }),
            window,
            cx,
        );
        url_input.focus_handle(cx).focus(window, cx);
    }

    pub(super) fn open_init_dialog(
        &mut self,
        group: Option<GroupId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dir_input =
            cx.new(|cx| TextField::new("新仓库目录路径（绝对路径）", String::new(), false, cx));
        let branch_input = cx.new(|cx| TextField::new("初始分支名", "main".to_owned(), false, cx));
        self.open_confirmation(
            Dialog::InitRepo(InitRepoForm {
                group,
                dir_input: dir_input.clone(),
                branch_input,
                error: None,
            }),
            window,
            cx,
        );
        dir_input.focus_handle(cx).focus(window, cx);
    }

    pub(super) fn open_remotes_dialog(
        &mut self,
        target: WorktreeTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name_input = cx
            .new(|cx| TextField::new("远端名称（如 origin / upstream）", String::new(), false, cx));
        let url_input =
            cx.new(|cx| TextField::new("远端地址（URL 或本地路径）", String::new(), false, cx));
        self.open_confirmation(
            Dialog::Remotes(RemotesDialog {
                target: target.clone(),
                name_input,
                url_input,
                error: None,
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(RepoOpAction::LoadRemotes { target }, cx);
    }

    pub(super) fn open_worktrees_dialog(
        &mut self,
        target: WorktreeTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_worktrees_dialog_with_ref(target, None, window, cx);
    }

    pub(super) fn open_worktrees_dialog_with_ref(
        &mut self,
        target: WorktreeTarget,
        start_ref: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial_ref = start_ref.unwrap_or_else(|| "HEAD".to_owned());
        let path_input = cx
            .new(|cx| TextField::new("新关联工作树目录路径（绝对路径）", String::new(), false, cx));
        let new_branch_input =
            cx.new(|cx| TextField::new("可选：新建分支名（-b）", String::new(), false, cx));
        let start_ref_input =
            cx.new(|cx| TextField::new("检出分支或起点引用", initial_ref, false, cx));
        self.open_confirmation(
            Dialog::Worktrees(WorktreesDialog {
                target: target.clone(),
                path_input,
                new_branch_input,
                start_ref_input,
                register_in_board: true,
                error: None,
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(RepoOpAction::LoadWorktrees { target }, cx);
    }

    pub(super) fn clone_repo_dialog(&self, form: &CloneRepoForm, cx: &mut Context<Self>) -> Div {
        let enabled = !self.workspace.repo_ops.admin_busy;
        div()
            .key_context("GroupNameDialog")
            .flex()
            .flex_col()
            .gap(px(10.))
            .on_action(cx.listener(|view, _: &Confirm, window, cx| view.submit_clone(window, cx)))
            .child(dialog_heading("克隆远程仓库（Clone）"))
            .child(form.url_input.clone())
            .child(form.dest_input.clone())
            .child(form.branch_input.clone())
            .children(
                form.error
                    .clone()
                    .map(|error| div().text_color(rgb(theme().error)).child(error)),
            )
            .child(self.review_buttons(
                "confirm-clone-repo",
                "开始克隆",
                enabled,
                Self::submit_clone,
                cx,
            ))
    }

    fn submit_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::CloneRepo(form))) = &mut self.overlay else {
            return;
        };
        let url = form.url_input.read(cx).text().trim().to_owned();
        let dest = form.dest_input.read(cx).text().trim().to_owned();
        let branch = form.branch_input.read(cx).text().trim().to_owned();
        if url.is_empty() {
            form.error = Some("请输入仓库地址".into());
            cx.notify();
            return;
        }
        if dest.is_empty() {
            form.error = Some("请输入目标目录路径".into());
            cx.notify();
            return;
        }
        let action = RepoOpAction::CloneRepository {
            url,
            destination: PathBuf::from(dest),
            branch: (!branch.is_empty()).then_some(branch),
            group: form.group,
        };
        self.close_overlay(window, cx);
        self.dispatch_repo_op(action, cx);
    }

    pub(super) fn init_repo_dialog(&self, form: &InitRepoForm, cx: &mut Context<Self>) -> Div {
        let enabled = !self.workspace.repo_ops.admin_busy;
        div()
            .key_context("GroupNameDialog")
            .flex()
            .flex_col()
            .gap(px(10.))
            .on_action(cx.listener(|view, _: &Confirm, window, cx| view.submit_init(window, cx)))
            .child(dialog_heading("初始化新 Git 仓库（Init）"))
            .child(form.dir_input.clone())
            .child(form.branch_input.clone())
            .children(
                form.error
                    .clone()
                    .map(|error| div().text_color(rgb(theme().error)).child(error)),
            )
            .child(self.review_buttons(
                "confirm-init-repo",
                "初始化并添加",
                enabled,
                Self::submit_init,
                cx,
            ))
    }

    fn submit_init(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::InitRepo(form))) = &mut self.overlay else {
            return;
        };
        let dir = form.dir_input.read(cx).text().trim().to_owned();
        let branch = form.branch_input.read(cx).text().trim().to_owned();
        if dir.is_empty() {
            form.error = Some("请输入目标目录路径".into());
            cx.notify();
            return;
        }
        let action = RepoOpAction::InitRepository {
            directory: PathBuf::from(dir),
            initial_branch: (!branch.is_empty()).then_some(branch),
            group: form.group,
        };
        self.close_overlay(window, cx);
        self.dispatch_repo_op(action, cx);
    }

    pub(super) fn remotes_dialog(
        &self,
        form: &RemotesDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.workspace.repo_ops;
        let writable = self.daily_can_write() && self.active_worktree() == Some(&form.target);
        let remotes = state
            .remotes
            .as_ref()
            .filter(|(t, _)| t == &form.target)
            .map(|(_, list)| list.as_slice());

        let list_view = match remotes {
            Some(list) if !list.is_empty() => div()
                .id("remotes-list")
                .max_h((viewport_height * 0.30).clamp(px(90.), px(220.)))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(5.))
                .children(list.iter().enumerate().map(|(idx, r)| {
                    let target = form.target.clone();
                    let name = r.name.clone();
                    div()
                        .p(px(8.))
                        .rounded(px(6.))
                        .bg(rgb(theme().panel))
                        .border_1()
                        .border_color(rgb(theme().border))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .child(pill(
                                            r.name.clone(),
                                            theme().accent_bg,
                                            theme().accent_border,
                                            theme().link,
                                        ))
                                        .child(elide(r.fetch_url.clone()).flex_1()),
                                )
                                .when(r.push_url != r.fetch_url, |box_div| {
                                    box_div.child(
                                        elide(format!("Push: {}", r.push_url))
                                            .text_size(px(11.))
                                            .text_color(rgb(theme().subtle)),
                                    )
                                }),
                        )
                        .child(
                            danger_button(("remove-remote", idx), "删除", writable)
                                .h(px(23.))
                                .text_size(px(11.5))
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(move |view, _, _, cx| {
                                        view.dispatch_repo_op(
                                            RepoOpAction::RemoveRemote {
                                                target: target.clone(),
                                                name: name.clone(),
                                            },
                                            cx,
                                        );
                                    }))
                                }),
                        )
                }))
                .into_any_element(),
            Some(_) => empty("当前仓库未配置任何远端").into_any_element(),
            None if state.remotes_loading => empty("正在读取远端配置…").into_any_element(),
            None => empty("无法读取远端配置").into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading("管理远端仓库（Remotes）"))
            .child(review_detail(
                tr("仓库", "Repository"),
                form.target.root.display().to_string(),
            ))
            .child(list_view)
            .child(
                div()
                    .p(px(9.))
                    .rounded(px(6.))
                    .bg(rgb(theme().card))
                    .border_1()
                    .border_color(rgb(theme().border))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(theme().muted))
                            .child("添加或更新远端"),
                    )
                    .child(form.name_input.clone())
                    .child(form.url_input.clone())
                    .children(
                        form.error
                            .clone()
                            .map(|err| div().text_color(rgb(theme().error)).child(err)),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(6.))
                            .child(
                                command_button("add-remote-btn", "添加远端", true, writable).when(
                                    writable,
                                    |btn| {
                                        btn.on_click(cx.listener(|view, _, _, cx| {
                                            view.submit_remote_upsert(false, cx);
                                        }))
                                    },
                                ),
                            )
                            .child(
                                command_button(
                                    "update-remote-btn",
                                    "更新已有地址",
                                    false,
                                    writable,
                                )
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(|view, _, _, cx| {
                                        view.submit_remote_upsert(true, cx);
                                    }))
                                }),
                            ),
                    ),
            )
            .child(
                div().flex().justify_end().child(
                    text_button("close-remotes", "完成", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ),
                ),
            )
    }

    fn submit_remote_upsert(&mut self, update_existing: bool, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::Remotes(form))) = &mut self.overlay else {
            return;
        };
        let name = form.name_input.read(cx).text().trim().to_owned();
        let url = form.url_input.read(cx).text().trim().to_owned();
        if name.is_empty() || url.is_empty() {
            form.error = Some("请填写远端名称与地址".into());
            cx.notify();
            return;
        }
        form.error = None;
        let target = form.target.clone();
        form.name_input.update(cx, TextField::reset);
        form.url_input.update(cx, TextField::reset);
        let action = if update_existing {
            RepoOpAction::SetRemoteUrl {
                target,
                name,
                fetch_url: url,
                push_url: None,
            }
        } else {
            RepoOpAction::AddRemote { target, name, url }
        };
        self.dispatch_repo_op(action, cx);
    }

    pub(super) fn worktrees_dialog(
        &self,
        form: &WorktreesDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.workspace.repo_ops;
        let writable = self.daily_can_write()
            && !state.admin_busy
            && self.active_worktree() == Some(&form.target);
        let worktrees = state
            .worktrees
            .as_ref()
            .filter(|(t, _)| t == &form.target)
            .map(|(_, list)| list.as_slice());

        let list_view = match worktrees {
            Some(list) if !list.is_empty() => div()
                .id("worktrees-list")
                .max_h((viewport_height * 0.30).clamp(px(100.), px(230.)))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(5.))
                .children(list.iter().enumerate().map(|(idx, wt)| {
                    let target = form.target.clone();
                    let wt_path = wt.path.clone();
                    let is_locked = wt.locked.is_some();
                    let is_prunable = wt.prunable.is_some();
                    let already_registered = self
                        .workspace
                        .repos
                        .iter()
                        .any(|r| r.target.root == wt.path);
                    div()
                        .p(px(8.))
                        .rounded(px(6.))
                        .bg(rgb(theme().panel))
                        .border_1()
                        .border_color(rgb(theme().border))
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.))
                                        .when(wt.is_main, |r| {
                                            r.child(pill(
                                                "主工作树",
                                                theme().accent_bg,
                                                theme().accent_border,
                                                theme().link,
                                            ))
                                        })
                                        .when_some(wt.branch.clone(), |r, b| {
                                            r.child(pill(
                                                b,
                                                theme().success_bg,
                                                theme().success_border,
                                                theme().success,
                                            ))
                                        })
                                        .when(wt.detached, |r| {
                                            r.child(pill(
                                                "分离 HEAD",
                                                theme().warn_bg,
                                                theme().warn_border,
                                                theme().warning,
                                            ))
                                        })
                                        .when(is_locked, |r| {
                                            r.child(pill(
                                                "已锁定",
                                                theme().remote_bg,
                                                theme().remote_border,
                                                theme().remote_text,
                                            ))
                                        })
                                        .when(is_prunable, |r| {
                                            r.child(pill(
                                                "已失效",
                                                theme().danger_bg,
                                                theme().danger_border,
                                                theme().error,
                                            ))
                                        }),
                                )
                                .child(
                                    elide(wt.path.display().to_string())
                                        .text_size(px(11.5))
                                        .text_color(rgb(theme().muted)),
                                ),
                        )
                        .when(!already_registered && !is_prunable, |row| {
                            let add_path = wt_path.clone();
                            row.child(
                                command_button(("wt-register", idx), "添加到列表", false, writable)
                                    .h(px(23.))
                                    .text_size(px(11.5))
                                    .when(writable, |btn| {
                                        btn.on_click(cx.listener(move |view, _, _, cx| {
                                            view.dispatch(
                                                Action::AddRepo {
                                                    path: add_path.clone(),
                                                    group: None,
                                                },
                                                cx,
                                            );
                                        }))
                                    }),
                            )
                        })
                        .when(!wt.is_main && !is_prunable, |row| {
                            let lock_target = target.clone();
                            let lock_path = wt_path.clone();
                            let rem_target = target.clone();
                            let rem_path = wt_path.clone();
                            row.child(
                                command_button(
                                    ("wt-lock", idx),
                                    if is_locked { "解锁" } else { "锁定" },
                                    false,
                                    writable,
                                )
                                .h(px(23.))
                                .text_size(px(11.5))
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(move |view, _, _, cx| {
                                        let action = if is_locked {
                                            RepoOpAction::UnlockLinkedWorktree {
                                                target: lock_target.clone(),
                                                path: lock_path.clone(),
                                            }
                                        } else {
                                            RepoOpAction::LockLinkedWorktree {
                                                target: lock_target.clone(),
                                                path: lock_path.clone(),
                                                reason: None,
                                            }
                                        };
                                        view.dispatch_repo_op(action, cx);
                                    }))
                                }),
                            )
                            .child(
                                danger_button(("wt-remove", idx), "移除", writable && !is_locked)
                                    .h(px(23.))
                                    .text_size(px(11.5))
                                    .when(writable && !is_locked, |btn| {
                                        btn.on_click(cx.listener(move |view, _, _, cx| {
                                            view.dispatch_repo_op(
                                                RepoOpAction::RemoveLinkedWorktree {
                                                    target: rem_target.clone(),
                                                    path: rem_path.clone(),
                                                    force: false,
                                                },
                                                cx,
                                            );
                                        }))
                                    }),
                            )
                        })
                }))
                .into_any_element(),
            Some(_) => empty("暂无工作树记录").into_any_element(),
            None if state.worktrees_loading => empty("正在读取关联工作树…").into_any_element(),
            None => empty("无法读取关联工作树").into_any_element(),
        };

        let prune_target = form.target.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(dialog_heading("管理关联工作树（Git Worktrees）"))
                    .child(div().flex_1())
                    .child(
                        command_button("prune-worktrees", "清理失效记录", false, writable)
                            .h(px(24.))
                            .text_size(px(11.5))
                            .when(writable, |btn| {
                                btn.on_click(cx.listener(move |view, _, _, cx| {
                                    view.dispatch_repo_op(
                                        RepoOpAction::PruneLinkedWorktrees {
                                            target: prune_target.clone(),
                                        },
                                        cx,
                                    );
                                }))
                            }),
                    ),
            )
            .child(list_view)
            .child(
                div()
                    .p(px(9.))
                    .rounded(px(6.))
                    .bg(rgb(theme().card))
                    .border_1()
                    .border_color(rgb(theme().border))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(theme().muted))
                            .child("创建新的关联工作树"),
                    )
                    .child(form.path_input.clone())
                    .child(form.new_branch_input.clone())
                    .child(form.start_ref_input.clone())
                    .child(
                        div()
                            .id("wt-register-toggle")
                            .role(Role::CheckBox)
                            .aria_label("创建后添加到仓库列表")
                            .aria_toggled(if form.register_in_board {
                                Toggled::True
                            } else {
                                Toggled::False
                            })
                            .tab_index(0)
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(check_indicator(form.register_in_board))
                            .child("创建后自动添加到仓库列表")
                            .on_click(cx.listener(|view, _, _, cx| {
                                if let Some(Overlay::Dialog(Dialog::Worktrees(form))) =
                                    &mut view.overlay
                                {
                                    form.register_in_board = !form.register_in_board;
                                    cx.notify();
                                }
                            })),
                    )
                    .children(
                        form.error
                            .clone()
                            .map(|err| div().text_color(rgb(theme().error)).child(err)),
                    )
                    .child(
                        div().flex().justify_end().child(
                            command_button("create-worktree-btn", "创建关联工作树", true, writable)
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(|view, _, _, cx| {
                                        view.submit_add_worktree(cx);
                                    }))
                                }),
                        ),
                    ),
            )
            .child(
                div().flex().justify_end().child(
                    text_button("close-worktrees", "完成", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ),
                ),
            )
    }

    fn submit_add_worktree(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::Worktrees(form))) = &mut self.overlay else {
            return;
        };
        let path = form.path_input.read(cx).text().trim().to_owned();
        let new_branch = form.new_branch_input.read(cx).text().trim().to_owned();
        let start_ref = form.start_ref_input.read(cx).text().trim().to_owned();
        if path.is_empty() || start_ref.is_empty() {
            form.error = Some("请填写新工作树路径和目标分支/提交".into());
            cx.notify();
            return;
        }
        form.error = None;
        let target = form.target.clone();
        let register = form.register_in_board;
        form.path_input.update(cx, TextField::reset);
        form.new_branch_input.update(cx, TextField::reset);
        self.dispatch_repo_op(
            RepoOpAction::AddLinkedWorktree {
                target,
                path: PathBuf::from(path),
                new_branch: (!new_branch.is_empty()).then_some(new_branch),
                commit_or_branch: start_ref,
                register,
            },
            cx,
        );
    }

    pub(super) fn open_file_inspect_dialog(
        &mut self,
        revision: String,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_file_inspect_dialog_mode(revision, path, true, window, cx);
    }

    pub(super) fn open_file_inspect_dialog_mode(
        &mut self,
        revision: String,
        path: PathBuf,
        show_blame: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        let filter_input = cx.new(|cx| {
            TextField::new(
                tr(
                    "过滤作者 / Hash / 代码，或输入 :行号 跳转…",
                    "Filter author / hash / code, or :line to jump…",
                ),
                String::new(),
                false,
                cx,
            )
        });
        cx.subscribe(&filter_input, |_, _, _: &text_field::Changed, cx| {
            cx.notify()
        })
        .detach();
        self.open_confirmation(
            Dialog::FileInspect(FileInspectDialog {
                target: target.clone(),
                revision: revision.clone(),
                path: path.clone(),
                show_blame,
                filter_input,
                selected_oid: None,
                blame_scroll: UniformListScrollHandle::new(),
                history_scroll: UniformListScrollHandle::new(),
                cache: Rc::new(RefCell::new(None)),
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(
            RepoOpAction::LoadFileInspect {
                target,
                revision,
                path,
            },
            cx,
        );
    }

    pub(super) fn open_compare_revisions_dialog(
        &mut self,
        from_oid: String,
        to_oid: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_compare_revisions_labeled(from_oid, to_oid, None, None, window, cx);
    }

    pub(super) fn open_compare_revisions_labeled(
        &mut self,
        from_oid: String,
        to_oid: String,
        from_label: Option<String>,
        to_label: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        self.open_confirmation(
            Dialog::CompareRevisions(CompareRevisionsDialog {
                target: target.clone(),
                from_oid: from_oid.clone(),
                to_oid: to_oid.clone(),
                from_label,
                to_label,
                to_working_tree: false,
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(
            RepoOpAction::LoadCompareRevisions {
                target,
                from_oid,
                to_oid,
            },
            cx,
        );
    }

    pub(super) fn open_compare_working_tree_dialog(
        &mut self,
        from_oid: String,
        from_label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        self.open_confirmation(
            Dialog::CompareRevisions(CompareRevisionsDialog {
                target: target.clone(),
                from_oid: from_oid.clone(),
                to_oid: String::new(),
                from_label: Some(from_label),
                to_label: Some(tr("当前工作树", "Working Tree").to_owned()),
                to_working_tree: true,
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(
            RepoOpAction::LoadCompareWorkingTree { target, from_oid },
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn open_blame_commit(
        &mut self,
        target: WorktreeTarget,
        oid: &str,
        short_oid: &str,
        path: PathBuf,
        force_diff_tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlay(window, cx);
        let in_loaded_log = !force_diff_tab && self.workspace.commits.iter().any(|c| c.oid == oid);
        if in_loaded_log {
            self.custom_tabs.active_id = None;
            self.deactivate_other_main_panes(cx);
            self.jump_to_commit_oid(oid, cx);
            return;
        }
        // Commit is older than the loaded log window or user held ⌘/Ctrl:
        // open its exact file diff in a dedicated tab so clicking a Blame hash never dead-ends.
        self.open_commit_file_diff_tab(
            target,
            vec![oid.to_owned()],
            ChangedFile {
                path,
                status: "M".to_owned(),
                previous_path: None,
            },
            None,
            cx,
        );
        if !force_diff_tab {
            self.notice = Some(if is_zh() {
                format!("提交 {short_oid} 超出当前日志加载范围，已在新标签页打开该提交的文件差异")
            } else {
                format!(
                    "Commit {short_oid} is outside loaded log; opened its file diff in a new tab"
                )
            });
            cx.notify();
        }
    }

    pub(super) fn file_inspect_dialog(
        &self,
        dialog: &FileInspectDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.workspace.repo_ops;
        let data = state
            .file_inspect
            .as_ref()
            .filter(|d| d.target == dialog.target && d.path == dialog.path);
        let body_h = (viewport_height * 0.66).clamp(px(300.), px(640.));
        let light = app_theme::is_light();

        // Lazily build O(N) precomputed cache once per `(FileInspectData, theme)` and kick off
        // background `syntect` syntax highlighting so rendering is strictly O(viewport) / 120 FPS.
        let cached: Option<Rc<FileInspectViewCache>> = data.map(|inspect| {
            let ptr = Arc::as_ptr(inspect) as usize;
            let mut slot = dialog.cache.borrow_mut();
            let needs_rebuild = slot
                .as_ref()
                .is_none_or(|c| c.data_ptr != ptr || c.light != light);
            if needs_rebuild {
                let (view_cache, highlight_lines) = FileInspectViewCache::build(inspect, light);
                let rc = Rc::new(view_cache);
                if !highlight_lines.is_empty() {
                    let weak_syntax = Rc::downgrade(&rc.syntax);
                    let path_for_bg = dialog.path.clone();
                    cx.spawn(async move |this, cx| {
                        let spans = cx
                            .background_executor()
                            .spawn(async move {
                                let refs: Vec<&str> =
                                    highlight_lines.iter().map(String::as_str).collect();
                                syntax::highlight_code_lines_for_theme(&path_for_bg, &refs, light)
                            })
                            .await;
                        let _ = this.update(cx, |_, cx| {
                            if let Some(cell) = weak_syntax.upgrade() {
                                let _ = cell.set(spans);
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                }
                *slot = Some(rc.clone());
                rc
            } else {
                slot.as_ref().unwrap().clone()
            }
        });

        let query = dialog.filter_input.read(cx).text().to_owned();
        let (jumped_idx, filtered_blame, filtered_history) = match (data, cached.as_ref()) {
            (Some(inspect), Some(c)) => c.ensure_filter(&query, inspect, &dialog.blame_scroll),
            _ => (None, None, None),
        };
        let is_filtering_blame = filtered_blame.is_some();
        let is_filtering_history = filtered_history.is_some();

        let blame_total = cached.as_ref().map_or(0, |c| c.rows.len());
        let blame_visible = filtered_blame.as_ref().map_or(blame_total, Vec::len);
        let history_total = data.map_or(0, |d| d.commits.len());
        let history_visible = filtered_history.as_ref().map_or(history_total, Vec::len);

        let selected_oid = dialog.selected_oid.clone();
        let selected_summary = selected_oid
            .as_deref()
            .and_then(|oid| cached.as_ref().and_then(|c| c.commits.get(oid)));

        let body: AnyElement = match (data, cached.clone()) {
            (Some(_), Some(view_cache)) if dialog.show_blame => {
                if blame_visible == 0 {
                    div()
                        .h(body_h)
                        .rounded(px(8.))
                        .bg(rgb(theme().panel))
                        .border_1()
                        .border_color(rgb(theme().border))
                        .child(empty(if blame_total == 0 {
                            tr(
                                "当前修订没有可显示的 Blame 归属信息",
                                "No Blame annotations available for this revision",
                            )
                        } else {
                            tr(
                                "没有匹配当前筛选条件的 Blame 行",
                                "No Blame lines match filter",
                            )
                        }))
                        .into_any_element()
                } else {
                    let list_cache = view_cache.clone();
                    let list_filtered = filtered_blame.map(Rc::new);
                    let list_selected = selected_oid.clone();
                    let blame_target = dialog.target.clone();
                    let blame_file_path = dialog.path.clone();
                    let weak_view = cx.entity().downgrade();
                    let blame_header = div()
                        .h(px(26.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .bg(rgb(theme().sidebar_bg))
                        .border_b_1()
                        .border_color(rgb(theme().border))
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(theme().muted))
                        .child(div().w(px(3.)).h_full().flex_shrink_0())
                        .child(
                            div()
                                .w(px(74.))
                                .px(px(6.))
                                .flex_shrink_0()
                                .child(tr("提交", "Commit")),
                        )
                        .child(
                            div()
                                .w(px(96.))
                                .px(px(4.))
                                .flex_shrink_0()
                                .child(tr("作者", "Author")),
                        )
                        .child(
                            div()
                                .w(px(84.))
                                .px(px(4.))
                                .flex_shrink_0()
                                .child(tr("日期", "Date")),
                        )
                        .child(
                            div()
                                .w(px(52.))
                                .pr(px(8.))
                                .flex_shrink_0()
                                .border_l_1()
                                .border_r_1()
                                .border_color(rgb(theme().border))
                                .text_align(gpui::TextAlign::Right)
                                .child(tr("行号", "Line")),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .px(px(10.))
                                .child(tr("代码内容", "Code")),
                        );

                    let list = uniform_list(
                        "file-inspect-blame-list",
                        blame_visible,
                        move |range: Range<usize>, _, _| {
                            let t = theme();
                            let palette = t.editor_blame_palette;
                            let syntax_ready = list_cache.syntax.get();
                            range
                                .filter_map(|visible_idx| {
                                    let raw_idx = match list_filtered.as_deref() {
                                        Some(indices) => *indices.get(visible_idx)?,
                                        None => visible_idx,
                                    };
                                    let row = list_cache.rows.get(raw_idx)?;
                                    let is_selected_commit =
                                        list_selected.as_deref() == Some(row.oid.as_str());
                                    let is_jumped = jumped_idx == Some(raw_idx);
                                    let show_meta =
                                        !row.same_as_prev || is_filtering_blame || visible_idx == 0;
                                    let show_group_rule =
                                        !row.same_as_prev && visible_idx > 0 && !is_filtering_blame;
                                    let heat_color = palette[row.age_bucket.min(palette.len() - 1)];

                                    let row_bg = if is_jumped {
                                        t.warn_bg
                                    } else if is_selected_commit {
                                        t.active
                                    } else {
                                        t.panel
                                    };
                                    let meta_bg =
                                        if is_selected_commit { t.active } else { t.card };
                                    let gutter_bg = if is_jumped {
                                        t.warn_bg
                                    } else if is_selected_commit {
                                        t.active
                                    } else {
                                        t.editor_gutter_bg
                                    };

                                    let code_el: AnyElement = match syntax_ready
                                        .and_then(|all| all.get(raw_idx))
                                    {
                                        Some(spans) if !spans.is_empty() => {
                                            let text = row.display_content.as_ref();
                                            let highlights: Vec<(Range<usize>, HighlightStyle)> =
                                                spans
                                                    .iter()
                                                    .filter(|s| {
                                                        s.start < s.end
                                                            && s.end <= text.len()
                                                            && text.is_char_boundary(s.start)
                                                            && text.is_char_boundary(s.end)
                                                    })
                                                    .map(|s| {
                                                        (
                                                            s.start..s.end,
                                                            HighlightStyle {
                                                                color: Some(rgb(s.color).into()),
                                                                font_weight: s
                                                                    .bold
                                                                    .then_some(FontWeight::BOLD),
                                                                font_style: s
                                                                    .italic
                                                                    .then_some(FontStyle::Italic),
                                                                ..Default::default()
                                                            },
                                                        )
                                                    })
                                                    .collect();
                                            if highlights.is_empty() {
                                                row.display_content.clone().into_any_element()
                                            } else {
                                                StyledText::new(row.display_content.clone())
                                                    .with_highlights(highlights)
                                                    .into_any_element()
                                            }
                                        }
                                        _ => row.display_content.clone().into_any_element(),
                                    };

                                    let click_oid = row.oid.clone();
                                    let click_view = weak_view.clone();
                                    let hash_cell: AnyElement = if show_meta {
                                        let nav_oid = row.oid.clone();
                                        let nav_short = row.short_oid.clone();
                                        let nav_view = weak_view.clone();
                                        let nav_target = blame_target.clone();
                                        let nav_path = blame_file_path.clone();
                                        let hash_tip = if let Some(meta) =
                                            list_cache.commits.get(row.oid.as_str())
                                        {
                                            let mod_key = if cfg!(target_os = "macos") {
                                                "⌘"
                                            } else {
                                                "Ctrl+"
                                            };
                                            if meta.summary.is_empty() {
                                                if is_zh() {
                                                    format!(
                                                        "{} · {} · {}（归属 {} 行）\n点击查看提交 · {mod_key}点击查看此文件差异",
                                                        meta.short_oid,
                                                        meta.author,
                                                        meta.date,
                                                        meta.line_count
                                                    )
                                                } else {
                                                    format!(
                                                        "{} · {} · {} ({} lines)\nClick to locate commit · {mod_key}Click to open file diff",
                                                        meta.short_oid,
                                                        meta.author,
                                                        meta.date,
                                                        meta.line_count
                                                    )
                                                }
                                            } else if is_zh() {
                                                format!(
                                                    "{}  {}\n{} · {}（归属 {} 行）· 点击定位提交 / {mod_key}点击看差异",
                                                    meta.short_oid,
                                                    meta.summary,
                                                    meta.author,
                                                    meta.date,
                                                    meta.line_count
                                                )
                                            } else {
                                                format!(
                                                    "{}  {}\n{} · {} ({} lines) · Click to locate / {mod_key}Click for diff",
                                                    meta.short_oid,
                                                    meta.summary,
                                                    meta.author,
                                                    meta.date,
                                                    meta.line_count
                                                )
                                            }
                                        } else {
                                            tr(
                                                "点击定位此提交或查看差异",
                                                "Click to locate commit or view diff",
                                            )
                                            .to_owned()
                                        };
                                        div()
                                            .w(px(74.))
                                            .px(px(4.))
                                            .h_full()
                                            .flex_shrink_0()
                                            .flex()
                                            .items_center()
                                            .child(
                                                div()
                                                    .id(("blame-oid-link", raw_idx))
                                                    .role(Role::Button)
                                                    .aria_label(hash_tip.clone())
                                                    .px(px(4.))
                                                    .py(px(1.))
                                                    .rounded(px(4.))
                                                    .font_family(mono_font())
                                                    .text_color(rgb(if is_selected_commit {
                                                        t.brand_text
                                                    } else {
                                                        t.link
                                                    }))
                                                    .hover(move |s| {
                                                        s.bg(rgb(t.accent_bg))
                                                            .text_color(rgb(t.link))
                                                            .underline()
                                                    })
                                                    .tooltip(tooltip(hash_tip))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        |_, _, cx| {
                                                            cx.stop_propagation();
                                                        },
                                                    )
                                                    .on_click(move |event, window, cx| {
                                                        cx.stop_propagation();
                                                        let mods = event.modifiers();
                                                        let force_diff =
                                                            mods.platform || mods.control;
                                                        let _ =
                                                            nav_view.update(cx, |view, cx| {
                                                                view.open_blame_commit(
                                                                    nav_target.clone(),
                                                                    &nav_oid,
                                                                    nav_short.as_ref(),
                                                                    nav_path.clone(),
                                                                    force_diff,
                                                                    window,
                                                                    cx,
                                                                );
                                                            });
                                                    })
                                                    .child(row.short_oid.clone()),
                                            )
                                            .into_any_element()
                                    } else {
                                        div()
                                            .w(px(74.))
                                            .px(px(6.))
                                            .flex_shrink_0()
                                            .into_any_element()
                                    };
                                    Some(
                                        div()
                                            .id(("blame-row", raw_idx))
                                            .h(px(22.))
                                            .w_full()
                                            .flex()
                                            .items_center()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .bg(rgb(row_bg))
                                            .when(show_group_rule, |r| {
                                                r.border_t_1().border_color(rgb(t.border))
                                            })
                                            .hover(move |s| {
                                                if is_selected_commit {
                                                    s.bg(rgb(t.active))
                                                } else {
                                                    s.bg(rgb(t.hover))
                                                }
                                            })
                                            .on_click(move |_, _, cx| {
                                                let _ = click_view.update(cx, |view, cx| {
                                                    if let Some(Overlay::Dialog(
                                                        Dialog::FileInspect(d),
                                                    )) = &mut view.overlay
                                                    {
                                                        if d.selected_oid.as_deref()
                                                            == Some(click_oid.as_str())
                                                        {
                                                            d.selected_oid = None;
                                                        } else {
                                                            d.selected_oid =
                                                                Some(click_oid.clone());
                                                        }
                                                        cx.notify();
                                                    }
                                                });
                                            })
                                            // 3px age heatmap bar
                                            .child(
                                                div()
                                                    .w(px(3.))
                                                    .h_full()
                                                    .flex_shrink_0()
                                                    .bg(rgb(heat_color)),
                                            )
                                            // Commit / Author / Date metadata gutter
                                            .child(
                                                div()
                                                    .h_full()
                                                    .flex_shrink_0()
                                                    .flex()
                                                    .items_center()
                                                    .bg(rgb(meta_bg))
                                                    .text_size(px(11.5))
                                                    .child(hash_cell)
                                                    .child(
                                                        elide(if show_meta {
                                                            row.author.clone()
                                                        } else {
                                                            SharedString::default()
                                                        })
                                                        .w(px(96.))
                                                        .px(px(4.))
                                                        .flex_shrink_0()
                                                        .text_color(rgb(t.muted)),
                                                    )
                                                    .child(
                                                        div()
                                                            .w(px(84.))
                                                            .px(px(4.))
                                                            .flex_shrink_0()
                                                            .font_family(mono_font())
                                                            .text_size(px(11.))
                                                            .text_color(rgb(t.subtle))
                                                            .child(if show_meta {
                                                                row.date.clone()
                                                            } else {
                                                                SharedString::default()
                                                            }),
                                                    ),
                                            )
                                            // Line number gutter
                                            .child(
                                                div()
                                                    .w(px(52.))
                                                    .h_full()
                                                    .pr(px(8.))
                                                    .flex_shrink_0()
                                                    .flex()
                                                    .items_center()
                                                    .justify_end()
                                                    .bg(rgb(gutter_bg))
                                                    .border_l_1()
                                                    .border_r_1()
                                                    .border_color(rgb(t.editor_gutter_border))
                                                    .font_family(mono_font())
                                                    .text_size(px(11.))
                                                    .text_color(rgb(t.editor_line_num))
                                                    .child(row.line_no_str.clone()),
                                            )
                                            // Syntax-highlighted code content
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .h_full()
                                                    .px(px(10.))
                                                    .flex()
                                                    .items_center()
                                                    .overflow_hidden()
                                                    .font_family(mono_font())
                                                    .text_size(px(12.))
                                                    .text_color(rgb(t.text))
                                                    .child(code_el),
                                            )
                                            .into_any_element(),
                                    )
                                })
                                .collect::<Vec<_>>()
                        },
                    )
                    .track_scroll(&dialog.blame_scroll)
                    .flex_1()
                    .min_h_0()
                    .w_full();

                    div()
                        .h(body_h)
                        .flex()
                        .flex_col()
                        .rounded(px(8.))
                        .overflow_hidden()
                        .bg(rgb(theme().panel))
                        .border_1()
                        .border_color(rgb(theme().border))
                        .child(blame_header)
                        .child(list)
                        .into_any_element()
                }
            }
            (Some(inspect), _) => {
                if history_visible == 0 {
                    div()
                        .h(body_h)
                        .rounded(px(8.))
                        .bg(rgb(theme().panel))
                        .border_1()
                        .border_color(rgb(theme().border))
                        .child(empty(if history_total == 0 {
                            tr(
                                "没有找到该文件的提交历史",
                                "No commit history found for this file",
                            )
                        } else {
                            tr("没有匹配当前筛选条件的提交记录", "No commits match filter")
                        }))
                        .into_any_element()
                } else {
                    let commits_arc = inspect.clone();
                    let list_filtered = filtered_history.map(Rc::new);
                    let list_selected = selected_oid.clone();
                    let file_path = dialog.path.clone();
                    let target_wt = dialog.target.clone();
                    let weak_view = cx.entity().downgrade();

                    let list = uniform_list(
                        "file-inspect-history-list",
                        history_visible,
                        move |range: Range<usize>, _, _| {
                            let t = theme();
                            range
                                .filter_map(|visible_idx| {
                                    let raw_idx = match list_filtered.as_deref() {
                                        Some(indices) => *indices.get(visible_idx)?,
                                        None => visible_idx,
                                    };
                                    let c = commits_arc.commits.get(raw_idx)?;
                                    let short = c.oid.get(..8).unwrap_or(&c.oid).to_owned();
                                    let is_selected =
                                        list_selected.as_deref() == Some(c.oid.as_str());
                                    let avatar_bg = blame_avatar_color(&c.author);
                                    let initials = blame_author_initials(&c.author);

                                    let sel_oid = c.oid.clone();
                                    let sel_view = weak_view.clone();
                                    let jump_oid = c.oid.clone();
                                    let jump_view = weak_view.clone();
                                    let cmp_oid = c.oid.clone();
                                    let cmp_target = target_wt.clone();
                                    let cmp_path = file_path.clone();
                                    let cmp_view = weak_view.clone();
                                    let rev_oid = c.oid.clone();
                                    let rev_target = target_wt.clone();
                                    let rev_path = file_path.clone();
                                    let rev_view = weak_view.clone();

                                    Some(
                                        div()
                                            .id(("file-hist-row", raw_idx))
                                            .h(px(40.))
                                            .w_full()
                                            .px(px(10.))
                                            .flex()
                                            .items_center()
                                            .gap(px(10.))
                                            .overflow_hidden()
                                            .bg(rgb(if is_selected { t.active } else { t.panel }))
                                            .border_b_1()
                                            .border_color(rgb(t.border))
                                            .hover(move |s| {
                                                s.bg(rgb(if is_selected {
                                                    t.active
                                                } else {
                                                    t.hover
                                                }))
                                            })
                                            .on_click(move |_, _, cx| {
                                                let _ = sel_view.update(cx, |view, cx| {
                                                    if let Some(Overlay::Dialog(
                                                        Dialog::FileInspect(d),
                                                    )) = &mut view.overlay
                                                    {
                                                        d.selected_oid = Some(sel_oid.clone());
                                                        cx.notify();
                                                    }
                                                });
                                            })
                                            .child(
                                                div()
                                                    .size(px(22.))
                                                    .flex_shrink_0()
                                                    .rounded(px(999.))
                                                    .bg(rgb(avatar_bg))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .text_size(px(10.))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(rgb(t.avatar_fg))
                                                    .child(initials),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .flex()
                                                    .flex_col()
                                                    .justify_center()
                                                    .gap(px(1.))
                                                    .child(
                                                        elide(c.subject.clone())
                                                            .text_size(px(12.5))
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .text_color(rgb(t.text)),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .gap(px(8.))
                                                            .text_size(px(11.))
                                                            .text_color(rgb(t.subtle))
                                                            .child(
                                                                div()
                                                                    .font_family(mono_font())
                                                                    .text_color(rgb(t.link))
                                                                    .child(short),
                                                            )
                                                            .child(c.author.clone())
                                                            .child("·")
                                                            .child(c.date.clone()),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_shrink_0()
                                                    .items_center()
                                                    .gap(px(5.))
                                                    .child(
                                                        command_button(
                                                            ("fh-cmp", raw_idx),
                                                            tr("与本地比较", "Compare Local"),
                                                            false,
                                                            true,
                                                        )
                                                        .h(px(22.))
                                                        .px(px(7.))
                                                        .on_click(move |_, window, cx| {
                                                            cx.stop_propagation();
                                                            let _ =
                                                                cmp_view.update(cx, |view, cx| {
                                                                    view.close_overlay(window, cx);
                                                                    view.open_local_compare_tab(
                                                                        cmp_target.clone(),
                                                                        vec![cmp_oid.clone()],
                                                                        ChangedFile {
                                                                            path: cmp_path.clone(),
                                                                            status: "M".to_owned(),
                                                                            previous_path: None,
                                                                        },
                                                                        false,
                                                                        None,
                                                                        cx,
                                                                    );
                                                                });
                                                        }),
                                                    )
                                                    .child(
                                                        command_button(
                                                            ("fh-rev", raw_idx),
                                                            tr("查看版本", "Open Version"),
                                                            false,
                                                            true,
                                                        )
                                                        .h(px(22.))
                                                        .px(px(7.))
                                                        .on_click(move |_, window, cx| {
                                                            cx.stop_propagation();
                                                            let _ =
                                                                rev_view.update(cx, |view, cx| {
                                                                    view.close_overlay(window, cx);
                                                                    view.open_repo_version_tab(
                                                                        rev_target.clone(),
                                                                        rev_oid.clone(),
                                                                        rev_path.clone(),
                                                                        cx,
                                                                    );
                                                                });
                                                        }),
                                                    )
                                                    .child(
                                                        command_button(
                                                            ("fh-jump", raw_idx),
                                                            tr("定位提交", "Locate"),
                                                            false,
                                                            true,
                                                        )
                                                        .h(px(22.))
                                                        .px(px(7.))
                                                        .on_click(move |_, window, cx| {
                                                            cx.stop_propagation();
                                                            let _ =
                                                                jump_view.update(cx, |view, cx| {
                                                                    view.close_overlay(window, cx);
                                                                    view.jump_to_commit_oid(
                                                                        &jump_oid, cx,
                                                                    );
                                                                });
                                                        }),
                                                    ),
                                            )
                                            .into_any_element(),
                                    )
                                })
                                .collect::<Vec<_>>()
                        },
                    )
                    .track_scroll(&dialog.history_scroll)
                    .flex_1()
                    .min_h_0()
                    .w_full();

                    div()
                        .h(body_h)
                        .flex()
                        .flex_col()
                        .rounded(px(8.))
                        .overflow_hidden()
                        .bg(rgb(theme().panel))
                        .border_1()
                        .border_color(rgb(theme().border))
                        .child(list)
                        .into_any_element()
                }
            }
            (None, _) if state.file_inspect_loading => div()
                .h(body_h)
                .rounded(px(8.))
                .bg(rgb(theme().panel))
                .border_1()
                .border_color(rgb(theme().border))
                .child(empty(tr(
                    "正在并行读取文件历史与 Blame 归属…",
                    "Loading file history and Blame annotations…",
                )))
                .into_any_element(),
            (None, _) => div()
                .h(body_h)
                .rounded(px(8.))
                .bg(rgb(theme().panel))
                .border_1()
                .border_color(rgb(theme().border))
                .child(empty(tr("无法读取文件信息", "Unable to read file info")))
                .into_any_element(),
        };

        let rev_short = dialog
            .revision
            .get(..8)
            .unwrap_or(&dialog.revision)
            .to_owned();
        let file_name = dialog
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| dialog.path.display().to_string());
        let blame_tab_label = if blame_total > 0 {
            if is_zh() {
                format!("Blame 归属 · {blame_total} 行")
            } else {
                format!("Blame · {blame_total} lines")
            }
        } else {
            tr("Blame 归属", "Blame").to_owned()
        };
        let history_tab_label = if history_total > 0 {
            if is_zh() {
                format!("文件历史 · {history_total} 条")
            } else {
                format!("History · {history_total}")
            }
        } else {
            tr("文件历史", "File History").to_owned()
        };

        let filter_badge = if dialog.show_blame && is_filtering_blame {
            Some(if is_zh() {
                format!("匹配 {blame_visible} / {blame_total} 行")
            } else {
                format!("{blame_visible} / {blame_total} lines")
            })
        } else if !dialog.show_blame && is_filtering_history {
            Some(if is_zh() {
                format!("匹配 {history_visible} / {history_total} 条")
            } else {
                format!("{history_visible} / {history_total} commits")
            })
        } else {
            None
        };

        let selected_footer: AnyElement = if let Some(oid) = selected_oid.clone() {
            let (short, author, date, summary, line_cnt) = match selected_summary {
                Some(s) => (
                    s.short_oid.clone(),
                    s.author.clone(),
                    s.date.clone(),
                    s.summary.clone(),
                    Some(s.line_count),
                ),
                None => {
                    let c = data.and_then(|d| d.commits.iter().find(|c| c.oid == oid));
                    (
                        SharedString::from(oid.get(..8).unwrap_or(&oid).to_owned()),
                        SharedString::from(c.map_or("", |c| c.author.as_str()).to_owned()),
                        SharedString::from(c.map_or("", |c| c.date.as_str()).to_owned()),
                        SharedString::from(c.map_or("", |c| c.subject.as_str()).to_owned()),
                        None,
                    )
                }
            };
            let copy_oid = oid.clone();
            let jump_oid = oid;
            let jump_short = short.clone();
            let jump_target = dialog.target.clone();
            let jump_path = dialog.path.clone();
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(pill(
                    short,
                    theme().brand_bg,
                    theme().brand_border,
                    theme().brand_text,
                ))
                .child(
                    elide(if summary.is_empty() {
                        format!("{author} · {date}")
                    } else {
                        format!("{summary}  —  {author} · {date}")
                    })
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(rgb(theme().text)),
                )
                .when_some(line_cnt, |bar, n| {
                    bar.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(rgb(theme().subtle))
                            .child(if is_zh() {
                                format!("归属 {n} 行")
                            } else {
                                format!("{n} lines")
                            }),
                    )
                })
                .child(
                    command_button(
                        "inspect-copy-oid",
                        tr("复制 Hash", "Copy Hash"),
                        false,
                        true,
                    )
                    .h(px(24.))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy_oid.clone()));
                        view.notice = Some(tr("已复制提交 Hash", "Copied commit hash").to_owned());
                        cx.notify();
                    })),
                )
                .child(
                    command_button(
                        "inspect-jump-log",
                        tr("在日志中定位", "Locate in Log"),
                        false,
                        true,
                    )
                    .h(px(24.))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.open_blame_commit(
                            jump_target.clone(),
                            &jump_oid,
                            jump_short.as_ref(),
                            jump_path.clone(),
                            false,
                            window,
                            cx,
                        );
                    })),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(11.5))
                .text_color(rgb(theme().subtle))
                .child(tr(
                    "点击蓝色提交 Hash 直达该提交（⌘点击新标签页看差异）· 点击行内高亮同提交归属行 · 输入 :行号 跳行",
                    "Click commit hash to open commit (⌘Click for diff tab) · Click row to highlight commit · Type :line to jump",
                ))
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            // Header row: file icon, file name, revision pill, relative path, close X
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(file_type_icon(&dialog.path))
                    .child(dialog_heading(file_name))
                    .child(pill(
                        rev_short,
                        theme().accent_bg,
                        theme().accent_border,
                        theme().link,
                    ))
                    .child(
                        elide(dialog.path.display().to_string())
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(rgb(theme().muted)),
                    )
                    .child(
                        icon_button("close-file-inspect-x", Icon::Close, tr("关闭", "Close"))
                            .size(px(24.))
                            .on_click(
                                cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                            ),
                    ),
            )
            // Control bar: segmented tabs + live search/jump input + match count
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .p(px(2.))
                            .gap(px(2.))
                            .rounded(px(6.))
                            .bg(rgb(theme().sidebar_bg))
                            .border_1()
                            .border_color(rgb(theme().border))
                            .child(
                                command_button(
                                    "inspect-tab-blame",
                                    blame_tab_label,
                                    dialog.show_blame,
                                    true,
                                )
                                .h(px(24.))
                                .on_click(cx.listener(
                                    |view, _, _, cx| {
                                        if let Some(Overlay::Dialog(Dialog::FileInspect(d))) =
                                            &mut view.overlay
                                        {
                                            d.show_blame = true;
                                            cx.notify();
                                        }
                                    },
                                )),
                            )
                            .child(
                                command_button(
                                    "inspect-tab-history",
                                    history_tab_label,
                                    !dialog.show_blame,
                                    true,
                                )
                                .h(px(24.))
                                .on_click(cx.listener(
                                    |view, _, _, cx| {
                                        if let Some(Overlay::Dialog(Dialog::FileInspect(d))) =
                                            &mut view.overlay
                                        {
                                            d.show_blame = false;
                                            cx.notify();
                                        }
                                    },
                                )),
                            ),
                    )
                    .child(div().flex_1())
                    .children(filter_badge.map(|badge| {
                        div()
                            .flex_shrink_0()
                            .px(px(7.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(rgb(theme().card))
                            .border_1()
                            .border_color(rgb(theme().border))
                            .text_size(px(11.))
                            .text_color(rgb(theme().muted))
                            .child(badge)
                    }))
                    .when(selected_oid.is_some(), |bar| {
                        bar.child(
                            command_button(
                                "inspect-clear-sel",
                                tr("清除高亮", "Clear Highlight"),
                                false,
                                true,
                            )
                            .h(px(24.))
                            .on_click(cx.listener(|view, _, _, cx| {
                                if let Some(Overlay::Dialog(Dialog::FileInspect(d))) =
                                    &mut view.overlay
                                {
                                    d.selected_oid = None;
                                    cx.notify();
                                }
                            })),
                        )
                    })
                    .child(
                        div()
                            .w(px(270.))
                            .flex_shrink_0()
                            .child(dialog.filter_input.clone()),
                    ),
            )
            .child(body)
            // Bottom status & action bar
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .pt(px(2.))
                    .child(selected_footer)
                    .child(
                        text_button("close-file-inspect", tr("关闭", "Close"), false).on_click(
                            cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                        ),
                    ),
            )
    }

    pub(super) fn compare_revisions_dialog(
        &self,
        dialog: &CompareRevisionsDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.workspace.repo_ops;
        let data = state.compare.as_ref().filter(|c| {
            c.target == dialog.target
                && c.from_oid == dialog.from_oid
                && c.to_oid == dialog.to_oid
                && c.to_working_tree == dialog.to_working_tree
        });
        let body_h = (viewport_height * 0.56).clamp(px(220.), px(480.));
        let from_short = dialog.from_label.clone().unwrap_or_else(|| {
            dialog
                .from_oid
                .get(..8)
                .unwrap_or(&dialog.from_oid)
                .to_owned()
        });
        let to_short = dialog.to_label.clone().unwrap_or_else(|| {
            if dialog.to_working_tree {
                tr("当前工作树", "Working Tree").to_owned()
            } else {
                dialog.to_oid.get(..8).unwrap_or(&dialog.to_oid).to_owned()
            }
        });
        let to_wt = dialog.to_working_tree;

        let body: AnyElement = match data {
            Some(data) => {
                let left = div()
                    .id("compare-file-list")
                    .w(px(240.))
                    .h(body_h)
                    .flex_shrink_0()
                    .overflow_y_scroll()
                    .rounded(px(6.))
                    .bg(rgb(theme().panel))
                    .border_1()
                    .border_color(rgb(theme().border))
                    .p(px(5.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(data.files.iter().enumerate().map(|(idx, file)| {
                        let selected = data.selected_file.as_ref() == Some(file);
                        let color = match file.status.as_bytes().first() {
                            Some(b'A') => theme().success,
                            Some(b'D') => theme().error,
                            Some(b'R' | b'C') => theme().warning,
                            _ => theme().link,
                        };
                        let target = dialog.target.clone();
                        let from = dialog.from_oid.clone();
                        let to = dialog.to_oid.clone();
                        let f = file.clone();
                        div()
                            .id(("compare-file", idx))
                            .h(px(24.))
                            .px(px(6.))
                            .rounded(px(4.))
                            .when(selected, |r| r.bg(rgb(theme().active)))
                            .hover(move |s| {
                                s.bg(rgb(if selected {
                                    theme().active
                                } else {
                                    theme().hover
                                }))
                            })
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .w(px(18.))
                                    .flex_shrink_0()
                                    .text_size(px(10.5))
                                    .text_color(rgb(color))
                                    .child(file.status.clone()),
                            )
                            .child(elide(file.path.display().to_string()).flex_1())
                            .on_click(cx.listener(move |view, _, _, cx| {
                                if to_wt {
                                    view.dispatch_repo_op(
                                        RepoOpAction::LoadCompareWorkingTreeDiff {
                                            target: target.clone(),
                                            from_oid: from.clone(),
                                            file: f.clone(),
                                        },
                                        cx,
                                    );
                                } else {
                                    view.dispatch_repo_op(
                                        RepoOpAction::LoadCompareDiff {
                                            target: target.clone(),
                                            from_oid: from.clone(),
                                            to_oid: to.clone(),
                                            file: f.clone(),
                                        },
                                        cx,
                                    );
                                }
                            }))
                    }))
                    .when(data.files.is_empty(), |box_div| {
                        box_div.child(empty(if to_wt {
                            tr(
                                "与当前工作树没有文件差异",
                                "No differences with Working Tree",
                            )
                        } else {
                            tr(
                                "两次修订之间没有文件差异",
                                "No file differences between revisions",
                            )
                        }))
                    });

                let right = div()
                    .id("compare-diff-view")
                    .flex_1()
                    .min_w_0()
                    .h(body_h)
                    .overflow_y_scroll()
                    .rounded(px(6.))
                    .bg(rgb(theme().panel))
                    .border_1()
                    .border_color(rgb(theme().border))
                    .p(px(6.))
                    .font_family("Menlo")
                    .text_size(px(11.5))
                    .line_height(px(17.))
                    .child(if state.compare_diff_loading {
                        div()
                            .p(px(4.))
                            .text_color(rgb(theme().subtle))
                            .child(tr("正在读取差异…", "Loading diff…"))
                    } else if let Some(diff) = &data.diff {
                        div().flex().flex_col().children(diff.lines().map(|line| {
                            let (bg, fg) = if line.starts_with("@@") {
                                (Some(theme().accent_bg), theme().link)
                            } else if line.starts_with('+') && !line.starts_with("+++") {
                                (Some(theme().success_bg), theme().success)
                            } else if line.starts_with('-') && !line.starts_with("---") {
                                (Some(theme().danger_bg), theme().error)
                            } else if line.starts_with("diff ")
                                || line.starts_with("index ")
                                || line.starts_with("---")
                                || line.starts_with("+++")
                            {
                                (None, theme().subtle)
                            } else {
                                (None, theme().text)
                            };
                            div()
                                .px(px(6.))
                                .when_some(bg, |row, color| row.bg(rgb(color)))
                                .text_color(rgb(fg))
                                .child(if line.is_empty() {
                                    " ".to_owned()
                                } else {
                                    line.to_owned()
                                })
                        }))
                    } else {
                        div().p(px(4.)).text_color(rgb(theme().subtle)).child(tr(
                            "点击左侧文件查看差异",
                            "Select a file on the left to inspect diff",
                        ))
                    });

                div()
                    .flex()
                    .gap(px(8.))
                    .min_w_0()
                    .child(left)
                    .child(right)
                    .into_any_element()
            }
            None if state.compare_loading => div()
                .h(body_h)
                .child(empty(tr("正在比较文件变更…", "Comparing file changes…")))
                .into_any_element(),
            None => div()
                .h(body_h)
                .child(empty(tr(
                    "无法读取比较结果",
                    "Unable to load comparison result",
                )))
                .into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .child(dialog_heading(if to_wt {
                        tr(
                            "与工作树对比（Diff with Working Tree）",
                            "Diff with Working Tree",
                        )
                    } else {
                        tr("修订比较（Compare Revisions）", "Compare Revisions")
                    }))
                    .child(pill(
                        from_short,
                        theme().remote_bg,
                        theme().remote_border,
                        theme().remote_text,
                    ))
                    .child(div().text_color(rgb(theme().subtle)).child("→"))
                    .child(pill(
                        to_short,
                        theme().accent_bg,
                        theme().accent_border,
                        theme().link,
                    )),
            )
            .child(body)
            .child(
                div().flex().justify_end().child(
                    text_button("close-compare-revisions", tr("关闭", "Close"), false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ),
                ),
            )
    }

    pub(super) fn open_tags_dialog(
        &mut self,
        target: WorktreeTarget,
        highlight_tag: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_confirmation(
            Dialog::Tags(TagsDialog {
                target: target.clone(),
                highlight_tag,
                selected_remote: None,
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(
            RepoOpAction::LoadTags {
                target: target.clone(),
            },
            cx,
        );
        self.dispatch_repo_op(RepoOpAction::LoadRemotes { target }, cx);
    }

    pub(super) fn tags_dialog(
        &self,
        dialog: &TagsDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let writable = self.daily_can_write() && self.active_worktree() == Some(&dialog.target);
        let tags = self
            .workspace
            .repo_ops
            .tags
            .as_ref()
            .filter(|(t, _)| t == &dialog.target)
            .map(|(_, list)| list.as_slice());
        let remotes = self
            .workspace
            .repo_ops
            .remotes
            .as_ref()
            .filter(|(t, _)| t == &dialog.target)
            .map(|(_, list)| list.as_slice())
            .unwrap_or(&[]);
        let active_remote = dialog
            .selected_remote
            .as_deref()
            .filter(|name| remotes.iter().any(|r| r.name == *name))
            .or_else(|| {
                remotes
                    .iter()
                    .find(|r| r.name == "origin")
                    .or_else(|| remotes.first())
                    .map(|r| r.name.as_str())
            })
            .map(ToOwned::to_owned);

        let remote_selector = if remotes.is_empty() {
            div()
                .text_size(px(11.5))
                .text_color(rgb(theme().subtle))
                .child("未配置远端，无法推送标签（可在 Remotes 中添加）")
        } else {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(theme().muted))
                        .child("推送目标远端："),
                )
                .children(remotes.iter().enumerate().map(|(idx, r)| {
                    let selected = active_remote.as_deref() == Some(r.name.as_str());
                    let name = r.name.clone();
                    div()
                        .id(("tag-target-remote", idx))
                        .role(Role::Button)
                        .aria_selected(selected)
                        .tab_index(0)
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(rgb(if selected {
                            theme().accent_border
                        } else {
                            theme().border
                        }))
                        .bg(rgb(if selected {
                            theme().accent_bg
                        } else {
                            theme().card
                        }))
                        .text_size(px(11.5))
                        .text_color(rgb(if selected { theme().link } else { theme().text }))
                        .hover(|s| {
                            s.bg(rgb(if selected {
                                theme().accent_bg
                            } else {
                                theme().hover
                            }))
                        })
                        .child(r.name.clone())
                        .on_click(cx.listener(move |view, _, _, cx| {
                            if let Some(Overlay::Dialog(Dialog::Tags(d))) = &mut view.overlay {
                                d.selected_remote = Some(name.clone());
                                cx.notify();
                            }
                        }))
                }))
        };

        let list_view = match tags {
            _ if self.workspace.repo_ops.tags_loading => div()
                .text_color(rgb(theme().subtle))
                .child("正在读取标签列表…")
                .into_any_element(),
            Some(list) if !list.is_empty() => div()
                .id("tags-list")
                .max_h((viewport_height * 0.45).clamp(px(160.), px(360.)))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(5.))
                .children(list.iter().enumerate().map(|(idx, tag)| {
                    let highlighted = dialog.highlight_tag.as_deref() == Some(tag.name.as_str());
                    let push_target = dialog.target.clone();
                    let push_name = tag.name.clone();
                    let push_remote = active_remote.clone();
                    let can_push = writable && push_remote.is_some();
                    let del_target = dialog.target.clone();
                    let del_name = tag.name.clone();
                    let short = tag.oid.get(..8).unwrap_or(&tag.oid).to_owned();
                    div()
                        .px(px(10.))
                        .py(px(7.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(rgb(if highlighted {
                            theme().warn_border
                        } else {
                            theme().border
                        }))
                        .bg(rgb(if highlighted {
                            theme().warn_bg
                        } else {
                            theme().card
                        }))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .child(pill(
                                            tag.name.clone(),
                                            theme().warn_bg,
                                            theme().warn_border,
                                            theme().warning,
                                        ))
                                        .child(
                                            div()
                                                .text_size(px(11.5))
                                                .font_family("Menlo")
                                                .text_color(rgb(theme().subtle))
                                                .child(short),
                                        ),
                                )
                                .when(!tag.subject.is_empty(), |col| {
                                    col.child(
                                        elide(tag.subject.clone())
                                            .text_size(px(11.5))
                                            .text_color(rgb(theme().muted)),
                                    )
                                }),
                        )
                        .child(
                            command_button(("push-tag-row", idx), "推送", highlighted, can_push)
                                .h(px(24.))
                                .text_size(px(11.5))
                                .when(can_push, |btn| {
                                    btn.on_click(cx.listener(move |view, _, _, cx| {
                                        if let Some(remote) = push_remote.clone() {
                                            view.dispatch_repo_op(
                                                RepoOpAction::PushTag {
                                                    target: push_target.clone(),
                                                    remote,
                                                    name: push_name.clone(),
                                                },
                                                cx,
                                            );
                                        }
                                    }))
                                }),
                        )
                        .child(
                            danger_button(("delete-tag-row", idx), "删除", writable)
                                .h(px(24.))
                                .text_size(px(11.5))
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(move |view, _, _, cx| {
                                        view.dispatch_repo_op(
                                            RepoOpAction::DeleteTag {
                                                target: del_target.clone(),
                                                name: del_name.clone(),
                                            },
                                            cx,
                                        );
                                    }))
                                }),
                        )
                }))
                .into_any_element(),
            _ => div()
                .py(px(16.))
                .text_color(rgb(theme().subtle))
                .child("当前仓库尚无本地标签（可在提交日志右键某条提交新建标签）")
                .into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading("标签管理与推送（Tags）"))
            .child(review_detail(
                tr("仓库", "Repository"),
                dialog.target.root.display().to_string(),
            ))
            .child(remote_selector)
            .child(list_view)
            .child(
                div().flex().justify_end().child(
                    text_button("close-tags-dialog", "关闭", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ),
                ),
            )
    }

    pub(super) fn open_settings_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_confirmation(Dialog::Settings, window, cx);
    }

    pub(super) fn settings_dialog(&self, cx: &mut Context<Self>) -> Div {
        let active_section = self.settings_section;
        let nav_items = [
            (
                SettingsSection::Appearance,
                Icon::Eye,
                tr("外观与显示", "Appearance"),
            ),
            (
                SettingsSection::General,
                Icon::Branch,
                tr("通用与 Git", "General & Git"),
            ),
            (
                SettingsSection::Workspace,
                Icon::PanelLeft,
                tr("工作区布局", "Workspace"),
            ),
            (
                SettingsSection::Shortcuts,
                Icon::Target,
                tr("快捷键", "Shortcuts"),
            ),
        ];

        let (page_title, page_subtitle) = match active_section {
            SettingsSection::Appearance => (
                tr("外观与显示", "Appearance & Display"),
                tr(
                    "选择跟随系统的明暗主题，并按屏幕密度等比缩放界面",
                    "Match your system appearance and tune proportional interface scaling",
                ),
            ),
            SettingsSection::General => (
                tr("通用与 Git 策略", "General & Git Strategy"),
                tr(
                    "配置界面显示语言以及拉取远端更新时的默认合并策略",
                    "Configure display language and default update strategy for Pull",
                ),
            ),
            SettingsSection::Workspace => (
                tr("工作区与面板布局", "Workspace & Layout"),
                tr(
                    "自定义侧边栏分支抽屉、日志分栏及默认差异对比方式",
                    "Customize branch panels, commit details, and default diff layout",
                ),
            ),
            SettingsSection::Shortcuts => (
                tr("键盘快捷键", "Keyboard Shortcuts"),
                tr(
                    "常用导航、Git 操作与编辑快捷键速查",
                    "Quick reference for navigation, Git actions, and editor shortcuts",
                ),
            ),
        };

        let left_rail = div()
            .w(px(188.))
            .flex_shrink_0()
            .h_full()
            .bg(rgb(theme().sidebar_bg))
            .border_r_1()
            .border_color(rgb(theme().border))
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(14.))
                    .pt(px(15.))
                    .pb(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .child(
                        div()
                            .size(px(28.))
                            .rounded(px(7.))
                            .bg(rgb(theme().brand_bg))
                            .border_1()
                            .border_color(rgb(theme().brand_border))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(Icon::Settings, theme().brand)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme().text))
                                    .child(tr("偏好设置", "Preferences")),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(theme().subtle))
                                    .child("GitHalo"),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .px(px(8.))
                    .py(px(4.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(nav_items.into_iter().enumerate().map(
                        |(idx, (section, ico, label))| {
                            let selected = active_section == section;
                            div()
                                .id(("settings-nav", idx))
                                .role(Role::Tab)
                                .aria_selected(selected)
                                .tab_index(0)
                                .relative()
                                .h(px(32.))
                                .px(px(10.))
                                .rounded(px(6.))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .bg(rgb(if selected {
                                    theme().active
                                } else {
                                    theme().sidebar_bg
                                }))
                                .text_size(px(12.5))
                                .text_color(rgb(if selected {
                                    theme().text
                                } else {
                                    theme().muted
                                }))
                                .when(selected, |item| {
                                    item.font_weight(FontWeight::MEDIUM).child(selection_bar())
                                })
                                .hover(move |s| {
                                    s.bg(rgb(if selected {
                                        theme().active
                                    } else {
                                        theme().hover
                                    }))
                                    .text_color(rgb(theme().text))
                                })
                                .child(icon(
                                    ico,
                                    if selected {
                                        theme().brand
                                    } else {
                                        theme().muted
                                    },
                                ))
                                .child(elide(label))
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    view.settings_section = section;
                                    cx.notify();
                                }))
                        },
                    )),
            )
            .child(
                div()
                    .px(px(14.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(rgb(theme().border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_family("Menlo")
                            .text_size(px(10.5))
                            .text_color(rgb(theme().subtle))
                            .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
                    )
                    .child(
                        div()
                            .id("settings-open-about")
                            .role(Role::Button)
                            .tab_index(0)
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .text_size(px(11.))
                            .text_color(rgb(theme().muted))
                            .hover(|s| s.bg(rgb(theme().hover)).text_color(rgb(theme().text)))
                            .child(tr("关于", "About"))
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.open_confirmation(Dialog::About, window, cx);
                            })),
                    ),
            );

        let body_content: AnyElement = match active_section {
            SettingsSection::Appearance => self.settings_appearance_pane(cx).into_any_element(),
            SettingsSection::General => self.settings_general_pane(cx).into_any_element(),
            SettingsSection::Workspace => self.settings_workspace_pane(cx).into_any_element(),
            SettingsSection::Shortcuts => self.settings_shortcuts_pane().into_any_element(),
        };

        let right_pane = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(rgb(theme().panel))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(56.))
                    .flex_shrink_0()
                    .px(px(22.))
                    .border_b_1()
                    .border_color(rgb(theme().border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(14.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme().text))
                                    .child(page_title),
                            )
                            .child(
                                elide(page_subtitle)
                                    .text_size(px(11.5))
                                    .text_color(rgb(theme().subtle)),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-close-x")
                            .role(Role::Button)
                            .aria_label(tr("关闭设置", "Close Settings"))
                            .tab_index(0)
                            .size(px(26.))
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(|s| s.bg(rgb(theme().hover)))
                            .child(icon(Icon::Close, theme().muted))
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.close_overlay(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id("settings-scroll-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(22.))
                    .py(px(18.))
                    .child(body_content),
            )
            .child(
                div()
                    .h(px(44.))
                    .flex_shrink_0()
                    .px(px(22.))
                    .bg(rgb(theme().sidebar_bg))
                    .border_t_1()
                    .border_color(rgb(theme().border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .child(div().size(px(6.)).rounded(px(999.)).bg(rgb(theme().brand)))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(theme().subtle))
                                    .child(tr(
                                        "所有更改实时生效并自动保存",
                                        "All changes apply immediately and save automatically",
                                    )),
                            ),
                    )
                    .child(
                        text_button("close-settings-dialog", tr("完成", "Done"), true).on_click(
                            cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                        ),
                    ),
            );

        div()
            .flex()
            .w_full()
            .h_full()
            .min_h_0()
            .child(left_rail)
            .child(right_pane)
    }

    fn settings_appearance_pane(&self, cx: &mut Context<Self>) -> Div {
        let system_mode = app_theme::detect_system_theme();
        let system_badge = match system_mode {
            ThemeMode::Light => tr("系统当前：浅色", "OS now: Light"),
            ThemeMode::Dark => tr("系统当前：深色", "OS now: Dark"),
        };
        let theme_cards = [
            (
                ThemePreference::System,
                tr("跟随系统", "Auto"),
                tr("随系统自动切换", "Match OS"),
            ),
            (
                ThemePreference::Light,
                tr("浅色主题", "Light"),
                tr("冷白石板与翡翠青绿", "Crisp slate light"),
            ),
            (
                ThemePreference::Dark,
                tr("深色主题", "Dark"),
                tr("低噪暗色与品牌青绿", "Calm slate dark"),
            ),
        ];

        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            // 1. Visual Theme Cards
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(settings_section_label(tr("外观主题", "THEME")))
                            .child(pill(
                                system_badge,
                                theme().card,
                                theme().border,
                                theme().muted,
                            )),
                    )
                    .child(div().flex().gap(px(10.)).children(
                        theme_cards.into_iter().enumerate().map(|(idx, (pref, label, desc))| {
                            let selected = self.theme_pref == pref;
                            div()
                                .id(("settings-theme", idx))
                                .role(Role::Button)
                                .aria_selected(selected)
                                .tab_index(0)
                                .flex_1()
                                .min_w_0()
                                .p(px(8.))
                                .rounded(px(9.))
                                .border_1()
                                .border_color(rgb(if selected {
                                    theme().brand
                                } else {
                                    theme().border
                                }))
                                .bg(rgb(if selected {
                                    theme().brand_bg
                                } else {
                                    theme().card
                                }))
                                .hover(move |s| {
                                    s.border_color(rgb(if selected {
                                        theme().brand
                                    } else {
                                        theme().border_strong
                                    }))
                                    .bg(rgb(if selected {
                                        theme().brand_bg
                                    } else {
                                        theme().hover
                                    }))
                                })
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .child(theme_preview_thumbnail(pref))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(6.))
                                        .px(px(2.))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .min_w_0()
                                                .child(
                                                    elide(label)
                                                        .text_size(px(12.))
                                                        .font_weight(if selected {
                                                            FontWeight::SEMIBOLD
                                                        } else {
                                                            FontWeight::MEDIUM
                                                        })
                                                        .text_color(rgb(if selected {
                                                            theme().brand_text
                                                        } else {
                                                            theme().text
                                                        })),
                                                )
                                                .child(
                                                    elide(desc)
                                                        .text_size(px(10.5))
                                                        .text_color(rgb(theme().subtle)),
                                                ),
                                        )
                                        .child(radio_dot(selected)),
                                )
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    view.set_theme(pref, window, cx);
                                }))
                        }),
                    )),
            )
            // 2. Display Scaling Card
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(settings_section_label(tr("显示与比例", "DISPLAY & SCALE")))
                    .child(
                        settings_card().child(
                            div()
                                .px(px(14.))
                                .py(px(12.))
                                .flex()
                                .flex_col()
                                .gap(px(10.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(12.))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(2.))
                                                .min_w_0()
                                                .child(
                                                    div()
                                                        .text_size(px(12.5))
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(rgb(theme().text))
                                                        .child(tr("界面缩放", "Interface Scale")),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(11.))
                                                        .text_color(rgb(theme().subtle))
                                                        .child(if cfg!(target_os = "macos") {
                                                            tr(
                                                                "文字、行高与矢量图标等比无损缩放 · 快捷键 ⌘= / ⌘- / ⌘0",
                                                                "Proportional text, rows & icons · Shortcuts ⌘= / ⌘- / ⌘0",
                                                            )
                                                        } else {
                                                            tr(
                                                                "文字、行高与矢量图标等比无损缩放 · 快捷键 Ctrl+= / Ctrl+- / Ctrl+0",
                                                                "Proportional text, rows & icons · Shortcuts Ctrl+= / Ctrl+- / Ctrl+0",
                                                            )
                                                        }),
                                                ),
                                        )
                                        .child(pill(
                                            format!("{}%", self.ui_scale),
                                            theme().brand_bg,
                                            theme().brand_border,
                                            theme().brand_text,
                                        )),
                                )
                                .child(
                                    div()
                                        .p(px(2.5))
                                        .rounded(px(7.))
                                        .bg(rgb(theme().chrome))
                                        .border_1()
                                        .border_color(rgb(theme().border))
                                        .flex()
                                        .gap(px(2.))
                                        .children(UI_SCALE_PRESETS.into_iter().enumerate().map(
                                            |(idx, preset)| {
                                                let selected = self.ui_scale == preset;
                                                div()
                                                    .id(("settings-ui-scale", idx))
                                                    .role(Role::Button)
                                                    .aria_selected(selected)
                                                    .tab_index(0)
                                                    .flex_1()
                                                    .h(px(26.))
                                                    .rounded(px(5.))
                                                    .border_1()
                                                    .border_color(rgb(if selected {
                                                        theme().border_strong
                                                    } else {
                                                        theme().chrome
                                                    }))
                                                    .bg(rgb(if selected {
                                                        theme().select_soft
                                                    } else {
                                                        theme().chrome
                                                    }))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .text_size(px(12.))
                                                    .text_color(rgb(if selected {
                                                        theme().text
                                                    } else {
                                                        theme().muted
                                                    }))
                                                    .when(selected, |s| {
                                                        s.font_weight(FontWeight::SEMIBOLD)
                                                    })
                                                    .hover(move |s| {
                                                        s.bg(rgb(if selected {
                                                            theme().select_soft
                                                        } else {
                                                            theme().hover
                                                        }))
                                                        .text_color(rgb(theme().text))
                                                    })
                                                    .child(format!("{preset}%"))
                                                    .on_click(cx.listener(
                                                        move |view, _, window, cx| {
                                                            view.set_ui_scale(preset, window, cx);
                                                        },
                                                    ))
                                            },
                                        )),
                                ),
                        ),
                    ),
            )
    }

    fn settings_general_pane(&self, cx: &mut Context<Self>) -> Div {
        let languages = [
            (LanguagePreference::System, tr("跟随系统", "System")),
            (LanguagePreference::ZhCn, "简体中文"),
            (LanguagePreference::EnUs, "English"),
        ];
        let system_detected = match i18n::detect_system_locale() {
            Locale::ZhCn => tr("当前系统语言：简体中文", "Detected OS locale: Chinese"),
            Locale::EnUs => tr("当前系统语言：英文", "Detected OS locale: English"),
        };
        let strategies = [
            (
                PullStrategy::FastForwardOnly,
                Some(tr("推荐", "Recommended")),
            ),
            (PullStrategy::Merge, None),
            (PullStrategy::Rebase, None),
        ];

        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            // 1. Language
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(settings_section_label(tr("语言与区域", "LANGUAGE")))
                    .child(
                        settings_card().child(
                            div()
                                .px(px(14.))
                                .py(px(12.))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(14.))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.))
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_size(px(12.5))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(theme().text))
                                                .child(tr("界面语言", "Display Language")),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(rgb(theme().subtle))
                                                .child(system_detected),
                                        ),
                                )
                                .child(
                                    div()
                                        .p(px(2.5))
                                        .rounded(px(7.))
                                        .bg(rgb(theme().chrome))
                                        .border_1()
                                        .border_color(rgb(theme().border))
                                        .flex()
                                        .flex_shrink_0()
                                        .gap(px(2.))
                                        .children(languages.into_iter().enumerate().map(
                                            |(idx, (pref, label))| {
                                                let selected = self.language_pref == pref;
                                                div()
                                                    .id(("settings-language", idx))
                                                    .role(Role::Button)
                                                    .aria_selected(selected)
                                                    .tab_index(0)
                                                    .h(px(25.))
                                                    .px(px(10.))
                                                    .rounded(px(5.))
                                                    .border_1()
                                                    .border_color(rgb(if selected {
                                                        theme().border_strong
                                                    } else {
                                                        theme().chrome
                                                    }))
                                                    .bg(rgb(if selected {
                                                        theme().select_soft
                                                    } else {
                                                        theme().chrome
                                                    }))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .text_size(px(11.5))
                                                    .text_color(rgb(if selected {
                                                        theme().text
                                                    } else {
                                                        theme().muted
                                                    }))
                                                    .when(selected, |s| {
                                                        s.font_weight(FontWeight::MEDIUM)
                                                    })
                                                    .hover(move |s| {
                                                        s.bg(rgb(if selected {
                                                            theme().select_soft
                                                        } else {
                                                            theme().hover
                                                        }))
                                                        .text_color(rgb(theme().text))
                                                    })
                                                    .child(label)
                                                    .on_click(cx.listener(move |view, _, _, cx| {
                                                        view.set_language(pref, cx);
                                                    }))
                                            },
                                        )),
                                ),
                        ),
                    ),
            )
            // 2. Default Pull Strategy
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(settings_section_label(tr(
                        "默认 PULL 更新策略",
                        "DEFAULT PULL STRATEGY",
                    )))
                    .child(
                        settings_card().children(strategies.into_iter().enumerate().map(
                            |(idx, (strategy, badge))| {
                                let selected = self.default_pull_strategy == strategy;
                                div()
                                    .id(("settings-pull-strategy", idx))
                                    .role(Role::Button)
                                    .aria_selected(selected)
                                    .tab_index(0)
                                    .when(idx > 0, |row| {
                                        row.border_t_1().border_color(rgb(theme().border))
                                    })
                                    .px(px(14.))
                                    .py(px(11.))
                                    .bg(rgb(if selected {
                                        theme().brand_bg
                                    } else {
                                        theme().card
                                    }))
                                    .hover(move |s| {
                                        s.bg(rgb(if selected {
                                            theme().brand_bg
                                        } else {
                                            theme().hover
                                        }))
                                    })
                                    .flex()
                                    .items_start()
                                    .gap(px(10.))
                                    .child(div().pt(px(2.)).child(radio_dot(selected)))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(7.))
                                                    .child(
                                                        div()
                                                            .text_size(px(12.5))
                                                            .font_weight(if selected {
                                                                FontWeight::SEMIBOLD
                                                            } else {
                                                                FontWeight::MEDIUM
                                                            })
                                                            .text_color(rgb(if selected {
                                                                theme().brand_text
                                                            } else {
                                                                theme().text
                                                            }))
                                                            .child(strategy.label()),
                                                    )
                                                    .when_some(badge, |row, text| {
                                                        row.child(pill(
                                                            text,
                                                            theme().panel,
                                                            theme().brand_border,
                                                            theme().brand_text,
                                                        ))
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.5))
                                                    .line_height(px(16.))
                                                    .text_color(rgb(theme().subtle))
                                                    .child(strategy.description()),
                                            ),
                                    )
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        view.default_pull_strategy = strategy;
                                        view.save_preferences();
                                        cx.notify();
                                    }))
                            },
                        )),
                    ),
            )
            // 3. Software Update
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(settings_section_label(tr("软件更新", "SOFTWARE UPDATE")))
                    .child(
                        settings_card()
                            .child(settings_toggle_row(
                                "settings-toggle-auto-updates",
                                tr("自动检查新版本", "Automatically Check for Updates"),
                                tr(
                                    "启动后在后台静默检查 GitHub Releases 新版本（每 6 小时最多一次），绝不打断工作",
                                    "Silently check GitHub Releases in the background on launch (at most once every 6 hours)",
                                ),
                                None,
                                self.auto_check_updates,
                                false,
                                cx.listener(|view, _, _, cx| {
                                    view.auto_check_updates = !view.auto_check_updates;
                                    view.save_preferences();
                                    cx.notify();
                                }),
                            ))
                            .child(
                                div()
                                    .border_t_1()
                                    .border_color(rgb(theme().border))
                                    .px(px(14.))
                                    .py(px(11.))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(12.))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.))
                                            .child(
                                                div()
                                                    .text_size(px(12.5))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(rgb(theme().text))
                                                    .child(format!(
                                                        "{} v{}",
                                                        tr("当前版本", "Current Version"),
                                                        env!("CARGO_PKG_VERSION")
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.))
                                                    .text_color(rgb(theme().subtle))
                                                    .child(tr(
                                                        "支持应用内下载、校验并一键重启更新",
                                                        "Download, verify, and restart to update in-app",
                                                    )),
                                            ),
                                    )
                                    .child(
                                        text_button(
                                            "settings-check-updates-now",
                                            tr("检查更新…", "Check Now…"),
                                            false,
                                        )
                                        .on_click(cx.listener(|view, _, window, cx| {
                                            view.open_update_dialog(window, cx);
                                        })),
                                    ),
                            ),
                    ),
            )
    }

    fn settings_workspace_pane(&self, cx: &mut Context<Self>) -> Div {
        let mod_key = if cfg!(target_os = "macos") {
            "⌘B"
        } else {
            "Ctrl+B"
        };
        let show_details = self.show_commit_details;

        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(settings_section_label(tr(
                        "面板与提交日志",
                        "PANELS & COMMIT LOG",
                    )))
                    .child(
                        settings_card()
                            .child(settings_toggle_row(
                                "settings-toggle-branches-drawer",
                                tr("侧边栏分支与标签抽屉", "Sidebar Branches & Tags Drawer"),
                                tr(
                                    "在仓库列表底部显示可拖拽高度的紧凑分支树",
                                    "Show a resizable compact branch tree below repositories",
                                ),
                                None,
                                self.show_branches_drawer,
                                false,
                                cx.listener(|view, _, _, cx| {
                                    view.show_branches_drawer = !view.show_branches_drawer;
                                    if view.show_branches_drawer
                                        && view.active_branch_snapshot().is_none()
                                        && !view.workspace.daily.branches_loading
                                    {
                                        view.dispatch_daily(
                                            githalo::workspace::DailyAction::LoadBranches,
                                            cx,
                                        );
                                    }
                                    view.save_preferences();
                                    cx.notify();
                                }),
                            ))
                            .child(settings_toggle_row(
                                "settings-toggle-branches-panel",
                                tr("Log 独立分支树分栏", "Standalone Log Branches Column"),
                                tr(
                                    "在提交图左侧展开全高分支与标签筛选分栏",
                                    "Show a full-height Branches & Tags column beside the log",
                                ),
                                Some(mod_key),
                                self.show_branches_panel,
                                true,
                                cx.listener(|view, _, _, cx| {
                                    view.toggle_branches_panel(cx);
                                }),
                            ))
                            .child(settings_toggle_row(
                                "settings-toggle-details",
                                tr("右侧提交详情与变更文件", "Commit Details & Changed Files"),
                                tr(
                                    "选中提交时在右侧展示提交元信息、文件树与内联 Diff",
                                    "Display commit metadata, changed files tree, and inline diff",
                                ),
                                None,
                                show_details,
                                true,
                                cx.listener(|view, _, _, cx| {
                                    view.show_commit_details = !view.show_commit_details;
                                    view.save_preferences();
                                    view.refresh_app_menus(cx);
                                    cx.notify();
                                }),
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(settings_section_label(tr("差异对比视图", "DIFF VIEWER")))
                    .child(settings_card().child(settings_toggle_row(
                        "settings-toggle-split-diff",
                        tr(
                            "默认左右并排对比（Side-by-Side）",
                            "Side-by-Side Diff by Default",
                        ),
                        tr(
                            "并排对比变更并启用词级行内差异高亮；关闭则使用统一视图",
                            "Compare old and new side by side with inline word highlights",
                        ),
                        None,
                        self.diff_side_by_side,
                        false,
                        cx.listener(|view, _, _, cx| {
                            view.set_diff_side_by_side(!view.diff_side_by_side, cx);
                        }),
                    ))),
            )
    }

    fn settings_shortcuts_pane(&self) -> Div {
        let m = if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl"
        };
        let shift = if cfg!(target_os = "macos") {
            "⇧"
        } else {
            "Shift"
        };

        let nav_shortcuts: Vec<(Vec<&'static str>, &'static str)> = vec![
            (
                vec![m, shift, "P"],
                tr("全局命令与仓库快速跳转面板", "Command & Repository Palette"),
            ),
            (
                vec![m, "L"],
                tr("聚焦提交日志搜索与筛选框", "Focus Commit Log Search"),
            ),
            (
                vec![m, "B"],
                tr(
                    "切换 Log 左侧独立分支分栏",
                    "Toggle Standalone Branches Column",
                ),
            ),
            (
                vec![m, "\\"],
                tr("显示或隐藏左侧主边栏", "Toggle Primary Sidebar"),
            ),
            (
                vec![m, "=", "/", "-"],
                tr(
                    "放大 / 缩小界面比例（0 重置）",
                    "Zoom Interface In / Out (0 Reset)",
                ),
            ),
            (
                vec!["Esc"],
                tr("关闭当前弹窗或上下文菜单", "Dismiss Dialog or Menu"),
            ),
        ];

        let git_shortcuts: Vec<(Vec<&'static str>, &'static str)> = vec![
            (
                vec![m, "K"],
                tr("切换到 Commit 工作区变更面板", "Open Commit Changes Panel"),
            ),
            (
                vec![m, "T"],
                tr("打开当前分支 Pull 拉取弹窗", "Pull Current Branch"),
            ),
            (
                vec![m, shift, "K"],
                tr("打开当前分支 Push 推送弹窗", "Push Current Branch"),
            ),
            (
                vec![m, shift, "T"],
                tr("获取远端更新（Fetch）", "Fetch Remote Updates"),
            ),
            (
                vec!["F2"],
                tr("在分支树中重命名选中分支", "Rename Selected Branch"),
            ),
            (
                vec!["⌫"],
                tr("在分支树中删除选中分支", "Delete Selected Branch"),
            ),
        ];

        let render_group = |title: &'static str, rows: Vec<(Vec<&'static str>, &'static str)>| {
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(settings_section_label(title))
                .child(settings_card().children(rows.into_iter().enumerate().map(
                    |(idx, (keys, desc))| {
                        div()
                            .when(idx > 0, |row| {
                                row.border_t_1().border_color(rgb(theme().border))
                            })
                            .px(px(14.))
                            .py(px(8.5))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.))
                            .child(
                                elide(desc)
                                    .flex_1()
                                    .text_size(px(12.))
                                    .text_color(rgb(theme().text)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(3.))
                                    .flex_shrink_0()
                                    .children(keys.into_iter().map(|k| {
                                        if k == "/" {
                                            div()
                                                .px(px(2.))
                                                .text_size(px(10.5))
                                                .text_color(rgb(theme().subtle))
                                                .child("/")
                                        } else {
                                            keycap(k)
                                        }
                                    })),
                            )
                    },
                )))
        };

        div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(render_group(
                tr("导航与视图", "NAVIGATION & VIEW"),
                nav_shortcuts,
            ))
            .child(render_group(
                tr("GIT 与分支操作", "GIT & BRANCHES"),
                git_shortcuts,
            ))
    }
}

fn settings_section_label(title: &'static str) -> Div {
    div()
        .px(px(2.))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(theme().muted))
        .child(title)
}

fn settings_card() -> Div {
    div()
        .rounded(px(8.))
        .border_1()
        .border_color(rgb(theme().border))
        .bg(rgb(theme().card))
        .overflow_hidden()
        .flex()
        .flex_col()
}

fn radio_dot(selected: bool) -> Div {
    div()
        .size(px(15.))
        .flex_shrink_0()
        .rounded(px(999.))
        .border_1()
        .border_color(rgb(if selected {
            theme().brand
        } else {
            theme().border_strong
        }))
        .bg(rgb(if selected {
            theme().brand
        } else {
            theme().panel
        }))
        .flex()
        .items_center()
        .justify_center()
        .when(selected, |dot| {
            dot.child(
                div()
                    .size(px(5.5))
                    .rounded(px(999.))
                    .bg(rgb(theme().primary_text)),
            )
        })
}

fn toggle_switch(checked: bool) -> Div {
    let t = theme();
    div()
        .w(px(34.))
        .h(px(19.))
        .flex_shrink_0()
        .rounded(px(999.))
        .p(px(2.))
        .bg(rgb(if checked {
            t.primary_bg
        } else {
            t.border_strong
        }))
        .flex()
        .items_center()
        .when(checked, |s| s.justify_end())
        .when(!checked, |s| s.justify_start())
        .child(div().size(px(15.)).rounded(px(999.)).bg(rgb(if checked {
            t.primary_text
        } else {
            t.panel
        })))
}

fn keycap(label: &'static str) -> Div {
    div()
        .min_w(px(21.))
        .h(px(20.))
        .px(px(5.5))
        .rounded(px(4.))
        .bg(rgb(theme().panel))
        .border_1()
        .border_color(rgb(theme().border_strong))
        .flex()
        .items_center()
        .justify_center()
        .font_family("Menlo")
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(theme().text))
        .child(label)
}

fn settings_toggle_row(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    shortcut: Option<&'static str>,
    checked: bool,
    with_top_divider: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .role(Role::CheckBox)
        .aria_label(title)
        .aria_toggled(if checked {
            Toggled::True
        } else {
            Toggled::False
        })
        .tab_index(0)
        .when(with_top_divider, |row| {
            row.border_t_1().border_color(rgb(theme().border))
        })
        .px(px(14.))
        .py(px(11.))
        .hover(|s| s.bg(rgb(theme().hover)))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(14.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(theme().text))
                                .child(title),
                        )
                        .when_some(shortcut, |row, sc| row.child(keycap(sc))),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .line_height(px(15.5))
                        .text_color(rgb(theme().subtle))
                        .child(description),
                ),
        )
        .child(toggle_switch(checked))
        .on_click(on_click)
}

/// 精致的迷你 GitHalo 窗口缩略图，用于「外观主题」三张预览卡（跟随系统采用左右浅/深双拼）。
fn theme_preview_thumbnail(pref: ThemePreference) -> Div {
    let mini_window = |p: &ThemePalette| {
        div()
            .w_full()
            .h_full()
            .bg(rgb(p.panel))
            .flex()
            .flex_col()
            // Mini titlebar with macOS traffic lights
            .child(
                div()
                    .h(px(14.))
                    .px(px(6.))
                    .bg(rgb(p.chrome))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(3.))
                            .child(div().size(px(4.5)).rounded(px(999.)).bg(rgb(0xff5f57)))
                            .child(div().size(px(4.5)).rounded(px(999.)).bg(rgb(0xfebc2e)))
                            .child(div().size(px(4.5)).rounded(px(999.)).bg(rgb(0x28c840))),
                    )
                    .child(
                        div()
                            .w(px(22.))
                            .h(px(5.))
                            .rounded(px(2.))
                            .bg(rgb(p.border_strong)),
                    ),
            )
            // Mini body: sidebar + commit graph
            .child(
                div()
                    .flex_1()
                    .flex()
                    .min_h_0()
                    .child(
                        div()
                            .w(px(36.))
                            .h_full()
                            .bg(rgb(p.sidebar_bg))
                            .border_r_1()
                            .border_color(rgb(p.border))
                            .p(px(5.))
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .h(px(4.))
                                    .w(px(22.))
                                    .rounded(px(2.))
                                    .bg(rgb(p.border_strong)),
                            )
                            .child(
                                div()
                                    .h(px(6.))
                                    .w_full()
                                    .rounded(px(2.))
                                    .bg(rgb(p.active))
                                    .border_l_2()
                                    .border_color(rgb(p.brand)),
                            )
                            .child(div().h(px(4.)).w(px(24.)).rounded(px(2.)).bg(rgb(p.border))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .p(px(6.))
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .child(div().size(px(5.)).rounded(px(999.)).bg(rgb(p.brand)))
                                    .child(
                                        div().h(px(4.5)).w(px(44.)).rounded(px(2.)).bg(rgb(p.text)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .child(div().size(px(5.)).rounded(px(999.)).bg(rgb(p.link)))
                                    .child(
                                        div().h(px(4.)).w(px(54.)).rounded(px(2.)).bg(rgb(p.muted)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .child(
                                        div().size(px(5.)).rounded(px(999.)).bg(rgb(p.remote_text)),
                                    )
                                    .child(
                                        div()
                                            .h(px(4.))
                                            .w(px(36.))
                                            .rounded(px(2.))
                                            .bg(rgb(p.border_strong)),
                                    ),
                            ),
                    ),
            )
    };

    let frame = div()
        .h(px(66.))
        .w_full()
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(theme().border_strong))
        .overflow_hidden();

    match pref {
        ThemePreference::Light => frame.child(mini_window(&LIGHT_PALETTE)),
        ThemePreference::Dark => frame.child(mini_window(&DARK_PALETTE)),
        ThemePreference::System => frame
            .flex()
            .child(
                div()
                    .w_1_2()
                    .h_full()
                    .overflow_hidden()
                    .border_r_1()
                    .border_color(rgb(theme().border_strong))
                    .child(mini_window(&LIGHT_PALETTE)),
            )
            .child(
                div().w_1_2().h_full().overflow_hidden().child(
                    div()
                        .w_full()
                        .h_full()
                        .bg(rgb(DARK_PALETTE.panel))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .h(px(14.))
                                .px(px(6.))
                                .bg(rgb(DARK_PALETTE.chrome))
                                .border_b_1()
                                .border_color(rgb(DARK_PALETTE.border))
                                .flex()
                                .items_center()
                                .justify_end()
                                .child(
                                    div()
                                        .w(px(22.))
                                        .h(px(5.))
                                        .rounded(px(2.))
                                        .bg(rgb(DARK_PALETTE.border_strong)),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .p(px(6.))
                                .flex()
                                .flex_col()
                                .gap(px(5.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.))
                                        .child(
                                            div()
                                                .size(px(5.))
                                                .rounded(px(999.))
                                                .bg(rgb(DARK_PALETTE.brand)),
                                        )
                                        .child(
                                            div()
                                                .h(px(4.5))
                                                .w(px(36.))
                                                .rounded(px(2.))
                                                .bg(rgb(DARK_PALETTE.text)),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.))
                                        .child(
                                            div()
                                                .size(px(5.))
                                                .rounded(px(999.))
                                                .bg(rgb(DARK_PALETTE.link)),
                                        )
                                        .child(
                                            div()
                                                .h(px(4.))
                                                .w(px(44.))
                                                .rounded(px(2.))
                                                .bg(rgb(DARK_PALETTE.muted)),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.))
                                        .child(
                                            div()
                                                .size(px(5.))
                                                .rounded(px(999.))
                                                .bg(rgb(DARK_PALETTE.remote_text)),
                                        )
                                        .child(
                                            div()
                                                .h(px(4.))
                                                .w(px(28.))
                                                .rounded(px(2.))
                                                .bg(rgb(DARK_PALETTE.border_strong)),
                                        ),
                                ),
                        ),
                ),
            ),
    }
}
