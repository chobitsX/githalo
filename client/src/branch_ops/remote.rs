//! Resolve configured targets and run the confirmed single-ref operation.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{HEADS, STALE, WorktreeState, exact_ref_oid, operation_failure};
use crate::git::{
    FetchOutcome, checked, commit_oid, config_values, failure, is_oid, line_content, os_string,
    read_command, ref_exists, refspec_match, run, split_at_byte,
};
use crate::worktree::{UpstreamConfig, WorktreeTarget};

mod recovery;
use recovery::fast_forward;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteOperation {
    Pull,
    Push,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PullStrategy {
    #[default]
    FastForwardOnly,
    Merge,
    Rebase,
}

impl PullStrategy {
    pub fn label(self) -> &'static str {
        match self {
            Self::FastForwardOnly => {
                crate::i18n::tr("仅快进（--ff-only）", "Fast-forward (--ff-only)")
            }
            Self::Merge => crate::i18n::tr("合并（Merge）", "Merge"),
            Self::Rebase => crate::i18n::tr("变基（Rebase）", "Rebase"),
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::FastForwardOnly => crate::i18n::tr(
                "仅当本地没有分叉提交时快进更新，最安全且不产生额外提交",
                "Update only when the branch can fast-forward cleanly without extra commits",
            ),
            Self::Merge => crate::i18n::tr(
                "分叉时创建合并提交整合上游改动，遇冲突进入合并解决流程",
                "Create a merge commit when diverged; conflicts open the merge resolver",
            ),
            Self::Rebase => crate::i18n::tr(
                "将本地提交重放到上游最新提交之上（自动启用 --autostash），保持线性历史",
                "Replay local commits onto upstream with --autostash to keep history linear",
            ),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PushOptions {
    pub force_with_lease: bool,
    pub up_to_oid: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemotePreview {
    kind: RemoteOperation,
    state: WorktreeState,
    remote: OsString,
    remote_ref: OsString,
    endpoint: Option<RemoteEndpoint>,
    tracking_ref: Option<OsString>,
}

impl RemotePreview {
    pub fn kind(&self) -> RemoteOperation {
        self.kind
    }

    pub fn worktree(&self) -> &WorktreeTarget {
        &self.state.target
    }

    pub fn branch_ref(&self) -> &OsStr {
        self.state.branch_ref.as_deref().expect("preview branch")
    }

    pub fn head_oid(&self) -> &str {
        self.state.head_oid.as_deref().expect("preview HEAD")
    }

    /// `.` is Git's name for an upstream in the same local repository.
    pub fn remote(&self) -> &OsStr {
        &self.remote
    }

    pub fn remote_ref(&self) -> &OsStr {
        &self.remote_ref
    }

    /// The effective URL with authority credentials and query removed.
    pub fn remote_url(&self) -> Option<&OsStr> {
        self.endpoint
            .as_ref()
            .map(|endpoint| endpoint.display.as_os_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteOutcome {
    Pulled {
        head_oid: String,
        fetched: Option<FetchOutcome>,
    },
    PulledWithConflict {
        strategy: PullStrategy,
        fetched: Option<FetchOutcome>,
    },
    Pushed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteFailure {
    pub message: String,
    /// Pull may have already updated the upstream tracking ref before its fast-forward failed.
    pub fetched: Option<FetchOutcome>,
}

impl std::fmt::Display for RemoteFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RemoteFailure {}

impl From<String> for RemoteFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            fetched: None,
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
struct RemoteEndpoint {
    url: OsString,
    display: OsString,
    pack: Option<OsString>,
}

impl std::fmt::Debug for RemoteEndpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("RemoteEndpoint")
            .field(&self.display)
            .finish()
    }
}

pub fn preview_pull(target: &WorktreeTarget) -> Result<RemotePreview, String> {
    let state = WorktreeState::read(target)?;
    let branch = checked_out_branch(&state)?;
    let root = &target.root;
    let name = branch_name(branch)?;
    let merge_refs = config_values(root, &branch_key(&name, ".merge"))?;
    let remote_ref = upstream_ref(&merge_refs)?.to_os_string();
    check_ref(root, &remote_ref)?;
    let remote = fetch_remote(root, &name)?;
    let (endpoint, tracking_ref) = if remote == "." {
        exact_ref_oid(&state, &remote_ref)?;
        (None, None)
    } else {
        let endpoint = remote_endpoint(&state, &remote, RemoteOperation::Pull)?;
        let tracking_ref = tracking_ref(&state, &remote, &remote_ref, RemoteOperation::Pull)?;
        (Some(endpoint), tracking_ref)
    };
    Ok(RemotePreview {
        kind: RemoteOperation::Pull,
        state,
        remote,
        remote_ref,
        endpoint,
        tracking_ref,
    })
}

pub fn preview_push(target: &WorktreeTarget) -> Result<RemotePreview, String> {
    let state = WorktreeState::read(target)?;
    let branch = checked_out_branch(&state)?;
    let root = &target.root;
    let name = branch_name(branch)?;
    let fetch_remote = fetch_remote(root, &name)?;
    let remote = preferred_config(root, &branch_key(&name, ".pushremote"))?
        .or(preferred_config(root, OsStr::new("remote.pushdefault"))?)
        .unwrap_or_else(|| fetch_remote.clone());
    if remote == "." {
        return Err("推送目标是同一仓库中的本地分支，请配置推送远端".into());
    }
    if config_bool(root, &remote_key(&remote, ".mirror"))? {
        return Err("该远端启用了镜像推送，无法确认单个分支的推送目标".into());
    }
    let explicit = config_values(root, &remote_key(&remote, ".push"))?;
    let remote_ref = if explicit.is_empty() {
        default_push_ref(root, branch, &name, remote == fetch_remote)?
    } else {
        configured_push_ref(branch, &name, &explicit)?
    };
    if !remote_ref.as_encoded_bytes().starts_with(HEADS) {
        return Err("普通推送的目标需要是远端分支".into());
    }
    check_ref(root, &remote_ref)?;
    let endpoint = remote_endpoint(&state, &remote, RemoteOperation::Push)?;
    // A named push also updates its local tracking ref. Check it before Git
    // can follow a symbolic ref or write into the local branch/tag namespaces.
    let tracking_ref = tracking_ref(&state, &remote, &remote_ref, RemoteOperation::Push)?;
    Ok(RemotePreview {
        kind: RemoteOperation::Push,
        state,
        remote,
        remote_ref,
        endpoint: Some(endpoint),
        tracking_ref,
    })
}

fn checked_out_branch(state: &WorktreeState) -> Result<&OsStr, String> {
    let branch = state
        .branch_ref
        .as_deref()
        .ok_or("当前处于分离 HEAD 状态")?;
    if state.head_oid.is_none() {
        return Err("当前分支尚无提交".into());
    }
    Ok(branch)
}

fn branch_name(reference: &OsStr) -> Result<OsString, String> {
    os_string(
        reference
            .as_encoded_bytes()
            .strip_prefix(HEADS)
            .ok_or("无效的本地分支引用")?,
    )
}

fn branch_key(name: &OsStr, suffix: &str) -> OsString {
    config_key("branch.", name, suffix)
}

fn remote_key(name: &OsStr, suffix: &str) -> OsString {
    config_key("remote.", name, suffix)
}

fn config_key(prefix: &str, name: &OsStr, suffix: &str) -> OsString {
    let mut key = OsString::from(prefix);
    key.push(name);
    key.push(suffix);
    key
}

fn preferred_config(root: &Path, key: &OsStr) -> Result<Option<OsString>, String> {
    Ok(config_values(root, key)?.pop())
}

fn config_bool(root: &Path, key: &OsStr) -> Result<bool, String> {
    let output = run(read_command(root)
        .args(["config", "--bool", "--get", "--"])
        .arg(key))?;
    match output.status.code() {
        Some(0) => Ok(line_content(&output.stdout)? == b"true"),
        Some(1) if output.stderr.is_empty() => Ok(false),
        _ => Err(failure("无法读取 Git 配置", &output)),
    }
}

fn fetch_remote(root: &Path, name: &OsStr) -> Result<OsString, String> {
    if let Some(remote) = preferred_config(root, &branch_key(name, ".remote"))? {
        return Ok(remote);
    }
    let output = checked(read_command(root).arg("remote"), "无法读取 Git 远端")?;
    if let Ok(remote) = line_content(&output)
        && !remote.contains(&b'\n')
    {
        return os_string(remote);
    }
    Ok("origin".into())
}

fn upstream_ref(references: &[OsString]) -> Result<&OsStr, String> {
    match references {
        [] => Err("当前分支未设置上游".into()),
        [reference] => Ok(reference),
        _ => Err("当前分支配置了多个上游，请先选择单一上游".into()),
    }
}

fn default_push_ref(
    root: &Path,
    branch: &OsStr,
    name: &OsStr,
    same_remote: bool,
) -> Result<OsString, String> {
    let mode = preferred_config(root, OsStr::new("push.default"))?;
    let mode = mode.as_deref().unwrap_or_else(|| OsStr::new("simple"));
    match mode.as_encoded_bytes() {
        b"current" => Ok(branch.to_os_string()),
        b"simple" if !same_remote => Ok(branch.to_os_string()),
        b"simple" => {
            let merge = config_values(root, &branch_key(name, ".merge"))?;
            if !merge.is_empty() && upstream_ref(&merge)? != branch {
                return Err("本地分支与上游分支名称不同，push.default=simple 无法选择目标".into());
            }
            Ok(branch.to_os_string())
        }
        b"upstream" | b"tracking" if same_remote => {
            let merge = config_values(root, &branch_key(name, ".merge"))?;
            if merge.is_empty() && config_bool(root, OsStr::new("push.autosetupremote"))? {
                return Ok(branch.to_os_string());
            }
            Ok(upstream_ref(&merge)?.to_os_string())
        }
        b"upstream" | b"tracking" => Err("push.default=upstream 要求推送到上游远端".into()),
        b"matching" => Err("push.default=matching 无法确认单个分支的推送目标".into()),
        b"nothing" => Err("push.default=nothing，未配置推送目标".into()),
        _ => Err(format!("无法识别 push.default={}", mode.to_string_lossy())),
    }
}

fn configured_push_ref(
    branch: &OsStr,
    short_name: &OsStr,
    rules: &[OsString],
) -> Result<OsString, String> {
    let branch = branch.as_encoded_bytes();
    let short_name = short_name.as_encoded_bytes();
    let mut target = None;
    for rule in rules {
        let rule = rule.as_encoded_bytes();
        if rule.starts_with(b"+") {
            return Err("远端配置了强制推送规则，请检查 remote.*.push".into());
        }
        if let Some(excluded) = rule.strip_prefix(b"^") {
            if excluded == b"HEAD"
                || refspec_match(excluded, branch).is_some()
                || refspec_match(excluded, short_name).is_some()
            {
                return Err("远端的推送规则排除了当前分支".into());
            }
            continue;
        }
        let (source, destination) = split_at_byte(rule, b':').unwrap_or((rule, rule));
        if source.is_empty() || destination.is_empty() {
            return Err("远端配置了匹配推送或删除引用规则，无法执行单分支推送".into());
        }
        let wildcard = source.contains(&b'*');
        let capture = if wildcard {
            refspec_match(source, branch).or_else(|| refspec_match(source, short_name))
        } else {
            (source == b"HEAD" || source == branch || source == short_name).then_some(&[][..])
        };
        let Some(capture) = capture else { continue };
        let destination = if rule == b"HEAD" {
            branch.to_vec()
        } else if let Some((prefix, suffix)) = split_at_byte(destination, b'*') {
            [prefix, capture, suffix].concat()
        } else {
            destination.to_vec()
        };
        if destination == b"HEAD" || destination.starts_with(b"^") {
            return Err("配置的推送目标需要是具体的远端分支".into());
        }
        let destination = if destination.starts_with(b"refs/") {
            os_string(&destination)?
        } else {
            os_string(&[HEADS, &destination].concat())?
        };
        if let Some(previous) = &target
            && *previous != destination
        {
            return Err("当前分支配置了多个推送目标".into());
        }
        target = Some(destination);
    }
    target.ok_or_else(|| "远端的推送规则没有包含当前分支".into())
}

fn remote_endpoint(
    state: &WorktreeState,
    remote: &OsStr,
    kind: RemoteOperation,
) -> Result<RemoteEndpoint, String> {
    let output = if kind == RemoteOperation::Push {
        checked(
            state
                .command()
                .args(["remote", "get-url", "--push", "--all", "--"])
                .arg(remote),
            "无法读取推送远端",
        )?
    } else {
        checked(
            state
                .command()
                .args(["remote", "get-url", "--"])
                .arg(remote),
            "无法读取拉取远端",
        )?
    };
    let value = line_content(&output)?;
    if value.is_empty() || value.contains(&b'\n') || value.contains(&b'\r') {
        return Err("远端需要一个可明确确认的有效地址".into());
    }
    let pack_key = match kind {
        RemoteOperation::Pull => ".uploadpack",
        RemoteOperation::Push => ".receivepack",
    };
    let pack = config_values(&state.target.root, &remote_key(remote, pack_key))?
        .into_iter()
        .next();
    Ok(RemoteEndpoint {
        url: os_string(value)?,
        display: display_url(value)?,
        pack,
    })
}

fn display_url(url: &[u8]) -> Result<OsString, String> {
    let Some(scheme) = url.windows(3).position(|window| window == b"://") else {
        return os_string(url);
    };
    let (prefix, address) = url.split_at(scheme + 3);
    let authority_end = address
        .iter()
        .position(|byte| matches!(byte, b'/' | b'?' | b'#'))
        .unwrap_or(address.len());
    let (authority, path) = address.split_at(authority_end);
    let host = &authority[authority
        .iter()
        .rposition(|&byte| byte == b'@')
        .map_or(0, |i| i + 1)..];
    let path = path
        .split(|byte| matches!(byte, b'?' | b'#'))
        .next()
        .unwrap_or_default();
    os_string(&[prefix, host, path].concat())
}

fn tracking_ref(
    state: &WorktreeState,
    remote: &OsStr,
    source: &OsStr,
    kind: RemoteOperation,
) -> Result<Option<OsString>, String> {
    let root = &state.target.root;
    let source = source.as_encoded_bytes();
    let rules = config_values(root, &remote_key(remote, ".fetch"))?;
    if kind == RemoteOperation::Pull
        && rules.iter().any(|rule| {
            rule.as_encoded_bytes()
                .strip_prefix(b"^")
                .is_some_and(|pattern| refspec_match(pattern, source).is_some())
        })
    {
        return Err("远端的 Fetch 规则排除了所选引用".into());
    }
    for rule in rules {
        let rule = rule.as_encoded_bytes();
        let rule = rule.strip_prefix(b"+").unwrap_or(rule);
        let Some((from, to)) = split_at_byte(rule, b':') else {
            continue;
        };
        let Some(capture) = refspec_match(from, source) else {
            continue;
        };
        if to.is_empty() {
            continue;
        }
        let name = match split_at_byte(to, b'*') {
            Some((prefix, suffix)) => os_string(&[prefix, capture, suffix].concat())?,
            None => os_string(to)?,
        };
        check_ref(root, &name)?;
        if [
            HEADS,
            b"refs/tags/",
            b"refs/replace/",
            b"refs/bisect/",
            b"refs/rewritten/",
            b"refs/worktree/",
        ]
        .iter()
        .any(|prefix| {
            name.as_encoded_bytes()
                .get(..prefix.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
        }) {
            return Err("远端的跟踪引用与本地 Git 引用冲突，无法安全更新".into());
        }
        direct_ref(state, &name)?;
        return Ok(Some(name));
    }
    Ok(None)
}

fn check_ref(root: &Path, reference: &OsStr) -> Result<(), String> {
    if !reference.as_encoded_bytes().starts_with(b"refs/") {
        return Err("操作需要完整的 Git 引用名".into());
    }
    checked(
        read_command(root).arg("check-ref-format").arg(reference),
        "无效的 Git 引用名",
    )
    .map(|_| ())
}

fn direct_ref(state: &WorktreeState, reference: &OsStr) -> Result<(), String> {
    let lookup = run(state
        .command()
        .args(["symbolic-ref", "--quiet", "--no-recurse"])
        .arg(reference))?;
    match lookup.status.code() {
        Some(1) => Ok(()),
        Some(0) => Err("目标跟踪引用是符号引用，无法安全更新".into()),
        _ => Err(failure("无法检查跟踪引用", &lookup)),
    }
}

fn optional_tracking_oid(
    state: &WorktreeState,
    reference: &OsStr,
) -> Result<Option<String>, String> {
    direct_ref(state, reference)?;
    if !ref_exists(&state.target.root, reference)? {
        return Ok(None);
    }
    exact_ref_oid(state, reference).map(Some)
}

pub fn run_remote(preview: &RemotePreview) -> Result<RemoteOutcome, RemoteFailure> {
    run_remote_with_options(
        preview,
        PullStrategy::FastForwardOnly,
        &PushOptions::default(),
    )
}

pub fn run_remote_with_options(
    preview: &RemotePreview,
    pull: PullStrategy,
    push: &PushOptions,
) -> Result<RemoteOutcome, RemoteFailure> {
    verify_preview(preview)?;
    match preview.kind {
        RemoteOperation::Pull => run_pull(preview, pull),
        RemoteOperation::Push => run_push(preview, push).map_err(RemoteFailure::from),
    }
}

fn verify_preview(preview: &RemotePreview) -> Result<(), String> {
    let current = match preview.kind {
        RemoteOperation::Pull => preview_pull(preview.worktree()),
        RemoteOperation::Push => preview_push(preview.worktree()),
    }
    .map_err(|reason| format!("{STALE}（{reason}）"))?;
    if current != *preview {
        return Err(STALE.into());
    }
    Ok(())
}

enum PullIntegration {
    Updated(String),
    Conflict,
}

fn integrate_pull(
    preview: &RemotePreview,
    oid: &str,
    strategy: PullStrategy,
) -> Result<PullIntegration, String> {
    match strategy {
        PullStrategy::FastForwardOnly => fast_forward(preview, oid).map(PullIntegration::Updated),
        PullStrategy::Merge => integrate_pull_merge(preview, oid),
        PullStrategy::Rebase => integrate_pull_rebase(preview, oid),
    }
}

fn integrate_pull_merge(preview: &RemotePreview, oid: &str) -> Result<PullIntegration, String> {
    let state = &preview.state;
    state.verify()?;
    let mut command = state.command();
    use_stored_history(&mut command);
    let output = run(command.env("GIT_EDITOR", "true").env("LC_ALL", "C").args([
        "-c",
        "core.editor=true",
        "-c",
        "submodule.recurse=false",
        "merge",
        "--no-edit",
        "--no-overwrite-ignore",
        "--",
        oid,
    ]))?;
    if output.status.success() {
        return Ok(PullIntegration::Updated(commit_oid(
            &state.target.root,
            preview.branch_ref(),
        )?));
    }
    if state.identity.git_dir.join("MERGE_HEAD").exists()
        || crate::merge_ops::read_state(&state.target)?.is_some()
    {
        return Ok(PullIntegration::Conflict);
    }
    Err(operation_failure("无法合并上游提交", &output))
}

fn integrate_pull_rebase(preview: &RemotePreview, oid: &str) -> Result<PullIntegration, String> {
    let state = &preview.state;
    state.verify()?;
    let mut command = state.command();
    use_stored_history(&mut command);
    let output = run(command
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true")
        .env("LC_ALL", "C")
        .args([
            "-c",
            "core.editor=true",
            "-c",
            "submodule.recurse=false",
            "rebase",
            "--autostash",
            "--",
            oid,
        ]))?;
    if output.status.success() {
        return Ok(PullIntegration::Updated(commit_oid(
            &state.target.root,
            preview.branch_ref(),
        )?));
    }
    if state.identity.git_dir.join("rebase-merge").exists()
        || state.identity.git_dir.join("rebase-apply").exists()
    {
        return Ok(PullIntegration::Conflict);
    }
    Err(operation_failure("无法变基到上游提交", &output))
}

fn run_pull(
    preview: &RemotePreview,
    strategy: PullStrategy,
) -> Result<RemoteOutcome, RemoteFailure> {
    if preview.endpoint.is_none() {
        let oid = exact_ref_oid(&preview.state, &preview.remote_ref)?;
        return match integrate_pull(preview, &oid, strategy)? {
            PullIntegration::Updated(head_oid) => Ok(RemoteOutcome::Pulled {
                head_oid,
                fetched: None,
            }),
            PullIntegration::Conflict => Ok(RemoteOutcome::PulledWithConflict {
                strategy,
                fetched: None,
            }),
        };
    }
    let state = &preview.state;
    let tracking_before = preview
        .tracking_ref
        .as_deref()
        .map(|reference| optional_tracking_oid(state, reference))
        .transpose()?
        .flatten();
    let temporary_ref = format!("refs/githalo/pull/{}", operation_id());
    let mut fetched = None;
    let result = fetch_for_pull(preview, &temporary_ref).and_then(|oid| {
        // Network I/O can outlast a checkout, a commit, or a configuration edit.
        verify_preview(preview)?;
        fetched = match preview.tracking_ref.as_deref() {
            Some(reference)
                if update_tracking_ref(state, reference, &oid, tracking_before.as_deref())? =>
            {
                Some(FetchOutcome::Fetched {
                    config: UpstreamConfig {
                        remote: preview.remote.clone(),
                        merge_ref: preview.remote_ref.clone(),
                    },
                    tracking_ref: reference.to_os_string(),
                    oid: oid.clone(),
                })
            }
            _ => None,
        };
        integrate_pull(preview, &oid, strategy)
    });
    let cleanup = checked(
        state
            .command()
            .args(["update-ref", "--no-deref", "-d", &temporary_ref]),
        "无法清理拉取使用的临时引用",
    );
    match (result, cleanup) {
        (Ok(PullIntegration::Updated(head_oid)), Ok(_)) => {
            Ok(RemoteOutcome::Pulled { head_oid, fetched })
        }
        (Ok(PullIntegration::Conflict), Ok(_)) => {
            Ok(RemoteOutcome::PulledWithConflict { strategy, fetched })
        }
        (Ok(PullIntegration::Updated(oid)), Err(cleanup)) => Err(RemoteFailure {
            message: format!("已拉取至提交 {oid}。{cleanup}"),
            fetched,
        }),
        (Ok(PullIntegration::Conflict), Err(cleanup)) => Err(RemoteFailure {
            message: format!("拉取已产生冲突，请在冲突横幅中处理。{cleanup}"),
            fetched,
        }),
        (Err(reason), Ok(_)) => Err(RemoteFailure {
            message: reason,
            fetched,
        }),
        (Err(reason), Err(cleanup)) => Err(RemoteFailure {
            message: format!("{reason}。{cleanup}"),
            fetched,
        }),
    }
}

fn fetch_for_pull(preview: &RemotePreview, temporary_ref: &str) -> Result<String, String> {
    let endpoint = preview.endpoint.as_ref().expect("remote Pull endpoint");
    let mut command = preview.state.command();
    pin_endpoint(&mut command, preview, endpoint);
    use_stored_history(&mut command);
    command
        .args([
            "fetch",
            "--porcelain",
            "--no-tags",
            "--no-prune",
            "--no-prune-tags",
            "--no-recurse-submodules",
            "--no-update-head-ok",
            "--no-auto-maintenance",
            "--no-write-commit-graph",
            "--no-write-fetch-head",
            "--refmap=",
            "--upload-pack",
        ])
        .arg(
            endpoint
                .pack
                .as_deref()
                .unwrap_or_else(|| OsStr::new("git-upload-pack")),
        );
    let mut refspec = preview.remote_ref.clone();
    refspec.push(":");
    refspec.push(temporary_ref);
    command.arg("--").arg(&preview.remote).arg(refspec);
    // Exclude Git's abbreviation fallbacks if the exact source was deleted.
    for (prefix, suffix) in [
        ("^refs/", ""),
        ("^refs/tags/", ""),
        ("^refs/heads/", ""),
        ("^refs/remotes/", ""),
        ("^refs/remotes/", "/HEAD"),
    ] {
        command.arg(config_key(prefix, &preview.remote_ref, suffix));
    }
    checked(&mut command, "无法拉取上游引用").map_err(|reason| redact_error(&reason, endpoint))?;
    exact_ref_oid(&preview.state, OsStr::new(temporary_ref))
}

fn update_tracking_ref(
    state: &WorktreeState,
    reference: &OsStr,
    oid: &str,
    before: Option<&str>,
) -> Result<bool, String> {
    let current = optional_tracking_oid(state, reference)?;
    if current.as_deref() == Some(oid) {
        return Ok(true);
    }
    if current.as_deref() != before {
        // A concurrent refresh owns this observation. Keep its tracking ref.
        return Ok(false);
    }
    let zero = "0".repeat(oid.len());
    let output = run(state
        .command()
        .args([
            "update-ref",
            "--no-deref",
            "-m",
            "pull: fetch upstream",
            "--",
        ])
        .arg(reference)
        .arg(oid)
        .arg(before.unwrap_or(&zero)))?;
    if output.status.success() {
        return Ok(true);
    }
    let current = optional_tracking_oid(state, reference)?;
    if current.as_deref() == before {
        return Err(failure("无法更新跟踪引用", &output));
    }
    Ok(current.as_deref() == Some(oid))
}

fn run_push(preview: &RemotePreview, options: &PushOptions) -> Result<RemoteOutcome, String> {
    let endpoint = preview.endpoint.as_ref().expect("Push endpoint");
    let push_oid = match options.up_to_oid.as_deref() {
        Some(raw) => {
            if !is_oid(raw.as_bytes()) {
                return Err("指定的推送上界提交 ID 无效".into());
            }
            let resolved = commit_oid(&preview.state.target.root, OsStr::new(raw))?;
            let mut ancestor = preview.state.command();
            use_stored_history(&mut ancestor);
            let check =
                run(ancestor.args(["merge-base", "--is-ancestor", &resolved, preview.head_oid()]))?;
            if !check.status.success() {
                return Err("所选提交不在当前分支的历史祖先链中，无法作为推送上界".into());
            }
            resolved
        }
        None => preview.head_oid().to_owned(),
    };
    let mut command = preview.state.command();
    pin_endpoint(&mut command, preview, endpoint);
    use_stored_history(&mut command);
    let mut refspec = OsString::from(&push_oid);
    refspec.push(":");
    refspec.push(&preview.remote_ref);
    command.args(["push", "--porcelain", "--no-force"]);
    if options.force_with_lease {
        let expected = preview.tracking_ref.as_deref().and_then(|reference| {
            optional_tracking_oid(&preview.state, reference)
                .ok()
                .flatten()
        });
        let mut lease = OsString::from("--force-with-lease=");
        lease.push(&preview.remote_ref);
        if let Some(expected_oid) = expected {
            lease.push(":");
            lease.push(expected_oid);
        }
        command.arg(lease);
    } else {
        command.arg("--no-force-with-lease");
    }
    let output = run(command
        .args([
            "--no-force-if-includes",
            "--no-mirror",
            "--no-follow-tags",
            "--no-recurse-submodules",
            "--no-prune",
            "--receive-pack",
        ])
        .arg(
            endpoint
                .pack
                .as_deref()
                .unwrap_or_else(|| OsStr::new("git-receive-pack")),
        )
        .arg("--")
        .arg(&preview.remote)
        .arg(refspec))?;
    if !output.status.success() {
        let reason = failure("推送失败", &output);
        if let Some(detail) = output
            .stdout
            .split(|&byte| byte == b'\n')
            .filter(|line| line.starts_with(b"!\t"))
            .find_map(|line| line.split(|&byte| byte == b'\t').nth(2))
        {
            return Err(redact_error(
                &format!("{}。{reason}", String::from_utf8_lossy(detail)),
                endpoint,
            ));
        }
        return Err(redact_error(&reason, endpoint));
    }
    Ok(RemoteOutcome::Pushed)
}

fn pin_endpoint(command: &mut Command, preview: &RemotePreview, endpoint: &RemoteEndpoint) {
    // Reset Git's multi-valued URL setting, then resolve a unique alias to
    // the already resolved URL. This keeps the real remote name for hooks
    // without reapplying a different insteadOf rule to the pinned endpoint.
    let alias = OsString::from(format!("githalo:preview-{}", operation_id()));
    let rewrite = config_key("url.", &endpoint.url, ".insteadof");
    let url_key = remote_key(
        &preview.remote,
        match preview.kind {
            RemoteOperation::Pull => ".url",
            RemoteOperation::Push => ".pushurl",
        },
    );
    let mirror = remote_key(&preview.remote, ".mirror");
    command_config(
        command,
        &[
            (&rewrite, &alias),
            (&url_key, OsStr::new("")),
            (&url_key, &alias),
            (&mirror, OsStr::new("false")),
        ],
    );
}

fn command_config(command: &mut Command, entries: &[(&OsStr, &OsStr)]) {
    command.env("GIT_CONFIG_COUNT", entries.len().to_string());
    for (index, (key, value)) in entries.iter().enumerate() {
        command.env(format!("GIT_CONFIG_KEY_{index}"), key);
        command.env(format!("GIT_CONFIG_VALUE_{index}"), value);
    }
}

fn use_stored_history(command: &mut Command) {
    // Local replacement refs and grafts can make diverged commits appear
    // related. Fast-forward checks must use the parents stored in the objects.
    command
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env(
            "GIT_GRAFT_FILE",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .args(["-c", "advice.graftFileDeprecated=false"]);
}

fn redact_error(reason: &str, endpoint: &RemoteEndpoint) -> String {
    reason.replace(
        &*endpoint.url.to_string_lossy(),
        &endpoint.display.to_string_lossy(),
    )
}

fn operation_id() -> String {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{now}-{sequence}", std::process::id())
}
