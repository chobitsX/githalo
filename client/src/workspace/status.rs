use super::*;
use crate::registry::HistoricalCounts;
use crate::worktree::{SyncCounts, SyncStatus, UpstreamStatus};
use std::ffi::OsStr;
use std::sync::atomic::AtomicBool;

/// Snapshot of an in-flight group or parent+nested batch fetch operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchFetchProgress {
    pub label: String,
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub cancelling: bool,
}

pub(super) struct BatchFetchTracker {
    pub(super) label: String,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) pending: BTreeSet<WorktreeId>,
    pub(super) total: usize,
    pub(super) completed: usize,
    pub(super) failed: usize,
    pub(super) skipped: usize,
    pub(super) cancelled_count: usize,
    pub(super) cancelling: bool,
}

impl BatchFetchTracker {
    pub(super) fn snapshot(&self) -> BatchFetchProgress {
        BatchFetchProgress {
            label: self.label.clone(),
            total: self.total,
            completed: self.completed,
            failed: self.failed,
            skipped: self.skipped,
            cancelling: self.cancelling,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RepoRow {
    pub target: WorktreeTarget,
    pub name: String,
    pub name_lower: String,
    pub path_lower: String,
    pub branch: String,
    pub upstream: Option<String>,
    pub upstream_is_local: bool,
    pub counts: Option<SyncCounts>,
    pub issue: Option<String>,
    pub historical: bool,
    pub historical_at: Option<SystemTime>,
    pub loading: bool,
    pub fetching: bool,
    pub stale: bool,
    pub last_fetch: Option<SystemTime>,
    snapshot: Option<SyncSnapshot>,
    common: Option<PathBuf>,
    stamp: Option<git::RepoStateStamp>,
    db_lookup_key: Option<(OsString, UpstreamConfig, OsString)>,
    read_again: bool,
    failure: Option<(SyncStatus, String)>,
    confirmed: Option<HistoricalCounts>,
    receipt: Option<Receipt>,
}

#[derive(Clone, Debug)]
struct Receipt {
    config: UpstreamConfig,
    tracking: OsString,
    oid: String,
    at: SystemTime,
}

type Source<'a> = (&'a OsStr, &'a UpstreamConfig, Option<&'a OsStr>);

fn source(status: &SyncStatus) -> Option<Source<'_>> {
    let SyncStatus::Branch {
        branch_ref,
        upstream,
        ..
    } = status
    else {
        return None;
    };
    let (config, tracking) = match upstream {
        UpstreamStatus::Resolved {
            config,
            tracking_ref,
            ..
        } => (config, Some(tracking_ref.as_os_str())),
        UpstreamStatus::Unavailable {
            config,
            tracking_ref,
        }
        | UpstreamStatus::Deleted {
            config,
            tracking_ref,
        } => (config, tracking_ref.as_deref()),
        _ => return None,
    };
    Some((branch_ref.as_os_str(), config, tracking))
}

fn tracking(status: &SyncStatus) -> Option<(&UpstreamConfig, &OsStr)> {
    source(status).and_then(|(_, config, tracking)| tracking.map(|r| (config, r)))
}

fn display_ref(reference: &OsStr) -> String {
    let text = reference.to_string_lossy();
    text.strip_prefix("refs/heads/")
        .or_else(|| text.strip_prefix("refs/remotes/"))
        .unwrap_or(&text)
        .to_owned()
}

impl RepoRow {
    /// Whether at least one sync snapshot has been loaded for this repository.
    pub fn has_status(&self) -> bool {
        self.snapshot.is_some()
    }

    #[doc(hidden)]
    pub fn test_stub(target: WorktreeTarget) -> Self {
        Self::new(target)
    }

    pub(super) fn new(target: WorktreeTarget) -> Self {
        let name = target
            .root
            .file_name()
            .unwrap_or(target.root.as_os_str())
            .to_string_lossy()
            .into_owned();
        let name_lower = name.to_lowercase();
        let path_lower = target.root.to_string_lossy().to_lowercase();
        Self {
            target,
            name,
            name_lower,
            path_lower,
            branch: "—".into(),
            upstream: None,
            upstream_is_local: false,
            counts: None,
            issue: None,
            historical: false,
            historical_at: None,
            loading: false,
            fetching: false,
            stale: false,
            last_fetch: None,
            snapshot: None,
            common: None,
            stamp: None,
            db_lookup_key: None,
            read_again: false,
            failure: None,
            confirmed: None,
            receipt: None,
        }
    }

