use std::sync::Arc;

use super::{Event, Panel, Workspace, daily::Effect};
use crate::stash::{
    self, StashEntry, StashFile, StashRestoreMode, StashRestoreOutcome, StashSaveOptions,
    StashSaveOutcome, StashSnapshot,
};

#[derive(Clone, Debug)]
pub enum StashAction {
    Reload,
    SelectEntry {
        snapshot: Arc<StashSnapshot>,
        entry: StashEntry,
    },
    SelectFile {
        entry: StashEntry,
        file: StashFile,
    },
    CloseDiff,
    Save {
        snapshot: Arc<StashSnapshot>,
        options: StashSaveOptions,
    },
    Restore {
        snapshot: Arc<StashSnapshot>,
        entry: StashEntry,
        mode: StashRestoreMode,
    },
    Drop {
        snapshot: Arc<StashSnapshot>,
        entry: StashEntry,
    },
}

#[derive(Default)]
pub struct StashState {
    pub snapshot: Option<Arc<StashSnapshot>>,
    pub selected: Option<StashEntry>,
    pub files: Vec<StashFile>,
    pub selected_file: Option<StashFile>,
    pub diff: Option<Arc<str>>,
    pub loading: bool,
    pub files_loading: bool,
    pub diff_loading: bool,
    pub error: Option<String>,
    list_request: u64,
    files_request: u64,
    diff_request: u64,
}

impl StashState {
    pub(super) fn suspend_reads(&mut self) {
        self.files_request = 0;
        self.diff_request = 0;
        self.files_loading = false;
        self.diff_loading = false;
    }
}

pub(super) enum StashEvent {
    List(u64, Result<StashSnapshot, String>),
    Files(u64, Result<Vec<StashFile>, String>),
    Diff(u64, Result<String, String>),
}

impl Workspace {
    pub(super) fn load_stashes(&mut self) {
        let Some(target) = self
            .current_target()
            .filter(|target| !self.writing(target.id))
        else {
            return;
        };
        let request = self.next_id();
        self.stashes.list_request = request;
        self.stashes.loading = true;
        self.runtime.detail.run(move || {
            Event::Stash(Box::new(StashEvent::List(
                request,
                stash::read_stashes(&target),
            )))
        });
    }

