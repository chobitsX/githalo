use std::{ffi::OsString, path::PathBuf, sync::Arc};

use super::{Event, Workspace, daily::Effect};
use crate::{
    branch_ops::{self, BranchSnapshot},
    conflict_ops::{self, ConflictStrategy, ThreeWayConflict},
    history::{self, BlameLine, ChangedFile, Commit},
    history_ops::{
        self, ResetMode, ResetPreview, RevertAbortOutcome, RevertOptions, RevertOutcome,
        RevertPreview, RevertStatus, TagInfo,
    },
    merge_ops::{self, MergeAbortOutcome, MergeMode, MergeOutcome, MergePreview, MergeStatus},
    rebase_ops::{
        self, InteractiveRebasePreview, RebaseAbortOutcome, RebaseOutcome, RebasePreview,
        RebaseStatus, RebaseTodoItem,
    },
    repo_admin::{self, LinkedWorktreeInfo, RemoteEntry},
    staging::{ChangesHead, DiffSide, FileChange},
    worktree::WorktreeTarget,
};

#[derive(Clone, Debug)]
pub enum RepoOpAction {
    ReloadStates,
    CancelPreview,
    UndoLastCommit {
        target: WorktreeTarget,
        head_oid: String,
    },
    PrepareReset {
        target: WorktreeTarget,
        oid: String,
    },
    ExecuteReset {
        preview_id: u64,
        mode: ResetMode,
    },
    PrepareRevert {
        target: WorktreeTarget,
        oid: String,
    },
    ExecuteRevert {
        preview_id: u64,
        options: RevertOptions,
    },
    MarkRevertResolved {
        status: Arc<RevertStatus>,
        path: PathBuf,
    },
    ContinueRevert {
        status: Arc<RevertStatus>,
    },
    AbortRevert {
        status: Arc<RevertStatus>,
    },
    PrepareMerge {
        target: WorktreeTarget,
        source_ref: OsString,
    },
    ExecuteMerge {
        preview_id: u64,
        mode: MergeMode,
    },
    MarkMergeResolved {
        status: Arc<MergeStatus>,
        path: PathBuf,
    },
    CommitMerge {
        status: Arc<MergeStatus>,
        message: Option<String>,
    },
    AbortMerge {
        status: Arc<MergeStatus>,
    },
    CreateTag {
        target: WorktreeTarget,
        oid: String,
        name: String,
        message: Option<String>,
    },
    DeleteTag {
        target: WorktreeTarget,
        name: String,
    },
    PushTag {
        target: WorktreeTarget,
        remote: String,
        name: String,
    },
    CreateBranchAt {
        target: WorktreeTarget,
        name: String,
        start_oid: String,
        checkout: bool,
    },
    RenameBranch {
        snapshot: Arc<BranchSnapshot>,
        reference: OsString,
        new_name: String,
    },
    DeleteBranch {
        snapshot: Arc<BranchSnapshot>,
        reference: OsString,
        force: bool,
    },
    CheckoutRemoteBranch {
        snapshot: Arc<BranchSnapshot>,
        remote_ref: OsString,
        local_name: String,
    },
    DeleteRemoteBranch {
        target: WorktreeTarget,
        remote: String,
        branch: String,
    },
    CheckoutAndRebase {
        target: WorktreeTarget,
        branch_ref: OsString,
        onto_ref: OsString,
        autostash: bool,
    },
    OpenConflict {
        target: WorktreeTarget,
        path: PathBuf,
    },
    CloseConflict,
    ResolveConflict {
        conflict: Arc<ThreeWayConflict>,
        strategy: ConflictStrategy,
    },
    RollbackFile {
        target: WorktreeTarget,
        head: ChangesHead,
        side: DiffSide,
        file: FileChange,
    },
    LoadHeadMessage {
        target: WorktreeTarget,
    },
    AmendCommit {
        target: WorktreeTarget,
        head_oid: String,
        message: String,
    },
    PrepareRebase {
        target: WorktreeTarget,
        onto_ref: OsString,
    },
    ExecuteRebase {
        preview_id: u64,
        autostash: bool,
    },
    PrepareInteractiveRebase {
        target: WorktreeTarget,
        from_oid: String,
    },
    ExecuteInteractiveRebase {
        preview_id: u64,
        items: Vec<RebaseTodoItem>,
        autostash: bool,
    },
    MarkRebaseResolved {
        status: Arc<RebaseStatus>,
        path: PathBuf,
    },
    ContinueRebase {
        status: Arc<RebaseStatus>,
    },
    SkipRebase {
        status: Arc<RebaseStatus>,
    },
    AbortRebase {
        status: Arc<RebaseStatus>,
    },
    CloneRepository {
        url: String,
        destination: PathBuf,
        branch: Option<String>,
        group: Option<super::GroupId>,
    },
    InitRepository {
        directory: PathBuf,
        initial_branch: Option<String>,
        group: Option<super::GroupId>,
    },
    LoadRemotes {
        target: WorktreeTarget,
    },
    AddRemote {
        target: WorktreeTarget,
        name: String,
        url: String,
    },
    SetRemoteUrl {
        target: WorktreeTarget,
        name: String,
        fetch_url: String,
        push_url: Option<String>,
    },
    RemoveRemote {
        target: WorktreeTarget,
        name: String,
    },
    LoadWorktrees {
        target: WorktreeTarget,
    },
    AddLinkedWorktree {
        target: WorktreeTarget,
        path: PathBuf,
        new_branch: Option<String>,
        commit_or_branch: String,
        register: bool,
    },
    LockLinkedWorktree {
        target: WorktreeTarget,
        path: PathBuf,
        reason: Option<String>,
    },
    UnlockLinkedWorktree {
        target: WorktreeTarget,
        path: PathBuf,
    },
    RemoveLinkedWorktree {
        target: WorktreeTarget,
        path: PathBuf,
        force: bool,
    },
    PruneLinkedWorktrees {
        target: WorktreeTarget,
    },
    LoadFileInspect {
        target: WorktreeTarget,
        revision: String,
        path: PathBuf,
    },
    LoadCompareRevisions {
        target: WorktreeTarget,
        from_oid: String,
        to_oid: String,
    },
    LoadCompareDiff {
        target: WorktreeTarget,
        from_oid: String,
        to_oid: String,
        file: ChangedFile,
    },
    LoadCompareWorkingTree {
        target: WorktreeTarget,
        from_oid: String,
    },
    LoadCompareWorkingTreeDiff {
        target: WorktreeTarget,
        from_oid: String,
        file: ChangedFile,
    },
    LoadTags {
        target: WorktreeTarget,
    },
    LoadRemoteBranches {
        target: WorktreeTarget,
    },
    SetBranchUpstream {
        snapshot: Arc<BranchSnapshot>,
        reference: OsString,
        upstream: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub struct FileInspectData {
    pub target: WorktreeTarget,
    pub path: PathBuf,
    pub revision: String,
    pub commits: Vec<Commit>,
    pub blame: Vec<BlameLine>,
}

#[derive(Clone, Debug)]
pub struct CompareRevisionsData {
    pub target: WorktreeTarget,
    pub from_oid: String,
    pub to_oid: String,
    pub to_working_tree: bool,
    pub files: Vec<ChangedFile>,
    pub selected_file: Option<ChangedFile>,
    pub diff: Option<String>,
}

#[derive(Clone, Debug)]
pub enum RepoOpPreview {
    Reset(Arc<ResetPreview>),
    Revert(Arc<RevertPreview>),
    Merge(Arc<MergePreview>),
    Rebase(Arc<RebasePreview>),
    InteractiveRebase(Arc<InteractiveRebasePreview>),
}

#[derive(Default)]
pub struct RepoOpsState {
    pub merge_status: Option<Arc<MergeStatus>>,
    pub revert_status: Option<Arc<RevertStatus>>,
    pub rebase_status: Option<Arc<RebaseStatus>>,
    pub conflict: Option<Arc<ThreeWayConflict>>,
    pub conflict_loading: bool,
    pub head_message: Option<(WorktreeTarget, String)>,
    pub remotes: Option<(WorktreeTarget, Vec<RemoteEntry>)>,
    pub remotes_loading: bool,
    pub tags: Option<(WorktreeTarget, Vec<TagInfo>)>,
    pub tags_loading: bool,
    pub remote_branches: Option<(WorktreeTarget, Vec<String>)>,
    pub remote_branches_loading: bool,
    pub worktrees: Option<(WorktreeTarget, Vec<LinkedWorktreeInfo>)>,
    pub worktrees_loading: bool,
    pub file_inspect: Option<Arc<FileInspectData>>,
    pub file_inspect_loading: bool,
    pub compare: Option<CompareRevisionsData>,
    pub compare_loading: bool,
    pub compare_diff_loading: bool,
    pub admin_busy: bool,
    pub loading: bool,
    pub preparing: bool,
    pub preview: Option<RepoOpPreview>,
    pub preview_id: u64,
    pub error: Option<String>,
    state_request: u64,
    preview_request: u64,
    conflict_request: u64,
    head_msg_request: u64,
    remotes_request: u64,
    tags_request: u64,
    remote_branches_request: u64,
    worktrees_request: u64,
    admin_request: u64,
    inspect_request: u64,
    compare_request: u64,
    compare_diff_request: u64,
}

pub(super) struct RepoOpStates {
    merge: Result<Option<MergeStatus>, String>,
    revert: Result<Option<RevertStatus>, String>,
    rebase: Result<Option<RebaseStatus>, String>,
}

pub(super) enum RepoOpEvent {
    States(u64, Box<RepoOpStates>),
    Preview(u64, Result<RepoOpPreview, String>),
    Conflict(u64, Result<Box<ThreeWayConflict>, String>),
    HeadMessage(u64, WorktreeTarget, Result<String, String>),
    Remotes(u64, WorktreeTarget, Result<Vec<RemoteEntry>, String>),
    Tags(u64, WorktreeTarget, Result<Vec<TagInfo>, String>),
    RemoteBranches(u64, WorktreeTarget, Result<Vec<String>, String>),
    Worktrees(u64, WorktreeTarget, Result<Vec<LinkedWorktreeInfo>, String>),
    AdminCreated {
        serial: u64,
        group: Option<super::GroupId>,
        result: Result<(PathBuf, String), String>,
    },
    FileInspect(u64, Result<Box<FileInspectData>, String>),
    CompareRevisions(u64, Result<Box<CompareRevisionsData>, String>),
    CompareDiff(u64, ChangedFile, Result<String, String>),
}

fn event(e: RepoOpEvent) -> Event {
    Event::RepoOp(Box::new(e))
}

impl Workspace {
    pub(super) fn load_repo_op_states(&mut self) {
        let Some(target) = self
            .current_target()
            .filter(|target| !self.writing(target.id))
        else {
            return;
        };
        let request = self.next_id();
        self.repo_ops.state_request = request;
        self.repo_ops.loading = true;
        self.runtime.detail.run(move || {
            let merge = merge_ops::read_state(&target);
            let revert = history_ops::read_revert_state(&target);
            let rebase = rebase_ops::read_state(&target);
            event(RepoOpEvent::States(
                request,
                Box::new(RepoOpStates {
                    merge,
                    revert,
                    rebase,
                }),
            ))
        });
    }

    pub(super) fn cancel_repo_op_preview(&mut self) {
        self.repo_ops.preview_request = self.next_id();
        self.repo_ops.preparing = false;
        self.repo_ops.preview = None;
        self.repo_ops.preview_id = 0;
    }

    fn current_merge_status(&self, status: &Arc<MergeStatus>) -> bool {
        self.current_target().as_ref() == Some(status.worktree())
            && self
                .repo_ops
                .merge_status
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, status))
    }

