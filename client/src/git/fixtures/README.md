# Git 同步测试夹具

`sync.bundle` 包含 Scap 公开历史中的 9 个原始提交及其 Git 对象。夹具引用从三个既有提交建立，离线克隆后即可改变临时引用来复现同步状态。Scap 的 MIT 许可证随夹具附上。[Scap 源仓库](https://github.com/zed-industries/scap) · [MIT 许可证](SCAP-LICENSE.txt)

| 引用 | 原始提交 |
| --- | --- |
| `base` | `e6283f19a602c1e6053d789c8b42f4b2a4cbeb45` |
| `local` | `2723c89e3dff59c2ac85d1de129abd5cbbc15f21` |
| `upstream` | `f79784940c06decd657c97c15fc2a9c07f385087` |

以 `base` 为共同祖先，`local` 多两个提交，`upstream` 多一个提交。`base~3` 比 `base` 少三个提交。`base` 和 `upstream` 在 `src/main.rs` 中包含不同内容，可用于验证历史和 Diff。

在 `client/` 下运行以下命令可以创建一个临时远端和独立工作树。

```sh
fixture_dir=$(mktemp -d)
git clone --bare src/git/fixtures/sync.bundle "$fixture_dir/remote.git"
git clone --no-local --single-branch --branch base "$fixture_dir/remote.git" "$fixture_dir/repo"
```
