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

## 4. 工作原理

```
用户输入
   │
   ├─ 采集上下文：drawing.inspect（图层/范围/选择集/实体样本）
   │
   ├─ 离线：rules::offline_draft（中英文规则）
   └─ 在线：POST /chat/completions → {"reply","script"}
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
- 系统提示词禁止在脚本里写 `UNDO`，撤销边界由执行器统一管理
- 分层：`crates/ai` 位于 L4（`engine → ai`），UI 在 L5；`cargo xtask layers` 强制

## 5. 目录与改动

| 路径 | 内容 |
| --- | --- |
| `crates/ai/` | 新增 AI crate：`assistant`、`provider`、`openai`、`rules`、`executor`、`prompts/system.md` |
| `crates/ui-egui/src/ai_chat.rs` | AI 助手面板（会话、草稿、设置、后台请求） |
| `crates/ui-egui/src/docking.rs` | `Panel::AiChat` 面板注册（按需打开） |
| `crates/ui-egui/src/menus.rs` | 菜单项 `ui.panel.aiChat` |
| `crates/ui-egui/src/{icons,theme}.rs` | AI 图标、错误色 |
| `crates/ui-egui/src/i18n/zh.tsv` | 中文文案 |
| `xtask/src/layers.rs` | `ai` 分层登记与同层顺序 |
| `AICAD.md` / 本文件 | 衍生版说明 |

## 6. 验证情况

在本机实际执行（Linux x86_64，Rust 1.99.1）：

| 检查 | 结果 |
| --- | --- |
| `cargo test -p cadcraft-ai` | 43 通过（含“规则草稿在真实 Session 上可执行”） |
| `cargo test -p cadcraft-ui-egui --lib` | 261 通过（含 6 个面板测试） |
| `cargo clippy -p cadcraft-ai -p cadcraft-ui-egui --all-targets -- -D warnings` | 0 警告 |
| `cargo fmt --check` | 通过 |
| `cargo xtask layers` | 通过（17 crates，无违规） |
| `cargo check -p cadcraft-ai --target wasm32-unknown-unknown` | 通过 |
| 真实界面（Xvfb + lavapipe） | 输入“以 0,0 为圆心画半径 30 的圆”→ 草稿 `circle 0,0 30` → 应用后 Circle 7→8 → 一次 `UNDO`（Group）回到 7 |

## 7. 已知限制

- 离线规则覆盖常见模板，不是通用自然语言理解；复杂需求请接入大模型
- 暂不支持流式输出、草稿内联编辑（可“放弃”后重问）
- 对话仅当前会话保留；只有 provider 设置会持久化
- 其他语言（uk/de/es 等）的新文案暂时回退英文；中文已完整
- 未做 CAD 语义校验以外的沙箱：草稿可能包含任意合法命令，应用前请审查

## 8. 上游、许可与贡献

- 代码来自 CADCraft，遵循 **MIT OR Apache-2.0**；分发时保留 `LICENSE-MIT`、`LICENSE-APACHE` 与上游归属
- 本仓库是独立衍生版，不代表 storytold/CADCraft 官方
- 上游同步：`git remote add upstream https://github.com/storytold/cadcraft.git`，
  `git fetch upstream && git merge upstream/main`
- 提交前建议跑：`cargo xtask ci`（fmt、clippy、测试、资源、分层、wasm）

---

**一句话**：把“说需求”变成“可审查的 CAD 命令”，并把整份命令变成一步撤销——这就是 AICAD。
