use std::{path::PathBuf, sync::Arc};

use super::{Event, Panel, Workspace, daily::Effect};
use crate::{
    shelf::{self, ShelfEntry, ShelfFile, ShelfSnapshot, UnshelveOutcome},
    worktree::WorktreeTarget,
};

#[derive(Clone, Debug)]
pub enum ShelfAction {
    Reload,
    SelectEntry {
        snapshot: Arc<ShelfSnapshot>,
        entry: ShelfEntry,
    },
    SelectFile {
        entry: ShelfEntry,
        file: ShelfFile,
    },
    CloseDiff,
    Shelve {
        target: WorktreeTarget,
        name: String,
        paths: Vec<PathBuf>,
        keep_local: bool,
    },
    Unshelve {
        snapshot: Arc<ShelfSnapshot>,
        entry: ShelfEntry,
        keep_entry: bool,
    },
    Delete {
        snapshot: Arc<ShelfSnapshot>,
        entry: ShelfEntry,
    },
}

#[derive(Default)]
pub struct ShelfState {
    pub snapshot: Option<Arc<ShelfSnapshot>>,
    pub selected: Option<ShelfEntry>,
    pub selected_file: Option<ShelfFile>,
    pub diff: Option<Arc<str>>,
    pub loading: bool,
    pub diff_loading: bool,
    pub error: Option<String>,
    list_request: u64,
    diff_request: u64,
}

impl ShelfState {
    pub(super) fn suspend_reads(&mut self) {
        self.list_request = 0;
        self.diff_request = 0;
        self.loading = false;
        self.diff_loading = false;
    }
}

pub(super) enum ShelfEvent {
    List(u64, Result<ShelfSnapshot, String>),
    Diff(u64, Result<String, String>),
}

fn event(e: ShelfEvent) -> Event {
    Event::Shelf(Box::new(e))
}

impl Workspace {
    pub(super) fn load_shelves(&mut self) {
        if self.panel != Panel::Shelf {
            return;
        }
        let Some(target) = self
            .current_target()
            .filter(|target| !self.writing(target.id))
        else {
            return;
        };
        let request = self.next_id();
        self.shelves.list_request = request;
        self.shelves.loading = true;
        self.runtime
            .detail
            .run(move || event(ShelfEvent::List(request, shelf::list_shelves(&target))));
    }

    fn current_shelf_snapshot(&self, snapshot: &Arc<ShelfSnapshot>) -> bool {
        self.panel == Panel::Shelf
            && self.current_target().as_ref() == Some(snapshot.worktree())
            && self
                .shelves
                .snapshot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, snapshot))
    }

    fn close_shelf_diff(&mut self) {
        self.shelves.diff_request = self.next_id();
        self.shelves.diff_loading = false;
        self.shelves.selected_file = None;
        self.shelves.diff = None;
    }

    fn open_shelf_file(&mut self, entry: ShelfEntry, file: ShelfFile) {
        let Some(target) = self.current_target() else {
            return;
        };
        if self.panel != Panel::Shelf
            || self.shelves.selected.as_ref() != Some(&entry)
            || !entry.files.contains(&file)
        {
            return;
        }
        self.close_shelf_diff();
        self.shelves.selected_file = Some(file.clone());
        self.shelves.diff_loading = true;
        self.shelves.error = None;
        let request = self.shelves.diff_request;
        self.runtime.detail.run(move || {
            event(ShelfEvent::Diff(
                request,
                shelf::file_diff(&target, &entry, &file),
            ))
        });
    }

    pub(super) fn dispatch_shelf(&mut self, action: ShelfAction) {
        match action {
            ShelfAction::Reload => self.load_shelves(),
            ShelfAction::CloseDiff => self.close_shelf_diff(),
            ShelfAction::SelectEntry { snapshot, entry } => {
                if self.current_shelf_snapshot(&snapshot) && snapshot.entries().contains(&entry) {
                    self.close_shelf_diff();
                    self.shelves.selected = Some(entry);
                }
            }
            ShelfAction::SelectFile { entry, file } => self.open_shelf_file(entry, file),
            ShelfAction::Shelve {
                target,
                name,
                paths,
                keep_local,
            } => {
                self.write(target.clone(), move || {
                    let entry = shelf::shelve_files(&target, &name, &paths, keep_local)?;
                    Ok(Effect::Changed(format!(
                        "已搁置 {} 个文件到 Shelf（{}）",
                        entry.files.len(),
                        entry.name
                    )))
                });
            }
            ShelfAction::Unshelve {
                snapshot,
                entry,
                keep_entry,
            } => {
                if self.shelves.loading || !self.current_shelf_snapshot(&snapshot) {
                    self.daily_error("工作树或 Shelf 列表已变化，请刷新后再试");
                    return;
                }
                let name = entry.name.clone();
                self.write(snapshot.worktree().clone(), move || {
                    let UnshelveOutcome::Restored { kept } =
                        shelf::unshelve(&snapshot, &entry, keep_entry)?;
                    let tail = if kept {
                        "（原条目已保留）"
                    } else {
                        "（已移出 Shelf）"
                    };
                    Ok(Effect::Changed(format!("已恢复 Shelf「{name}」{tail}")))
                });
            }
            ShelfAction::Delete { snapshot, entry } => {
                if self.shelves.loading || !self.current_shelf_snapshot(&snapshot) {
                    self.daily_error("工作树或 Shelf 列表已变化，请刷新后再试");
                    return;
                }
                let name = entry.name.clone();
                self.write(snapshot.worktree().clone(), move || {
                    shelf::delete_shelf(&snapshot, &entry)?;
                    Ok(Effect::Changed(format!("已删除 Shelf「{name}」")))
                });
            }
        }
    }

    pub(super) fn apply_shelf(&mut self, event: ShelfEvent) {
        match event {
            ShelfEvent::List(serial, result) if serial == self.shelves.list_request => {
                self.shelves.loading = false;
                let snapshot = match result {
                    Ok(s) => s,
                    Err(err) => {
                        self.shelves.snapshot = None;
                        self.shelves.selected = None;
                        self.close_shelf_diff();
                        self.shelves.error = Some(err);
                        return;
                    }
                };
                self.shelves.error = None;
                let prev_id = self.shelves.selected.as_ref().map(|e| e.id.clone());
                let selected = prev_id
                    .and_then(|id| snapshot.entries().iter().find(|e| e.id == id))
                    .or_else(|| snapshot.entries().first())
                    .cloned();
                if self.shelves.selected != selected {
                    self.close_shelf_diff();
                    self.shelves.selected = selected;
                }
                self.shelves.snapshot = Some(Arc::new(snapshot));
            }
            ShelfEvent::Diff(serial, result) if serial == self.shelves.diff_request => {
                self.shelves.diff_loading = false;
                match result {
                    Ok(diff) => self.shelves.diff = Some(diff.into()),
                    Err(err) => {
                        self.shelves.diff = None;
                        self.shelves.error = Some(err);
                    }
                }
            }
            _ => {}
        }
    }
}
