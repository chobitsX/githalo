//! Desktop-independent application state. Slow operations send tagged results back to the UI.
mod cherry_pick;
mod daily;
mod details;
mod repo_ops;
mod runtime;
mod shelves;
mod stashes;
mod status;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::git::{self, FetchOutcome};
pub use crate::history::{BlameLine, ChangedFile, Commit, CommitRef, CommitRefKind};
pub use crate::nested::{
    DEFAULT_MAX_DEPTH, DEFAULT_MAX_DIRS, DiscoveredNestedRepo, NestedScanOutcome,
};
pub use crate::project_files::{
    FileGitStatus, GutterDiffHunk, GutterDiffKind, GutterDiffMap, ProjectEntry, ProjectFileContent,
};
pub use crate::registry::GroupId;
use crate::registry::{Registry, RegistryBoard, RegistryError};
pub use crate::worktree::WorktreeId;
use crate::worktree::{SyncSnapshot, UpstreamConfig, WorktreeTarget};
use async_channel::Receiver;
pub use cherry_pick::{CherryPickAction, CherryPickState};
pub use daily::{CommitReceipt, DailyAction, DailyReview, DailyState, ReviewKind};
pub use repo_ops::{RepoOpAction, RepoOpPreview, RepoOpsState};
use runtime::Runtime;
pub use shelves::{ShelfAction, ShelfState};
pub use stashes::{StashAction, StashState};
pub use status::RepoRow;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Panel {
    Repositories,
    ProjectFiles,
    Commit,
    Stash,
    Shelf,
}

