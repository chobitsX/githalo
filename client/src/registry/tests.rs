use super::*;
use std::fs;
use std::sync::Barrier;
use std::thread;
use tempfile::{TempDir, tempdir};

fn root_in(directory: &TempDir, name: &str) -> PathBuf {
    directory.path().canonicalize().unwrap().join(name)
}

fn upstream() -> UpstreamConfig {
    UpstreamConfig {
        remote: "origin".into(),
        merge_ref: "refs/heads/main".into(),
    }
}

#[test]
fn reopening_preserves_cross_group_memberships_and_expansion() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("registry.sqlite");
    let mut registry = Registry::open(&database).unwrap();
    assert_eq!(
        registry.board().unwrap(),
        RegistryBoard {
            worktrees: Vec::new(),
            groups: Vec::new(),
            ungrouped_expanded: true,
            nested: BTreeMap::new(),
            dismissed_nested: BTreeMap::new(),
        }
    );

    let storefront = registry.create_group("商城").unwrap();
    let shared = registry.create_group("公共组件").unwrap();
    let root = root_in(&directory, "clones/first/repository");
    let main = registry.register(&root, Some(storefront)).unwrap();
    assert_eq!(registry.register(&root, Some(storefront)).unwrap(), main);
    assert_eq!(registry.register(&root, Some(shared)).unwrap(), main);
    registry.add_member(main, shared).unwrap();

    let clone = registry
        .register(
            &root_in(&directory, "clones/second/repository"),
            Some(shared),
        )
        .unwrap();
    let linked = registry
        .register(&root_in(&directory, "linked/repository"), None)
        .unwrap();
    registry.rename_group(storefront, "商城源码").unwrap();
    registry
        .set_group_expanded(Some(storefront), false)
        .unwrap();
    registry.set_group_expanded(None, false).unwrap();
    drop(registry);

    let reopened = Registry::open(&database).unwrap();
    let board = reopened.board().unwrap();
    assert_eq!(
        board
            .worktrees
            .iter()
            .map(|worktree| worktree.id)
            .collect::<Vec<_>>(),
        [main, clone, linked]
    );
    assert_eq!(board.worktrees[0].root, root);
    assert_eq!(board.worktrees[0].path_revision, 1);
    assert_eq!(
        board.groups,
        [
            RegistryGroup {
                id: storefront,
                name: "商城源码".into(),
                expanded: false,
                members: vec![main],
            },
            RegistryGroup {
                id: shared,
                name: "公共组件".into(),
                expanded: true,
                members: vec![main, clone],
            },
        ]
    );
    assert!(!board.ungrouped_expanded);
}

#[test]
fn moving_and_deleting_groups_keep_worktree_registrations() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("registry.sqlite");
    let mut registry = Registry::open(&database).unwrap();
    let source = registry.create_group("Source").unwrap();
    let existing = registry.create_group("Existing").unwrap();
    let destination = registry.create_group("Destination").unwrap();
    let root = root_in(&directory, "working-copy");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("unchanged"), "repository content").unwrap();
    let worktree = registry.register(&root, Some(source)).unwrap();
    registry.add_member(worktree, existing).unwrap();

    registry.move_member(worktree, source, destination).unwrap();
    registry.move_member(worktree, source, destination).unwrap();
    let board = registry.board().unwrap();
    assert!(board.groups[0].members.is_empty());
    assert_eq!(board.groups[1].members, [worktree]);
    assert_eq!(board.groups[2].members, [worktree]);

    registry.remove_member(worktree, destination).unwrap();
    registry.remove_member(worktree, destination).unwrap();
    registry.delete_group(destination).unwrap();
    assert!(
        registry
            .move_member(worktree, existing, destination)
            .is_err()
    );
    assert_eq!(registry.board().unwrap().groups[1].members, [worktree]);

    // Registration and optional membership are committed together.
    let other_root = root_in(&directory, "never-registered");
    assert!(registry.register(&other_root, Some(destination)).is_err());
    registry.delete_group(existing).unwrap();
    let board = registry.board().unwrap();
    assert_eq!(board.worktrees.len(), 1);
    assert_eq!(board.worktrees[0].id, worktree);
    assert!(board.groups.iter().all(|group| group.members.is_empty()));

    registry.add_member(worktree, source).unwrap();
    registry.unregister(worktree).unwrap();
    let board = registry.board().unwrap();
    assert!(board.worktrees.is_empty());
    assert!(board.groups[0].members.is_empty());
    assert_eq!(
        fs::read_to_string(root.join("unchanged")).unwrap(),
        "repository content"
    );
    assert_ne!(registry.register(&root, None).unwrap(), worktree);
}

