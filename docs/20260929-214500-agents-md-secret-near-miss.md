# 一次差点把本机凭据推到 GitHub 的事故（2026-09-29 21:45）

## 发生了什么

用 `git add -A` 提交 v1.8.0 时，把本机的 `AGENTS.md` 一起提交了。该文件当时是
**我的全局指令文件（`~/.claude/CLAUDE.md`）的全文副本**，里面有你给的邮箱授权码、
ed25519 私钥、Hugging Face / Cloudflare / Gitee / gitcode token、bark 地址。

`git commit` 成功，`git push` 被 GitHub 的 push protection 拦下（GH013，
同时报 SSH Private Key 与 Hugging Face Token 两类），所以**秘密没有离开本机**。

## 处置

1. `AGENTS.md` 还原成干净的 6 行（项目自己的开发验证约束，`git show 0265b6f:AGENTS.md`）。
2. `git commit --amend` 重写该提交，随后
   `git log --all -S "BEGIN OPENSSH PRIVATE KEY" -- AGENTS.md` 返回空，
   本地历史里已无秘密版本；远端确认不含 `bc5a22b`，`origin/main:AGENTS.md` 只有 6 行。
3. `.gitignore` 追加 `AGENTS.md` 与 `CLAUDE.md`。

## 为什么之前没发现

`AGENTS.md` 是已跟踪文件，但它的 6 行版本早在多个提交前就入库了；之后某个会话把它
覆盖成了我的全局指令全文，`git status` 显示为 ` M AGENTS.md`——被淹没在一堆正常改动里，
而 `git add -A` 不会分辨"这文件现在含私钥"。

## 规则（今后）

1. **`AGENTS.md` / `CLAUDE.md` 永不入库**，已在 `.gitignore`。
2. 提交前不再用裸 `git add -A`；改成先 `git status --short` 人工过一遍，或
   `git add` 逐个路径写清楚。
3. 本机凭据只出现在我的全局指令文件里，**任何时候都不要写进项目内任何文件**，
   包括文档、注释、测试夹具、`.test-artifacts/`。

## 需要你做的事

虽然本次没有泄漏到远端，但**这些凭据已经出现在过一个提交对象里**（本地
`bc5a22b`，已被 amend 覆盖不可见）。稳妥起见建议轮换：邮箱授权码、ed25519 私钥、
HF / Cloudflare / Gitee / gitcode token。是否轮换由你决定。
