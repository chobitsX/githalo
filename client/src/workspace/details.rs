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
        self.branch_filter = None;
        self.history_limit = history::RECENT_LIMIT;
        self.changes.clear();
        self.history_loading = false;
        self.changes_loading = false;
        self.expanded_dirs.clear();
        self.dir_requests.clear();
        self.project_status_request = self.next_id();
        self.project_file_requests.clear();
        self.project_blame_requests.clear();
        self.project_gutter_requests.clear();
        self.dirs.clear();
        self.tree.clear();
        self.project_git_status.clear();
        self.open_project_files.clear();
        self.active_project_file = None;
        self.tree_loading = false;
        self.message = None;
        self.reset_daily();
        self.stashes = StashState::default();
        self.cherry_pick = CherryPickState::default();
        self.repo_ops = RepoOpsState::default();
        self.shelves = ShelfState::default();
        if id.is_some() {
            self.load_history();
            self.load_branches();
            self.load_cherry_pick_state();
            self.load_repo_op_states();
            if self.panel == Panel::ProjectFiles {
                self.load_dir(PathBuf::new());
                self.load_project_git_status();
            }
            self.load_working_changes();
            self.load_stashes();
            self.load_shelves();
        }
    }

    pub(super) fn load_history(&mut self) {
        let Some(target) = self.current_target() else {
            return;
        };
        let tag = self.next_id();
        let limit = self.history_limit;
        let filter = self.branch_filter.clone();
        self.history_request = tag;
        self.history_loading = true;
        self.runtime.detail.run(move || {
            Event::History(
                tag,
                history::recent_for_branch(&target.root, limit, filter.as_deref()),
            )
        });
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
        let same_commit = self.selected_commit.as_deref() == Some(oid);
        if !same_commit {
            self.close_diff();
            self.changes.clear();
        }
        self.selected_commit = Some(oid.into());
        self.message = None;
        let tag = self.next_id();
        self.changes_request = tag;
        self.changes_loading = !same_commit || self.changes.is_empty();
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
        self.load_project_git_status();
    }

    /// Re-read the root and currently expanded directories in place without clearing the visible tree.
    pub(super) fn refresh_tree_silently(&mut self) {
        if self.dirs.is_empty() {
            self.load_dir(PathBuf::new());
            self.load_project_git_status();
            return;
        }
        let mut to_reload: Vec<PathBuf> = vec![PathBuf::new()];
        to_reload.extend(self.expanded_dirs.iter().cloned());
        for rel in to_reload {
            self.dir_requests.remove(&rel);
            self.load_dir(rel);
        }
        self.load_project_git_status();
    }

    pub(super) fn load_project_git_status(&mut self) {
        let Some(target) = self.current_target() else {
            return;
        };
        let tag = self.next_id();
        self.project_status_request = tag;
        self.runtime
            .detail
            .run(move || Event::ProjectGitStatus(tag, project_files::git_status_map(&target.root)));
    }

    pub(super) fn collapse_all_dirs(&mut self) {
        self.expanded_dirs.clear();
        self.dir_requests.retain(|p, _| p.as_os_str().is_empty());
        self.dirs.retain(|p, _| p.as_os_str().is_empty());
        self.tree_loading = !self.dir_requests.is_empty();
        self.rebuild_tree();
    }

    pub(super) fn open_project_file(&mut self, path: PathBuf, force_reload: bool) {
        let Some(target) = self.current_target() else {
            return;
        };
        self.active_project_file = Some(path.clone());
        let already_loaded = self
            .open_project_files
            .iter()
            .find(|f| f.path == path)
            .is_some_and(|f| !f.loading && f.content.is_some() && f.error.is_none());
        if already_loaded && !force_reload {
            return;
        }
        if let Some(entry) = self.open_project_files.iter_mut().find(|f| f.path == path) {
            entry.loading = true;
            entry.error = None;
        } else {
            self.open_project_files.push(OpenProjectFile {
                path: path.clone(),
                loading: true,
                content: None,
                error: None,
                revision: 0,
                gutter_diff: GutterDiffMap::default(),
                inline_blame: None,
                inline_blame_loading: false,
            });
        }
        let request = self.next_id();
        self.project_file_requests.insert(path.clone(), request);
        self.runtime.detail.run(move || {
            let result = project_files::read_file(&target.root, &path);
            let gutter = project_files::line_diff_markers(&target.root, &path).unwrap_or_default();
            Event::ProjectFileLoaded {
                request,
                path,
                result,
                gutter,
            }
        });
    }

    pub(super) fn load_project_file_blame(&mut self, path: PathBuf) {
        let Some(target) = self.current_target() else {
            return;
        };
        if let Some(entry) = self.open_project_files.iter_mut().find(|f| f.path == path) {
            entry.inline_blame_loading = true;
        }
        let request = self.next_id();
        self.project_blame_requests.insert(path.clone(), request);
        self.runtime.detail.run(move || {
            let result = history::file_blame(&target.root, "HEAD", &path);
            Event::ProjectFileBlameLoaded {
                request,
                path,
                result,
            }
        });
    }

    pub(super) fn refresh_project_file_gutter(&mut self, path: PathBuf) {
        let Some(target) = self.current_target() else {
            return;
        };
        let request = self.next_id();
        self.project_gutter_requests.insert(path.clone(), request);
        self.runtime.detail.run(move || {
            let gutter = project_files::line_diff_markers(&target.root, &path).unwrap_or_default();
            Event::ProjectFileGutterLoaded {
                request,
                path,
                gutter,
            }
        });
    }

    pub(super) fn activate_project_file(&mut self, path: Option<PathBuf>) {
        match path {
            None => self.active_project_file = None,
            Some(path) => {
                if self.open_project_files.iter().any(|f| f.path == path) {
                    self.active_project_file = Some(path);
                } else {
                    self.open_project_file(path, false);
                }
            }
        }
    }

    pub(super) fn close_project_file(&mut self, path: &Path) {
        self.project_file_requests.remove(path);
        self.project_blame_requests.remove(path);
        self.project_gutter_requests.remove(path);
        let Some(pos) = self.open_project_files.iter().position(|f| f.path == path) else {
            return;
        };
        self.open_project_files.remove(pos);
        if self.active_project_file.as_deref() == Some(path) {
            self.active_project_file = if pos < self.open_project_files.len() {
                Some(self.open_project_files[pos].path.clone())
            } else {
                self.open_project_files.last().map(|f| f.path.clone())
            };
        }
    }

    pub(super) fn save_project_file(&mut self, path: PathBuf, content: String) {
        let Some(target) = self.current_target() else {
            return;
        };
        let target_clone = target.clone();
        self.runtime.write.run(move || {
            let result = project_files::write_file(&target_clone.root, &path, &content)
                .map(|()| ProjectFileMutation::Saved { path, content });
            Event::ProjectFileMutated {
                target: target_clone,
                result,
            }
        });
    }

    pub(super) fn create_project_file(&mut self, path: PathBuf) {
        let Some(target) = self.current_target() else {
            return;
        };
        let target_clone = target.clone();
        self.runtime.write.run(move || {
            let result = project_files::create_file(&target_clone.root, &path)
                .map(|()| ProjectFileMutation::CreatedFile(path));
            Event::ProjectFileMutated {
                target: target_clone,
                result,
            }
        });
    }

    pub(super) fn create_project_dir(&mut self, path: PathBuf) {
        let Some(target) = self.current_target() else {
            return;
        };
        let target_clone = target.clone();
        self.runtime.write.run(move || {
            let result = project_files::create_dir(&target_clone.root, &path)
                .map(|()| ProjectFileMutation::CreatedDir(path));
            Event::ProjectFileMutated {
                target: target_clone,
                result,
            }
        });
    }

    pub(super) fn rename_project_entry(&mut self, from: PathBuf, to: PathBuf) {
        let Some(target) = self.current_target() else {
            return;
        };
        let target_clone = target.clone();
        self.runtime.write.run(move || {
            let result = project_files::rename_entry(&target_clone.root, &from, &to)
                .map(|()| ProjectFileMutation::Renamed { from, to });
            Event::ProjectFileMutated {
                target: target_clone,
                result,
            }
        });
    }

    pub(super) fn delete_project_entry(&mut self, path: PathBuf) {
        let Some(target) = self.current_target() else {
            return;
        };
        let target_clone = target.clone();
        self.runtime.write.run(move || {
            let result = project_files::delete_entry(&target_clone.root, &path)
                .map(|()| ProjectFileMutation::Deleted(path));
            Event::ProjectFileMutated {
                target: target_clone,
                result,
            }
        });
    }

    pub(super) fn apply_project_file_loaded(
        &mut self,
        path: PathBuf,
        result: Result<ProjectFileContent, String>,
        gutter: GutterDiffMap,
    ) {
        self.project_file_requests.remove(&path);
        let Some(entry) = self.open_project_files.iter_mut().find(|f| f.path == path) else {
            return;
        };
        entry.loading = false;
        entry.gutter_diff = gutter;
        match result {
            Ok(content) => {
                entry.content = Some(content);
                entry.error = None;
                entry.revision = entry.revision.saturating_add(1);
            }
            Err(err) => {
                entry.error = Some(err);
            }
        }
    }

    pub(super) fn apply_project_file_blame_loaded(
        &mut self,
        path: PathBuf,
        result: Result<Vec<BlameLine>, String>,
    ) {
        self.project_blame_requests.remove(&path);
        let Some(entry) = self.open_project_files.iter_mut().find(|f| f.path == path) else {
            return;
        };
        entry.inline_blame_loading = false;
        match result {
            Ok(lines) => {
                entry.inline_blame = Some(lines);
            }
            Err(err) => {
                self.message = Some(format!(
                    "{}（{err}）",
                    crate::i18n::tr("无法加载 Git Blame", "Failed to load Git Blame")
                ));
            }
        }
    }

    pub(super) fn apply_project_file_mutated(
        &mut self,
        target: WorktreeTarget,
        result: Result<ProjectFileMutation, String>,
    ) {
        if self.current_target().as_ref() != Some(&target) {
            return;
        }
        match result {
            Ok(ProjectFileMutation::Saved { path, content }) => {
                let bytes = content.len() as u64;
                if let Some(entry) = self.open_project_files.iter_mut().find(|f| f.path == path) {
                    entry.content = Some(ProjectFileContent::Text {
                        content,
                        total_bytes: bytes,
                        truncated: false,
                    });
                    entry.error = None;
                    entry.loading = false;
                    entry.revision = entry.revision.saturating_add(1);
                }
                self.message = Some(format!(
                    "{}{}",
                    crate::i18n::tr("已保存文件：", "Saved file: "),
                    path.display()
                ));
                self.load_project_git_status();
                self.refresh_project_file_gutter(path.clone());
                self.load_working_changes();
            }
            Ok(ProjectFileMutation::CreatedFile(path)) => {
                // Ensure ancestor dirs are expanded so the newly created file is visible.
                let mut cur = path.parent();
                while let Some(parent) = cur {
                    if !parent.as_os_str().is_empty() {
                        self.expanded_dirs.insert(parent.to_path_buf());
                    }
                    cur = parent.parent();
                }
                self.message = Some(format!(
                    "{}{}",
                    crate::i18n::tr("已新建文件：", "Created file: "),
                    path.display()
                ));
                self.reload_tree();
                self.load_working_changes();
                self.open_project_file(path, true);
            }
            Ok(ProjectFileMutation::CreatedDir(path)) => {
                let mut cur = Some(path.as_path());
                while let Some(dir) = cur {
                    if !dir.as_os_str().is_empty() {
                        self.expanded_dirs.insert(dir.to_path_buf());
                    }
                    cur = dir.parent();
                }
                self.message = Some(format!(
                    "{}{}",
                    crate::i18n::tr("已新建文件夹：", "Created folder: "),
                    path.display()
                ));
                self.reload_tree();
                self.load_working_changes();
            }
            Ok(ProjectFileMutation::Renamed { from, to }) => {
                for open in &mut self.open_project_files {
                    if open.path == from {
                        open.path = to.clone();
                    } else if let Ok(suffix) = open.path.strip_prefix(&from) {
                        open.path = to.join(suffix);
                    }
                }
                if let Some(active) = &mut self.active_project_file {
                    if *active == from {
                        *active = to.clone();
                    } else if let Ok(suffix) = active.strip_prefix(&from) {
                        *active = to.join(suffix);
                    }
                }
                self.message = Some(format!(
                    "{}{} → {}",
                    crate::i18n::tr("已重命名：", "Renamed: "),
                    from.display(),
                    to.display()
                ));
                self.reload_tree();
                self.load_working_changes();
            }
            Ok(ProjectFileMutation::Deleted(path)) => {
                let to_close: Vec<PathBuf> = self
                    .open_project_files
                    .iter()
                    .filter(|f| f.path == path || f.path.starts_with(&path))
                    .map(|f| f.path.clone())
                    .collect();
                for p in to_close {
                    self.close_project_file(&p);
                }
                self.message = Some(format!(
                    "{}{}",
                    crate::i18n::tr("已删除：", "Deleted: "),
                    path.display()
                ));
                self.reload_tree();
                self.load_working_changes();
            }
            Err(err) => {
                self.message = Some(err);
            }
        }
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