#[test]
fn path_repair_keeps_the_id_and_rejects_old_fetch_results() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("registry.sqlite");
    let mut registry = Registry::open(&database).unwrap();
    let group = registry.create_group("Repositories").unwrap();
    let original = root_in(&directory, "original");
    let replacement = root_in(&directory, "replacement");
    let occupied = root_in(&directory, "occupied");
    fs::create_dir(&original).unwrap();
    let worktree = registry.register(&original, Some(group)).unwrap();
    let old_target = registry.worktree(worktree).unwrap().unwrap();
    registry.register(&occupied, None).unwrap();

    let upstream = upstream();
    let tracking_ref = OsStr::new("refs/remotes/origin/main");
    let now = SystemTime::now();
    assert!(
        registry
            .record_fetch_success(&old_target, &upstream, tracking_ref, now)
            .unwrap()
    );
    fs::rename(&original, &replacement).unwrap();
    assert!(!original.exists());
    assert!(
        matches!(registry.repair_path(worktree, &occupied), Err(RegistryError::WorktreeRootInUse(path)) if path == occupied)
    );
    assert_eq!(registry.register(&original, Some(group)).unwrap(), worktree);
    assert_eq!(
        registry.worktree(worktree).unwrap(),
        Some(old_target.clone())
    );
    assert_eq!(
        registry
            .last_fetch_success(&old_target, &upstream, tracking_ref)
            .unwrap(),
        Some(now)
    );

    let repaired = registry.repair_path(worktree, &replacement).unwrap();
    assert_eq!(repaired.id, worktree);
    assert_eq!(repaired.root, replacement);
    assert_eq!(repaired.path_revision, old_target.path_revision + 1);
    assert_eq!(registry.board().unwrap().groups[0].members, [worktree]);
    for target in [&old_target, &repaired] {
        assert_eq!(
            registry
                .last_fetch_success(target, &upstream, tracking_ref)
                .unwrap(),
            None
        );
    }
    assert!(
        !registry
            .record_fetch_success(&old_target, &upstream, tracking_ref, now)
            .unwrap()
    );

    let replaced_again = registry.repair_path(worktree, &replacement).unwrap();
    assert_eq!(replaced_again.path_revision, repaired.path_revision + 1);
    fs::rename(&replacement, &original).unwrap();
    let reused = registry.repair_path(worktree, &original).unwrap();
    assert_eq!(reused.root, old_target.root);
    assert!(
        !registry
            .record_fetch_success(&old_target, &upstream, tracking_ref, now)
            .unwrap()
    );
    drop(registry);

    let reopened = Registry::open(&database).unwrap();
    assert_eq!(reopened.worktree(worktree).unwrap(), Some(reused));
    assert_eq!(reopened.board().unwrap().groups[0].members, [worktree]);
}