    fn current_revert_status(&self, status: &Arc<RevertStatus>) -> bool {
        self.current_target().as_ref() == Some(status.worktree())
            && self
                .repo_ops
                .revert_status
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, status))
    }

    fn current_rebase_status(&self, status: &Arc<RebaseStatus>) -> bool {
        self.current_target().as_ref() == Some(status.worktree())
            && self
                .repo_ops
                .rebase_status
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, status))
    }

    pub(super) fn dispatch_repo_op(&mut self, action: RepoOpAction) {
        match action {
            RepoOpAction::ReloadStates => self.load_repo_op_states(),
            RepoOpAction::CancelPreview => self.cancel_repo_op_preview(),
            RepoOpAction::UndoLastCommit { target, head_oid } => {
                self.write(target.clone(), move || {
                    let outcome = history_ops::undo_last_commit(&target, &head_oid)?;
                    Ok(Effect::Changed(format!(
                        "已撤销提交 {}（{}），更改已保留在暂存区",
                        short_oid(&outcome.undone_oid),
                        outcome.undone_subject
                    )))
                });
            }
            RepoOpAction::PrepareReset { target, oid } => {
                if !self.check_prepare_target(&target) {
                    return;
                }
                self.start_repo_op_preview(move || {
                    Ok(RepoOpPreview::Reset(Arc::new(history_ops::preview_reset(
                        &target, &oid,
                    )?)))
                });
            }
            RepoOpAction::ExecuteReset { preview_id, mode } => {
                let Some(RepoOpPreview::Reset(preview)) = self
                    .repo_ops
                    .preview
                    .as_ref()
                    .filter(|_| self.repo_ops.preview_id == preview_id)
                    .cloned()
                else {
                    self.daily_error("Reset 预览已过期，请重新选择目标提交");
                    return;
                };
                if self.current_target().as_ref() != Some(preview.worktree()) {
                    self.daily_error("工作树已变更，请重新核对 Reset 目标");
                    return;
                }
                let target = preview.worktree().clone();
                self.cancel_repo_op_preview();
                self.write(target, move || {
                    let new_head = history_ops::execute_reset(&preview, mode)?;
                    Ok(Effect::Changed(format!(
                        "已执行 {} Reset 至 {}",
                        mode.short_name(),
                        short_oid(&new_head)
                    )))
                });
            }
            RepoOpAction::PrepareRevert { target, oid } => {
                if !self.check_prepare_target(&target) {
                    return;
                }
                self.start_repo_op_preview(move || {
                    Ok(RepoOpPreview::Revert(Arc::new(
                        history_ops::preview_revert(&target, &oid)?,
                    )))
                });
            }
            RepoOpAction::ExecuteRevert {
                preview_id,
                options,
            } => {
                let Some(RepoOpPreview::Revert(preview)) = self
                    .repo_ops
                    .preview
                    .as_ref()
                    .filter(|_| self.repo_ops.preview_id == preview_id)
                    .cloned()
                else {
                    self.daily_error("Revert 预览已过期，请重新选择提交");
                    return;
                };
                if self.current_target().as_ref() != Some(preview.worktree()) {
                    self.daily_error("工作树已变更，请重新核对 Revert 目标");
                    return;
                }
                let target = preview.worktree().clone();
                self.cancel_repo_op_preview();
                self.write(target, move || {
                    let message = match history_ops::execute_revert(&preview, &options)? {
                        RevertOutcome::Reverted { new_head_oid } => format!(
                            "已创建反向提交 {}（回退 {}）",
                            short_oid(&new_head_oid),
                            short_oid(&preview.commit().oid)
                        ),
                        RevertOutcome::Stopped { status, .. } => {
                            if !status.conflicts().is_empty() {
                                let names: Vec<_> = status
                                    .conflicts()
                                    .iter()
                                    .map(|p| p.display().to_string())
                                    .collect();
                                format!(
                                    "Revert 遇到冲突（{} 个文件：{}），请解决冲突后继续或中止",
                                    names.len(),
                                    names.join("、")
                                )
                            } else if status.is_empty_commit() {
                                format!(
                                    "Revert {} 未产生新变更（空提交），可选择中止",
                                    short_oid(status.revert_head())
                                )
                            } else {
                                format!("Revert {} 已暂停", short_oid(status.revert_head()))
                            }
                        }
                    };
                    Ok(Effect::Changed(message))
                });
            }
            RepoOpAction::MarkRevertResolved { status, path } => {
                if self.repo_ops.loading || !self.current_revert_status(&status) {
                    self.daily_error("Revert 状态已变化，请刷新后再试");
                    return;
                }
                let target = status.worktree().clone();
                let label = path.display().to_string();
                self.write(target, move || {
                    history_ops::mark_revert_resolved(&status, &path)?;
                    Ok(Effect::IndexChanged(format!(
                        "已标记冲突文件已解决：{label}"
                    )))
                });
            }
            RepoOpAction::ContinueRevert { status } => {
                if self.repo_ops.loading || !self.current_revert_status(&status) {
                    self.daily_error("Revert 状态已变化，请刷新后再试");
                    return;
                }
                let target = status.worktree().clone();
                self.write(target, move || {
                    let message = match history_ops::continue_revert(&status)? {
                        RevertOutcome::Reverted { new_head_oid } => {
                            format!("已继续完成 Revert，新提交 {}", short_oid(&new_head_oid))
                        }
                        RevertOutcome::Stopped { status: next, .. } => {
                            format!("Revert {} 仍处于暂停状态", short_oid(next.revert_head()))
                        }
                    };
                    Ok(Effect::Changed(message))
                });
            }
            RepoOpAction::AbortRevert { status } => {
                if self.repo_ops.loading || !self.current_revert_status(&status) {
                    self.daily_error("Revert 状态已变化，请刷新后再试");
                    return;
                }
                let target = status.worktree().clone();
                self.write(target, move || {
                    let RevertAbortOutcome::Aborted { head_oid } =
                        history_ops::abort_revert(&status)?;
                    Ok(Effect::Changed(format!(
                        "已中止 Revert，恢复至 {}",
                        short_oid(&head_oid)
                    )))
                });
            }
            RepoOpAction::PrepareMerge { target, source_ref } => {
                if !self.check_prepare_target(&target) {
                    return;
                }
                self.start_repo_op_preview(move || {
                    Ok(RepoOpPreview::Merge(Arc::new(merge_ops::preview(
                        &target,
                        &source_ref,
                    )?)))
                });
            }
            RepoOpAction::ExecuteMerge { preview_id, mode } => {
                let Some(RepoOpPreview::Merge(preview)) = self
                    .repo_ops
                    .preview
                    .as_ref()
                    .filter(|_| self.repo_ops.preview_id == preview_id)
                    .cloned()
                else {
                    self.daily_error("合并预览已过期，请重新选择分支");
                    return;
                };
                if self.current_target().as_ref() != Some(preview.worktree()) {
                    self.daily_error("工作树已变更，请重新核对合并目标");
                    return;
                }
                let target = preview.worktree().clone();
                self.cancel_repo_op_preview();
                self.write(target, move || {
                    let source = preview.source_display().to_owned();
                    let message = match merge_ops::execute(&preview, mode)? {
                        MergeOutcome::AlreadyUpToDate => {
                            format!("当前分支已包含 {source} 的全部提交，无需合并")
                        }
                        MergeOutcome::FastForwarded { new_head_oid } => format!(
                            "已快进合并 {source} 至 {}",
                            short_oid(&new_head_oid)
                        ),
                        MergeOutcome::Merged { new_head_oid } => format!(
                            "已合并 {source}，合并提交 {}",
                            short_oid(&new_head_oid)
                        ),
                        MergeOutcome::Squashed => {
                            format!("已压缩合并 {source} 到暂存区，请前往 Commit 页提交")
                        }
                        MergeOutcome::Stopped { status, .. } => {
                            let names: Vec<_> = status
                                .conflicts()
                                .iter()
                                .map(|p| p.display().to_string())
                                .collect();
                            format!(
                                "合并 {source} 遇到冲突（{} 个文件：{}），请解决冲突后完成合并或中止",
                                names.len(),
                                names.join("、")
                            )
                        }
                    };
                    Ok(Effect::Changed(message))
                });
            }
            RepoOpAction::MarkMergeResolved { status, path } => {
                if self.repo_ops.loading || !self.current_merge_status(&status) {
                    self.daily_error("合并状态已变化，请刷新后再试");
                    return;
                }
                let target = status.worktree().clone();
                let label = path.display().to_string();
                self.write(target, move || {
                    merge_ops::mark_resolved(&status, &path)?;
                    Ok(Effect::IndexChanged(format!(
                        "已标记冲突文件已解决：{label}"
                    )))
                });
            }
            RepoOpAction::CommitMerge { status, message } => {
                if self.repo_ops.loading || !self.current_merge_status(&status) {
                    self.daily_error("合并状态已变化，请刷新后再试");
                    return;
                }
                let target = status.worktree().clone();
                self.write(target, move || {
                    let oid = merge_ops::commit_merge(&status, message.as_deref())?;
                    Ok(Effect::Changed(format!(
                        "已完成合并提交 {}",
                        short_oid(&oid)
                    )))
                });
            }
            RepoOpAction::AbortMerge { status } => {
                if self.repo_ops.loading || !self.current_merge_status(&status) {
                    self.daily_error("合并状态已变化，请刷新后再试");
                    return;
                }
                let target = status.worktree().clone();
                self.write(target, move || {
                    let MergeAbortOutcome::Aborted { head_oid } = merge_ops::abort_merge(&status)?;
                    Ok(Effect::Changed(format!(
                        "已中止合并，恢复至 {}",
                        short_oid(&head_oid)
                    )))
                });
            }
            RepoOpAction::CreateTag {
                target,
                oid,
                name,
                message,
            } => {
                self.write(target.clone(), move || {
                    let tag = history_ops::create_tag(&target, &oid, &name, message.as_deref())?;
                    Ok(Effect::Changed(format!(
                        "已在 {} 创建标签 {}",
                        short_oid(&tag.oid),
                        tag.name
                    )))
                });
            }
            RepoOpAction::DeleteTag { target, name } => {
                self.write(target.clone(), move || {
                    history_ops::delete_tag(&target, &name)?;
                    Ok(Effect::Changed(format!("已删除本地标签 {name}")))
                });
            }
            RepoOpAction::PushTag {
                target,
                remote,
                name,
            } => {
                self.write(target.clone(), move || {
                    history_ops::push_tag(&target, &remote, &name)?;
                    Ok(Effect::Changed(format!("已推送标签 {name} 到 {remote}")))
                });
            }
            RepoOpAction::CreateBranchAt {
                target,
                name,
                start_oid,
                checkout,
            } => {
                self.write(target.clone(), move || {
                    branch_ops::create_branch_at_commit(&target, &name, &start_oid, checkout)?;
                    let verb = if checkout {
                        "已创建并切换到"
                    } else {
                        "已创建分支"
                    };
                    Ok(Effect::Changed(format!(
                        "{verb} {name}（基于 {}）",
                        short_oid(&start_oid)
                    )))
                });
            }
            RepoOpAction::RenameBranch {
                snapshot,
                reference,
                new_name,
            } => {
                if self
                    .daily
                    .branches
                    .as_ref()
                    .is_none_or(|current| !Arc::ptr_eq(current, &snapshot))
                {
                    self.daily_error("分支列表已更新，请重新选择");
                    return;
                }
                self.write(snapshot.worktree().clone(), move || {
                    branch_ops::rename_branch(&snapshot, &reference, &new_name)?;
                    Ok(Effect::Changed(format!("已将分支重命名为 {new_name}")))
                });
            }
            RepoOpAction::DeleteBranch {
                snapshot,
                reference,
                force,
            } => {
                if self
                    .daily
                    .branches
                    .as_ref()
                    .is_none_or(|current| !Arc::ptr_eq(current, &snapshot))
                {
                    self.daily_error("分支列表已更新，请重新选择");
                    return;
                }
                let display = reference
                    .to_string_lossy()
                    .trim_start_matches("refs/heads/")
                    .to_owned();
                self.write(snapshot.worktree().clone(), move || {
                    branch_ops::delete_branch(&snapshot, &reference, force)?;
                    Ok(Effect::Changed(format!("已删除分支 {display}")))
                });
            }
            RepoOpAction::CheckoutRemoteBranch {
                snapshot,
                remote_ref,
                local_name,
            } => {
                if self
                    .daily
                    .branches
                    .as_ref()
                    .is_none_or(|current| !Arc::ptr_eq(current, &snapshot))
                {
                    self.daily_error("分支列表已更新，请重新选择");
                    return;
                }
                let local_display = local_name.trim().to_owned();
                let remote_display = remote_ref
                    .to_string_lossy()
                    .trim_start_matches("refs/remotes/")
                    .to_owned();
                self.write(snapshot.worktree().clone(), move || {
                    branch_ops::checkout_remote_branch(&snapshot, &remote_ref, &local_display)?;
                    Ok(Effect::Changed(format!(
                        "已从 {remote_display} 检出并创建追踪分支 {local_display}"
                    )))
                });
            }
            RepoOpAction::DeleteRemoteBranch {
                target,
                remote,
                branch,
            } => {
                let display = format!("{remote}/{branch}");
                self.write(target.clone(), move || {
                    branch_ops::delete_remote_branch(&target, &remote, &branch)?;
                    Ok(Effect::Changed(format!("已删除远端分支 {display}")))
                });
            }
            RepoOpAction::CheckoutAndRebase {
                target,
                branch_ref,
                onto_ref,
                autostash,
            } => {
                let branch_display = branch_ref
                    .to_string_lossy()
                    .trim_start_matches("refs/heads/")
                    .to_owned();
                let onto_display = onto_ref
                    .to_string_lossy()
                    .trim_start_matches("refs/heads/")
                    .trim_start_matches("refs/remotes/")
                    .to_owned();
                self.write(target.clone(), move || {
                    let message = match rebase_ops::checkout_and_rebase(
                        &target,
                        &branch_ref,
                        &onto_ref,
                        autostash,
                    )? {
                        RebaseOutcome::Completed { new_head_oid } => format!(
                            "已检出 {branch_display} 并 Rebase 到 {onto_display}，当前 HEAD {}",
                            short_oid(&new_head_oid)
                        ),
                        RebaseOutcome::Stopped { status, .. } => rebase_stopped_message(&status),
                    };
                    Ok(Effect::Changed(message))
                });
            }
            RepoOpAction::OpenConflict { target, path } => {
                if !self.check_prepare_target(&target) {
                    return;
                }
                let request = self.next_id();
                self.repo_ops.conflict_request = request;
                self.repo_ops.conflict_loading = true;
                self.repo_ops.conflict = None;
                self.repo_ops.error = None;
                self.runtime.detail.run(move || {
                    event(RepoOpEvent::Conflict(
                        request,
                        conflict_ops::read_conflict(&target, &path).map(Box::new),
                    ))
                });
            }
            RepoOpAction::CloseConflict => {
                self.repo_ops.conflict_request = self.next_id();
                self.repo_ops.conflict_loading = false;
                self.repo_ops.conflict = None;
            }
            RepoOpAction::ResolveConflict { conflict, strategy } => {
                if self.current_target().as_ref() != Some(conflict.worktree()) {
                    self.daily_error("工作树已变更，请重新打开冲突文件");
                    return;
                }
                let target = conflict.worktree().clone();
                let label = conflict.path.display().to_string();
                self.repo_ops.conflict_request = self.next_id();
                self.repo_ops.conflict = None;
                self.repo_ops.conflict_loading = false;
                self.write(target, move || {
                    conflict_ops::resolve_conflict(&conflict, &strategy)?;
                    Ok(Effect::IndexChanged(format!("已解决冲突并暂存：{label}")))
                });
            }
            RepoOpAction::RollbackFile {
                target,
                head,
                side,
                file,
            } => {
                let label = file.path.display().to_string();
                self.write(target.clone(), move || {
                    conflict_ops::rollback_file(&target, &head, side, &file)?;
                    Ok(Effect::IndexChanged(format!("已回滚文件改动：{label}")))
                });
            }
            RepoOpAction::LoadHeadMessage { target } => {
                if self.current_target().as_ref() != Some(&target) {
                    return;
                }
                let request = self.next_id();
                self.repo_ops.head_msg_request = request;
                self.runtime.detail.run(move || {
                    let result = conflict_ops::read_head_message(&target);
                    event(RepoOpEvent::HeadMessage(request, target, result))
                });
            }
            RepoOpAction::AmendCommit {
                target,
                head_oid,
                message,
            } => {
                self.write(target.clone(), move || {
                    let oid = conflict_ops::amend_commit(&target, &head_oid, &message)?;
                    Ok(Effect::Committed { message, oid })
                });
            }
            RepoOpAction::PrepareRebase { target, onto_ref } => {
                if !self.check_prepare_target(&target) {
                    return;
                }
                self.start_repo_op_preview(move || {
                    Ok(RepoOpPreview::Rebase(Arc::new(rebase_ops::preview_rebase(
                        &target, &onto_ref,
                    )?)))
                });
            }
            RepoOpAction::ExecuteRebase {
                preview_id,
                autostash,
            } => {
                let Some(RepoOpPreview::Rebase(preview)) = self
                    .repo_ops
                    .preview
                    .as_ref()
                    .filter(|_| self.repo_ops.preview_id == preview_id)
                    .cloned()
                else {
                    self.daily_error("Rebase 预览已过期，请重新选择目标分支");
                    return;
                };
                if self.current_target().as_ref() != Some(preview.worktree()) {
                    self.daily_error("工作树已变更，请重新核对 Rebase 目标");
                    return;
                }
                let target = preview.worktree().clone();
                self.cancel_repo_op_preview();
                self.write(target, move || {
                    let onto = preview.onto_display().to_owned();
                    let message = match rebase_ops::execute_rebase(&preview, autostash)? {
                        RebaseOutcome::Completed { new_head_oid } => format!(
                            "已 Rebase 到 {onto}，当前 HEAD {}",
                            short_oid(&new_head_oid)
                        ),
                        RebaseOutcome::Stopped { status, .. } => rebase_stopped_message(&status),
                    };
                    Ok(Effect::Changed(message))
                });
            }
            RepoOpAction::PrepareInteractiveRebase { target, from_oid } => {
                if !self.check_prepare_target(&target) {
                    return;
                }
                self.start_repo_op_preview(move || {
                    Ok(RepoOpPreview::InteractiveRebase(Arc::new(
                        rebase_ops::preview_interactive(&target, &from_oid)?,
                    )))
                });
            }
            RepoOpAction::ExecuteInteractiveRebase {
                preview_id,
                items,
                autostash,
            } => {
                let Some(RepoOpPreview::InteractiveRebase(preview)) = self
                    .repo_ops
                    .preview
                    .as_ref()
                    .filter(|_| self.repo_ops.preview_id == preview_id)
                    .cloned()
                else {
                    self.daily_error("交互式 Rebase 预览已过期，请重新选择起始提交");
                    return;
                };
                if self.current_target().as_ref() != Some(preview.worktree()) {
                    self.daily_error("工作树已变更，请重新核对 Rebase 目标");
                    return;
                }
                let target = preview.worktree().clone();
                self.cancel_repo_op_preview();
                self.write(target, move || {
                    let message =
                        match rebase_ops::execute_interactive(&preview, &items, autostash)? {
                            RebaseOutcome::Completed { new_head_oid } => format!(
                                "交互式 Rebase 完成，当前 HEAD {}",
                                short_oid(&new_head_oid)
                            ),
                            RebaseOutcome::Stopped { status, .. } => {
                                rebase_stopped_message(&status)
                            }
                        };
                    Ok(Effect::Changed(message))
                });
            }
            RepoOpAction::MarkRebaseResolved { status, path } => {
                if self.repo_ops.loading || !self.current_rebase_status(&status) {
                    self.daily_error("Rebase 状态已变化，请刷新后再试");
                    return;
                }
                let target = status.worktree().clone();
                let label = path.display().to_string();
                self.write(target, move || {
                    rebase_ops::mark_resolved(&status, &path)?;
                    Ok(Effect::IndexChanged(format!(
                        "已标记冲突文件已解决：{label}"
                    )))
                });
            }
            RepoOpAction::ContinueRebase { status } => {
                if self.repo_ops.loading || !self.current_rebase_status(&status) {
                    self.daily_error("Rebase 状态已变化，请刷新后再试");
                    return;
                }
                let target = status.worktree().clone();
                self.write(target, move || {
                    let message = match rebase_ops::continue_rebase(&status)? {
                        RebaseOutcome::Completed { new_head_oid } => {
                            format!("Rebase 已全部完成，当前 HEAD {}", short_oid(&new_head_oid))
                        }
                        RebaseOutcome::Stopped { status: next, .. } => {
                            rebase_stopped_message(&next)
                        }
                    };
                    Ok(Effect::Changed(message))
                });
            }
            RepoOpAction::SkipRebase { status } => {
                if self.repo_ops.loading || !self.current_rebase_status(&status) {
                    self.daily_error("Rebase 状态已变化，请刷新后再试");
                    return;
                }
                let target = status.worktree().clone();
                self.write(target, move || {
                    let message = match rebase_ops::skip_rebase(&status)? {
                        RebaseOutcome::Completed { new_head_oid } => format!(
                            "已跳过提交并完成 Rebase，当前 HEAD {}",
                            short_oid(&new_head_oid)
                        ),
                        RebaseOutcome::Stopped { status: next, .. } => {
                            rebase_stopped_message(&next)
                        }
                    };
                    Ok(Effect::Changed(message))
                });
            }
            RepoOpAction::AbortRebase { status } => {
                if self.repo_ops.loading || !self.current_rebase_status(&status) {
                    self.daily_error("Rebase 状态已变化，请刷新后再试");
                    return;
                }
                let target = status.worktree().clone();
                self.write(target, move || {
                    let RebaseAbortOutcome::Aborted { head_oid } =
                        rebase_ops::abort_rebase(&status)?;
                    Ok(Effect::Changed(format!(
                        "已中止 Rebase，恢复至 {}",
                        short_oid(&head_oid)
                    )))
                });
            }
            RepoOpAction::CloneRepository {
                url,
                destination,
                branch,
                group,
            } => {
                let serial = self.next_id();
                self.repo_ops.admin_request = serial;
                self.repo_ops.admin_busy = true;
                self.repo_ops.error = None;
                self.runtime.write.run(move || {
                    let result =
                        repo_admin::clone_repository(&url, &destination, branch.as_deref()).map(
                            |root| {
                                let msg = format!("已克隆仓库到 {}", root.display());
                                (root, msg)
                            },
                        );
                    event(RepoOpEvent::AdminCreated {
                        serial,
                        group,
                        result,
                    })
                });
            }
            RepoOpAction::InitRepository {
                directory,
                initial_branch,
                group,
            } => {
                let serial = self.next_id();
                self.repo_ops.admin_request = serial;
                self.repo_ops.admin_busy = true;
                self.repo_ops.error = None;
                self.runtime.write.run(move || {
                    let result = repo_admin::init_repository(&directory, initial_branch.as_deref())
                        .map(|root| {
                            let msg = format!("已初始化仓库 {}", root.display());
                            (root, msg)
                        });
                    event(RepoOpEvent::AdminCreated {
                        serial,
                        group,
                        result,
                    })
                });
            }
            RepoOpAction::LoadRemotes { target } => {
                if self.current_target().as_ref() != Some(&target) {
                    return;
                }
                let request = self.next_id();
                self.repo_ops.remotes_request = request;
                self.repo_ops.remotes_loading = true;
                self.runtime.detail.run(move || {
                    let result = repo_admin::list_remotes(&target);
                    event(RepoOpEvent::Remotes(request, target, result))
                });
            }
            RepoOpAction::AddRemote { target, name, url } => {
                let label = name.clone();
                self.write(target.clone(), move || {
                    repo_admin::add_remote(&target, &name, &url)?;
                    Ok(Effect::Changed(format!("已添加远端 {label}")))
                });
            }
            RepoOpAction::SetRemoteUrl {
                target,
                name,
                fetch_url,
                push_url,
            } => {
                let label = name.clone();
                self.write(target.clone(), move || {
                    repo_admin::set_remote_url(&target, &name, &fetch_url, push_url.as_deref())?;
                    Ok(Effect::Changed(format!("已更新远端 {label} 的地址")))
                });
            }
            RepoOpAction::RemoveRemote { target, name } => {
                let label = name.clone();
                self.write(target.clone(), move || {
                    repo_admin::remove_remote(&target, &name)?;
                    Ok(Effect::Changed(format!("已删除远端 {label}")))
                });
            }
            RepoOpAction::LoadWorktrees { target } => {
                if self.current_target().as_ref() != Some(&target) {
                    return;
                }
                let request = self.next_id();
                self.repo_ops.worktrees_request = request;
                self.repo_ops.worktrees_loading = true;
                self.runtime.detail.run(move || {
                    let result = repo_admin::list_worktrees(&target);
                    event(RepoOpEvent::Worktrees(request, target, result))
                });
            }
            RepoOpAction::AddLinkedWorktree {
                target,
                path,
                new_branch,
                commit_or_branch,
                register,
            } => {
                let serial = self.next_id();
                self.repo_ops.admin_request = serial;
                self.repo_ops.admin_busy = true;
                self.runtime.write.run(move || {
                    let result = repo_admin::add_worktree(
                        &target,
                        &path,
                        new_branch.as_deref(),
                        &commit_or_branch,
                    )
                    .map(|root| {
                        let msg = format!("已创建关联工作树 {}", root.display());
                        (root, msg)
                    });
                    if register {
                        event(RepoOpEvent::AdminCreated {
                            serial,
                            group: None,
                            result,
                        })
                    } else {
                        let list = repo_admin::list_worktrees(&target);
                        match result {
                            Ok(_) => event(RepoOpEvent::Worktrees(serial, target, list)),
                            Err(err) => event(RepoOpEvent::AdminCreated {
                                serial,
                                group: None,
                                result: Err(err),
                            }),
                        }
                    }
                });
            }
            RepoOpAction::LockLinkedWorktree {
                target,
                path,
                reason,
            } => {
                self.write(target.clone(), move || {
                    repo_admin::lock_worktree(&target, &path, reason.as_deref())?;
                    Ok(Effect::Changed(format!("已锁定工作树 {}", path.display())))
                });
            }
            RepoOpAction::UnlockLinkedWorktree { target, path } => {
                self.write(target.clone(), move || {
                    repo_admin::unlock_worktree(&target, &path)?;
                    Ok(Effect::Changed(format!("已解锁工作树 {}", path.display())))
                });
            }
            RepoOpAction::RemoveLinkedWorktree {
                target,
                path,
                force,
            } => {
                self.write(target.clone(), move || {
                    repo_admin::remove_worktree(&target, &path, force)?;
                    Ok(Effect::Changed(format!(
                        "已移除关联工作树 {}",
                        path.display()
                    )))
                });
            }
            RepoOpAction::PruneLinkedWorktrees { target } => {
                self.write(target.clone(), move || {
                    repo_admin::prune_worktrees(&target)?;
                    Ok(Effect::Changed("已清理失效的关联工作树记录".into()))
                });
            }
            RepoOpAction::LoadFileInspect {
                target,
                revision,
                path,
            } => {
                if self.current_target().as_ref() != Some(&target) {
                    return;
                }
                let request = self.next_id();
                self.repo_ops.inspect_request = request;
                self.repo_ops.file_inspect_loading = true;
                self.repo_ops.file_inspect = None;
                self.runtime.detail.run(move || {
                    let result = history::file_history(&target.root, &path).map(|commits| {
                        let blame =
                            history::file_blame(&target.root, &revision, &path).unwrap_or_default();
                        Box::new(FileInspectData {
                            target,
                            path,
                            revision,
                            commits,
                            blame,
                        })
                    });
                    event(RepoOpEvent::FileInspect(request, result))
                });
            }
            RepoOpAction::LoadCompareRevisions {
                target,
                from_oid,
                to_oid,
            } => {
                if self.current_target().as_ref() != Some(&target) {
                    return;
                }
                let request = self.next_id();
                self.repo_ops.compare_request = request;
                self.repo_ops.compare_loading = true;
                self.repo_ops.compare = None;
                self.runtime.detail.run(move || {
                    let result =
                        history::compare_revisions(&target.root, &from_oid, &to_oid).map(|files| {
                            Box::new(CompareRevisionsData {
                                target,
                                from_oid,
                                to_oid,
                                to_working_tree: false,
                                files,
                                selected_file: None,
                                diff: None,
                            })
                        });
                    event(RepoOpEvent::CompareRevisions(request, result))
                });
            }
            RepoOpAction::LoadCompareDiff {
                target,
                from_oid,
                to_oid,
                file,
            } => {
                if self.current_target().as_ref() != Some(&target) {
                    return;
                }
                let request = self.next_id();
                self.repo_ops.compare_diff_request = request;
                self.repo_ops.compare_diff_loading = true;
                self.runtime.detail.run(move || {
                    let result =
                        history::compare_file_diff(&target.root, &from_oid, &to_oid, &file);
                    event(RepoOpEvent::CompareDiff(request, file, result))
                });
            }
            RepoOpAction::LoadCompareWorkingTree { target, from_oid } => {
                if self.current_target().as_ref() != Some(&target) {
                    return;
                }
                let request = self.next_id();
                self.repo_ops.compare_request = request;
                self.repo_ops.compare_loading = true;
                self.repo_ops.compare = None;
                self.runtime.detail.run(move || {
                    let result =
                        history::compare_with_working_tree(&target.root, &from_oid).map(|files| {
                            Box::new(CompareRevisionsData {
                                target,
                                from_oid,
                                to_oid: String::new(),
                                to_working_tree: true,
                                files,
                                selected_file: None,
                                diff: None,
                            })
                        });
                    event(RepoOpEvent::CompareRevisions(request, result))
                });
            }
            RepoOpAction::LoadCompareWorkingTreeDiff {
                target,
                from_oid,
                file,
            } => {
                if self.current_target().as_ref() != Some(&target) {
                    return;
                }
                let request = self.next_id();
                self.repo_ops.compare_diff_request = request;
                self.repo_ops.compare_diff_loading = true;
                self.runtime.detail.run(move || {
                    let result =
                        history::compare_working_tree_file_diff(&target.root, &from_oid, &file);
                    event(RepoOpEvent::CompareDiff(request, file, result))
                });
            }
            RepoOpAction::LoadTags { target } => {
                if self.current_target().as_ref() != Some(&target) {
                    return;
                }
                let request = self.next_id();
                self.repo_ops.tags_request = request;
                self.repo_ops.tags_loading = true;
                self.runtime.detail.run(move || {
                    let result = history_ops::list_tags(&target);
                    event(RepoOpEvent::Tags(request, target, result))
                });
            }
            RepoOpAction::LoadRemoteBranches { target } => {
                if self.current_target().as_ref() != Some(&target) {
                    return;
                }
                let request = self.next_id();
                self.repo_ops.remote_branches_request = request;
                self.repo_ops.remote_branches_loading = true;
                self.runtime.detail.run(move || {
                    let result = branch_ops::list_remote_branches(&target);
                    event(RepoOpEvent::RemoteBranches(request, target, result))
                });
            }
            RepoOpAction::SetBranchUpstream {
                snapshot,
                reference,
                upstream,
            } => {
                if self
                    .daily
                    .branches
                    .as_ref()
                    .is_none_or(|current| !Arc::ptr_eq(current, &snapshot))
                {
                    self.daily_error("分支列表已更新，请重新选择");
                    return;
                }
                let branch_name = reference
                    .to_string_lossy()
                    .trim_start_matches("refs/heads/")
                    .to_owned();
                self.write(snapshot.worktree().clone(), move || {
                    branch_ops::set_branch_upstream(&snapshot, &reference, upstream.as_deref())?;
                    let msg = match upstream.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                        Some(up) => format!("已将分支 {branch_name} 的上游设为 {up}"),
                        None => format!("已解除分支 {branch_name} 的上游跟踪"),
                    };
                    Ok(Effect::Changed(msg))
                });
            }
        }
    }

    fn check_prepare_target(&mut self, target: &WorktreeTarget) -> bool {
        if self.current_target().as_ref() != Some(target) {
            self.daily_error("操作目标已改变，请在当前工作树重新选择");
            return false;
        }
        if self.writing(target.id) {
            self.daily_error("此工作树的 Git 操作仍在进行");
            return false;
        }
        true
    }

    fn start_repo_op_preview(
        &mut self,
        op: impl FnOnce() -> Result<RepoOpPreview, String> + Send + 'static,
    ) {
        self.cancel_repo_op_preview();
        self.repo_ops.preparing = true;
        self.repo_ops.error = None;
        let request = self.repo_ops.preview_request;
        self.runtime
            .detail
            .run(move || event(RepoOpEvent::Preview(request, op())));
    }

    pub(super) fn apply_repo_op(&mut self, event: RepoOpEvent) {
        match event {
            RepoOpEvent::States(serial, results) if serial == self.repo_ops.state_request => {
                let RepoOpStates {
                    merge,
                    revert,
                    rebase,
                } = *results;
                self.repo_ops.loading = false;
                match merge {
                    Ok(Some(status)) => {
                        if self.repo_ops.merge_status.as_deref() != Some(&status) {
                            self.repo_ops.merge_status = Some(Arc::new(status));
                        }
                    }
                    Ok(None) => self.repo_ops.merge_status = None,
                    Err(err) => {
                        self.repo_ops.merge_status = None;
                        self.repo_ops.error = Some(err);
                    }
                }
                match revert {
                    Ok(Some(status)) => {
                        if self.repo_ops.revert_status.as_deref() != Some(&status) {
                            self.repo_ops.revert_status = Some(Arc::new(status));
                        }
                    }
                    Ok(None) => self.repo_ops.revert_status = None,
                    Err(err) => {
                        self.repo_ops.revert_status = None;
                        self.repo_ops.error = Some(err);
                    }
                }
                match rebase {
                    Ok(Some(status)) => {
                        if self.repo_ops.rebase_status.as_deref() != Some(&status) {
                            self.repo_ops.rebase_status = Some(Arc::new(status));
                        }
                    }
                    Ok(None) => self.repo_ops.rebase_status = None,
                    Err(err) => {
                        self.repo_ops.rebase_status = None;
                        self.repo_ops.error = Some(err);
                    }
                }
            }
            RepoOpEvent::Preview(serial, result) if serial == self.repo_ops.preview_request => {
                self.repo_ops.preparing = false;
                match result {
                    Ok(preview) => {
                        self.repo_ops.preview = Some(preview);
                        self.repo_ops.preview_id = serial;
                        self.repo_ops.error = None;
                    }
                    Err(err) => {
                        self.repo_ops.preview = None;
                        self.repo_ops.preview_id = 0;
                        self.repo_ops.error = Some(err.clone());
                        self.daily_error(err);
                    }
                }
            }
            RepoOpEvent::Conflict(serial, result) if serial == self.repo_ops.conflict_request => {
                self.repo_ops.conflict_loading = false;
                match result {
                    Ok(conflict) => {
                        self.repo_ops.conflict = Some(Arc::from(conflict));
                        self.repo_ops.error = None;
                    }
                    Err(err) => {
                        self.repo_ops.conflict = None;
                        self.repo_ops.error = Some(err.clone());
                        self.daily_error(err);
                    }
                }
            }
            RepoOpEvent::HeadMessage(serial, target, result)
                if serial == self.repo_ops.head_msg_request =>
            {
                if let Ok(message) = result
                    && self.current_target().as_ref() == Some(&target)
                {
                    self.repo_ops.head_message = Some((target, message));
                }
            }
            RepoOpEvent::Remotes(serial, target, result)
                if serial == self.repo_ops.remotes_request =>
            {
                self.repo_ops.remotes_loading = false;
                if self.current_target().as_ref() == Some(&target) {
                    match result {
                        Ok(list) => self.repo_ops.remotes = Some((target, list)),
                        Err(err) => self.daily_error(err),
                    }
                }
            }
            RepoOpEvent::Tags(serial, target, result) if serial == self.repo_ops.tags_request => {
                self.repo_ops.tags_loading = false;
                if self.current_target().as_ref() == Some(&target) {
                    match result {
                        Ok(list) => self.repo_ops.tags = Some((target, list)),
                        Err(err) => self.daily_error(err),
                    }
                }
            }
            RepoOpEvent::RemoteBranches(serial, target, result)
                if serial == self.repo_ops.remote_branches_request =>
            {
                self.repo_ops.remote_branches_loading = false;
                if self.current_target().as_ref() == Some(&target) {
                    match result {
                        Ok(list) => self.repo_ops.remote_branches = Some((target, list)),
                        Err(err) => self.daily_error(err),
                    }
                }
            }
            RepoOpEvent::Worktrees(serial, target, result)
                if serial == self.repo_ops.worktrees_request =>
            {
                self.repo_ops.worktrees_loading = false;
                if self.current_target().as_ref() == Some(&target) {
                    match result {
                        Ok(list) => self.repo_ops.worktrees = Some((target, list)),
                        Err(err) => self.daily_error(err),
                    }
                }
            }
            RepoOpEvent::AdminCreated {
                serial,
                group,
                result,
            } if serial == self.repo_ops.admin_request => {
                self.repo_ops.admin_busy = false;
                match result {
                    Ok((root, message)) => {
                        self.daily.notice = Some(message.clone());
                        self.message = Some(message);
                        self.dispatch(super::Action::AddRepo { path: root, group });
                        if let Some(target) = self.current_target() {
                            self.dispatch_repo_op(RepoOpAction::LoadWorktrees { target });
                        }
                    }
                    Err(err) => self.daily_error(err),
                }
            }
            RepoOpEvent::FileInspect(serial, result) if serial == self.repo_ops.inspect_request => {
                self.repo_ops.file_inspect_loading = false;
                match result {
                    Ok(data) => self.repo_ops.file_inspect = Some(Arc::from(data)),
                    Err(err) => self.daily_error(err),
                }
            }
            RepoOpEvent::CompareRevisions(serial, result)
                if serial == self.repo_ops.compare_request =>
            {
                self.repo_ops.compare_loading = false;
                match result {
                    Ok(data) => self.repo_ops.compare = Some(*data),
                    Err(err) => self.daily_error(err),
                }
            }
            RepoOpEvent::CompareDiff(serial, file, result)
                if serial == self.repo_ops.compare_diff_request =>
            {
                self.repo_ops.compare_diff_loading = false;
                if let Some(compare) = &mut self.repo_ops.compare {
                    compare.selected_file = Some(file);
                    compare.diff =
                        Some(result.unwrap_or_else(|e| format!("读取 Diff 失败（{e}）")));
                }
            }
            _ => {}
        }
    }
}

fn rebase_stopped_message(status: &RebaseStatus) -> String {
    let step = status
        .progress()
        .map(|(cur, total)| format!("（步骤 {cur}/{total}）"))
        .unwrap_or_default();
    if !status.conflicts().is_empty() {
        let names: Vec<_> = status
            .conflicts()
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        format!(
            "Rebase{step}遇到冲突（{} 个文件：{}），请解决后继续、跳过或中止",
            names.len(),
            names.join("、")
        )
    } else {
        format!("Rebase{step}已暂停，可继续、跳过或中止")
    }
}

fn short_oid(oid: &str) -> &str {
    oid.get(..8).unwrap_or(oid)
}
