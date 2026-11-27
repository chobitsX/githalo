// 合并所有集成测试至单一测试二进制，显著减少 target/debug 磁盘占用与链接耗时。
// 按模块过滤运行示例：cargo test git_sync 或 cargo test --test integration workspace_flow

mod branch_ops;
mod branches_panel_ops;
mod browsing;
mod cherry_pick_ops;
mod cherry_pick_workspace;
mod commit_change_actions;
mod conflict_rollback_amend;
mod daily_workspace;
mod diff_block_apply;
mod git_sync;
mod history_advanced;
mod history_and_merge_ops;
mod rebase_ops;
mod remote_and_upstream_ops;
mod repo_admin_ops;
mod shelf_ops;
mod staging_ops;
mod stash_content;
mod stash_ops;
mod stash_workspace;
mod sync_recovery;
mod workspace_flow;