#[test]
fn fetch_success_is_specific_to_the_worktree_and_both_refs() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("registry.sqlite");
    let mut registry = Registry::open(&database).unwrap();
    let root = root_in(&directory, "primary");
    let first = registry.register(&root, None).unwrap();
    let linked = registry
        .register(&root_in(&directory, "linked"), None)
        .unwrap();
    let first = registry.worktree(first).unwrap().unwrap();
    let linked = registry.worktree(linked).unwrap().unwrap();
    let upstream = upstream();
    let tracking_ref = OsStr::new("refs/remotes/origin/main");
    let now = SystemTime::now();
    let earlier = now - Duration::from_secs(1);
    assert_eq!(
        registry
            .last_fetch_success(&first, &upstream, tracking_ref)
            .unwrap(),
        None
    );
    assert!(
        registry
            .record_fetch_success(&first, &upstream, tracking_ref, now)
            .unwrap()
    );
    assert!(
        registry
            .record_fetch_success(&first, &upstream, tracking_ref, earlier)
            .unwrap()
    );
    assert_eq!(
        registry
            .last_fetch_success(&linked, &upstream, tracking_ref)
            .unwrap(),
        None
    );
    assert!(
        registry
            .record_fetch_success(&linked, &upstream, tracking_ref, earlier)
            .unwrap()
    );
    drop(registry);

    let mut reopened = Registry::open(&database).unwrap();
    assert_eq!(
        reopened
            .last_fetch_success(&first, &upstream, tracking_ref)
            .unwrap(),
        Some(now)
    );
    assert_eq!(
        reopened
            .last_fetch_success(&linked, &upstream, tracking_ref)
            .unwrap(),
        Some(earlier)
    );
    let different_remote = UpstreamConfig {
        remote: "team".into(),
        ..upstream.clone()
    };
    let different_source = UpstreamConfig {
        merge_ref: "refs/heads/release".into(),
        ..upstream.clone()
    };
    let different_tracking = OsStr::new("refs/remotes/origin/release");
    for (source, tracked) in [
        (&different_remote, tracking_ref),
        (&different_source, tracking_ref),
        (&upstream, different_tracking),
    ] {
        assert_eq!(
            reopened
                .last_fetch_success(&first, source, tracked)
                .unwrap(),
            None
        );
    }
    let wrong_root = WorktreeTarget {
        root: linked.root.clone(),
        ..first.clone()
    };
    assert_eq!(
        reopened
            .last_fetch_success(&wrong_root, &upstream, tracking_ref)
            .unwrap(),
        None
    );
    assert!(
        !reopened
            .record_fetch_success(&wrong_root, &upstream, tracking_ref, now)
            .unwrap()
    );

    reopened.unregister(first.id).unwrap();
    assert!(
        !reopened
            .record_fetch_success(&first, &upstream, tracking_ref, now)
            .unwrap()
    );
    let new_id = reopened.register(&root, None).unwrap();
    let new_target = reopened.worktree(new_id).unwrap().unwrap();
    assert_eq!(
        reopened
            .last_fetch_success(&new_target, &upstream, tracking_ref)
            .unwrap(),
        None
    );
}

#[test]
fn concurrent_instances_share_one_registration() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("registry.sqlite");
    let root = root_in(&directory, "shared-root");
    let start = Barrier::new(2);
    let register = |name: &str| {
        start.wait();
        let mut registry = Registry::open(&database).unwrap();
        let group = registry.create_group(name).unwrap();
        registry.register(&root, Some(group)).unwrap()
    };

    let worktree = thread::scope(|scope| {
        let first = scope.spawn(|| register("First"));
        let second = scope.spawn(|| register("Second"));
        let worktree = first.join().unwrap();
        assert_eq!(second.join().unwrap(), worktree);
        worktree
    });
    let reopened = Registry::open(&database).unwrap();
    let board = reopened.board().unwrap();
    assert_eq!(board.worktrees.len(), 1);
    assert_eq!(board.worktrees[0].id, worktree);
    assert_eq!(board.groups.len(), 2);
    assert!(board.groups.iter().all(|group| group.members == [worktree]));
}

#[test]
fn expansion_toggles_use_the_latest_value_across_connections() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("registry.sqlite");
    let mut first = Registry::open(&database).unwrap();
    let mut second = Registry::open(&database).unwrap();
    let group = Some(first.create_group("Repositories").unwrap());
    let original = first.board().unwrap();
    let start = Barrier::new(2);

    thread::scope(|scope| {
        for registry in [&mut first, &mut second] {
            let start = &start;
            scope.spawn(move || {
                start.wait();
                registry.toggle_group_expanded(group).unwrap();
                registry.toggle_group_expanded(None).unwrap();
            });
        }
    });
    assert_eq!(first.board().unwrap(), original);

    second.toggle_group_expanded(group).unwrap();
    second.toggle_group_expanded(None).unwrap();
    let board = first.board().unwrap();
    assert!(!board.groups[0].expanded);
    assert!(!board.ungrouped_expanded);
}

