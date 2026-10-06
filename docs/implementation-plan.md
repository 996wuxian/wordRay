# WordRay 实现方案

状态：待评审（**本文件不含任何业务代码**）
版本：0.1.0
对应设计文档：[design.md](design.md)
最后更新：2026-10-04

---

## 1. 本文档的作用

`design.md` 回答"做成什么样"；本文档回答"**按什么顺序、落到哪些文件、每一步怎么验证**"。

目标颗粒度：评审通过后可以照着做，中途不需要再拍脑袋决策。

## 2. 本方案的施工范围：阶段 1（最小闭环）

**交付**：一个能真正跑起来的 Tauri 应用。

- 托盘常驻（显示 / 退出）
- 全局热键触发（默认 `Ctrl+Alt+T`）
- 取词：注入 `Ctrl+C` → 读剪贴板 → **事务化还原原剪贴板**
- 调 DeepSeek 流式接口，结果落在置顶无边框窗口
- API Key 从环境变量 `DEEPSEEK_API_KEY` 读取（**不落盘**）

**明确不做**（留到后续阶段）：UI Automation、鼠标钩子、选区上下文段落、设置页、DPAPI 加密、单实例、混合 DPI。

> 阶段 1 刻意绕开 UIA 与钩子，目的是用最低成本证明"取词之后的整条链路"是通的。这也是设计文档里定的第一个停止点。

## 3. 文件清单

```
selection-translator/
├── LICENSE                         MIT（已存在）
├── README.md                       项目说明（已存在）
├── .gitignore                      （已存在）
├── .env.example                    DEEPSEEK_API_KEY 占位
├── package.json                    仅为安装 @tauri-apps/cli
├── docs/
│   ├── design.md                   设计与纪律（已存在）
│   └── implementation-plan.md      本文档
├── ui/                             阶段 1 用静态前端
│   ├── index.html
│   ├── main.js
│   └── style.css
└── src-tauri/
    ├── Cargo.toml
    ├── build.rs
    ├── tauri.conf.json
    ├── capabilities/default.json
    ├── icons/                      由 `tauri icon` 生成
    └── src/
        ├── main.rs                 入口：插件装配、托盘、热键
        ├── clipboard.rs            注入 Ctrl+C + 读剪贴板 + 事务还原
        ├── deepseek.rs             SSE 流式客户端
        ├── config.rs               读环境变量
        └── window.rs               浮窗显示与定位
```

### 3.1 决策变更：v1 直接使用 React（原方案为静态前端）

**原方案**拟在阶段 1 用静态 HTML 以避免引入构建链。**wuxian 决定直接上 React + Vite + TypeScript**，理由是后面阶段 2/3 迟早要换，不如一次到位。

因此本文件第 3 节的目录结构、第 7 节 Step 1 的产出物，均以 React 方案为准。原静态方案作废。

**实际生效的结构**：

```
src/                     React 前端（Vite 构建）
├── main.tsx             入口（刻意不用 StrictMode：避免 effect 重复挂载）
├── App.tsx              翻译面板：事件订阅、流式渲染、Esc 关闭、复制
└── styles.css           面板样式
index.html               Vite 入口 HTML
vite.config.ts           固定 1420 端口，strictPort
tsconfig.json            strict + noUnusedLocals
package.json             依赖与脚本（dev / build / typecheck / app:dev）
pnpm-workspace.yaml      pnpm 11 的 allowBuilds 白名单（esbuild）
```

**保留的取舍**：`ui/` 目录与静态前端不再创建。

## 4. 依赖（拟定，落地时由 cargo 解析并锁定）

`src-tauri/Cargo.toml`：

```toml
[package]
name = "wordray"
version = "0.1.0"
edition = "2021"

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = ["tray-icon"] }
tauri-plugin-global-shortcut = "2"
tauri-plugin-clipboard-manager = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync"] }
reqwest = { version = "0.12", default-features = false, features = ["json", "stream", "rustls-tls"] }
futures-util = "0.3"
windows = { version = "0.58", features = ["Win32_UI_Input_KeyboardAndMouse", "Win32_Foundation"] }
```

`package.json`（只为一个 CLI）：

```json
{ "private": true, "devDependencies": { "@tauri-apps/cli": "^2" } }
```

**诚实说明**：上面的版本号是**拟定的主版本**，我没有在本机解析过。落地时必须用 `cargo add` 逐条加入并让 cargo 写出真实版本与 `Cargo.lock`，不要手抄这些数字。

**许可**：全部为 MIT / Apache-2.0，符合设计文档第 11 节的白名单。首次 `cargo build` 后据 `Cargo.lock` 生成 `THIRD_PARTY_NOTICES.md`（阶段 0 推迟的那件事，在这里补上）。

## 5. 接口契约

### 5.1 前端 ↔ Rust

```
command get_state()                      -> { busy: bool, lastError: Option<String> }
command translate(text: String)          -> ()
command copy_result(text: String)        -> ()
command close_panel()                    -> ()

event   translation://delta  { sessionId, text }
event   translation://done   { sessionId, fullText }
event   translation://error  { sessionId, message }
```

