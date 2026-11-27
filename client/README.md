# GitHalo Rust + GPUI 工程

本目录是 **GitHalo** 的独立 Cargo 工程（包名与可执行文件名均为 `githalo`）。完整产品介绍见根目录 [README.md](../README.md) 与 [README_ZH.md](../README_ZH.md)，参与开发与测试规范见 [CONTRIBUTING.md](../CONTRIBUTING.md)。

## 启动与使用

在项目根目录执行 `cargo run --manifest-path client/Cargo.toml --locked`（或 Release 版 `cargo run --release --manifest-path client/Cargo.toml --locked`）。首次启动从空列表添加本地工作树。左栏的加号菜单可新建分组、克隆或初始化仓库，组菜单可向指定组添加工作树，仓库行菜单可管理所属分组或修正登记路径。

- 仓库行和顶栏仓库选择器共用一份选择。左侧文件夹打开当前工作树的磁盘目录，子目录按需读取。从顶栏切仓会同时换掉目录根、历史和文件详情。返回仓库列表可继续浏览之前的分组。
- ↓ 表示相对配置上游的落后提交数，↑ 表示领先提交数。打开时读取本地引用。点击仓库或组的刷新入口会执行后台 Fetch，单个仓库的错误单独展示。在当前工作树、分支和上游匹配时，可查看已验证的历史计数及对应时间。
- 历史区默认读取最近 200 条可达提交并支持“加载更多”，可按作者、分支、日期（`author:`、`branch:`、`after:`、`before:`）、说明或哈希组合筛选。选择提交后，右侧显示变更文件与提交详情；支持左右分栏并排 Diff 与词级行内差异高亮，并可双击文件打开全宽「提交 Diff」标签页，或查看单文件历史与逐行 Blame。单文件 Diff 预览上限为 512 KiB，截断会在结尾标明。

工作树登记、组成员、展开状态和 Fetch 记录保存在系统本地应用数据目录的 `githalo/workspace.sqlite3`（macOS 默认为 `~/Library/Application Support/githalo/workspace.sqlite3`），视图偏好保存在同目录的 `preferences.ini`。可通过 `GITHALO_DB` 指定独立的 SQLite 工作区文件（向后兼容 `GIT_CLIENT_DB`），通过 `GITHALO_LANG=en` 或 `GITHALO_LANG=zh` 临时覆盖界面语言。

### 暂存、提交与分支

在左侧打开 **Commit**。选一个已暂存或未暂存文件可以查看对应一侧的 Diff。勾选文本变更块后，可以只处理这些块。新增、删除、二进制等显示为整文件操作。存在冲突时先解决冲突。

填写提交说明，使用“提交已暂存内容”提交索引中的内容，或使用“仅提交此文件”“仅提交所选块”限定本次范围。确认窗口展示候选文件与实际 Diff。提交使用 Git 的钩子和签名配置。外部变更导致预览过期，或两侧重叠改动无法安全分开时，界面会给出原因。每个工作树维护自己的提交草稿。

顶栏分支菜单可新建或切换本地分支。Pull 按当前上游执行仅快进更新，分叉时保留当前分支并报告原因。Push 显示实际推送远端和目标分支，包括向自己的 fork 推送的情况。该入口适用于单分支普通推送，多目标、镜像和强制配置会提示用户核对目标。

### 保存与恢复 Stash

选中工作树，打开左侧 **Stash → 保存 Stash…**。可以填说明、勾选包含未跟踪文件或保留暂存区。点一条记录就能查看当时已暂存、未暂存和未跟踪的文件。一个文件同时出现在两行时，分别选中即可对照两份 Diff。

要保留原记录，选 **Apply…**。要在恢复成功后移除记录，选 **Pop…**。需要恢复原来的已暂存内容时，勾选“恢复暂存状态”。当前分支已走远时，可点“恢复到新分支…”从原基点接着工作。**Drop…** 仅删除确认框里显示的那一条。关联工作树共享列表，恢复前可在确认框核对实际目录、分支和条目 ID。

Pop 遇到冲突时，先查看状态提示中的文件，原 Stash 仍在。使用 reftable 引用格式且有多条记录时，可用 Apply 恢复。Pop、Drop 和恢复到新分支有移除记录的步骤，当前版本会在写入前提示这一限制。

## 本地验证

本轮验证环境为 macOS Apple Silicon、Rust/Cargo 1.98.1、Xcode 26.6 和本机 Git 2.50.1。Git 需支持 `fetch --porcelain`、`show-ref --exists`，同文件两侧的局部提交还会用到 `merge-file --object-id`。首次构建需要取得清单中指定修订的 GPUI 源码和锁文件中的依赖。macOS 后端使用系统字体和运行时编译的 Metal 着色器。通过 rustup 安装工具链的终端若找不到 Cargo，可执行 `export PATH="$HOME/.cargo/bin:$PATH"`。

```sh
cd client
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

去掉默认的 `desktop` 特性也可运行整个数据层和集成测试。Git 测试使用[内置离线夹具](src/git/fixtures/README.md)和临时目录。手动基准覆盖后台结果应用到工作区的耗时。

```sh
cargo test --locked --no-default-features
cargo test --release --locked --no-default-features --test integration first_release_timings -- --ignored --nocapture
```

## 模块入口

- [桌面入口](src/main.rs) 与 [GPUI 窗口](src/ui.rs) 负责原生窗口、目录选择、导航和交互。
- [工作区](src/workspace.rs) 管理单一选择、后台任务与数据归属。[登记存储](src/registry.rs) 持久化分组和按实际上游核验的获取记录。
- [Git 同步](src/git.rs)、[历史与 Diff](src/history.rs) 和[项目目录](src/project_files.rs) 分别读取引用、提交和磁盘文件。[工作树模型](src/worktree.rs) 以稳定 ID 和路径版本标识请求目标。
- [暂存与局部提交](src/staging.rs)、[分支与远端操作](src/branch_ops.rs) 和[写操作调度](src/workspace/daily.rs) 共同校验预览、执行目标和完成结果。
- [Stash 列表与恢复](src/stash.rs)、[保存](src/stash/save.rs)及[条目内容](src/stash/content.rs)读写记录并按原始 Git 对象预览三部分 Diff。[工作区](src/workspace/stashes.rs)维护选中条目和切仓后的结果归属。