#[cfg(unix)]
#[test]
fn native_paths_and_refs_round_trip_without_unicode_conversion() {
    use std::os::unix::ffi::OsStringExt;

    let directory = tempdir().unwrap();
    let database = directory.path().join("registry.sqlite");
    let mut registry = Registry::open(&database).unwrap();
    let first_root = directory
        .path()
        .join(OsString::from_vec(b"working-\xff".to_vec()));
    let second_root = directory
        .path()
        .join(OsString::from_vec(b"working-\xfe".to_vec()));
    let first_id = registry.register(&first_root, None).unwrap();
    let second_id = registry.register(&second_root, None).unwrap();
    assert_ne!(first_id, second_id);

    let target = registry.worktree(first_id).unwrap().unwrap();
    let upstream = UpstreamConfig {
        remote: OsString::from_vec(b"remote-\xff".to_vec()),
        merge_ref: OsString::from_vec(b"refs/heads/branch-\xff".to_vec()),
    };
    let tracking_ref = OsString::from_vec(b"refs/remotes/remote-\xff/branch-\xff".to_vec());
    let now = SystemTime::now();
    assert!(
        registry
            .record_fetch_success(&target, &upstream, &tracking_ref, now)
            .unwrap()
    );
    drop(registry);

    let reopened = Registry::open(&database).unwrap();
    assert_eq!(
        reopened.worktree(first_id).unwrap().unwrap().root,
        first_root
    );
    assert_eq!(
        reopened.worktree(second_id).unwrap().unwrap().root,
        second_root
    );
    assert_eq!(
        reopened
            .last_fetch_success(&target, &upstream, &tracking_ref)
            .unwrap(),
        Some(now)
    );
}

#[test]
fn opening_a_newer_schema_refuses_to_overwrite_it() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("registry.sqlite");
    let connection = Connection::open(&database).unwrap();
    let newer = SCHEMA_VERSION + 1;
    connection
        .pragma_update(None, "user_version", newer)
        .unwrap();

    assert!(
        matches!(Registry::open(&database), Err(RegistryError::UnsupportedSchema(version)) if version == newer)
    );
    assert_eq!(schema_version(&connection).unwrap(), newer);
}

#[test]
fn version_one_migration_preserves_registrations_and_fetch_times() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("registry.sqlite");
    let connection = Connection::open(&database).unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    connection.execute_batch(SCHEMA_V1).unwrap();
    connection.pragma_update(None, "user_version", 1).unwrap();
    let mut legacy = Registry { connection };

    let group = legacy.create_group("Repositories").unwrap();
    let id = legacy
        .register(&root_in(&directory, "legacy"), Some(group))
        .unwrap();
    let target = legacy.worktree(id).unwrap().unwrap();
    legacy.set_group_expanded(Some(group), false).unwrap();
    legacy.set_group_expanded(None, false).unwrap();
    let upstream = upstream();
    let branch = OsStr::new("refs/heads/main");
    let tracking = OsStr::new("refs/remotes/origin/main");
    let at = SystemTime::now();
    assert!(
        legacy
            .record_fetch_success(&target, &upstream, tracking, at)
            .unwrap()
    );
    let expected_board = RegistryBoard {
        worktrees: vec![target.clone()],
        groups: vec![RegistryGroup {
            id: group,
            name: "Repositories".into(),
            expanded: false,
            members: vec![id],
        }],
        ungrouped_expanded: false,
        nested: BTreeMap::new(),
        dismissed_nested: BTreeMap::new(),
    };
    drop(legacy);

    let mut upgraded = Registry::open(&database).unwrap();
    assert_eq!(
        schema_version(&upgraded.connection).unwrap(),
        SCHEMA_VERSION
    );
    assert_eq!(upgraded.board().unwrap(), expected_board);
    assert_eq!(
        upgraded
            .last_fetch_success(&target, &upstream, tracking)
            .unwrap(),
        Some(at)
    );
    assert_eq!(
        upgraded
            .last_comparison(&target, branch, &upstream, tracking)
            .unwrap(),
        None
    );
    let counts = SyncCounts::from_left_right(u64::MAX, 17);
    assert!(
        upgraded
            .record_comparison(&target, branch, &upstream, tracking, at, counts)
            .unwrap()
    );
    drop(upgraded);

    let reopened = Registry::open(&database).unwrap();
    assert_eq!(
        reopened
            .last_comparison(&target, branch, &upstream, tracking)
            .unwrap(),
        Some(HistoricalCounts { at, counts })
    );
}