    fn publish(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        self.counts = snapshot.status.counts();
        self.upstream = None;
        self.upstream_is_local = false;
        self.historical = false;
        self.historical_at = None;
        let (branch, issue) = match &snapshot.status {
            SyncStatus::Branch {
                branch_ref,
                upstream,
                ..
            } => {
                if let Some((_, config, reference)) = source(&snapshot.status) {
                    self.upstream_is_local = config.remote == ".";
                    self.upstream = Some(reference.map(display_ref).unwrap_or_else(|| {
                        let name = display_ref(&config.merge_ref);
                        if config.remote == "." {
                            name
                        } else {
                            format!("{}/{name}", config.remote.to_string_lossy())
                        }
                    }));
                }
                (
                    display_ref(branch_ref),
                    match upstream {
                        UpstreamStatus::NotConfigured => Some("未设置上游".to_owned()),
                        UpstreamStatus::Unavailable { .. } => Some("上游引用不可用".into()),
                        UpstreamStatus::Deleted { .. } => Some("上游已删除".into()),
                        UpstreamStatus::ReadFailed { reason } => Some(reason.clone()),
                        UpstreamStatus::Resolved { comparison, .. } => {
                            comparison.as_ref().err().cloned()
                        }
                    },
                )
            }
            SyncStatus::DetachedHead { head_oid } => {
                (head_oid.chars().take(8).collect(), Some("分离 HEAD".into()))
            }
            SyncStatus::UnbornBranch { branch_ref, .. } => {
                (display_ref(branch_ref), Some("尚无提交".into()))
            }
            SyncStatus::WorktreeUnavailable { reason } => {
                ("—".into(), Some(format!("工作树不可访问（{reason}）")))
            }
            SyncStatus::ReadFailed { reason } => ("—".into(), Some(reason.clone())),
        };
        self.branch = branch;
        self.issue = issue;
        if let Some((previous, message)) = &self.failure {
            if source(previous).is_some() && source(previous) == source(&snapshot.status) {
                self.issue = Some(format!("Fetch 失败（{message}）"));
                self.counts = self.confirmed.as_ref().map(|c| c.counts);
                self.historical_at = self.confirmed.as_ref().map(|c| c.at);
                self.historical = self.counts.is_some();
            } else {
                self.failure = None;
            }
        }
    }
}

impl Workspace {
    pub(super) fn start_status(&mut self, id: WorktreeId) {
        self.start_status_inner(id, false);
    }

    /// Refreshes `id` only if its lightweight `.git` fingerprint changed since the last read.
    pub(super) fn start_status_if_changed(&mut self, id: WorktreeId) {
        self.start_status_inner(id, true);
    }

    fn start_status_inner(&mut self, id: WorktreeId, skip_if_unchanged: bool) {
        let Some(row) = self.repo_mut(id) else {
            return;
        };
        row.stale = false;
        if row.loading {
            row.read_again = true;
            return;
        }
        row.loading = true;
        let target = row.target.clone();
        let stamp = if skip_if_unchanged && row.has_status() {
            row.stamp.clone()
        } else {
            None
        };
        self.runtime.status.run(move || {
            if let Some(prev_stamp) = stamp
                && prev_stamp.is_unchanged()
            {
                return Event::StatusUnchanged { target };
            }
            let result = git::read_sync_with_common(&target);
            Event::Status {
                common: result.common,
                snapshot: result.snapshot,
                stamp: result.stamp,
            }
        });
    }

    pub(super) fn apply_status_unchanged(&mut self, target: WorktreeTarget) {
        let Some(row) = self.repo_mut(target.id).filter(|r| r.target == target) else {
            return;
        };
        row.loading = false;
        row.stale = false;
        if std::mem::take(&mut row.read_again) {
            self.start_status(target.id);
        }
    }

