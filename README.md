# AutoPassDoc

按专家批注智能改稿的 Word 桌面工具（Windows 优先，后续支持 macOS）。

需求与设计见 [docs/requirements.md](docs/requirements.md) 和 [docs/technical-design.md](docs/technical-design.md)。

## 当前进度：第一期（M1）

- 原生解析 .docx：标题、自动编号、段落、表格（含合并单元格）、图片、修订、批注及回复、已解决状态
- 大文档：22 万字、500 条批注的报告约 10 毫秒解析完成；界面只渲染可视区域
- 保真：未修改时另存，所有部件与原文件逐字节一致
- 界面：大纲导航、批注侧栏（按作者和状态筛选）、点击批注定位原文、点击高亮原文定位批注、浅色/深色主题、拖放打开

## 目录

| 路径 | 内容 |
|---|---|
| `crates/docx-engine` | .docx 解析与保存引擎（Rust） |
| `src-tauri` | Tauri 2 桌面应用壳 |
| `ui` | 前端（React + TypeScript + Vite） |
| `testdata/fixtures` | 回归测试用文档（LibreOffice 转存版、Word 模板生成版） |

## 开发

需要 Rust（stable）、Node 22、pnpm 10。Linux 还需 Tauri 的系统依赖（见 CI 配置）。

```bash
pnpm --dir ui install

# 启动桌面应用（开发模式）
cd src-tauri && ../ui/node_modules/.bin/tauri dev

# 只调前端：浏览器里用生成的示例报告
pnpm --dir ui demo-data && pnpm --dir ui dev

# 测试与性能基准
cargo test --workspace
cargo run --release -p docx-engine --example bench_open            # 生成 22 万字文档并计时
cargo run --release -p docx-engine --example bench_open -- 报告.docx # 用自己的文档计时
cargo run -p docx-engine --example inspect -- 报告.docx             # 查看解析结果

# 生成测试文档
cargo run --release -p docx-engine --example gen_testdoc -- testdata/generated/large-review.docx
```

Windows 安装包由 GitHub Actions 自动构建（CI → Windows installer → Artifacts）。