#[test]
fn comparison_cache_is_scoped_to_the_branch_and_ref_and_expires_on_path_changes() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("registry.sqlite");
    let mut registry = Registry::open(&database).unwrap();
    let root = root_in(&directory, "worktree");
    let id = registry.register(&root, None).unwrap();
    let target = registry.worktree(id).unwrap().unwrap();
    let branch = OsStr::new("refs/heads/main");
    let upstream = upstream();
    let tracking = OsStr::new("refs/remotes/origin/main");
    let at = SystemTime::now();
    let previous_at = at - Duration::from_secs(1);
    let later_at = at + Duration::from_secs(1);
    let counts = SyncCounts::from_left_right(2, 1);
    let different_counts = SyncCounts::from_left_right(2, 7);

    assert!(
        registry
            .record_comparison(
                &target,
                branch,
                &upstream,
                tracking,
                previous_at,
                different_counts
            )
            .unwrap()
    );
    assert!(
        registry
            .record_comparison(&target, branch, &upstream, tracking, at, counts)
            .unwrap()
    );
    for obsolete_at in [previous_at, at] {
        assert!(
            !registry
                .record_comparison(
                    &target,
                    branch,
                    &upstream,
                    tracking,
                    obsolete_at,
                    different_counts
                )
                .unwrap()
        );
    }
    assert!(
        registry
            .record_fetch_success(&target, &upstream, tracking, later_at)
            .unwrap()
    );
    assert_eq!(
        registry
            .last_comparison(&target, branch, &upstream, tracking)
            .unwrap(),
        Some(HistoricalCounts { at, counts })
    );

    let different_remote = UpstreamConfig {
        remote: "team".into(),
        ..upstream.clone()
    };
    let different_source = UpstreamConfig {
        merge_ref: "refs/heads/release".into(),
        ..upstream.clone()
    };
    let different_tracking = OsStr::new("refs/remotes/origin/release");
    let different_branch = OsStr::new("refs/heads/other");
    for (branch, config, tracking) in [
        (branch, &different_remote, tracking),
        (branch, &different_source, tracking),
        (branch, &upstream, different_tracking),
        (different_branch, &upstream, tracking),
    ] {
        assert_eq!(
            registry
                .last_comparison(&target, branch, config, tracking)
                .unwrap(),
            None
        );
    }

    let repaired = registry.repair_path(id, &root).unwrap();
    assert!(
        !registry
            .record_comparison(&target, branch, &upstream, tracking, later_at, counts)
            .unwrap()
    );
    for target in [&target, &repaired] {
        assert_eq!(
            registry
                .last_comparison(target, branch, &upstream, tracking)
                .unwrap(),
            None
        );
    }
    assert!(
        registry
            .record_comparison(&repaired, branch, &upstream, tracking, later_at, counts)
            .unwrap()
    );
    registry.unregister(id).unwrap();
    assert_eq!(
        registry
            .connection
            .query_row("SELECT count(*) FROM comparisons", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(
        !registry
            .record_comparison(&repaired, branch, &upstream, tracking, later_at, counts)
            .unwrap()
    );
}

#[test]
fn nested_repos_persist_expansion_dismissals_and_cascade_on_remove() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("nested.sqlite");
    let parent_root = root_in(&directory, "fuxing-octo");
    let child_a = root_in(&directory, "fuxing-octo/code/octo-web");
    let child_b = root_in(&directory, "fuxing-octo/code/octo-server");
    let ignored_c = root_in(&directory, "fuxing-octo/code/octo-old");

    let mut registry = Registry::open(&database).unwrap();
    let parent = registry.register(&parent_root, None).unwrap();
    let attached = registry
        .attach_nested_repos(parent, &[child_a.clone(), child_b.clone()])
        .unwrap();
    assert_eq!(attached.len(), 2);
    registry.toggle_nested_expanded(parent).unwrap();
    registry
        .dismiss_nested_paths(parent, std::slice::from_ref(&ignored_c))
        .unwrap();
    drop(registry);

    let mut reopened = Registry::open(&database).unwrap();
    let board = reopened.board().unwrap();
    let nested = board.nested.get(&parent).unwrap();
    assert!(!nested.expanded);
    assert_eq!(nested.children, attached);
    assert_eq!(
        board.dismissed_nested.get(&parent).map(Vec::as_slice),
        Some(std::slice::from_ref(&ignored_c))
    );

    reopened.detach_nested_repo(parent, attached[0]).unwrap();
    assert_eq!(
        reopened
            .board()
            .unwrap()
            .nested
            .get(&parent)
            .unwrap()
            .children,
        vec![attached[1]]
    );

    reopened.unregister(parent).unwrap();
    let after_delete = reopened.board().unwrap();
    assert!(!after_delete.nested.contains_key(&parent));
    assert!(!after_delete.dismissed_nested.contains_key(&parent));
}

