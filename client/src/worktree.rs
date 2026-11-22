//! Worktree identity and the result of reading its local sync status.

use std::ffi::OsString;
use std::path::PathBuf;

/// Positive database key assigned when the worktree is registered.
/// It survives path repairs and is never reassigned to another registration.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorktreeId(i64);

impl WorktreeId {
    pub const fn new(value: i64) -> Option<Self> {
        if value > 0 { Some(Self(value)) } else { None }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Registration and filesystem location captured when a request starts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeTarget {
    pub id: WorktreeId,
    /// Normalized root recorded for the worktree. This path may become inaccessible.
    pub root: PathBuf,
    /// Advances on path repair so a pending request can be identified even if the path is reused.
    pub path_revision: u64,
}

/// Configured source of a branch's upstream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpstreamConfig {
    /// Git remote name. Git uses `.` when tracking a local branch.
    pub remote: OsString,
    /// Full reference in the source repository, as in `branch.<name>.merge`.
    pub merge_ref: OsString,
}

/// Commit counts relative to the branch's configured upstream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyncCounts {
    pub ahead: u64,
    pub behind: u64,
}

impl SyncCounts {
    /// Git lists the `HEAD` side first for `rev-list --left-right --count HEAD...@{upstream}`.
    pub const fn from_left_right(head_only: u64, upstream_only: u64) -> Self {
        Self {
            ahead: head_only,
            behind: upstream_only,
        }
    }
}

/// The most specific upstream status available for a checked-out branch with a commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpstreamStatus {
    NotConfigured,
    Unavailable {
        config: UpstreamConfig,
        tracking_ref: Option<OsString>,
    },
    /// Requires evidence that the source ref was deleted.
    Deleted {
        config: UpstreamConfig,
        tracking_ref: Option<OsString>,
    },
    Resolved {
        config: UpstreamConfig,
        /// Full local ref used for comparison, including for a locally tracked branch.
        tracking_ref: OsString,
        /// Full object ID used for comparison.
        oid: String,
        comparison: Result<SyncCounts, String>,
    },
    /// An error prevented reading the configuration or resolving the upstream reliably.
    ReadFailed {
        reason: String,
    },
}

/// State derived from the worktree's HEAD and the upstream it tracks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SyncStatus {
    Branch {
        branch_ref: OsString,
        /// Full object ID used for comparison.
        head_oid: String,
        upstream: UpstreamStatus,
    },
    DetachedHead {
        head_oid: String,
    },
    UnbornBranch {
        branch_ref: OsString,
        upstream: Option<UpstreamConfig>,
    },
    WorktreeUnavailable {
        reason: String,
    },
    /// An error prevented reading HEAD reliably.
    ReadFailed {
        reason: String,
    },
}

impl SyncStatus {
    pub fn counts(&self) -> Option<SyncCounts> {
        match self {
            Self::Branch {
                upstream: UpstreamStatus::Resolved { comparison, .. },
                ..
            } => comparison.as_ref().ok().copied(),
            _ => None,
        }
    }
}

/// Result bound to the registration and path revision read by a request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncSnapshot {
    pub worktree: WorktreeTarget,
    pub status: SyncStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_ids_use_positive_database_keys() {
        for value in [i64::MIN, -1, 0] {
            assert_eq!(WorktreeId::new(value), None);
        }

        for value in [1, i64::MAX] {
            assert_eq!(WorktreeId::new(value).unwrap().get(), value);
        }
    }

    #[test]
    fn a_reused_path_has_a_new_request_target_with_the_same_identity() {
        let before_repair = WorktreeTarget {
            id: WorktreeId::new(1).unwrap(),
            root: PathBuf::from("/work/repository"),
            path_revision: 1,
        };
        let after_repair = WorktreeTarget {
            path_revision: 3,
            ..before_repair.clone()
        };

        assert_eq!(before_repair.id, after_repair.id);
        assert_ne!(before_repair, after_repair);
    }

    #[test]
    fn git_left_right_counts_map_to_local_ahead_and_upstream_behind() {
        let counts = SyncCounts::from_left_right(2, 1);

        assert_eq!(counts.behind, 1);
        assert_eq!(counts.ahead, 2);
    }

    #[test]
    fn zero_counts_are_only_available_after_a_successful_comparison() {
        let branch = |comparison| SyncStatus::Branch {
            branch_ref: "refs/heads/main".into(),
            head_oid: "a".repeat(40),
            upstream: UpstreamStatus::Resolved {
                config: UpstreamConfig {
                    remote: "origin".into(),
                    merge_ref: "refs/heads/main".into(),
                },
                tracking_ref: "refs/remotes/origin/main".into(),
                oid: "a".repeat(40),
                comparison,
            },
        };
        let zeros = SyncCounts::from_left_right(0, 0);

        assert_eq!(branch(Ok(zeros)).counts(), Some(zeros));
        assert_eq!(branch(Err("rev-list failed".into())).counts(), None);
        assert_eq!(
            SyncStatus::UnbornBranch {
                branch_ref: "refs/heads/main".into(),
                upstream: None,
            }
            .counts(),
            None,
        );
    }
}