**约定**：`translate` 立即返回，结果一律走事件。这样阶段 2 加"取消"时不需要改前端契约。

### 5.2 capture 模块

```rust
pub enum CaptureSource { Clipboard, Uia, Msaa }

pub struct CaptureResult {
    pub text: String,
    pub source: CaptureSource,
}

pub fn capture_selection(timeout: Duration) -> Result<CaptureResult, CaptureError>;
```

阶段 1 只实现 `Clipboard` 分支，但**枚举与签名先定型**，避免阶段 2 改动调用方。

### 5.3 deepseek 模块

```rust
pub struct ChatRequest {
    pub model: String,
    pub system: Option<String>,
    pub user: String,
}

pub async fn stream_chat<F>(req: ChatRequest, on_delta: F) -> Result<String, DeepSeekError>
where
    F: FnMut(&str);
```

### 5.4 helper 管道协议（阶段 2 才实现，此处先定契约）

帧格式：4 字节长度前缀 + UTF-8 JSON。

```json
{ "version": 1, "requestId": 42, "generation": 7, "kind": "capture", "payload": {} }
```

`generation` 用于丢弃迟到结果。**现在定这个字段，是为了阶段 2 不必返工协议。**

## 6. 与 TextLens 做法的逐条对应

参考来源是 TextLens 的**公开开发手册**（文档层面）。它的仓库**没有许可证**，因此本项目不复制其任何代码，只借鉴其中记录的架构做法与踩坑经验。

| 主题 | TextLens 记录的做法的 | 我们的做法 | 放在阶段 | 差异原因 |
| --- | --- | --- | --- | --- |
| 捕获隔离 | UIA/OLE 捕获放在可终止重建的 helper 子进程 | 同样隔离 | **3** | 阶段 1/2 先在主进程内验证链路，过早引入进程间协议会掩盖真实问题 |
| 剪贴板还原 | OLE 存多格式原内容；只有剪贴板仍属本次捕获才还原 | **阶段 1 就做** | **1** | 这是最容易踩、也最容易伤到用户的坑，不能留到后期 |
| 注入按键 | 注入的 Ctrl+C 带专属标记，且标记键被自己忽略 | 同样做法 | 1 | 否则会自我取消 |
| 危险场景 | 终端 / 密码管理器 / 远控不做剪贴板注入（allowlist） | **阶段 1 就做** | **1** | 安全相关，不能延后 |
| 注入扫描码 | 注入按键必须带扫描码，否则 Chromium 不认 | 同样做法 | 1 | |
| 窗口不抢焦点 | `WS_EX_NOACTIVATE` + `TOOLWINDOW` + `TOPMOST` | 同样做法 | 2 | 阶段 1 先允许抢焦点，功能优先 |
| 两阶段显示 | 透明创建 → renderer 就绪 → 提交可见 | 采用 | 2 | 阶段 1 窗口很小，闪烁可接受 |
| 状态机 | selection id + generation，取代固定延迟 | 采用 | 2 | 阶段 1 只有热键触发，没有手势竞争 |
| 手势判定 | 鼠标按下那刻拍选区快照，区分"拖动已选中元素"与"真划词" | 采用 | 2 | 需要钩子，阶段 1 无 |
| 混合 DPI | 物理像素 ↔ CSS 像素单一转换点 | 采用 | 3 | 单屏开发时可暂缓 |
| 单实例 | 插件接管，重复启动只提示 | 采用 | 3 | **阶段 1 已知风险**：可能起两个实例抢热键 |
| Hook 自愈 | instance token 驱动，不做定时重装 | 采用 | 2 | |
| 前端 | React + Vite | 阶段 1 静态 HTML，阶段 3 换 React | 3 | 见 3.1 |

**这张表就是"参考 TextLens 实现"的落地方式**：它的价值不在代码，而在它已经替我们踩过的坑清单。

## 7. 分步实施顺序

每一步都**独立可验证**，做完一步才做下一步。这是本方案的核心：任何一步失败都不需要回退超过一步。

### Step 1 —— 骨架能起来
做：`src-tauri/{Cargo.toml,build.rs,tauri.conf.json}`、`src/main.rs`（最小 Tauri 应用）、`ui/index.html`、`ui/main.js`、`ui/style.css`、`package.json`、`.env.example`、`capabilities/default.json`、`icons/`
验证：
```
pnpm install
pnpm tauri dev
```
预期：弹出一个空白窗口，无报错。

### Step 2 —— 托盘
做：`tauri.conf.json` 的 tray 配置、`main.rs` 里托盘菜单（显示 / 退出）
验证：`pnpm tauri dev` → 通知区域出现图标，右键"退出"能真正结束进程。
预期：关掉窗口后进程仍在（托盘常驻）。**注意**：这一步会暴露"窗口关闭 vs 进程退出"的语义，需明确"关闭 = 隐藏"。

