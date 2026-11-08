# AICAD 中文说明

> **AICAD** 是 [CADCraft](https://github.com/storytold/cadcraft) 的衍生版：在原有纯 Rust、clean-room 的
> AutoCAD 风格 CAD 应用之上，增加一个 **AI 制图窗口**——用对话生成绘图/修图命令，逐行审查后一键应用，
> 整份草稿只占一步撤销。

- 上游：storytold/cadcraft（MIT OR Apache-2.0）
- 本仓库：<https://github.com/yaoyongli1974-dotcom/cadcraft>
- 英文说明：[README.md](README.md) · 衍生说明：[AICAD.md](AICAD.md)

---

## 1. 它解决什么问题

传统 CAD 里，画一个法兰、改一批圆角、给选中的对象换层，需要记住命令、坐标和选项。
AICAD 把这一步交给大模型或内置规则引擎：你说“以 0,0 为圆心画半径 30 的圆”，
它在 **AI 助手**面板里给出一份命令草稿：

```
circle 0,0 30
```

你可以先看清每一行（完整命令、坐标、图层都在），再点 **应用**。草稿不是直接改图，
而是像 `.scr` 脚本一样逐行执行，并包在一个 `UNDO` 组里——**一次 Ctrl+Z 撤销整份草稿**。

## 2. 主要功能

| 功能 | 说明 |
| --- | --- |
| AI 对话面板 | 菜单 **窗口 ▸ AI 助手**（Window ▸ AI Chat），可停靠、可悬浮 |
| 图纸上下文 | 自动附带标题、范围、实体统计、图层、当前选择集与实体样本（`drawing.inspect`） |
| 草稿审查 | 每行完整命令行 + 命令/提示应答分类；应用前不触碰图纸 |
| 一键撤销 | 整份草稿一个撤销组，失败行单独标红且不阻断后续行 |
| 离线规则引擎 | 无需联网、无需 Key，中英文常用绘图与修改指令 |
| OpenAI 兼容接口 | OpenAI、DeepSeek、通义千问、Kimi、智谱、Ollama、vLLM、LM Studio 等 |
| 密钥安全 | API Key 只来自环境变量或当前会话输入，**绝不写入偏好文件** |
| 中文界面 | 新增文案全部有中文翻译；菜单/面板/提示一致 |
| 图片参考 | 把截图/草图附在提问里（拖拽、粘贴、文件选择或输入路径），最多 4 张，随消息发给多模态模型 |
| 更大的对话窗口 | AI 助手独占右边缘一整列（默认 360px、全高），消息区可滚动，输入区常驻 |
| wasm 兼容 | AI crate 可在 `wasm32-unknown-unknown` 编译；浏览器版使用离线引擎 |

## 3. 快速开始

要求：Rust 1.95+（edition 2024）。

```bash
git clone https://github.com/yaoyongli1974-dotcom/cadcraft.git
cd cadcraft
cargo run -p cadcraft -- --sample
```

启动后打开 **窗口 ▸ AI 助手**，在底部输入框写需求，回车或点 **提问**。

### 3.1 离线模式（默认）

不联网，直接可用。支持例如：

- 绘图：`在 0,0 到 100,40 画线`、`画宽 120 高 40 的矩形`、`以 (50,50) 为圆心画半径 20 的圆`、
  `以 (0,0) 为圆心画半径 50 从 30 到 120 的圆弧`、`在 10,20 写 你好`
- 修改：`把选中的圆半径改成 15`、`把选中的向上移动 10`、`复制选中的向右 20`、
  `旋转选中的 45 度`、`镜像选中的`、`放大选中的 2 倍`
- 图层：`新建红层 Walls`、`切换到 Walls`、`把选中的移到 Walls 层`

规则引擎识别不了的说法不会瞎猜，而是返回可用示例。

### 3.2 接入大模型

打开 **AI 助手 ▸ 设置**：

| 字段 | 示例 |
| --- | --- |
| 提供商 | 选择 `OpenAI 兼容` |
| Base URL | `https://api.openai.com/v1`、`https://api.deepseek.com/v1`、`http://127.0.0.1:11434/v1` |
| Model | `gpt-4o-mini`、`deepseek-chat`、`qwen-plus`、`llama3.1` … |
| API Key | 可留空，改用环境变量 |
| 温度 | 默认 `0.2`，制图建议 0～0.3 |

更安全的 Key 方式（优先于面板输入，且不会回写）：

```bash
export AICAD_AI_API_KEY=sk-...
```

请求发往 `POST {Base URL}/chat/completions`，要求模型返回单个 JSON：
`{"reply": "...", "script": "..."}`；也接受 Markdown 代码块兜底。超时 120 秒，
请求在后台线程执行，不阻塞界面。

### 3.3 图片参考

需要让模型“看着图改图”时，把图片放进下一次提问：

| 输入方式 | 说明 |
| --- | --- |
| 拖拽 | 把图片文件拖进 CADCraft 窗口；AI 助手面板打开时，图片自动加入待发送列表（原生与浏览器版） |
| 粘贴 | 点 **粘贴图片**，读取系统剪贴板中的位图（原生版；浏览器版请用拖拽或文件选择） |
| 文件选择 | 点 **添加图片**，选择本地文件（原生版；需系统文件对话框） |
| 路径 | 在输入框里输入或粘贴图片路径，点出现的 **作为图片添加** |

限制与处理：

- 格式：PNG、JPEG、BMP、TIFF（按文件内容识别，不只看扩展名）
- 大小：单张原始文件 ≤ 16 MB；最长边超过 2048 px 时自动等比缩小
- 数量：每次提问最多 4 张，面板会显示缩略图，可单张移除或一次清空
- 编码：小图保持原格式；较大或缩放后的图统一转成 JPEG（质量 88）以控制请求体积
- 发送：作为 OpenAI 兼容的 `image_url` 内容块（`data:<mime>;base64,…`）随最后一条用户消息发出；
  文本与上下文保持原样
- 兼容：不带图片时请求体与旧版**完全一致**；偏好、草稿、脚本格式均未改变
- 离线模式没有视觉能力：附了图片时回复会以
  `The offline engine sees no pixels.` 开头，再按文字规则处理（图片不会上传）

图片只在当前提问中发送，不会写入图纸、偏好或日志。

## 4. 工作原理

```
用户输入（文字 + 可选图片）
   │
   ├─ 采集上下文：drawing.inspect（图层/范围/选择集/实体样本）
   │
   ├─ 离线：rules::offline_draft（中英文规则；图片仅提示无法识别）
   └─ 在线：POST /chat/completions（文本 + image_url 内容块）→ {"reply","script"}
   │
草稿预览（executor::dry_run 逐行分类）
   │  用户点“应用”
   ▼
UNDO begin → 逐行 Session::cmdline（等同手敲） → UNDO end
   │
一次撤销即可整体回退
```

关键约束：

- **AI 不改文档内部状态**，只产生命令字符串；执行路径与命令行/脚本/MCP 完全相同
- 草稿在**分离的会话副本**上读取上下文，后台线程不碰正在编辑的图纸
- 图片只存在于当次请求的消息内容中，不进入图纸、偏好或撤销栈
- 系统提示词禁止在脚本里写 `UNDO`，撤销边界由执行器统一管理
- 分层：`crates/ai` 位于 L4（`engine → ai`），UI 在 L5；`cargo xtask layers` 强制

## 5. 目录与改动

| 路径 | 内容 |
| --- | --- |
| `crates/ai/` | 新增 AI crate：`assistant`、`provider`、`openai`、`rules`、`executor`、`images`、`prompts/system.md` |
| `crates/ui-egui/src/ai_chat.rs` | AI 助手面板（会话、草稿、设置、图片缩略图、后台请求） |
| `crates/ui-egui/src/docking.rs` | `Panel::AiChat` 面板注册；独占右边缘全高列（`AI_CHAT_WIDTH = 360`） |
| `crates/ui-egui/src/lib.rs` | 面板状态、偏好接线；窗口拖拽图片的接入点 |
| `crates/ui-egui/src/menus.rs` | 菜单项 `ui.panel.aiChat` |
| `crates/ui-egui/src/{icons,theme}.rs` | AI 图标、错误色 |
| `crates/ui-egui/src/i18n/zh.tsv` | 中文文案 |
| `apps/cadcraft/src/main.rs` | 图片文件对话框服务 |
| `xtask/src/layers.rs` | `ai` 分层登记与同层顺序 |
| `AICAD.md` / 本文件 | 衍生版说明 |

## 6. 验证情况

在本机实际执行（Linux x86_64，Rust 1.99.1）：

| 检查 | 结果 |
| --- | --- |
| `cargo test -p cadcraft-ai` | 49 通过（含规则草稿真实执行、图片缩放/编码/上限测试） |
| `cargo test -p cadcraft-ui-egui --lib` | 269 通过（含 9 个面板测试） |
| `cargo clippy -p cadcraft-ai -p cadcraft-ui-egui --all-targets -- -D warnings` | 0 警告 |
| `cargo fmt --check` | 通过 |
| `cargo xtask layers` | 通过（17 crates，无违规） |
| `cargo check -p cadcraft-ai --target wasm32-unknown-unknown` | 通过 |
| 真实界面（Xvfb + lavapipe） | AI 助手为独立全高列；输入图片路径→缩略图→提问，离线回复以 `The offline engine sees no pixels.` 开头并生成 `circle 0,0 30`；应用后实体 46→47，一次 `UNDO`（Group）回到 46 |

## 7. 已知限制

- **不会自动升级到上游**：衍生版移除了启动时的上游更新检查；**帮助 ▸ 检查更新…** 打开本项目的 Releases 页。请不要手动安装官方 CADCraft 覆盖本包——官方版本不含 AI 助手
- 离线规则覆盖常见模板，不是通用自然语言理解，也**看不懂图片**（附图的离线回复会明确说明）；复杂需求请接入多模态大模型
- 图片：单张 ≤ 16 MB、最长边 2048 px、每次最多 4 张；大图会转 JPEG（质量 88）；浏览器版的剪贴板读图不可用，请用拖拽
- 暂不支持流式输出、草稿内联编辑（可“放弃”后重问）
- 对话仅当前会话保留；只有 provider 设置会持久化；待发送的图片不持久化
- 其他语言（uk/de/es 等）的新文案暂时回退英文；中文已完整
- 未做 CAD 语义校验以外的沙箱：草稿可能包含任意合法命令，应用前请审查

## 8. 上游、许可与贡献

- 代码来自 CADCraft，遵循 **MIT OR Apache-2.0**；分发时保留 `LICENSE-MIT`、`LICENSE-APACHE` 与上游归属
- 本仓库是独立衍生版，不代表 storytold/CADCraft 官方
- 上游同步：`git remote add upstream https://github.com/storytold/cadcraft.git`，
  `git fetch upstream && git merge upstream/main`
- 提交前建议跑：`cargo xtask ci`（fmt、clippy、测试、资源、分层、wasm）

## 9. 跟进上游更新

上游迭代很快，AICAD 的增量很小（一个 `crates/ai` + 一个面板 + 约十个接线点），
所以策略是**小步、常合并**：每周或上游发版后同步一次。

```bash
scripts/sync-upstream.sh
```

脚本会：检查工作区干净 → 拉取 `upstream/main`（shallow 仓库自动加深历史）→ 在
`sync/upstream-<日期>-<上游提交>` 分支合并 → 跑 fmt、clippy（ai/ui）、AI 测试、
UI 库测试、`cargo xtask layers`、wasm 检查。它**不推送、不动 main**；冲突时列出
文件和热点，等你解决。

通过后人工画面验证一次（窗口 ▸ AI 助手 → 画圆 → 应用 → Ctrl+Z），再：

```bash
git switch aicad-ai-assistant
git merge --ff-only sync/upstream-<日期>-<上游提交>
git push origin aicad-ai-assistant:main
```

冲突热点、上游 API 变化的影响面、发布与回滚步骤见
[docs/upstream-sync.md](docs/upstream-sync.md)。


---

**一句话**：把“说需求”变成“可审查的 CAD 命令”，并把整份命令变成一步撤销——这就是 AICAD。