#[test]
fn delete_group_with_repos_removes_exclusive_repos_and_nested_children_but_keeps_shared() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("delete-group-with-repos.sqlite");
    let mut registry = Registry::open(&database).unwrap();

    let doomed_group = registry.create_group("Doomed").unwrap();
    let kept_group = registry.create_group("Kept").unwrap();

    let exclusive_parent = registry
        .register(&root_in(&directory, "exclusive-parent"), Some(doomed_group))
        .unwrap();
    let shared_repo = registry
        .register(&root_in(&directory, "shared-repo"), Some(doomed_group))
        .unwrap();
    registry.add_member(shared_repo, kept_group).unwrap();
    let other_parent = registry
        .register(&root_in(&directory, "other-parent"), Some(kept_group))
        .unwrap();

    let exclusive_child_path = root_in(&directory, "exclusive-parent/child-a");
    let shared_child_path = root_in(&directory, "shared-child");
    let grouped_child_path = root_in(&directory, "grouped-child");
    let nested_ids = registry
        .attach_nested_repos(
            exclusive_parent,
            &[
                exclusive_child_path.clone(),
                shared_child_path.clone(),
                grouped_child_path.clone(),
            ],
        )
        .unwrap();
    let exclusive_child = nested_ids[0];
    let shared_child = nested_ids[1];
    let grouped_child = nested_ids[2];

    // Attach shared_child under other_parent too, and place grouped_child in kept_group.
    registry
        .attach_nested_repos(other_parent, std::slice::from_ref(&shared_child_path))
        .unwrap();
    registry.add_member(grouped_child, kept_group).unwrap();

    // Multi-level nested grandchild under exclusive_child.
    let grandchild = registry
        .attach_nested_repos(
            exclusive_child,
            &[root_in(&directory, "exclusive-parent/child-a/grandchild")],
        )
        .unwrap()[0];

    let removed = registry.delete_group_with_repos(doomed_group).unwrap();
    // exclusive_parent + exclusive_child + grandchild = 3 removed
    assert_eq!(removed, 3);

    let board = registry.board().unwrap();
    let remaining_ids: Vec<_> = board.worktrees.iter().map(|w| w.id).collect();
    assert!(!remaining_ids.contains(&exclusive_parent));
    assert!(!remaining_ids.contains(&exclusive_child));
    assert!(!remaining_ids.contains(&grandchild));
    assert!(remaining_ids.contains(&shared_repo));
    assert!(remaining_ids.contains(&other_parent));
    assert!(remaining_ids.contains(&shared_child));
    assert!(remaining_ids.contains(&grouped_child));
    assert_eq!(board.groups.len(), 1);
    assert_eq!(board.groups[0].id, kept_group);
}