    fn current_stash_snapshot(&self, snapshot: &Arc<StashSnapshot>) -> bool {
        self.panel == Panel::Stash
            && self.current_target().as_ref() == Some(snapshot.worktree())
            && self
                .stashes
                .snapshot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, snapshot))
    }

    fn stash_action_available(
        &mut self,
        snapshot: &Arc<StashSnapshot>,
        entry: Option<&StashEntry>,
    ) -> bool {
        if self.stashes.loading
            || !self.current_stash_snapshot(snapshot)
            || entry.is_some_and(|e| {
                self.stashes.selected.as_ref() != Some(e) || !snapshot.entries().contains(e)
            })
        {
            self.daily_error("工作树或 Stash 列表已变化，请重新核对条目");
            return false;
        }
        true
    }

    fn close_stash_diff(&mut self) {
        self.stashes.diff_request = self.next_id();
        self.stashes.diff_loading = false;
        self.stashes.selected_file = None;
        self.stashes.diff = None;
    }

    fn read_stash_files(&mut self, entry: Option<StashEntry>, keep_file: bool) {
        self.stashes.files_request = self.next_id();
        let request = self.stashes.files_request;
        let selected_file = self.stashes.selected_file.take().filter(|_| keep_file);
        self.close_stash_diff();
        self.stashes.selected_file = selected_file;
        self.stashes.files.clear();
        self.stashes.files_loading = false;
        self.stashes.selected = entry.clone();
        let (Some(entry), Some(target)) = (entry, self.current_target()) else {
            return;
        };
        self.stashes.files_loading = true;
        self.stashes.error = None;
        self.runtime.detail.run(move || {
            Event::Stash(Box::new(StashEvent::Files(
                request,
                stash::files(&target, &entry),
            )))
        });
    }

    fn open_stash_file(&mut self, entry: StashEntry, file: StashFile) {
        let Some(target) = self.current_target() else {
            return;
        };
        if self.panel != Panel::Stash
            || self.stashes.selected.as_ref() != Some(&entry)
            || !self.stashes.files.contains(&file)
        {
            return;
        }
        self.close_stash_diff();
        self.stashes.selected_file = Some(file.clone());
        self.stashes.diff_loading = true;
        self.stashes.error = None;
        let request = self.stashes.diff_request;
        self.runtime.detail.run(move || {
            Event::Stash(Box::new(StashEvent::Diff(
                request,
                stash::diff(&target, &entry, &file),
            )))
        });
    }

    pub(super) fn dispatch_stash(&mut self, action: StashAction) {
        match action {
            StashAction::Reload => self.load_stashes(),
            StashAction::CloseDiff => self.close_stash_diff(),
            StashAction::SelectEntry { snapshot, entry } => {
                if self.current_stash_snapshot(&snapshot) && snapshot.entries().contains(&entry) {
                    self.read_stash_files(Some(entry), false);
                }
            }
            StashAction::SelectFile { entry, file } => self.open_stash_file(entry, file),
            StashAction::Save { snapshot, options } => {
                if !self.stash_action_available(&snapshot, None) {
                    return;
                }
                self.write(snapshot.worktree().clone(), move || {
                    let message = match stash::save_stash(&snapshot, &options)? {
                        StashSaveOutcome::Saved { oid } => {
                            format!("已保存 Stash（{}）", short_oid(&oid))
                        }
                        StashSaveOutcome::NoChanges => "没有可保存的改动".into(),
                    };
                    Ok(Effect::Changed(message))
                });
            }
            StashAction::Restore {
                snapshot,
                entry,
                mode,
            } => {
                if !self.stash_action_available(&snapshot, Some(&entry)) {
                    return;
                }
                self.write(snapshot.worktree().clone(), move || {
                    let text = match stash::restore_stash(&snapshot, &entry, &mode)? {
                        StashRestoreOutcome::Applied => "已应用改动，原 Stash 已保留".into(),
                        StashRestoreOutcome::Popped => "已恢复改动并移除所选 Stash".into(),
                        StashRestoreOutcome::Branched { name } => {
                            format!("已在分支 {name} 恢复改动")
                        }
                    };
                    Ok(Effect::Changed(text))
                });
            }
            StashAction::Drop { snapshot, entry } => {
                if !self.stash_action_available(&snapshot, Some(&entry)) {
                    return;
                }
                self.write(snapshot.worktree().clone(), move || {
                    stash::drop_stash(&snapshot, &entry)?;
                    Ok(Effect::Changed(format!(
                        "已删除 Stash（{}）",
                        short_oid(&entry.oid)
                    )))
                });
            }
        }
    }

    pub(super) fn apply_stash(&mut self, event: StashEvent) {
        match event {
            StashEvent::List(serial, result) if serial == self.stashes.list_request => {
                self.stashes.loading = false;
                let snapshot = match result {
                    Ok(snapshot) => snapshot,
                    Err(err) => {
                        self.stashes.snapshot = None;
                        self.read_stash_files(None, false);
                        if self.panel == Panel::Stash {
                            self.stashes.error = Some(err);
                        }
                        return;
                    }
                };
                self.stashes.error = None;
                if self.stashes.snapshot.as_deref() == Some(&snapshot) {
                    if self.panel == Panel::Stash
                        && let Some(entry) = self.stashes.selected.clone()
                    {
                        if self.stashes.files.is_empty() && !self.stashes.files_loading {
                            self.read_stash_files(Some(entry), true);
                        } else if self.stashes.diff.is_none()
                            && !self.stashes.diff_loading
                            && let Some(file) = self.stashes.selected_file.clone()
                        {
                            self.open_stash_file(entry, file);
                        }
                    }
                    return;
                }
                let old = self.stashes.selected.as_ref();
                let entries = snapshot.entries();
                let keep = old.and_then(|old| {
                    let mut matching = entries.iter().filter(|e| e.same_log_entry(old));
                    let first = matching.next();
                    if matching.next().is_some() {
                        None
                    } else {
                        first
                    }
                });
                let keep_file = keep.is_some();
                let selected = keep.or_else(|| entries.first()).cloned();
                self.stashes.snapshot = Some(Arc::new(snapshot));
                if self.panel == Panel::Stash {
                    self.read_stash_files(selected, keep_file);
                } else {
                    self.stashes.selected = selected;
                    self.stashes.files.clear();
                    self.close_stash_diff();
                }
            }
            StashEvent::Files(serial, result) if serial == self.stashes.files_request => {
                self.stashes.files_loading = false;
                match result {
                    Ok(files) => {
                        self.stashes.files = files;
                        if let Some(file) = self.stashes.selected_file.clone() {
                            if self.stashes.files.contains(&file)
                                && let Some(entry) = self.stashes.selected.clone()
                            {
                                self.open_stash_file(entry, file);
                            } else {
                                self.close_stash_diff();
                            }
                        }
                    }
                    Err(err) => {
                        self.close_stash_diff();
                        self.stashes.files.clear();
                        self.stashes.error = Some(err);
                    }
                }
            }
            StashEvent::Diff(serial, result) if serial == self.stashes.diff_request => {
                self.stashes.diff_loading = false;
                match result {
                    Ok(diff) => self.stashes.diff = Some(diff.into()),
                    Err(err) => {
                        self.stashes.diff = None;
                        self.stashes.error = Some(err);
                    }
                }
            }
            _ => {}
        }
    }
}

fn short_oid(oid: &str) -> &str {
    oid.get(..8).unwrap_or(oid)
}
