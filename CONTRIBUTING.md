# Contributing to GitHalo

Thank you for your interest in contributing to **GitHalo**! Whether you are fixing a bug, improving cross-platform support (Linux / Windows / macOS Intel), refining UI polish, or adding new Git workflows, contributions are warmly welcomed.

## Prerequisites

- **Rust**: stable toolchain supporting Rust 2024 edition (`rustup update stable`)
- **Git**: 2.43+ recommended (needs `git fetch --porcelain`, `git show-ref --exists`, and `git merge-file --object-id` for partial hunk commits)
- **macOS**: Xcode Command Line Tools (macOS backend uses system fonts and runtime-compiled Metal shaders)
- **Linux** *(experimental)*: Wayland / X11 & Vulkan development headers required by `gpui_platform`
- **Windows** *(experimental)*: MSVC toolchain & Windows SDK required by `gpui_platform`

## Building & Running Locally

```bash
# Run the desktop app in debug mode
cargo run --manifest-path client/Cargo.toml --locked

# Force English or Simplified Chinese UI
GITHALO_LANG=en cargo run --manifest-path client/Cargo.toml --locked
GITHALO_LANG=zh cargo run --manifest-path client/Cargo.toml --locked

# Use an isolated SQLite workspace database during development
GITHALO_DB=/tmp/githalo-dev.sqlite3 cargo run --manifest-path client/Cargo.toml --locked
```

## Quality Checks Before Opening a PR

Please make sure all of the following checks pass with zero warnings:

```bash
cd client

# 1. Formatting
cargo fmt --all -- --check

# 2. Type check across all targets
cargo check --locked --all-targets

# 3. Strict Clippy linting
cargo clippy --locked --all-targets -- -D warnings

# 4. Full test suite (desktop + data layer)
cargo test --locked

# 5. Pure data-layer test suite (headless, no GPUI dependency)
cargo test --locked --no-default-features
```

Git tests use the built-in offline bundle (`client/src/git/fixtures/sync.bundle`) and temporary directories, so the entire test suite runs fast and offline without touching your real repositories.

## Running the Performance Benchmark

```bash
cd client
cargo test --release --locked --no-default-features --test integration first_release_timings -- --ignored --nocapture
```

## Architecture Overview

- `client/src/worktree.rs` — Canonical worktree identity (`WorktreeId` + path generation)
- `client/src/registry.rs` — SQLite persistence for repositories, groups, memberships, and verified fetch records
- `client/src/git.rs` — Read-only status (`ahead/behind`, detached HEAD, upstream checks) and concurrent Fetch
- `client/src/history.rs` — Commit graph, diff parsing, file history, blame, revision comparison, structured filtering
- `client/src/staging.rs` & `client/src/staging/` — Staged/unstaged views, hunk-level staging, and partial commits
- `client/src/branch_ops.rs` & `client/src/branch_ops/` — Branch management, upstream tracking, Pull strategies, safe force Push
- `client/src/merge_ops.rs`, `client/src/rebase_ops.rs`, `client/src/cherry_pick.rs`, `client/src/history_ops.rs`, `client/src/conflict_ops.rs` — Merge, interactive Rebase, Cherry-pick, Reset/Revert, and 3-way conflict resolution
- `client/src/stash.rs` & `client/src/shelf.rs` — Stash inspection/restore and independent patch-based Shelf storage
- `client/src/workspace.rs` & `client/src/workspace/` — Async task runtime, snapshot verification, and state ownership
- `client/src/ui.rs` & `client/src/ui/` — GPUI desktop views, side-by-side word diff, modals, and keyboard shortcuts
- `client/src/i18n.rs` — System locale detection, Chinese/English translations, and preferences persistence

## High-Impact Areas for New Contributors

- **Linux (Wayland / X11) & Windows testing and packaging**
- **Syntax highlighting in Diff / Blame views**
- **Git Submodules & Git LFS workflows**
- **Light theme & customizable color themes**
- **Additional UI translations**

## Pull Request Guidelines

1. Keep PRs focused and well-scoped.
2. Add or update unit/integration tests for any Git behavior or state management change.
3. For UI changes, attach before/after screenshots or short recordings in the PR description.
4. By submitting a contribution, you agree that your work will be licensed under the [Apache License 2.0](LICENSE).