#[test]
fn clear_group_empties_custom_group_or_ungrouped_with_nested_children() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("clear-group.sqlite");
    let mut registry = Registry::open(&database).unwrap();

    let group_a = registry.create_group("GroupA").unwrap();
    let group_b = registry.create_group("GroupB").unwrap();

    let a_only = registry
        .register(&root_in(&directory, "a-only"), Some(group_a))
        .unwrap();
    let a_nested = registry
        .attach_nested_repos(a_only, &[root_in(&directory, "a-only/sub")])
        .unwrap()[0];
    let shared = registry
        .register(&root_in(&directory, "shared"), Some(group_a))
        .unwrap();
    registry.add_member(shared, group_b).unwrap();

    let ungrouped_parent = registry
        .register(&root_in(&directory, "ungrouped-parent"), None)
        .unwrap();
    let ungrouped_child = registry
        .attach_nested_repos(
            ungrouped_parent,
            &[root_in(&directory, "ungrouped-parent/sub-1")],
        )
        .unwrap()[0];

    // Clearing custom group A keeps group A itself (now empty), deletes a_only + a_nested, keeps shared in B.
    let cleared_a = registry.clear_group(Some(group_a)).unwrap();
    assert_eq!(cleared_a, 2);
    let board = registry.board().unwrap();
    assert_eq!(board.groups.len(), 2);
    assert!(
        board
            .groups
            .iter()
            .find(|g| g.id == group_a)
            .unwrap()
            .members
            .is_empty()
    );
    assert_eq!(
        board
            .groups
            .iter()
            .find(|g| g.id == group_b)
            .unwrap()
            .members,
        vec![shared]
    );
    let ids: Vec<_> = board.worktrees.iter().map(|w| w.id).collect();
    assert!(!ids.contains(&a_only));
    assert!(!ids.contains(&a_nested));
    assert!(ids.contains(&shared));
    assert!(ids.contains(&ungrouped_parent));
    assert!(ids.contains(&ungrouped_child));

    // Clearing Ungrouped removes ungrouped_parent + ungrouped_child while leaving GroupB intact.
    let cleared_ungrouped = registry.clear_group(None).unwrap();
    assert_eq!(cleared_ungrouped, 2);
    let board = registry.board().unwrap();
    let ids: Vec<_> = board.worktrees.iter().map(|w| w.id).collect();
    assert_eq!(ids, vec![shared]);
}

#[test]
fn unregister_with_nested_optionally_removes_exclusive_children_only() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("unregister-nested.sqlite");
    let mut registry = Registry::open(&database).unwrap();

    let group = registry.create_group("Keep").unwrap();
    let parent_one = registry
        .register(&root_in(&directory, "parent-one"), None)
        .unwrap();
    let parent_two = registry
        .register(&root_in(&directory, "parent-two"), None)
        .unwrap();

    let exclusive_path = root_in(&directory, "parent-one/only-here");
    let grouped_path = root_in(&directory, "parent-one/in-group");
    let children = registry
        .attach_nested_repos(parent_one, &[exclusive_path, grouped_path])
        .unwrap();
    let exclusive_child = children[0];
    let grouped_child = children[1];
    registry.add_member(grouped_child, group).unwrap();

    let removed = registry.unregister_with_nested(parent_one, true).unwrap();
    assert_eq!(removed, 2);
    let ids: Vec<_> = registry
        .board()
        .unwrap()
        .worktrees
        .iter()
        .map(|w| w.id)
        .collect();
    assert!(!ids.contains(&parent_one));
    assert!(!ids.contains(&exclusive_child));
    assert!(ids.contains(&grouped_child));
    assert!(ids.contains(&parent_two));

    // When remove_nested is false, nested children remain registered as independent entries.
    let kept_child = registry
        .attach_nested_repos(parent_two, &[root_in(&directory, "parent-two/child")])
        .unwrap()[0];
    let removed_parent_only = registry.unregister_with_nested(parent_two, false).unwrap();
    assert_eq!(removed_parent_only, 1);
    let ids: Vec<_> = registry
        .board()
        .unwrap()
        .worktrees
        .iter()
        .map(|w| w.id)
        .collect();
    assert!(!ids.contains(&parent_two));
    assert!(ids.contains(&kept_child));
}