#[derive(Clone, Debug)]
pub struct GroupView {
    pub id: Option<GroupId>,
    pub name: String,
    pub expanded: bool,
    pub members: Vec<WorktreeId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NestedChildrenView {
    pub expanded: bool,
    pub children: Vec<WorktreeId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerDiscovery {
    pub container: PathBuf,
    pub folder_name: String,
    pub group: Option<GroupId>,
    pub repos: Vec<DiscoveredNestedRepo>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NestedAdoptMode {
    UnderParent,
    NewGroup(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenProjectFile {
    pub path: PathBuf,
    pub loading: bool,
    pub content: Option<ProjectFileContent>,
    pub error: Option<String>,
    pub revision: u64,
    pub gutter_diff: GutterDiffMap,
    pub inline_blame: Option<Vec<BlameLine>>,
    pub inline_blame_loading: bool,
}

#[derive(Clone, Debug)]
pub enum Action {
    Daily(DailyAction),
    Stash(Box<StashAction>),
    CherryPick(Box<CherryPickAction>),
    RepoOp(Box<RepoOpAction>),
    Shelf(Box<ShelfAction>),
    Select(WorktreeId),
    ShowPanel(Panel),
    ToggleGroup(Option<GroupId>),
    RefreshRepo(WorktreeId),
    RefreshGroup(Option<GroupId>),
    Reload,
    AddRepo {
        path: PathBuf,
        group: Option<GroupId>,
    },
    AddRepos {
        paths: Vec<PathBuf>,
        group: Option<GroupId>,
    },
    RemoveRepo(WorktreeId),
    RepairRepo {
        id: WorktreeId,
        path: PathBuf,
    },
    CreateGroup(String),
    RenameGroup {
        id: GroupId,
        name: String,
    },
    DeleteGroup(GroupId),
    AddMember {
        id: WorktreeId,
        group: GroupId,
    },
    RemoveMember {
        id: WorktreeId,
        group: GroupId,
    },
    MoveMember {
        id: WorktreeId,
        from: GroupId,
        to: GroupId,
    },
    SelectCommit(String),
    SetBranchFilter(Option<String>),
    LoadMoreHistory,
    SelectFile(PathBuf),
    CloseDiff,
    ToggleDir(PathBuf),
    ReloadProjectTree,
    CollapseAllProjectDirs,
    OpenProjectFile(PathBuf),
    ReloadProjectFile(PathBuf),
    LoadProjectFileBlame(PathBuf),
    ActivateProjectFile(Option<PathBuf>),
    CloseProjectFile(PathBuf),
    SaveProjectFile {
        path: PathBuf,
        content: String,
    },
    CreateProjectFile(PathBuf),
    CreateProjectDir(PathBuf),
    RenameProjectEntry {
        from: PathBuf,
        to: PathBuf,
    },
    DeleteProjectEntry(PathBuf),
    ToggleNested(WorktreeId),
    ScanNested(WorktreeId),
    AdoptNested {
        parent: WorktreeId,
        paths: Vec<PathBuf>,
        mode: NestedAdoptMode,
    },
    DismissNested {
        parent: WorktreeId,
        paths: Vec<PathBuf>,
    },
    DetachNested {
        parent: WorktreeId,
        child: WorktreeId,
    },
    RefreshWithNested(WorktreeId),
    ImportContainerRepos {
        paths: Vec<PathBuf>,
        new_group_name: Option<String>,
        group: Option<GroupId>,
    },
    ClearContainerDiscovery,
    ClearMessage,
}

// Only the originating worker can construct a result. Consumers feed it back to apply().
pub struct Update(Event);

enum Event {
    Daily(Box<daily::DailyEvent>),
    Stash(Box<stashes::StashEvent>),
    CherryPick(Box<cherry_pick::CherryPickEvent>),
    RepoOp(Box<repo_ops::RepoOpEvent>),
    Shelf(Box<shelves::ShelfEvent>),
    Board {
        board: RegistryBoard,
        reload: bool,
        notice: Option<String>,
    },
    Error(String),
    Status {
        snapshot: SyncSnapshot,
        common: Option<PathBuf>,
    },
    Fetched {
        before: SyncSnapshot,
        common: Option<PathBuf>,
        result: Result<FetchOutcome, String>,
        at: SystemTime,
    },
    FetchTime {
        target: WorktreeTarget,
        config: UpstreamConfig,
        tracking: OsString,
        at: Option<SystemTime>,
        comparison: Option<(OsString, crate::registry::HistoricalCounts)>,
    },
    History(u64, Result<Vec<Commit>, String>),
    Changes(u64, Result<Vec<ChangedFile>, String>),
    Patch(u64, Result<String, String>),
    Directory(u64, PathBuf, Result<Vec<ProjectEntry>, String>),
    ProjectGitStatus(u64, BTreeMap<PathBuf, FileGitStatus>),
    ProjectFileLoaded {
        request: u64,
        path: PathBuf,
        result: Result<ProjectFileContent, String>,
        gutter: GutterDiffMap,
    },
    ProjectFileBlameLoaded {
        request: u64,
        path: PathBuf,
        result: Result<Vec<BlameLine>, String>,
    },
    ProjectFileGutterLoaded {
        request: u64,
        path: PathBuf,
        gutter: GutterDiffMap,
    },
    ProjectFileMutated {
        target: WorktreeTarget,
        result: Result<ProjectFileMutation, String>,
    },
    NestedScan {
        target: WorktreeTarget,
        outcome: NestedScanOutcome,
        manual: bool,
    },
    ContainerScan {
        container: PathBuf,
        group: Option<GroupId>,
        outcome: NestedScanOutcome,
    },
}

enum ProjectFileMutation {
    Saved { path: PathBuf, content: String },
    CreatedFile(PathBuf),
    CreatedDir(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
    Deleted(PathBuf),
}

pub struct Workspace {
    pub daily: DailyState,
    pub stashes: StashState,
    pub cherry_pick: CherryPickState,
    pub repo_ops: RepoOpsState,
    pub shelves: ShelfState,
    pub last_commit: Option<CommitReceipt>,
    writes: BTreeMap<WorktreeId, daily::WriteRecord>,
    pub ready: bool,
    pub groups: Vec<GroupView>,
    pub repos: Vec<RepoRow>,
    pub nested: BTreeMap<WorktreeId, NestedChildrenView>,
    dismissed_nested: BTreeMap<WorktreeId, BTreeSet<PathBuf>>,
    pub discovered_nested: BTreeMap<WorktreeId, Vec<DiscoveredNestedRepo>>,
    pub scanning_nested: BTreeSet<WorktreeId>,
    scanned_nested: BTreeSet<(WorktreeId, u64)>,
    pub container_discovery: Option<ContainerDiscovery>,
    pub selected: Option<WorktreeId>,
    pub panel: Panel,
    pub commits: Vec<Commit>,
    pub branch_filter: Option<String>,
    pub history_limit: usize,
    pub selected_commit: Option<String>,
    pub changes: Vec<ChangedFile>,
    pub selected_file: Option<PathBuf>,
    pub diff: Option<String>,
    pub tree: Vec<ProjectEntry>,
    pub project_git_status: BTreeMap<PathBuf, FileGitStatus>,
    pub open_project_files: Vec<OpenProjectFile>,
    pub active_project_file: Option<PathBuf>,
    pub message: Option<String>,
    pub history_loading: bool,
    pub changes_loading: bool,
    pub diff_loading: bool,
    pub tree_loading: bool,
    sequence: u64,
    history_request: u64,
    changes_request: u64,
    patch_request: u64,
    dir_requests: BTreeMap<PathBuf, u64>,
    project_status_request: u64,
    project_file_requests: BTreeMap<PathBuf, u64>,
    project_blame_requests: BTreeMap<PathBuf, u64>,
    project_gutter_requests: BTreeMap<PathBuf, u64>,
    dirs: BTreeMap<PathBuf, Vec<ProjectEntry>>,
    expanded_dirs: BTreeSet<PathBuf>,
    runtime: Runtime,
}

pub fn database_path() -> PathBuf {
    std::env::var_os("GITHALO_DB")
        .or_else(|| std::env::var_os("GIT_CLIENT_DB"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("githalo")
                .join("workspace.sqlite3")
        })
}

pub fn format_fetch_time(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(time)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

impl Workspace {
    pub fn new(db_path: PathBuf) -> (Self, Receiver<Update>) {
        let (runtime, events) = Runtime::new(db_path);
        (
            Self {
                daily: DailyState::default(),
                stashes: StashState::default(),
                cherry_pick: CherryPickState::default(),
                repo_ops: RepoOpsState::default(),
                shelves: ShelfState::default(),
                last_commit: None,
                writes: BTreeMap::new(),
                ready: false,
                groups: vec![],
                repos: vec![],
                nested: BTreeMap::new(),
                dismissed_nested: BTreeMap::new(),
                discovered_nested: BTreeMap::new(),
                scanning_nested: BTreeSet::new(),
                scanned_nested: BTreeSet::new(),
                container_discovery: None,
                selected: None,
                panel: Panel::Repositories,
                commits: vec![],
                branch_filter: None,
                history_limit: crate::history::RECENT_LIMIT,
                selected_commit: None,
                changes: vec![],
                selected_file: None,
                diff: None,
                tree: vec![],
                project_git_status: BTreeMap::new(),
                open_project_files: vec![],
                active_project_file: None,
                message: None,
                history_loading: false,
                changes_loading: false,
                diff_loading: false,
                tree_loading: false,
                sequence: 0,
                history_request: 0,
                changes_request: 0,
                patch_request: 0,
                dir_requests: BTreeMap::new(),
                project_status_request: 0,
                project_file_requests: BTreeMap::new(),
                project_blame_requests: BTreeMap::new(),
                project_gutter_requests: BTreeMap::new(),
                dirs: BTreeMap::new(),
                expanded_dirs: BTreeSet::new(),
                runtime,
            },
            events,
        )
    }

    pub fn repo(&self, id: WorktreeId) -> Option<&RepoRow> {
        self.repo_index(id).map(|index| &self.repos[index])
    }

    fn repo_mut(&mut self, id: WorktreeId) -> Option<&mut RepoRow> {
        self.repo_index(id).map(|index| &mut self.repos[index])
    }

    fn repo_index(&self, id: WorktreeId) -> Option<usize> {
        // Registry::board returns worktrees ordered by ID.
        self.repos
            .binary_search_by_key(&id, |repo| repo.target.id)
            .ok()
    }

    fn current_target(&self) -> Option<WorktreeTarget> {
        self.selected
            .and_then(|id| self.repo(id))
            .map(|r| r.target.clone())
    }

    fn next_id(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }

    fn mutate(&mut self, op: impl FnOnce(&mut Registry) -> Result<(), String> + Send + 'static) {
        self.runtime.store(move |db| {
            op(db)?;
            Ok(Event::Board {
                board: db.board().map_err(|e| e.to_string())?,
                reload: false,
                notice: None,
            })
        })
    }

    fn edit_catalog(
        &mut self,
        op: impl FnOnce(&mut Registry) -> Result<(), RegistryError> + Send + 'static,
    ) {
        self.mutate(move |db| op(db).map_err(|e| e.to_string()));
    }

    pub fn dispatch(&mut self, action: Action) {
        match action {
            Action::Daily(action) => self.dispatch_daily(action),
            Action::Stash(action) => self.dispatch_stash(*action),
            Action::CherryPick(action) => self.dispatch_cherry_pick(*action),
            Action::RepoOp(action) => self.dispatch_repo_op(*action),
            Action::Shelf(action) => self.dispatch_shelf(*action),
            Action::Select(id) => self.select(Some(id), false),
            Action::ShowPanel(p) => {
                if self.panel != p {
                    self.cancel_review();
                    self.cancel_cherry_pick_preview();
                    self.cancel_repo_op_preview();
                    if self.panel == Panel::Stash {
                        self.stashes.suspend_reads();
                    }
                    if self.panel == Panel::Shelf {
                        self.shelves.suspend_reads();
                    }
                }
                self.panel = p;
                match p {
                    Panel::ProjectFiles => self.reload_tree(),
                    Panel::Commit => self.load_working_changes(),
                    Panel::Stash => self.load_stashes(),
                    Panel::Shelf => self.load_shelves(),
                    Panel::Repositories => {}
                }
            }
            Action::ToggleGroup(id) => self.edit_catalog(move |db| db.toggle_group_expanded(id)),
            Action::RefreshRepo(id) => self.start_fetch(id),
            Action::RefreshGroup(id) => {
                if let Some(group) = self.groups.iter().find(|g| g.id == id) {
                    let members = group.members.clone();
                    for member in members {
                        self.start_fetch(member);
                        if let Some(nested) = self.nested.get(&member) {
                            for child in nested.children.clone() {
                                self.start_fetch(child);
                            }
                        }
                    }
                }
            }
            Action::Reload => self.runtime.store(|db| {
                Ok(Event::Board {
                    board: db.board().map_err(|e| e.to_string())?,
                    reload: true,
                    notice: None,
                })
            }),
            Action::CreateGroup(name) => {
                self.edit_catalog(move |db| db.create_group(&name).map(|_| ()))
            }
            Action::RenameGroup { id, name } => {
                self.edit_catalog(move |db| db.rename_group(id, &name))
            }
            Action::DeleteGroup(id) => self.edit_catalog(move |db| db.delete_group(id)),
            Action::AddRepo { path, group } => self.mutate(move |db| {
                db.register(&git::discover(&path)?, group)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }),
            Action::AddRepos { paths, group } => {
                if paths.is_empty() {
                    return;
                }
                let scan_pool = self.runtime.scan.clone();
                self.runtime.store(move |db| {
                    let mut roots = BTreeSet::new();
                    let mut non_git_dirs = Vec::new();
                    let mut skipped = 0usize;
                    let mut first_error = None;
                    for path in paths {
                        let candidate = if path.is_file() {
                            path.parent().map(PathBuf::from).unwrap_or(path)
                        } else {
                            path
                        };
                        match git::discover(&candidate) {
                            Ok(root) => {
                                roots.insert(root);
                            }
                            Err(err) => {
                                if candidate.is_dir() {
                                    non_git_dirs.push(candidate);
                                }
                                skipped += 1;
                                if first_error.is_none() {
                                    first_error = Some(err);
                                }
                            }
                        }
                    }
                    if roots.is_empty() {
                        if let Some(container) = non_git_dirs.into_iter().next() {
                            let container_scan = container.clone();
                            scan_pool.run(move || {
                                let outcome = crate::nested::scan_nested_repos(
                                    &container_scan,
                                    DEFAULT_MAX_DEPTH,
                                    DEFAULT_MAX_DIRS,
                                );
                                Event::ContainerScan {
                                    container: container_scan,
                                    group,
                                    outcome,
                                }
                            });
                            return Ok(Event::Board {
                                board: db.board().map_err(|e| e.to_string())?,
                                reload: false,
                                notice: Some(
                                    crate::i18n::tr(
                                        "正在扫描文件夹内的 Git 仓库…",
                                        "Scanning folder for Git repositories…",
                                    )
                                    .to_owned(),
                                ),
                            });
                        }
                        return Err(first_error.unwrap_or_else(|| {
                            crate::i18n::tr(
                                "所选路径不是有效的 Git 仓库目录",
                                "Dropped path is not a valid Git repository",
                            )
                            .to_owned()
                        }));
                    }
                    for root in &roots {
                        db.register(root, group).map_err(|e| e.to_string())?;
                    }
                    let _ = db.set_group_expanded(group, true);
                    let notice = (skipped > 0).then(|| {
                        if crate::i18n::is_zh() {
                            format!(
                                "已添加 {} 个仓库，跳过 {} 个非 Git 目录",
                                roots.len(),
                                skipped
                            )
                        } else {
                            format!(
                                "Added {} repositories; skipped {} non-Git item(s)",
                                roots.len(),
                                skipped
                            )
                        }
                    });
                    Ok(Event::Board {
                        board: db.board().map_err(|e| e.to_string())?,
                        reload: false,
                        notice,
                    })
                });
            }
            Action::RepairRepo { id, .. } | Action::RemoveRepo(id) if self.writing(id) => {
                self.message = Some("此工作树正在执行 Git 操作，完成后可修改登记".into());
            }
            Action::RepairRepo { id, path } => self.mutate(move |db| {
                db.repair_path(id, &git::discover(&path)?)
                    .map_err(|e| e.to_string())?;
                Ok(())
            }),
            Action::RemoveRepo(id) => self.edit_catalog(move |db| db.unregister(id)),
            Action::AddMember { id, group } => {
                self.edit_catalog(move |db| db.add_member(id, group))
            }
            Action::RemoveMember { id, group } => {
                self.edit_catalog(move |db| db.remove_member(id, group))
            }
            Action::MoveMember { id, from, to } => {
                self.edit_catalog(move |db| db.move_member(id, from, to))
            }
            Action::SelectCommit(oid) => self.select_commit(&oid),
            Action::SetBranchFilter(filter) => {
                if self.branch_filter != filter {
                    self.branch_filter = filter;
                    self.history_limit = crate::history::RECENT_LIMIT;
                    self.load_history();
                }
            }
            Action::LoadMoreHistory => {
                self.history_limit = self
                    .history_limit
                    .saturating_add(crate::history::RECENT_LIMIT);
                self.load_history();
            }
            Action::SelectFile(path) => self.select_file(&path),
            Action::CloseDiff => self.close_diff(),
            Action::ToggleDir(path) => self.toggle_dir(&path),
            Action::ReloadProjectTree => self.reload_tree(),
            Action::CollapseAllProjectDirs => self.collapse_all_dirs(),
            Action::OpenProjectFile(path) => self.open_project_file(path, false),
            Action::ReloadProjectFile(path) => self.open_project_file(path, true),
            Action::LoadProjectFileBlame(path) => self.load_project_file_blame(path),
            Action::ActivateProjectFile(path) => self.activate_project_file(path),
            Action::CloseProjectFile(path) => self.close_project_file(&path),
            Action::SaveProjectFile { path, content } => self.save_project_file(path, content),
            Action::CreateProjectFile(path) => self.create_project_file(path),
            Action::CreateProjectDir(path) => self.create_project_dir(path),
            Action::RenameProjectEntry { from, to } => self.rename_project_entry(from, to),
            Action::DeleteProjectEntry(path) => self.delete_project_entry(path),
            Action::ToggleNested(parent) => {
                self.edit_catalog(move |db| db.toggle_nested_expanded(parent));
            }
            Action::ScanNested(parent) => {
                if let Some(target) = self.repo(parent).map(|r| r.target.clone()) {
                    self.dismissed_nested.remove(&parent);
                    self.discovered_nested.remove(&parent);
                    self.scanning_nested.insert(parent);
                    self.edit_catalog(move |db| db.clear_dismissed_nested(parent));
                    self.start_nested_scan(target, true);
                }
            }
            Action::AdoptNested {
                parent,
                paths,
                mode,
            } => {
                if paths.is_empty() {
                    return;
                }
                self.discovered_nested.remove(&parent);
                self.runtime.store(move |db| {
                    let mut roots = Vec::new();
                    let mut seen = BTreeSet::new();
                    for path in paths {
                        if let Ok(root) = git::discover(&path)
                            && seen.insert(root.clone())
                        {
                            roots.push(root);
                        }
                    }
                    if roots.is_empty() {
                        return Err(crate::i18n::tr(
                            "没有可纳管的有效 Git 仓库",
                            "No valid Git repositories to adopt",
                        )
                        .to_owned());
                    }
                    match mode {
                        NestedAdoptMode::UnderParent => {
                            db.attach_nested_repos(parent, &roots)
                                .map_err(|e| e.to_string())?;
                        }
                        NestedAdoptMode::NewGroup(name) => {
                            let group_id = db.create_group(&name).map_err(|e| e.to_string())?;
                            for root in &roots {
                                db.register(root, Some(group_id))
                                    .map_err(|e| e.to_string())?;
                            }
                            let _ = db.set_group_expanded(Some(group_id), true);
                            // Remember these paths as handled for the parent so the banner won't re-appear.
                            let _ = db.dismiss_nested_paths(parent, &roots);
                        }
                    }
                    Ok(Event::Board {
                        board: db.board().map_err(|e| e.to_string())?,
                        reload: false,
                        notice: None,
                    })
                });
            }
            Action::DismissNested { parent, paths } => {
                self.discovered_nested.remove(&parent);
                if !paths.is_empty() {
                    self.edit_catalog(move |db| db.dismiss_nested_paths(parent, &paths));
                }
            }
            Action::DetachNested { parent, child } => {
                self.edit_catalog(move |db| db.detach_nested_repo(parent, child));
            }
            Action::RefreshWithNested(parent) => {
                self.start_fetch(parent);
                if let Some(nested) = self.nested.get(&parent) {
                    for child in nested.children.clone() {
                        self.start_fetch(child);
                    }
                }
            }
            Action::ImportContainerRepos {
                paths,
                new_group_name,
                group,
            } => {
                self.container_discovery = None;
                if paths.is_empty() {
                    return;
                }
                self.runtime.store(move |db| {
                    let target_group = if let Some(name) = new_group_name {
                        Some(db.create_group(&name).map_err(|e| e.to_string())?)
                    } else {
                        group
                    };
                    let mut count = 0usize;
                    let mut seen = BTreeSet::new();
                    for path in paths {
                        if let Ok(root) = git::discover(&path)
                            && seen.insert(root.clone())
                        {
                            db.register(&root, target_group)
                                .map_err(|e| e.to_string())?;
                            count += 1;
                        }
                    }
                    if count == 0 {
                        return Err(crate::i18n::tr(
                            "没有可导入的有效 Git 仓库",
                            "No valid Git repositories to import",
                        )
                        .to_owned());
                    }
                    let _ = db.set_group_expanded(target_group, true);
                    Ok(Event::Board {
                        board: db.board().map_err(|e| e.to_string())?,
                        reload: false,
                        notice: None,
                    })
                });
            }
            Action::ClearContainerDiscovery => {
                self.container_discovery = None;
            }
            Action::ClearMessage => self.clear_daily_message(),
        }
    }

    pub fn apply(&mut self, update: Update) {
        match update.0 {
            Event::Daily(event) => self.apply_daily(*event),
            Event::Stash(event) => self.apply_stash(*event),
            Event::CherryPick(event) => self.apply_cherry_pick(*event),
            Event::RepoOp(event) => self.apply_repo_op(*event),
            Event::Shelf(event) => self.apply_shelf(*event),
            Event::Board {
                board,
                reload,
                notice,
            } => {
                self.apply_board(board, reload);
                if notice.is_some() {
                    self.message = notice;
                }
            }
            Event::Error(message) => {
                self.ready = true;
                self.message = Some(message);
            }
            Event::Status { snapshot, common } => self.apply_status(snapshot, common),
            Event::Fetched {
                before,
                common,
                result,
                at,
            } => self.apply_fetch(before, common, result, at),
            Event::FetchTime {
                target,
                config,
                tracking,
                at,
                comparison,
            } => self.apply_time(target, config, tracking, at, comparison),
            Event::History(tag, data) if tag == self.history_request => self.apply_history(data),
            Event::Changes(tag, data) if tag == self.changes_request => self.apply_changes(data),
            Event::Patch(tag, data) if tag == self.patch_request => {
                self.diff_loading = false;
                self.diff = Some(data.unwrap_or_else(|e| format!("读取 Diff 失败（{e}）")));
            }
            Event::Directory(tag, path, data) if self.dir_requests.get(&path) == Some(&tag) => {
                self.apply_directory(path, data)
            }
            Event::ProjectGitStatus(tag, map) if tag == self.project_status_request => {
                self.project_git_status = map;
            }
            Event::ProjectFileLoaded {
                request,
                path,
                result,
                gutter,
            } if self.project_file_requests.get(&path) == Some(&request) => {
                self.apply_project_file_loaded(path, result, gutter);
            }
            Event::ProjectFileBlameLoaded {
                request,
                path,
                result,
            } if self.project_blame_requests.get(&path) == Some(&request) => {
                self.apply_project_file_blame_loaded(path, result);
            }
            Event::ProjectFileGutterLoaded {
                request,
                path,
                gutter,
            } if self.project_gutter_requests.get(&path) == Some(&request) => {
                self.project_gutter_requests.remove(&path);
                if let Some(entry) = self.open_project_files.iter_mut().find(|f| f.path == path) {
                    entry.gutter_diff = gutter;
                }
            }
            Event::ProjectFileMutated { target, result } => {
                self.apply_project_file_mutated(target, result);
            }
            Event::NestedScan {
                target,
                outcome,
                manual,
            } => self.apply_nested_scan(target, outcome, manual),
            Event::ContainerScan {
                container,
                group,
                outcome,
            } => self.apply_container_scan(container, group, outcome),
            _ => {} // The selected worktree, commit, file or directory changed in the meantime.
        }
    }

    fn apply_board(&mut self, board: RegistryBoard, reload: bool) {
        self.ready = true;
        let previous = self.current_target();
        let mut old: BTreeMap<_, _> = std::mem::take(&mut self.repos)
            .into_iter()
            .map(|r| (r.target.id, r))
            .collect();
        let mut reads = Vec::new();
        self.repos = board
            .worktrees
            .into_iter()
            .map(|target| {
                if let Some(row) = old.remove(&target.id).filter(|r| r.target == target) {
                    if reload {
                        reads.push(target.id);
                    }
                    row
                } else {
                    reads.push(target.id);
                    RepoRow::new(target)
                }
            })
            .collect();
        self.writes.retain(|id, record| {
            self.repos
                .binary_search_by_key(id, |r| r.target.id)
                .ok()
                .is_some_and(|i| self.repos[i].target == record.target)
        });
        let mut assigned = BTreeSet::new();
        self.groups = board
            .groups
            .into_iter()
            .map(|g| {
                assigned.extend(g.members.iter().copied());
                GroupView {
                    id: Some(g.id),
                    name: g.name,
                    expanded: g.expanded,
                    members: g.members,
                }
            })
            .collect();
        let mut nested_children = BTreeSet::new();
        self.nested = board
            .nested
            .into_iter()
            .map(|(parent, entry)| {
                nested_children.extend(entry.children.iter().copied());
                (
                    parent,
                    NestedChildrenView {
                        expanded: entry.expanded,
                        children: entry.children,
                    },
                )
            })
            .collect();
        self.dismissed_nested = board
            .dismissed_nested
            .into_iter()
            .map(|(parent, paths)| (parent, paths.into_iter().collect()))
            .collect();

        // Filter out already-attached or dismissed paths from pending discovery banners.
        let valid_ids: BTreeSet<WorktreeId> = self.repos.iter().map(|r| r.target.id).collect();
        self.discovered_nested
            .retain(|parent, _| valid_ids.contains(parent));
        self.scanning_nested
            .retain(|parent| valid_ids.contains(parent));
        let repo_roots: BTreeMap<WorktreeId, PathBuf> = self
            .repos
            .iter()
            .map(|r| (r.target.id, r.target.root.clone()))
            .collect();
        let parent_keys: Vec<_> = self.discovered_nested.keys().copied().collect();
        for parent in parent_keys {
            let attached_roots: BTreeSet<&PathBuf> = self
                .nested
                .get(&parent)
                .into_iter()
                .flat_map(|n| n.children.iter())
                .filter_map(|child| repo_roots.get(child))
                .collect();
            let dismissed = self.dismissed_nested.get(&parent);
            if let Some(items) = self.discovered_nested.get_mut(&parent) {
                items.retain(|item| {
                    !attached_roots.contains(&item.root)
                        && !dismissed.is_some_and(|set| set.contains(&item.root))
                });
                if items.is_empty() {
                    self.discovered_nested.remove(&parent);
                }
            }
        }

        let members: Vec<_> = self
            .repos
            .iter()
            .map(|r| r.target.id)
            .filter(|id| !assigned.contains(id) && !nested_children.contains(id))
            .collect();
        if !members.is_empty() {
            self.groups.push(GroupView {
                id: None,
                name: "未分组".into(),
                expanded: board.ungrouped_expanded,
                members,
            });
        }
        let id = self
            .selected
            .filter(|id| self.repo(*id).is_some())
            .or_else(|| self.repos.first().map(|r| r.target.id));
        let target_changed = previous != id.and_then(|id| self.repo(id)).map(|r| r.target.clone());
        self.select(id, target_changed);
        if reload && !target_changed {
            self.load_history();
            self.load_branches();
            self.load_cherry_pick_state();
            self.load_repo_op_states();
            if self.panel == Panel::ProjectFiles {
                self.refresh_tree_silently();
            }
            self.load_working_changes();
            self.load_stashes();
            self.load_shelves();
        }
        for id in reads {
            self.start_status(id);
        }
        // Trigger low-priority async nested scan once per top-level worktree revision.
        let targets_to_scan: Vec<WorktreeTarget> = self
            .repos
            .iter()
            .filter(|r| !nested_children.contains(&r.target.id))
            .filter(|r| {
                self.scanned_nested
                    .insert((r.target.id, r.target.path_revision))
            })
            .map(|r| r.target.clone())
            .collect();
        for target in targets_to_scan {
            self.start_nested_scan(target, false);
        }
    }

    fn start_nested_scan(&self, target: WorktreeTarget, manual: bool) {
        let root = target.root.clone();
        if manual {
            self.runtime.scan.run(move || {
                let outcome =
                    crate::nested::scan_nested_repos(&root, DEFAULT_MAX_DEPTH, DEFAULT_MAX_DIRS);
                Event::NestedScan {
                    target,
                    outcome,
                    manual: true,
                }
            });
        } else {
            let events = self.runtime.events_sender();
            self.runtime.scan.execute(move || {
                let outcome =
                    crate::nested::scan_nested_repos(&root, DEFAULT_MAX_DEPTH, DEFAULT_MAX_DIRS);
                if !outcome.repos.is_empty() {
                    let _ = events.send_blocking(Update(Event::NestedScan {
                        target,
                        outcome,
                        manual: false,
                    }));
                }
            });
        }
    }

    fn apply_nested_scan(
        &mut self,
        target: WorktreeTarget,
        outcome: NestedScanOutcome,
        manual: bool,
    ) {
        self.scanning_nested.remove(&target.id);
        let Some(current) = self.repo(target.id) else {
            return;
        };
        if current.target != target {
            return;
        }
        let attached_roots: BTreeSet<PathBuf> = self
            .nested
            .get(&target.id)
            .into_iter()
            .flat_map(|n| n.children.iter())
            .filter_map(|child| self.repo(*child).map(|r| r.target.root.clone()))
            .collect();
        let dismissed = self.dismissed_nested.get(&target.id);
        let fresh: Vec<DiscoveredNestedRepo> = outcome
            .repos
            .into_iter()
            .filter(|item| {
                !attached_roots.contains(&item.root)
                    && !dismissed.is_some_and(|set| set.contains(&item.root))
            })
            .collect();
        if fresh.is_empty() {
            self.discovered_nested.remove(&target.id);
            if manual {
                self.message = Some(
                    crate::i18n::tr(
                        "未在子目录中发现新的 Git 仓库",
                        "No new nested Git repositories found",
                    )
                    .to_owned(),
                );
            }
        } else {
            self.discovered_nested.insert(target.id, fresh);
        }
    }

    fn apply_container_scan(
        &mut self,
        container: PathBuf,
        group: Option<GroupId>,
        outcome: NestedScanOutcome,
    ) {
        if outcome.repos.is_empty() {
            self.message = Some(
                crate::i18n::tr(
                    "所选文件夹及其子目录中未找到 Git 仓库",
                    "No Git repositories found in the dropped folder",
                )
                .to_owned(),
            );
            return;
        }
        self.message = None;
        let folder_name = container
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Repositories".to_owned());
        self.container_discovery = Some(ContainerDiscovery {
            container,
            folder_name,
            group,
            repos: outcome.repos,
            truncated: outcome.truncated,
        });
    }
}
