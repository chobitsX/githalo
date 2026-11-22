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
pub use crate::project_files::ProjectEntry;
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
    LoadMoreHistory,
    SelectFile(PathBuf),
    CloseDiff,
    ToggleDir(PathBuf),
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
    pub selected: Option<WorktreeId>,
    pub panel: Panel,
    pub commits: Vec<Commit>,
    pub history_limit: usize,
    pub selected_commit: Option<String>,
    pub changes: Vec<ChangedFile>,
    pub selected_file: Option<PathBuf>,
    pub diff: Option<String>,
    pub tree: Vec<ProjectEntry>,
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
                selected: None,
                panel: Panel::Repositories,
                commits: vec![],
                history_limit: crate::history::RECENT_LIMIT,
                selected_commit: None,
                changes: vec![],
                selected_file: None,
                diff: None,
                tree: vec![],
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
                    for member in group.members.clone() {
                        self.start_fetch(member);
                    }
                }
            }
            Action::Reload => self.runtime.store(|db| {
                Ok(Event::Board {
                    board: db.board().map_err(|e| e.to_string())?,
                    reload: true,
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
            Action::LoadMoreHistory => {
                self.history_limit = self
                    .history_limit
                    .saturating_add(crate::history::RECENT_LIMIT);
                self.load_history();
            }
            Action::SelectFile(path) => self.select_file(&path),
            Action::CloseDiff => self.close_diff(),
            Action::ToggleDir(path) => self.toggle_dir(&path),
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
            Event::Board { board, reload } => self.apply_board(board, reload),
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
        let members: Vec<_> = self
            .repos
            .iter()
            .map(|r| r.target.id)
            .filter(|id| !assigned.contains(id))
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
            self.load_cherry_pick_state();
            self.load_repo_op_states();
            if self.panel == Panel::ProjectFiles {
                self.reload_tree();
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
        for id in reads {
            self.start_status(id);
        }
    }
}