    pub(super) fn apply_status(
        &mut self,
        snapshot: SyncSnapshot,
        common: Option<PathBuf>,
        stamp: Option<git::RepoStateStamp>,
    ) {
        let target = snapshot.worktree.clone();
        let Some(row) = self.repo_mut(target.id).filter(|r| r.target == target) else {
            return;
        };
        row.loading = false;
        row.stale = false;
        if std::mem::take(&mut row.read_again) {
            self.start_status(target.id);
            return;
        }
        if row.snapshot.as_ref().and_then(|s| tracking(&s.status)) != tracking(&snapshot.status) {
            row.last_fetch = None;
        }
        if row.snapshot.as_ref().and_then(|s| source(&s.status)) != source(&snapshot.status) {
            row.confirmed = None;
        }
        let verified = match (row.receipt.take(), &snapshot.status) {
            (
                Some(receipt),
                SyncStatus::Branch {
                    upstream:
                        UpstreamStatus::Resolved {
                            config,
                            tracking_ref,
                            oid,
                            comparison: Ok(counts),
                        },
                    ..
                },
            ) if receipt.config == *config
                && receipt.tracking == *tracking_ref
                && receipt.oid == *oid =>
            {
                Some(HistoricalCounts {
                    at: receipt.at,
                    counts: *counts,
                })
            }
            _ => None,
        };
        let lookup = source(&snapshot.status)
            .and_then(|(branch, c, r)| r.map(|r| (branch.to_owned(), c.clone(), r.to_owned())));
        row.common = common;
        row.stamp = stamp;
        row.snapshot = Some(snapshot);
        row.publish();
        self.bump_board_version();
        match lookup {
            Some(key) => {
                let needs_db = verified.is_some()
                    || self
                        .repo(target.id)
                        .is_none_or(|r| r.db_lookup_key.as_ref() != Some(&key));
                if needs_db {
                    if let Some(row) = self.repo_mut(target.id) {
                        row.db_lookup_key = Some(key.clone());
                    }
                    let (branch, config, tracking) = key;
                    self.runtime.store(move |db| {
                        if let Some(sample) = verified {
                            db.record_comparison(
                                &target,
                                &branch,
                                &config,
                                &tracking,
                                sample.at,
                                sample.counts,
                            )
                            .map_err(|e| e.to_string())?;
                        }
                        let at = db
                            .last_fetch_success(&target, &config, &tracking)
                            .map_err(|e| e.to_string())?;
                        let comparison = db
                            .last_comparison(&target, &branch, &config, &tracking)
                            .map_err(|e| e.to_string())?
                            .map(|sample| (branch, sample));
                        Ok(Event::FetchTime {
                            target,
                            config,
                            tracking,
                            at,
                            comparison,
                        })
                    });
                }
            }
            None => {
                if let Some(row) = self.repo_mut(target.id) {
                    row.db_lookup_key = None;
                }
            }
        }
    }

    pub(super) fn apply_time(
        &mut self,
        target: WorktreeTarget,
        config: UpstreamConfig,
        reference: OsString,
        at: Option<SystemTime>,
        comparison: Option<(OsString, HistoricalCounts)>,
    ) {
        if let Some(row) = self.repo_mut(target.id).filter(|r| r.target == target)
            && row.snapshot.as_ref().and_then(|s| tracking(&s.status))
                == Some((&config, reference.as_os_str()))
        {
            row.last_fetch = row.last_fetch.max(at);
            if let Some((branch, sample)) = comparison
                && row
                    .snapshot
                    .as_ref()
                    .and_then(|s| source(&s.status))
                    .is_some_and(|(b, _, _)| b == branch)
                && row.confirmed.as_ref().is_none_or(|old| old.at <= sample.at)
            {
                row.confirmed = Some(sample);
            }
            row.publish();
            self.bump_board_version();
        }
    }

    /// Current group/nested batch fetch progress, if any batch fetch is running.
    pub fn batch_fetch_progress(&self) -> Option<BatchFetchProgress> {
        self.batch_fetch.as_ref().map(BatchFetchTracker::snapshot)
    }

