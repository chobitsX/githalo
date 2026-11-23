//! Persistent worktree registrations, repository groups, and successful fetch times.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

use crate::worktree::{SyncCounts, UpstreamConfig, WorktreeId, WorktreeTarget};

const SCHEMA_VERSION: i64 = 3;

const SCHEMA_V1: &str = "
    CREATE TABLE worktrees (
        id INTEGER PRIMARY KEY AUTOINCREMENT CHECK (id > 0),
        root BLOB NOT NULL UNIQUE,
        path_revision INTEGER NOT NULL DEFAULT 1 CHECK (path_revision > 0),
        UNIQUE (id, path_revision)
    ) STRICT;

    CREATE TABLE groups (
        id INTEGER PRIMARY KEY AUTOINCREMENT CHECK (id > 0),
        name TEXT NOT NULL,
        expanded INTEGER NOT NULL DEFAULT 1 CHECK (expanded IN (0, 1))
    ) STRICT;

    CREATE TABLE group_members (
        group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
        worktree_id INTEGER NOT NULL REFERENCES worktrees(id) ON DELETE CASCADE,
        PRIMARY KEY (group_id, worktree_id)
    ) STRICT, WITHOUT ROWID;
    CREATE INDEX group_members_by_worktree ON group_members(worktree_id);

    CREATE TABLE board_preferences (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        ungrouped_expanded INTEGER NOT NULL CHECK (ungrouped_expanded IN (0, 1))
    ) STRICT;
    INSERT INTO board_preferences (id, ungrouped_expanded) VALUES (1, 1);

    CREATE TABLE fetch_successes (
        worktree_id INTEGER NOT NULL,
        path_revision INTEGER NOT NULL,
        remote BLOB NOT NULL,
        merge_ref BLOB NOT NULL,
        tracking_ref BLOB NOT NULL,
        fetched_at_nanos INTEGER NOT NULL,
        PRIMARY KEY (worktree_id, path_revision, remote, merge_ref, tracking_ref),
        FOREIGN KEY (worktree_id, path_revision) REFERENCES worktrees(id, path_revision)
            ON DELETE CASCADE
    ) STRICT, WITHOUT ROWID;
";

const MIGRATION_V2: &str = "
    CREATE TABLE comparisons (
        worktree_id INTEGER NOT NULL,
        path_revision INTEGER NOT NULL,
        branch_ref BLOB NOT NULL,
        remote BLOB NOT NULL,
        merge_ref BLOB NOT NULL,
        tracking_ref BLOB NOT NULL,
        compared_at_nanos INTEGER NOT NULL,
        ahead BLOB NOT NULL CHECK (length(ahead) = 8),
        behind BLOB NOT NULL CHECK (length(behind) = 8),
        PRIMARY KEY (worktree_id, path_revision, branch_ref, remote, merge_ref, tracking_ref),
        FOREIGN KEY (worktree_id, path_revision) REFERENCES worktrees(id, path_revision)
            ON DELETE CASCADE
    ) STRICT, WITHOUT ROWID;
";

const MIGRATION_V3: &str = "
    CREATE TABLE nested_repos (
        parent_id INTEGER NOT NULL REFERENCES worktrees(id) ON DELETE CASCADE,
        child_id INTEGER NOT NULL REFERENCES worktrees(id) ON DELETE CASCADE,
        PRIMARY KEY (parent_id, child_id),
        CHECK (parent_id != child_id)
    ) STRICT, WITHOUT ROWID;
    CREATE INDEX nested_repos_by_child ON nested_repos(child_id);

    CREATE TABLE nested_preferences (
        parent_id INTEGER PRIMARY KEY REFERENCES worktrees(id) ON DELETE CASCADE,
        expanded INTEGER NOT NULL DEFAULT 1 CHECK (expanded IN (0, 1))
    ) STRICT;

    CREATE TABLE dismissed_nested (
        parent_id INTEGER NOT NULL REFERENCES worktrees(id) ON DELETE CASCADE,
        child_root BLOB NOT NULL,
        PRIMARY KEY (parent_id, child_root)
    ) STRICT, WITHOUT ROWID;
