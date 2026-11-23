//! Fast, bounded asynchronous discovery of nested Git repositories under a directory.

use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

/// Maximum directory depth below the scanned root (depth 1 = immediate child).
pub const DEFAULT_MAX_DEPTH: usize = 4;
/// Maximum number of directories visited during one scan to stay responsive on huge trees.
pub const DEFAULT_MAX_DIRS: usize = 4_000;

const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    "vendor",
    "Pods",
    "venv",
    ".venv",
    "__pycache__",
    ".gradle",
    ".idea",
    ".vscode",
    ".cache",
    ".cargo",
    ".turbo",
    ".parcel-cache",
    "coverage",
    "DerivedData",
    "tmp",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredNestedRepo {
    pub root: PathBuf,
    pub name: String,
    pub relative_path: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NestedScanOutcome {
    pub repos: Vec<DiscoveredNestedRepo>,
    pub truncated: bool,
}

/// Scans subdirectories of `root` using breadth-first traversal with pruning.
///
/// Never includes `root` itself, never follows symlinks, skips heavy build/dependency
/// directories, and stops descending once a directory contains `.git`.
pub fn scan_nested_repos(root: &Path, max_depth: usize, max_dirs: usize) -> NestedScanOutcome {
    let Ok(canonical_root) = fs::canonicalize(root) else {
        return NestedScanOutcome::default();
    };
    if !canonical_root.is_dir() || max_depth == 0 || max_dirs == 0 {
        return NestedScanOutcome::default();
    }

    let mut queue = VecDeque::new();
    let mut seen_roots = BTreeSet::new();
    seen_roots.insert(canonical_root.clone());
    let mut repos = Vec::new();
    let mut visited_dirs = 0usize;
    let mut truncated = false;

    queue.push_back((canonical_root.clone(), 0usize));

    while let Some((dir, depth)) = queue.pop_front() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };

        let mut children: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .collect();
        children.sort();

        for path in children {
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() || !meta.is_dir() {
                continue;
            }
            let Some(name_os) = path.file_name() else {
                continue;
            };
            let name = name_os.to_string_lossy();
            if should_skip_dir(&name) {
                continue;
            }

            visited_dirs += 1;
            if visited_dirs > max_dirs {
                truncated = true;
                break;
            }

            let next_depth = depth + 1;
            if path.join(".git").exists() {
                let Ok(canonical) = fs::canonicalize(&path) else {
                    continue;
                };
                if !seen_roots.insert(canonical.clone()) {
                    continue;
                }
                let relative_path = canonical
                    .strip_prefix(&canonical_root)
                    .unwrap_or(&canonical)
                    .to_string_lossy()
                    .replace('\\', "/");
                let display_name = canonical
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| name.into_owned());
                repos.push(DiscoveredNestedRepo {
                    root: canonical,
                    name: display_name,
                    relative_path,
                });
                // Stop-at-repo-root: do not descend into source trees of discovered repos.
                continue;
            }

            if next_depth < max_depth {
                queue.push_back((path, next_depth));
            }
        }

        if truncated {
            break;
        }
    }

    repos.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    NestedScanOutcome { repos, truncated }
}

fn should_skip_dir(name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    SKIP_DIRS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_fake_repo(path: &Path, git_file: bool) {
        fs::create_dir_all(path).unwrap();
        if git_file {
            fs::write(path.join(".git"), "gitdir: /tmp/fake\n").unwrap();
        } else {
            fs::create_dir_all(path.join(".git")).unwrap();
        }
    }

    #[test]
    fn discovers_nested_git_dirs_and_files_while_pruning_ignored_trees() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        make_fake_repo(&root, false);

        // Depth 3 nested repos (matches fuxing-octo/项目代码/fosunpharma-octo/octo-*)
        let web = root.join("code/team/octo-web");
        let server = root.join("code/team/octo-server");
        make_fake_repo(&web, false);
        make_fake_repo(&server, true);

        // Nested repo inside a repo's source tree should be skipped (stop-at-repo-root)
        make_fake_repo(&web.join("inner-fixture"), false);

        // Repos inside pruned dirs or hidden dirs should be skipped
        make_fake_repo(&root.join("node_modules/some-pkg"), false);
        make_fake_repo(&root.join("target/debug/subrepo"), false);
        make_fake_repo(&root.join(".hidden/secret-repo"), false);

        // Too deep (depth 5 when max_depth = 4) should not be visited
        make_fake_repo(&root.join("a/b/c/d/too-deep"), false);

        let outcome = scan_nested_repos(&root, DEFAULT_MAX_DEPTH, DEFAULT_MAX_DIRS);
        assert!(!outcome.truncated);
        let names: Vec<_> = outcome.repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["octo-server", "octo-web"]);
        assert_eq!(outcome.repos[0].relative_path, "code/team/octo-server");
        assert_eq!(outcome.repos[1].relative_path, "code/team/octo-web");
    }

    #[test]
    fn respects_max_dirs_budget_and_reports_truncation() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for i in 0..10 {
            make_fake_repo(&root.join(format!("repo-{i:02}")), false);
        }

        let outcome = scan_nested_repos(&root, DEFAULT_MAX_DEPTH, 4);
        assert!(outcome.truncated);
        assert_eq!(outcome.repos.len(), 4);
    }
}