### Step 3 —— 热键 + 窗口
做：注册 `tauri-plugin-global-shortcut`，绑 `Ctrl+Alt+T`；`window.rs` 创建/定位无边框置顶窗口，显示固定文本
验证：任意应用里按 `Ctrl+Alt+T` → 窗口出现并显示固定文本；按 Esc 关闭。
预期：热键全局生效。**若被其他软件占用**，需要在日志里明确报出注册失败，而不是静默无反应。

### Step 4 —— 剪贴板取词（阶段 1 最危险的一步）
做：`clipboard.rs`
- 用 OLE/剪贴板 API 保存**多格式**原内容
- 通过 `SendInput` 注入带扫描码的 `Ctrl+C`，并让自己忽略该标记
- 等待剪贴板序号变化，读取文本
- **仅当剪贴板仍属本次捕获时**才还原原内容
- allowlist：终端 / 密码管理器 / 远控不注入
验证：
1. 在浏览器选中一句中文 → 按热键 → 窗口显示**选中文本**（不是别的）
2. **剪贴板未被污染**：划词前先在别处复制一段已知文本，划词后粘贴应仍是原文本
3. 在 Windows Terminal 里选中文字按热键 → **不注入**，窗口给出明确提示
预期：上述三条全通过。**任何一条不过都不要进 Step 5。**

### Step 5 —— DeepSeek 流式
做：`config.rs` 读 `DEEPSEEK_API_KEY`；`deepseek.rs` 实现 SSE 流式；接上事件推送
验证：选中中文 → 按热键 → 窗口**逐字**显示英文译文。
反向验证：把环境变量里的 Key 改错一位 → 必须**明确报错**，不得静默显示空白或回落其他行为。

### Step 6 —— 窗口体验
做：不抢焦点、跟随鼠标定位、Esc 关闭、复制按钮
验证：划词后原应用的光标与输入焦点**不变**；弹窗出现在鼠标附近且不出屏。
预期：达到阶段 1 的完整验收。

## 8. 阶段 1 验收

对照 `design.md` 第 10 节的子集：

| # | 验收项 | 通过标准 |
| --- | --- | --- |
| 3 | 译文来自 DeepSeek | 改错 Key 必须报错 |
| 5 | 退出后无残留 | 退出后热键完全失效，无残留进程 |
| 2 | 剪贴板不被污染 | 划词前后粘贴内容一致 |
| — | 链路可用 | 四个场景（浏览器 / 记事本 / WPS / 微信）各划一次中文，均出中→英译文 |
| — | 安全 allowlist | 终端与密码管理器内不注入 Ctrl+C |

第 6、7、8 条（不抢焦点、混合 DPI、allowlist）中，不抢焦点与 allowlist 在阶段 1 就要过；混合 DPI 留到阶段 3。

## 9. 阶段 2 / 3 概要

| 阶段 | 主要内容 | 关键产出 |
| --- | --- | --- |
| 2 | 低级鼠标钩子手势识别、UIA 读选区与所在段落、按下快照判定、状态机 + generation、非激活窗口与两阶段显示 | `src-tauri/src/helper/`（钩子、UIA）、helper 管道 |
| 3 | helper 进程隔离、剪贴板事务完善、混合 DPI、单实例、设置页、DPAPI 密钥加密、React 前端、`THIRD_PARTY_NOTICES.md` | 达到 `design.md` 全量验收 |

## 10. 前置条件与风险

**前置条件（开工前需满足）**
1. 安装 Tauri CLI：`pnpm add -D @tauri-apps/cli`（Node 已有）
2. 准备应用图标：Tauri 要求 `icons/`。用一张 PNG 跑 `pnpm tauri icon <源图>` 生成全套；源图可由脚本生成占位图
3. `DEEPSEEK_API_KEY` 环境变量（真实 Key 只进环境变量，**不写文件**）
4. **写项目目录的权限**：项目在会话工作区之外，落地文件与编译需要授权

**风险**
1. **首次 `cargo build` 会拉取并编译大量依赖**，可能 5–15 分钟。这一步没有捷径。
2. **我不能保证一次编译通过**。本机工具链已验证可用（cargo 1.97.1 + MSVC 14.41，已实测编译并运行成功），但 Tauri 与各插件的 API 细节需要真实编译来校验。
3. **热键冲突**：`Ctrl+Alt+T` 可能被其他软件占用。Step 3 必须显式报出注册失败。
4. **阶段 1 无单实例**：可能同时起两个进程抢热键。已知，阶段 3 修。
5. **步骤 4 有真实风险**：注入 `Ctrl+C` 会动用户的剪贴板。allowlist 与事务化还原是必须项，不是优化项。
6. **版本号未经验证**（见第 4 节），必须以 `cargo add` 的实际解析结果为准。

## 11. 评审检查点

请重点看这三处，它们决定后面返工量：

1. **第 3.1 节** —— 阶段 1 用静态前端而不是 React，是否接受这个"先简单、后重写"的取舍
2. **第 6 节** —— 阶段划分是否同意（尤其"捕获隔离推迟到阶段 3"、"单实例推迟到阶段 3"）
3. **第 7 节 Step 4** —— 剪贴板注入这一步的真实风险是否接受