";

/// Stable identity of a custom group. The built-in ungrouped section has no group ID.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GroupId(i64);

impl GroupId {
    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryGroup {
    pub id: GroupId,
    pub name: String,
    pub expanded: bool,
    pub members: Vec<WorktreeId>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RegistryNested {
    pub expanded: bool,
    pub children: Vec<WorktreeId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryBoard {
    pub worktrees: Vec<WorktreeTarget>,
    pub groups: Vec<RegistryGroup>,
    pub ungrouped_expanded: bool,
    pub nested: BTreeMap<WorktreeId, RegistryNested>,
    pub dismissed_nested: BTreeMap<WorktreeId, Vec<PathBuf>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoricalCounts {
    pub at: SystemTime,
    pub counts: SyncCounts,
}

#[derive(Debug)]
pub enum RegistryError {
    Sqlite(rusqlite::Error),
    UnsupportedSchema(i64),
    EmptyGroupName,
    UnknownGroup(GroupId),
    UnknownWorktree(WorktreeId),
    RelativeWorktreeRoot(PathBuf),
    WorktreeRootInUse(PathBuf),
    FetchTimeOutOfRange,
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => error.fmt(f),
            Self::UnsupportedSchema(version) => {
                write!(f, "当前客户端无法读取仓库登记库的版本 {version}")
            }
            Self::EmptyGroupName => f.write_str("请输入分组名称"),
            Self::UnknownGroup(group) => write!(f, "分组 {} 已被移除", group.get()),
            Self::UnknownWorktree(worktree) => {
                write!(f, "工作树登记 {} 已被移除", worktree.get())
            }
            Self::RelativeWorktreeRoot(path) => {
                write!(f, "工作树根目录需要绝对路径（{}）", path.display())
            }
            Self::WorktreeRootInUse(path) => {
                write!(f, "工作树 {} 已登记", path.display())
            }
            Self::FetchTimeOutOfRange => f.write_str("获取时间超出可保存的范围"),
        }
    }
}

impl Error for RegistryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for RegistryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

type Result<T> = std::result::Result<T, RegistryError>;

/// Synchronous access to one SQLite registry. Callers can move it to a background thread.
pub struct Registry {
    connection: Connection,
}

impl Registry {
    /// Opens the database at `path`, creating it in an existing directory if necessary.
    pub fn open(path: &Path) -> Result<Self> {
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", true)?;

        match schema_version(&connection)? {
            SCHEMA_VERSION => {}
            0..SCHEMA_VERSION => {
                // Another instance may already have migrated the database while we wait to write.
                let transaction =
                    connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let version = schema_version(&transaction)?;
                match version {
                    0 => {
                        transaction.execute_batch(SCHEMA_V1)?;
                        transaction.execute_batch(MIGRATION_V2)?;
                        transaction.execute_batch(MIGRATION_V3)?;
                        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
                    }
                    1 => {
                        transaction.execute_batch(MIGRATION_V2)?;
                        transaction.execute_batch(MIGRATION_V3)?;
                        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
                    }
                    2 => {
                        transaction.execute_batch(MIGRATION_V3)?;
                        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
                    }
                    SCHEMA_VERSION => {}
                    version => return Err(RegistryError::UnsupportedSchema(version)),
                }
                transaction.commit()?;
            }
            version => return Err(RegistryError::UnsupportedSchema(version)),
        }

        Ok(Self { connection })
    }

    /// Reads registrations and memberships from one consistent snapshot.
    pub fn board(&self) -> Result<RegistryBoard> {
        let transaction = self.connection.unchecked_transaction()?;
        let worktrees = {
            let mut statement =
                transaction.prepare("SELECT id, root, path_revision FROM worktrees ORDER BY id")?;
            statement
                .query_map([], read_worktree)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut groups = {
            let mut statement = transaction.prepare("SELECT id, name, expanded FROM groups")?;
            statement
                .query_map([], |row| {
                    let id = GroupId(row.get(0)?);
                    Ok((
                        id,
                        RegistryGroup {
                            id,
                            name: row.get(1)?,
                            expanded: row.get(2)?,
                            members: Vec::new(),
                        },
                    ))
                })?
                .collect::<rusqlite::Result<BTreeMap<_, _>>>()?
        };
        {
            let mut statement = transaction.prepare(
                "SELECT group_id, worktree_id FROM group_members ORDER BY group_id, worktree_id",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let group = GroupId(row.get(0)?);
                let worktree = read_worktree_id(row, 1)?;
                groups
                    .get_mut(&group)
                    .ok_or(RegistryError::UnknownGroup(group))?
                    .members
                    .push(worktree);
            }
        }
        let ungrouped_expanded = transaction.query_row(
            "SELECT ungrouped_expanded FROM board_preferences WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        let mut nested: BTreeMap<WorktreeId, RegistryNested> = BTreeMap::new();
        {
            let mut statement = transaction.prepare(
                "SELECT parent_id, child_id FROM nested_repos ORDER BY parent_id, child_id",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let parent = read_worktree_id(row, 0)?;
                let child = read_worktree_id(row, 1)?;
                nested
                    .entry(parent)
                    .or_insert_with(|| RegistryNested {
                        expanded: true,
                        children: Vec::new(),
                    })
                    .children
                    .push(child);
            }
        }
        {
            let mut statement =
                transaction.prepare("SELECT parent_id, expanded FROM nested_preferences")?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let parent = read_worktree_id(row, 0)?;
                let expanded: bool = row.get(1)?;
                if let Some(entry) = nested.get_mut(&parent) {
                    entry.expanded = expanded;
                }
            }
        }
        let mut dismissed_nested: BTreeMap<WorktreeId, Vec<PathBuf>> = BTreeMap::new();
        {
            let mut statement = transaction.prepare(
                "SELECT parent_id, child_root FROM dismissed_nested ORDER BY parent_id, child_root",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let parent = read_worktree_id(row, 0)?;
                let path = PathBuf::from(read_os_string(row, 1)?);
                dismissed_nested.entry(parent).or_default().push(path);
            }
        }
        transaction.commit()?;

        Ok(RegistryBoard {
            worktrees,
            groups: groups.into_values().collect(),
            ungrouped_expanded,
            nested,
            dismissed_nested,
        })
    }

    pub fn worktree(&self, id: WorktreeId) -> Result<Option<WorktreeTarget>> {
        Ok(self
            .connection
            .query_row(
                "SELECT id, root, path_revision FROM worktrees WHERE id = ?1",
                [id.get()],
                read_worktree,
            )
            .optional()?)
    }

    pub fn create_group(&mut self, name: &str) -> Result<GroupId> {
        let name = group_name(name)?;
        self.connection
            .execute("INSERT INTO groups (name) VALUES (?1)", [name])?;
        Ok(GroupId(self.connection.last_insert_rowid()))
    }

    pub fn rename_group(&mut self, group: GroupId, name: &str) -> Result<()> {
        let name = group_name(name)?;
        let rows = self.connection.execute(
            "UPDATE groups SET name = ?1 WHERE id = ?2",
            params![name, group.get()],
        )?;
        if rows == 0 {
            return Err(RegistryError::UnknownGroup(group));
        }
        Ok(())
    }

    pub fn delete_group(&mut self, group: GroupId) -> Result<()> {
        self.connection
            .execute("DELETE FROM groups WHERE id = ?1", [group.get()])?;
        Ok(())
    }

    pub fn set_group_expanded(&mut self, group: Option<GroupId>, expanded: bool) -> Result<()> {
        self.update_group_expansion(group, Some(expanded))
    }

    /// Toggles expansion in the database without using a potentially stale board snapshot.
    pub fn toggle_group_expanded(&mut self, group: Option<GroupId>) -> Result<()> {
        self.update_group_expansion(group, None)
    }

    fn update_group_expansion(
        &mut self,
        group: Option<GroupId>,
        expanded: Option<bool>,
    ) -> Result<()> {
        if let Some(group) = group {
            let rows = self.connection.execute(
                "UPDATE groups SET expanded = coalesce(?1, NOT expanded) WHERE id = ?2",
                params![expanded, group.get()],
            )?;
            if rows == 0 {
                return Err(RegistryError::UnknownGroup(group));
            }
        } else {
            self.connection.execute(
                "UPDATE board_preferences
                 SET ungrouped_expanded = coalesce(?1, NOT ungrouped_expanded) WHERE id = 1",
                [expanded],
            )?;
        }
        Ok(())
    }

    /// Registers an already normalized worktree root, reusing its ID on repeated calls.
    pub fn register(&mut self, root: &Path, group: Option<GroupId>) -> Result<WorktreeId> {
        let encoded_root = encode_root(root)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id = match worktree_id_at(&transaction, &encoded_root)? {
            Some(id) => id,
            None => transaction.query_row(
                "INSERT INTO worktrees (root) VALUES (?1) RETURNING id",
                [encoded_root],
                |row| read_worktree_id(row, 0),
            )?,
        };
        if let Some(group) = group {
            insert_member(&transaction, id, group)?;
        }
        transaction.commit()?;
        Ok(id)
    }

    pub fn unregister(&mut self, worktree: WorktreeId) -> Result<()> {
        self.connection
            .execute("DELETE FROM worktrees WHERE id = ?1", [worktree.get()])?;
        Ok(())
    }

    pub fn add_member(&mut self, worktree: WorktreeId, group: GroupId) -> Result<()> {
        insert_member(&self.connection, worktree, group)?;
        Ok(())
    }

    pub fn remove_member(&mut self, worktree: WorktreeId, group: GroupId) -> Result<()> {
        delete_member(&self.connection, worktree, group)?;
        Ok(())
    }

    /// Registers child repository roots and attaches them as nested repositories under `parent`.
    pub fn attach_nested_repos(
        &mut self,
        parent: WorktreeId,
        child_roots: &[PathBuf],
    ) -> Result<Vec<WorktreeId>> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let parent_exists: bool = transaction
            .query_row(
                "SELECT 1 FROM worktrees WHERE id = ?1",
                [parent.get()],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !parent_exists {
            return Err(RegistryError::UnknownWorktree(parent));
        }

        let mut attached = Vec::with_capacity(child_roots.len());
        for root in child_roots {
            let encoded_root = encode_root(root)?;
            let child_id = match worktree_id_at(&transaction, &encoded_root)? {
                Some(id) => id,
                None => transaction.query_row(
                    "INSERT INTO worktrees (root) VALUES (?1) RETURNING id",
                    [encoded_root.as_ref()],
                    |row| read_worktree_id(row, 0),
                )?,
            };
            if child_id == parent {
                continue;
            }
            transaction.execute(
                "INSERT OR IGNORE INTO nested_repos (parent_id, child_id) VALUES (?1, ?2)",
                params![parent.get(), child_id.get()],
            )?;
            transaction.execute(
                "DELETE FROM dismissed_nested WHERE parent_id = ?1 AND child_root = ?2",
                params![parent.get(), encoded_root.as_ref()],
            )?;
            attached.push(child_id);
        }
        if !attached.is_empty() {
            transaction.execute(
                "INSERT INTO nested_preferences (parent_id, expanded) VALUES (?1, 1)
                 ON CONFLICT(parent_id) DO UPDATE SET expanded = 1",
                [parent.get()],
            )?;
        }
        transaction.commit()?;
        Ok(attached)
    }

    pub fn detach_nested_repo(&mut self, parent: WorktreeId, child: WorktreeId) -> Result<()> {
        self.connection.execute(
            "DELETE FROM nested_repos WHERE parent_id = ?1 AND child_id = ?2",
            params![parent.get(), child.get()],
        )?;
        Ok(())
    }

    pub fn toggle_nested_expanded(&mut self, parent: WorktreeId) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<bool> = transaction
            .query_row(
                "SELECT expanded FROM nested_preferences WHERE parent_id = ?1",
                [parent.get()],
                |row| row.get(0),
            )
            .optional()?;
        let next = !current.unwrap_or(true);
        transaction.execute(
            "INSERT INTO nested_preferences (parent_id, expanded) VALUES (?1, ?2)
             ON CONFLICT(parent_id) DO UPDATE SET expanded = excluded.expanded",
            params![parent.get(), next],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn dismiss_nested_paths(&mut self, parent: WorktreeId, roots: &[PathBuf]) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for root in roots {
            let encoded = encode_root(root)?;
            transaction.execute(
                "INSERT OR IGNORE INTO dismissed_nested (parent_id, child_root) VALUES (?1, ?2)",
                params![parent.get(), encoded.as_ref()],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn clear_dismissed_nested(&mut self, parent: WorktreeId) -> Result<()> {
        self.connection.execute(
            "DELETE FROM dismissed_nested WHERE parent_id = ?1",
            [parent.get()],
        )?;
        Ok(())
    }

    pub fn move_member(
        &mut self,
        worktree: WorktreeId,
        source: GroupId,
        destination: GroupId,
    ) -> Result<()> {
        if source == destination {
            return Ok(());
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        insert_member(&transaction, worktree, destination)?;
        delete_member(&transaction, worktree, source)?;
        transaction.commit()?;
        Ok(())
    }

    /// Changes the normalized root and invalidates prior requests and cached results for this ID.
    pub fn repair_path(&mut self, worktree: WorktreeId, root: &Path) -> Result<WorktreeTarget> {
        let encoded_root = encode_root(root)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if worktree_id_at(&transaction, &encoded_root)?.is_some_and(|id| id != worktree) {
            return Err(RegistryError::WorktreeRootInUse(root.to_owned()));
        }
        transaction.execute(
            "DELETE FROM fetch_successes WHERE worktree_id = ?1",
            [worktree.get()],
        )?;
        transaction.execute(
            "DELETE FROM comparisons WHERE worktree_id = ?1",
            [worktree.get()],
        )?;
        let updated = transaction
            .query_row(
                "UPDATE worktrees SET root = ?1, path_revision = path_revision + 1
                 WHERE id = ?2 RETURNING id, root, path_revision",
                params![encoded_root, worktree.get()],
                read_worktree,
            )
            .optional()?
            .ok_or(RegistryError::UnknownWorktree(worktree))?;
        transaction.commit()?;
        Ok(updated)
    }

    /// Records an actual remote fetch for a particular comparison target.
    /// Returns `false` when the registration was removed or its path was repaired.
    pub fn record_fetch_success(
        &mut self,
        worktree: &WorktreeTarget,
        upstream: &UpstreamConfig,
        tracking_ref: &OsStr,
        fetched_at: SystemTime,
    ) -> Result<bool> {
        let Ok(path_revision) = i64::try_from(worktree.path_revision) else {
            return Ok(false);
        };
        let rows = self.connection.execute(
            "INSERT INTO fetch_successes
                 (worktree_id, path_revision, remote, merge_ref, tracking_ref, fetched_at_nanos)
             SELECT id, path_revision, ?4, ?5, ?6, ?7 FROM worktrees
             WHERE id = ?1 AND path_revision = ?2 AND root = ?3
             ON CONFLICT (worktree_id, path_revision, remote, merge_ref, tracking_ref)
             DO UPDATE SET fetched_at_nanos = max(fetch_successes.fetched_at_nanos, excluded.fetched_at_nanos)",
            params![
                worktree.id.get(),
                path_revision,
                encode_os_str(worktree.root.as_os_str()),
                encode_os_str(&upstream.remote),
                encode_os_str(&upstream.merge_ref),
                encode_os_str(tracking_ref),
                encode_fetch_time(fetched_at)?,
            ],
        )?;
        Ok(rows != 0)
    }

    /// `None` represents an unknown fetch time for this comparison target.
    pub fn last_fetch_success(
        &self,
        worktree: &WorktreeTarget,
        upstream: &UpstreamConfig,
        tracking_ref: &OsStr,
    ) -> Result<Option<SystemTime>> {
        let Ok(path_revision) = i64::try_from(worktree.path_revision) else {
            return Ok(None);
        };
        let nanos = self
            .connection
            .query_row(
                "SELECT successes.fetched_at_nanos FROM fetch_successes AS successes
                 JOIN worktrees ON worktrees.id = successes.worktree_id
                     AND worktrees.path_revision = successes.path_revision
                 WHERE successes.worktree_id = ?1 AND successes.path_revision = ?2
                   AND worktrees.root = ?3 AND successes.remote = ?4
                   AND successes.merge_ref = ?5 AND successes.tracking_ref = ?6",
                params![
                    worktree.id.get(),
                    path_revision,
                    encode_os_str(worktree.root.as_os_str()),
                    encode_os_str(&upstream.remote),
                    encode_os_str(&upstream.merge_ref),
                    encode_os_str(tracking_ref),
                ],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        nanos.map(decode_fetch_time).transpose()
    }

    /// Keeps counts verified for a successful fetch on the named branch.
    /// Returns `false` when the registration changed or the stored comparison is as recent.
    pub fn record_comparison(
        &mut self,
        worktree: &WorktreeTarget,
        branch: &OsStr,
        upstream: &UpstreamConfig,
        tracking_ref: &OsStr,
        at: SystemTime,
        counts: SyncCounts,
    ) -> Result<bool> {
        let Ok(path_revision) = i64::try_from(worktree.path_revision) else {
            return Ok(false);
        };
        let rows = self.connection.execute(
            "INSERT INTO comparisons
                 (worktree_id, path_revision, branch_ref, remote, merge_ref, tracking_ref,
                  compared_at_nanos, ahead, behind)
             SELECT id, path_revision, ?4, ?5, ?6, ?7, ?8, ?9, ?10 FROM worktrees
             WHERE id = ?1 AND path_revision = ?2 AND root = ?3
             ON CONFLICT (worktree_id, path_revision, branch_ref, remote, merge_ref, tracking_ref)
             DO UPDATE SET compared_at_nanos = excluded.compared_at_nanos,
                           ahead = excluded.ahead, behind = excluded.behind
             WHERE excluded.compared_at_nanos > comparisons.compared_at_nanos",
            params![
                worktree.id.get(),
                path_revision,
                encode_os_str(worktree.root.as_os_str()),
                encode_os_str(branch),
                encode_os_str(&upstream.remote),
                encode_os_str(&upstream.merge_ref),
                encode_os_str(tracking_ref),
                encode_fetch_time(at)?,
                counts.ahead.to_be_bytes(),
                counts.behind.to_be_bytes(),
            ],
        )?;
        Ok(rows != 0)
    }

    pub fn last_comparison(
        &self,
        worktree: &WorktreeTarget,
        branch: &OsStr,
        upstream: &UpstreamConfig,
        tracking_ref: &OsStr,
    ) -> Result<Option<HistoricalCounts>> {
        let Ok(path_revision) = i64::try_from(worktree.path_revision) else {
            return Ok(None);
        };
        let comparison = self
            .connection
            .query_row(
                "SELECT comparisons.compared_at_nanos, comparisons.ahead, comparisons.behind
                 FROM comparisons JOIN worktrees ON worktrees.id = comparisons.worktree_id
                     AND worktrees.path_revision = comparisons.path_revision
                 WHERE comparisons.worktree_id = ?1 AND comparisons.path_revision = ?2
                   AND worktrees.root = ?3 AND comparisons.branch_ref = ?4
                   AND comparisons.remote = ?5 AND comparisons.merge_ref = ?6
                   AND comparisons.tracking_ref = ?7",
                params![
                    worktree.id.get(),
                    path_revision,
                    encode_os_str(worktree.root.as_os_str()),
                    encode_os_str(branch),
                    encode_os_str(&upstream.remote),
                    encode_os_str(&upstream.merge_ref),
                    encode_os_str(tracking_ref),
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        SyncCounts {
                            ahead: u64::from_be_bytes(row.get(1)?),
                            behind: u64::from_be_bytes(row.get(2)?),
                        },
                    ))
                },
            )
            .optional()?;
        comparison
            .map(|(nanos, counts)| {
                Ok(HistoricalCounts {
                    at: decode_fetch_time(nanos)?,
                    counts,
                })
            })
            .transpose()
    }
}

fn schema_version(connection: &Connection) -> rusqlite::Result<i64> {
    connection.pragma_query_value(None, "user_version", |row| row.get(0))
}

fn group_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(RegistryError::EmptyGroupName);
    }
    Ok(name)
}

fn read_worktree_id(row: &Row<'_>, column: usize) -> rusqlite::Result<WorktreeId> {
    let value = row.get(column)?;
    WorktreeId::new(value).ok_or(rusqlite::Error::IntegralValueOutOfRange(column, value))
}

fn read_worktree(row: &Row<'_>) -> rusqlite::Result<WorktreeTarget> {
    let path_revision: i64 = row.get(2)?;
    Ok(WorktreeTarget {
        id: read_worktree_id(row, 0)?,
        root: PathBuf::from(read_os_string(row, 1)?),
        path_revision: u64::try_from(path_revision)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, path_revision))?,
    })
}

fn worktree_id_at(connection: &Connection, root: &[u8]) -> rusqlite::Result<Option<WorktreeId>> {
    connection
        .query_row("SELECT id FROM worktrees WHERE root = ?1", [root], |row| {
            read_worktree_id(row, 0)
        })
        .optional()
}

fn insert_member(
    connection: &Connection,
    worktree: WorktreeId,
    group: GroupId,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO group_members (group_id, worktree_id) VALUES (?1, ?2)
         ON CONFLICT (group_id, worktree_id) DO NOTHING",
        params![group.get(), worktree.get()],
    )?;
    Ok(())
}

fn delete_member(
    connection: &Connection,
    worktree: WorktreeId,
    group: GroupId,
) -> rusqlite::Result<()> {
    connection.execute(
        "DELETE FROM group_members WHERE group_id = ?1 AND worktree_id = ?2",
        params![group.get(), worktree.get()],
    )?;
    Ok(())
}

fn encode_root(root: &Path) -> Result<Cow<'_, [u8]>> {
    if !root.is_absolute() {
        return Err(RegistryError::RelativeWorktreeRoot(root.to_owned()));
    }
    Ok(encode_os_str(root.as_os_str()))
}

