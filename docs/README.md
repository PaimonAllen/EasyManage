# 平台文档

此目录是 VitePress 文档源。`pnpm --dir docs install --frozen-lockfile --ignore-scripts` 与 `pnpm --dir docs build` 生成 `docs/.vitepress/dist`；`./scripts/run-server.sh --build` 也会构建它。

文档静态文件部署到配置的 Web 根目录下 `guide/`，由控制端同源 HTTPS `/guide/` 提供。系统级默认 `/opt/easymanage/web/guide`，普通用户 默认 `~/.local/share/easymanage/web/guide`。VitePress 的 `base` 为 `/guide/`；默认页是迁入的历史操作手册，`/guide/easymanage.html` 为平台文档。页面不依赖本机 Nginx，原站保留且不自动同步。`.vitepress/config.mts` 入 Git，`.vitepress/dist/` 和 `.vitepress/.temp/` 不入 Git。

需要重新从原站提取正文时，可用 `pandoc http://127.0.0.1/ -f html -t gfm --wrap=none --lua-filter docs/tools/extract_legacy_manual.lua -o docs/index.md`，然后重新加入页面顶部的迁移说明并审核差异。该命令只读取原站，但会覆盖 `docs/index.md`；不要在未审核的情况下对生产手册自动运行。日常编辑可直接修改 Markdown，再构建和部署。
