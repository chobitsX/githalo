use super::*;
use crate::registry::HistoricalCounts;
use crate::worktree::{SyncCounts, SyncStatus, UpstreamStatus};
use std::ffi::OsStr;

#[derive(Clone, Debug)]
pub struct RepoRow {
    pub target: WorktreeTarget,
    pub name: String,
    pub branch: String,
    pub upstream: Option<String>,
    pub upstream_is_local: bool,
    pub counts: Option<SyncCounts>,
    pub issue: Option<String>,
    pub historical: bool,
    pub historical_at: Option<SystemTime>,
    pub loading: bool,
    pub fetching: bool,
    pub last_fetch: Option<SystemTime>,
    snapshot: Option<SyncSnapshot>,
    common: Option<PathBuf>,
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
    pub(super) fn new(target: WorktreeTarget) -> Self {
        let name = target
            .root
            .file_name()
            .unwrap_or(target.root.as_os_str())
            .to_string_lossy()
            .into_owned();
        Self {
            target,
            name,
            branch: "—".into(),
            upstream: None,
            upstream_is_local: false,
            counts: None,
            issue: None,
            historical: false,
            historical_at: None,
            loading: false,
            fetching: false,
            last_fetch: None,
            snapshot: None,
            common: None,
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
        let Some(row) = self.repo_mut(id) else {
            return;
        };
        if row.loading {
            row.read_again = true;
            return;
        }
        row.loading = true;
        let target = row.target.clone();
        self.runtime.status.run(move || Event::Status {
            common: git::common_dir(&target.root).ok(),
            snapshot: git::read_sync(&target),
        });
    }

    pub(super) fn apply_status(&mut self, snapshot: SyncSnapshot, common: Option<PathBuf>) {
        let target = snapshot.worktree.clone();
        let Some(row) = self.repo_mut(target.id).filter(|r| r.target == target) else {
            return;
        };
        row.loading = false;
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
        row.snapshot = Some(snapshot);
        row.publish();
        if let Some((branch, config, tracking)) = lookup {
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
        }
    }

    pub(super) fn start_fetch(&mut self, id: WorktreeId) {
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
            return;
        }
        let Some(row) = self.repo_mut(id) else {
            return;
        };
        if row.fetching {
            return;
        }
        row.fetching = true;
        let target = row.target.clone();
        let fetch = self.runtime.fetch.clone();
        self.runtime.status.execute(move || {
            let before = git::read_sync(&target);
            let common = git::common_dir(&target.root).ok();
            fetch.run(move || {
                let result = git::fetch_upstream(&before);
                Event::Fetched {
                    before,
                    common,
                    result,
                    at: SystemTime::now(),
                }
            });
        });
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
        let Some(row) = self.repo_mut(target.id).filter(|r| r.target == target) else {
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
        if self.selected.is_some_and(|id| affected.contains(&id)) {
            self.load_history();
            self.load_branches();
        }
        for id in affected {
            self.start_status(id);
        }
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
