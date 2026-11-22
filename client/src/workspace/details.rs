use super::*;
use crate::{history, project_files};

impl Workspace {
    pub(super) fn select(&mut self, id: Option<WorktreeId>, force: bool) {
        if id.is_some_and(|key| self.repo(key).is_none()) || (!force && self.selected == id) {
            return;
        }
        self.selected = id;
        self.history_request = self.next_id();
        self.changes_request = self.next_id();
        self.close_diff();
        self.selected_commit = None;
        self.commits.clear();
        self.history_limit = history::RECENT_LIMIT;
        self.changes.clear();
        self.history_loading = false;
        self.changes_loading = false;
        self.expanded_dirs.clear();
        self.dir_requests.clear();
        self.dirs.clear();
        self.tree.clear();
        self.tree_loading = false;
        self.message = None;
        self.reset_daily();
        self.stashes = StashState::default();
        self.cherry_pick = CherryPickState::default();
        self.repo_ops = RepoOpsState::default();
        self.shelves = ShelfState::default();
        if id.is_some() {
            self.load_history();
            self.load_cherry_pick_state();
            self.load_repo_op_states();
            if self.panel == Panel::ProjectFiles {
                self.load_dir(PathBuf::new());
            }
            if self.panel == Panel::Commit {
                self.load_working_changes();
            }
            if self.panel == Panel::Stash {
                self.load_stashes();
            }
            if self.panel == Panel::Shelf {
                self.load_shelves();
            }
        }
    }

    pub(super) fn load_history(&mut self) {
        let Some(target) = self.current_target() else {
            return;
        };
        let tag = self.next_id();
        let limit = self.history_limit;
        self.history_request = tag;
        self.history_loading = true;
        self.runtime
            .detail
            .run(move || Event::History(tag, history::recent_with_limit(&target.root, limit)));
    }

    pub(super) fn apply_history(&mut self, result: Result<Vec<Commit>, String>) {
        self.history_loading = false;
        let commits = result.unwrap_or_else(|e| {
            self.message = Some(format!("无法读取提交历史（{e}）"));
            vec![]
        });
        let chosen = self
            .selected_commit
            .as_ref()
            .filter(|oid| commits.iter().any(|c| &c.oid == *oid))
            .cloned()
            .or_else(|| commits.first().map(|c| c.oid.clone()));
        self.commits = commits;
        if let Some(oid) = chosen {
            if self.selected_commit.as_ref() != Some(&oid)
                || (self.changes.is_empty() && !self.changes_loading)
            {
                self.select_commit(&oid);
            }
        } else {
            self.selected_commit = None;
            self.changes_request = self.next_id();
            self.changes_loading = false;
            self.changes.clear();
            self.close_diff();
        }
    }

    pub(super) fn select_commit(&mut self, oid: &str) {
        let Some(root) = self.current_target().map(|t| t.root) else {
            return;
        };
        if !self.commits.iter().any(|c| c.oid == oid) {
            return;
        }
        self.close_diff();
        self.changes.clear();
        self.selected_commit = Some(oid.into());
        self.message = None;
        let tag = self.next_id();
        self.changes_request = tag;
        self.changes_loading = true;
        let oid = oid.to_owned();
        self.runtime
            .detail
            .run(move || Event::Changes(tag, history::changed_files(&root, &oid)));
    }

    pub(super) fn apply_changes(&mut self, result: Result<Vec<ChangedFile>, String>) {
        self.changes_loading = false;
        match result {
            Ok(changed) => self.changes = changed,
            Err(e) => {
                self.changes.clear();
                self.message = Some(format!("无法读取本次提交的变更（{e}）"));
            }
        }
    }

    pub(super) fn select_file(&mut self, path: &Path) {
        let (Some(target), Some(oid), Some(file)) = (
            self.current_target(),
            self.selected_commit.clone(),
            self.changes.iter().find(|f| f.path == path).cloned(),
        ) else {
            return;
        };
        self.selected_file = Some(path.to_owned());
        self.diff = None;
        self.diff_loading = true;
        let tag = self.next_id();
        self.patch_request = tag;
        self.runtime
            .detail
            .run(move || Event::Patch(tag, history::file_diff(&target.root, &oid, &file)));
    }

    pub(super) fn close_diff(&mut self) {
        self.patch_request = self.next_id();
        self.selected_file = None;
        self.diff = None;
        self.diff_loading = false;
    }

    pub(super) fn load_dir(&mut self, relative: PathBuf) {
        let Some(target) = self.current_target() else {
            return;
        };
        if self.dir_requests.contains_key(&relative) {
            return;
        }
        let tag = self.next_id();
        self.dir_requests.insert(relative.clone(), tag);
        self.tree_loading = true;
        self.runtime.detail.run(move || {
            let result = project_files::children(&target.root, &relative);
            Event::Directory(tag, relative, result)
        });
    }

    pub(super) fn reload_tree(&mut self) {
        self.dir_requests.clear();
        self.dirs.clear();
        self.tree.clear();
        self.tree_loading = false;
        self.load_dir(PathBuf::new());
    }

    pub(super) fn toggle_dir(&mut self, path: &Path) {
        if !self.tree.iter().any(|r| r.is_dir && r.path == path) {
            return;
        }
        if !self.expanded_dirs.remove(path) {
            self.expanded_dirs.insert(path.into());
            self.load_dir(path.to_owned());
        } else {
            self.dir_requests.retain(|p, _| !p.starts_with(path));
            self.dirs.retain(|p, _| !p.starts_with(path));
            self.tree_loading = !self.dir_requests.is_empty();
        }
        self.rebuild_tree();
    }

    pub(super) fn apply_directory(
        &mut self,
        path: PathBuf,
        result: Result<Vec<ProjectEntry>, String>,
    ) {
        self.dir_requests.remove(&path);
        self.tree_loading = !self.dir_requests.is_empty();
        match result {
            Ok(entries) => {
                let next: Vec<_> = entries
                    .iter()
                    .filter(|e| e.is_dir && self.expanded_dirs.contains(&e.path))
                    .map(|e| e.path.clone())
                    .collect();
                self.dirs.insert(path, entries);
                for dir in next {
                    if !self.dirs.contains_key(&dir) {
                        self.load_dir(dir);
                    }
                }
            }
            Err(e) => {
                self.expanded_dirs.remove(&path);
                self.message = Some(e);
            }
        }
        self.rebuild_tree();
    }

    fn rebuild_tree(&mut self) {
        self.tree.clear();
        let Some(roots) = self.dirs.get(Path::new("")) else {
            return;
        };
        let mut stack: Vec<_> = roots.iter().rev().collect();
        while let Some(entry) = stack.pop() {
            let mut row = entry.clone();
            row.expanded = row.is_dir && self.expanded_dirs.contains(&row.path);
            if row.expanded
                && let Some(children) = self.dirs.get(&row.path)
            {
                stack.extend(children.iter().rev());
            }
            self.tree.push(row);
        }
    }
}