// The native encodings preserve whole worktree roots and ref names, including non-Unicode names.
#[cfg(unix)]
fn encode_os_str(value: &OsStr) -> Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;

    Cow::Borrowed(value.as_bytes())
}

#[cfg(unix)]
fn read_os_string(row: &Row<'_>, column: usize) -> rusqlite::Result<OsString> {
    use std::os::unix::ffi::OsStringExt;

    Ok(OsString::from_vec(row.get(column)?))
}

#[cfg(windows)]
fn encode_os_str(value: &OsStr) -> Cow<'_, [u8]> {
    use std::os::windows::ffi::OsStrExt;

    Cow::Owned(value.encode_wide().flat_map(u16::to_le_bytes).collect())
}

#[cfg(windows)]
fn read_os_string(row: &Row<'_>, column: usize) -> rusqlite::Result<OsString> {
    use std::io::{Error, ErrorKind};
    use std::os::windows::ffi::OsStringExt;

    let bytes: Vec<u8> = row.get(column)?;
    let (code_units, remainder) = bytes.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Blob,
            Box::new(Error::new(
                ErrorKind::InvalidData,
                "Incomplete Windows path encoding",
            )),
        ));
    }
    let wide: Vec<_> = code_units.iter().copied().map(u16::from_le_bytes).collect();
    Ok(OsString::from_wide(&wide))
}

fn encode_fetch_time(time: SystemTime) -> Result<i64> {
    let nanos = match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    };
    i64::try_from(nanos).map_err(|_| RegistryError::FetchTimeOutOfRange)
}

fn decode_fetch_time(nanos: i64) -> Result<SystemTime> {
    let duration = Duration::from_nanos(nanos.unsigned_abs());
    let time = if nanos < 0 {
        UNIX_EPOCH.checked_sub(duration)
    } else {
        UNIX_EPOCH.checked_add(duration)
    };
    time.ok_or(RegistryError::FetchTimeOutOfRange)
}

#[cfg(test)]
mod tests;