    /// Starts a tracked batch fetch for `ids` (group or parent + nested sub-repos), skipping
    /// repositories known to be detached or without an upstream and supporting one-click cancel.
    pub(super) fn start_batch_fetch(&mut self, label: String, ids: Vec<WorktreeId>) {
        if ids.is_empty() {
            return;
        }
        // Deduplicate while preserving order.
        let mut unique = Vec::with_capacity(ids.len());
        let mut seen = BTreeSet::new();
        for id in ids {
            if seen.insert(id) {
                unique.push(id);
            }
        }

        let reuse_active = self
            .batch_fetch
            .as_ref()
            .is_some_and(|tracker| !tracker.cancelling);
        let cancel = if reuse_active {
            self.batch_fetch.as_ref().unwrap().cancel.clone()
        } else {
            Arc::new(AtomicBool::new(false))
        };

        let mut queued = Vec::new();
        let mut skipped = 0usize;
        for id in unique {
            let skip_fetch = self
                .repo(id)
                .and_then(|row| row.snapshot.as_ref())
                .is_some_and(|snap| {
                    matches!(
                        snap.status,
                        SyncStatus::DetachedHead { .. }
                            | SyncStatus::Branch {
                                upstream: UpstreamStatus::NotConfigured,
                                ..
                            }
                            | SyncStatus::UnbornBranch { upstream: None, .. }
                    )
                });
            if skip_fetch {
                skipped += 1;
                self.start_status_if_changed(id);
            } else if self.start_fetch_inner(id, Some(cancel.clone())) {
                queued.push(id);
            }
        }

        if queued.is_empty() {
            return;
        }

        if let Some(tracker) = self.batch_fetch.as_mut().filter(|t| !t.cancelling) {
            if tracker.label != label {
                tracker.label = crate::i18n::tr("多个分组", "Multiple Groups").to_owned();
            }
            for id in queued {
                if tracker.pending.insert(id) {
                    tracker.total += 1;
                }
            }
            tracker.skipped += skipped;
        } else {
            let total = queued.len();
            self.batch_fetch = Some(BatchFetchTracker {
                label,
                cancel,
                pending: queued.into_iter().collect(),
                total,
                completed: 0,
                failed: 0,
                skipped,
                cancelled_count: 0,
                cancelling: false,
            });
        }
        self.bump_board_version();
    }

    pub(super) fn cancel_batch_fetch(&mut self) {
        let Some(tracker) = self.batch_fetch.as_mut() else {
            return;
        };
        if tracker.cancelling {
            return;
        }
        tracker.cancelling = true;
        tracker.cancel.store(true, Ordering::Relaxed);
        self.bump_board_version();
    }

    pub(super) fn apply_fetch_cancelled(&mut self, target: WorktreeTarget) {
        if let Some(row) = self.repo_mut(target.id).filter(|r| r.target == target) {
            row.fetching = false;
        }
        self.record_batch_fetch_settlement(target.id, false, true);
        self.bump_board_version();
    }

    fn record_batch_fetch_settlement(&mut self, id: WorktreeId, failed: bool, cancelled: bool) {
        let finished_notice = if let Some(tracker) = self.batch_fetch.as_mut() {
            if !tracker.pending.remove(&id) {
                return;
            }
            if cancelled {
                tracker.cancelled_count += 1;
            } else {
                tracker.completed += 1;
                if failed {
                    tracker.failed += 1;
                }
            }
            if tracker.pending.is_empty() {
                let notice = tracker.cancelling.then(|| {
                    if crate::i18n::is_zh() {
                        format!(
                            "已取消批量刷新（已完成 {}/{}，跳过剩余 {} 个）",
                            tracker.completed, tracker.total, tracker.cancelled_count
                        )
                    } else {
                        format!(
                            "Cancelled batch fetch ({}/{} finished, {} skipped)",
                            tracker.completed, tracker.total, tracker.cancelled_count
                        )
                    }
                });
                Some(notice)
            } else {
                None
            }
        } else {
            return;
        };
        if let Some(notice) = finished_notice {
            self.batch_fetch = None;
            if let Some(text) = notice {
                self.message = Some(text);
            }
        }
    }

    pub(super) fn start_fetch(&mut self, id: WorktreeId) {
        let _ = self.start_fetch_inner(id, None);
    }

