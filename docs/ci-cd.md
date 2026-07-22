# CI/CD（GitHub Actions）

## Workflows

| 文件 | 触发 | 作用 |
|---|---|---|
| `.github/workflows/ci.yml` | push/PR 到 main | `cargo test` + clippy/fmt + desktop check |
| `.github/workflows/release.yml` | tag `v*` 或手动 | 构建 macOS arm64/x64、Windows、Linux 安装包，并上传 Release |

## 产物矩阵

| Platform | Runner | 产物（典型） |
|---|---|---|
| macOS aarch64 | `macos-14` | `.dmg` / `.app` |
| macOS x86_64 | `macos-15` | `.dmg` / `.app`（ARM64 runner 交叉编译 x86_64 target） |
| Windows x86_64 | `windows-latest` | `.msi` / NSIS `.exe` |
| Linux x86_64 | `ubuntu-22.04` | `.AppImage` / `.deb` |

> 注：`macos-13` 已从 GitHub-hosted runners 退役，macOS x86_64 改用 `macos-15`（Apple Silicon）交叉编译 `x86_64-apple-darwin`。

## 使用方式

```bash
# 日常：推 main 跑测试
git push origin main

# 发版：打 tag 触发三平台打包 + GitHub Release
git tag v0.1.0
git push origin v0.1.0

# 或手动跑 Release workflow
gh workflow run release.yml
```

## 注意

- 当前 **未配置 Apple 签名/公证** 与 Windows 代码签名；CI 产出为未签名安装包，本地可装、分发需后续补证书。
- `pnpm-lock.yaml` 必须提交，CI 用 `pnpm install --frozen-lockfile`。
- Release 仅在 `refs/tags/v*` 时创建 GitHub Release；`workflow_dispatch` 只构建 artifact。