    fn start_fetch_inner(&mut self, id: WorktreeId, cancel: Option<Arc<AtomicBool>>) -> bool {
        let common = self.repo(id).and_then(|row| row.common.as_ref());
        if self.writes.iter().any(|(other, write)| {
            write.running()
                && (*other == id
                    || common.is_some()
                        && self.repo(*other).and_then(|r| r.common.as_ref()) == common)
        }) {
            if self.selected == Some(id) {
                self.message = Some("当前仓库的写操作结束后可再次刷新".into());
            }
            return false;
        }
        let Some(row) = self.repo_mut(id) else {
            return false;
        };
        if row.fetching {
            return false;
        }
        row.fetching = true;
        let target = row.target.clone();
        self.bump_board_version();
        let fetch = self.runtime.fetch.clone();
        let events = self.runtime.events_sender();
        self.runtime.status.execute(move || {
            if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                let _ = events.send_blocking(Update(Event::FetchCancelled { target }));
                return;
            }
            let result = git::read_sync_with_common(&target);
            let before = result.snapshot;
            let common = result.common;
            fetch.run(move || {
                if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                    return Event::FetchCancelled {
                        target: before.worktree,
                    };
                }
                let result = git::fetch_upstream(&before);
                Event::Fetched {
                    before,
                    common,
                    result,
                    at: SystemTime::now(),
                }
            });
        });
        true
    }

    pub(super) fn related_worktrees(&self, target: &WorktreeTarget) -> Vec<WorktreeId> {
        let common = self.repo(target.id).and_then(|row| row.common.as_ref());
        self.repos
            .iter()
            .filter(|row| {
                row.target == *target || (common.is_some() && row.common.as_ref() == common)
            })
            .map(|row| row.target.id)
            .collect()
    }

    pub(super) fn apply_fetch(
        &mut self,
        before: SyncSnapshot,
        common: Option<PathBuf>,
        result: Result<FetchOutcome, String>,
        at: SystemTime,
    ) {
        let target = before.worktree;
        let failed = result.is_err();
        let Some(row) = self.repo_mut(target.id).filter(|r| r.target == target) else {
            self.record_batch_fetch_settlement(target.id, failed, false);
            return;
        };
        row.fetching = false;
        row.failure = None;
        let mut affected = vec![target.id];
        match result {
            Err(err) => {
                if source(&before.status).is_none() {
                    self.message = Some(err.clone());
                }
                if let Some(row) = self.repo_mut(target.id) {
                    row.failure = Some((before.status, err));
                    row.publish();
                }
            }
            Ok(outcome) => {
                affected = self.record_fetch_outcome(&target, common.as_deref(), outcome, at)
            }
        }
        self.record_batch_fetch_settlement(target.id, failed, false);
        if self.selected.is_some_and(|id| affected.contains(&id)) {
            self.load_history();
            self.load_branches();
        }
        for id in affected {
            self.start_status(id);
        }
        self.bump_board_version();
    }
    // Standalone Fetch and the network phase of Pull share exactly the same timestamp rules.
    pub(super) fn record_fetch_outcome(
        &mut self,
        target: &WorktreeTarget,
        common: Option<&Path>,
        outcome: FetchOutcome,
        at: SystemTime,
    ) -> Vec<WorktreeId> {
        let mut affected = vec![target.id];
        let FetchOutcome::Fetched {
            config,
            tracking_ref,
            oid,
        } = outcome
        else {
            return affected;
        };
        let reftext = tracking_ref.to_string_lossy();
        let shared = !["refs/bisect/", "refs/worktree/", "refs/rewritten/"]
            .iter()
            .any(|s| reftext.starts_with(s));
        let mut save = vec![target.clone()];
        if shared && common.is_some() {
            for sibling in &self.repos {
                if sibling.target.id == target.id || sibling.common.as_deref() != common {
                    continue;
                }
                affected.push(sibling.target.id);
                if sibling.snapshot.as_ref().and_then(|s| tracking(&s.status))
                    == Some((&config, tracking_ref.as_os_str()))
                {
                    save.push(sibling.target.clone());
                }
            }
        }
        for worktree in save {
            let config = config.clone();
            let reference = tracking_ref.clone();
            if let Some(row) = self.repo_mut(worktree.id) {
                row.receipt = Some(Receipt {
                    config: config.clone(),
                    tracking: reference.clone(),
                    oid: oid.clone(),
                    at,
                });
                if row.snapshot.as_ref().and_then(|s| tracking(&s.status))
                    == Some((&config, reference.as_os_str()))
                {
                    row.failure = None;
                }
            }
            self.runtime.store(move |db| {
                let stored = db
                    .record_fetch_success(&worktree, &config, &reference, at)
                    .map_err(|e| e.to_string())?;
                Ok(Event::FetchTime {
                    target: worktree,
                    config,
                    tracking: reference,
                    at: stored.then_some(at),
                    comparison: None,
                })
            });
        }
        affected
    }

    pub(super) fn known_common_dir(&self, id: WorktreeId) -> Option<PathBuf> {
        self.repo(id).and_then(|row| row.common.clone())
    }
}
