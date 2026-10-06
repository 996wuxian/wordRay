# WordRay 设计文档

状态：阶段 0 产出，待评审
版本：0.1.0
最后更新：2026-10-04

---

## 1. 目标

在 Windows 上，于**任意应用**中选中一段中文文本后，在选区附近弹出翻译，调用 **DeepSeek**（OpenAI 兼容）译为英文。

- 触发方式：**选中即弹**，不占用全局热键，不占用 `Ctrl+A`（该键在所有应用中均为"全选"）
- 目标平台：Windows 10 / 11 x64
- 交付形态：单 exe + 系统 WebView2，产物目标 5 MB 量级

## 2. 范围与非目标

**做：**
- 系统级划词捕获（鼠标手势 + 键盘选区）
- 选区 + 所在段落的读取（带上下文翻译）
- DeepSeek 流式翻译、结果窗、术语与提示词可配置
- 托盘常驻、单实例、设置页、API Key 本地加密

**不做（本阶段）：**
- macOS / Linux（架构留出平台层，不实现）
- 截图 OCR、图片翻译
- 划词历史云端同步
- 多翻译服务商聚合

## 3. 技术选型

| 层 | 选型 | 理由 |
| --- | --- | --- |
| 桌面框架 | Tauri 2 | 复用系统 WebView2，不打包 Chromium |
| 后端 | Rust 2021 | 取词、钩子、窗口、网络 |
| 前端 | React + TypeScript + Vite | 结果窗 Markdown/公式渲染成本低 |
| Windows 原生 | `windows-rs`（UI Automation、Win32、低级输入钩子、OLE） | 许可干净、无需 C++ 桥 |
| HTTP / 流式 | `reqwest` + `tokio` + `futures-util` | SSE |
| 单实例 | `tauri-plugin-single-instance` | 官方维护 |
| 全局热键 | `tauri-plugin-global-shortcut` | 官方维护 |
| 密钥存储 | Windows DPAPI | 不落明文 |
| 数据校验 | `serde` + `zod` | 跨 IPC 契约 |

## 4. 架构

```
┌─ Tauri 应用（单个 exe） ────────────────────────────┐
│ 前端 WebView2（React + TS + Vite）                  │
│  · 划词工具条  · 结果窗（流式 / Markdown）  · 设置页  │
├─────────────────────────────────────────────────────┤
│ Rust 运行层                                          │
│  · 生命周期、托盘、单实例、全局热键                   │
│  · 状态机（selection id + generation）               │
│  · DeepSeek 流式客户端（reqwest + SSE）              │
│  · 密钥存储（DPAPI，不落明文）                        │
├─────────────────────────────────────────────────────┤
│ 捕获层 —— helper 子进程（可终止、可重建）             │
│  · 低级鼠标/键盘钩子 → 手势识别                       │
│  · UIA TextPattern → 选区 + 所在段落                  │
│  · MSAA / 剪贴板兜底（OLE 事务化 + 条件还原）         │
└─────────────────────────────────────────────────────┘
        ↕ 匿名管道，版本化协议（requestID / generation）
```

**为什么捕获层要独立进程**：UI Automation 与 OLE 都是同步 COM 调用，第三方 provider 无响应时会阻塞调用线程。放在可终止、可重建的子进程里，主进程不受影响。

## 5. 模块职责

| 模块 | 职责 |
| --- | --- |
| `src-tauri/src/app` | 生命周期、托盘、单实例、快捷键注册 |
| `src-tauri/src/state` | 手势/选区状态机，generation 与 selection id |
| `src-tauri/src/translate` | DeepSeek 客户端、SSE 流式、提示词组装 |
| `src-tauri/src/secret` | API Key 加密落盘（DPAPI） |
| `src-tauri/src/capture` | helper 生命周期、管道协议、超时与重建 |
| `src-tauri/src/helper` | 子进程入口：钩子、UIA、剪贴板兜底 |
| `src-tauri/src/window` | 非激活置顶浮窗、两阶段 present、DPI/多屏换算 |
| `src/renderer/toolbar` | 划词后浮现的工具条 |
| `src/renderer/result` | 结果窗、流式渲染、Markdown |
| `src/renderer/settings` | 服务商、提示词、术语表、窗口行为 |

### 5.1 第一版的真实布局（与上表的映射）

上表是**目标架构**。第一版刻意只做了其中一条竖线，用来说明映射关系：

| 目标模块 | 第一版位置 | 说明 |
| --- | --- | --- |
| `app`（生命周期、快捷键） | `src-tauri/src/main.rs` | 尚未拆目录 |
| `translate`（DeepSeek 客户端、流式、提示词） | `src-tauri/src/deepseek.rs` | 提示词常量暂放 `main.rs` |
| `capture` / `helper`（取词） | `src-tauri/src/clipboard.rs` | **只有剪贴板兜底这一条路径**，无 UIA、无钩子、无 helper 子进程 |
| `state`（状态机、generation） | `main.rs` 里的 `SESSION` 计数器 | 只够 v1 用；阶段 2 才升级为完整状态机 |
| `window`（非激活置顶） | `tauri.conf.json` 的窗口配置 + `main.rs` 的 `show_panel` | **会抢焦点**，未做非激活与两阶段显示 |
| `secret`（DPAPI） | 无 | v1 改为从环境变量读，**不落盘**，反而更安全 |
| `src/renderer/*` | `src/App.tsx`、`src/main.tsx`、`src/styles.css` | 单一面板，尚未分 toolbar / result / settings |

这份映射的目的：避免读者以为第一版已经实现了全部设计。**目标架构不变**，只是分阶段落地。

## 6. 十条设计纪律

这十条是评审任何 PR 的检查项。

1. **用状态机 + generation / selection id，而不是任何固定延迟。** 固定 delay 是这类工具的万恶之源。
2. **捕获隔离到可重建的 helper 进程。** 第三方 UIA/OLE provider 卡死不得拖住主进程。
3. **剪贴板事务化 + 条件还原。** 只有剪贴板仍属本次捕获才还原。
4. **窗口非激活 + 两阶段显示**（prepare 透明创建 → renderer 就绪 → commit 可见），不抢焦点。
5. **进程族软关联**，替代 exact HWND/PID 硬门控。
6. **DPI / 多屏换算只留一个转换点。**
7. **主线程不阻塞。** 窗口创建移出同步 IPC，否则界面一直转圈。
8. **兼容性走 allowlist**，不做全局回退。
9. **Hook 自愈由 instance token + 线程退出状态驱动。**
10. **删掉轮询式补丁与定时等待，比新增等待更有价值。**

## 7. 平台坑清单

评审与实现时必须逐条对照。

**UIA 取词**
- 先 `ElementFromPoint`，失败再用 focused element
- 沿 Control / Raw View 祖先链找 TextPattern（命中的元素常是外壳）
- `DocumentRange` 回退**仅限 focused**，否则可能返回整篇文档
- 矩形不可信时弃用坐标，改用鼠标释放点
- Chromium / WebView2 冷启动首次 UIA 必失败（被问到才打开无障碍树）→ 必须重试
- `GetBoundingRectangles` 返回的矩形**没有 height 字段**，需按 `right-left`/`bottom-top` 计算

**剪贴板兜底**
- 用 OLE 保存**多格式**原内容，还原时全部写回
- 注入的 `Ctrl+C` 要带专属标记，且该标记键必须被自己忽略（否则自我取消）
- **只有剪贴板仍属本次捕获才还原**，防止覆盖用户新复制的内容
- 注入按键必须带扫描码，否则 Chromium 不认
- 密码控件：状态未知或为真时**不自动复制**
- allowlist 外一律不注入：终端、PowerShell、密码管理器、远程桌面

**钩子**
- 自愈由 instance token + 线程退出状态驱动，**禁用"每 30 秒重装"式轮询**

**手势状态机**
- 前台窗口变化会落在 Down/Up 之间从而清空手势 → 保留 pending、rebase generation、过滤迟到同源事件
- **区分"拖动一个已选中的元素"与"真划词"**：UIA 给出的选区完全一致，必须在**鼠标按下那一刻**拍选区快照

**窗口与 DPI**
- 物理桌面像素 ↔ WebView2 CSS 像素互转，多屏负坐标与工作区钳制
- `WS_EX_NOACTIVATE` + `TOOLWINDOW` + `TOPMOST`
- 失焦关闭要延迟核验真实前台 HWND

## 8. 数据流：一次划词翻译

1. 用户在任意应用选中文本
2. helper 的低级鼠标钩子捕获 Down/Up，判定"拖选"或"双击选词"
3. helper 取**按下那一刻的选区快照**，与松开后的选区比较 → 判定是否真的产生了新选区
4. helper 读文本：UIA TextPattern（选区 + 所在段落）→ 失败退 MSAA → 再失败退剪贴板兜底（OLE 存原内容 → 注入带标记 Ctrl+C → 校验 → 放回原内容）
5. helper 经匿名管道发送 `{selectionId, generation, text, paragraph, 屏幕矩形, DPI}`
6. 主进程状态机 rebase generation，准备浮窗（prepare：透明创建，不抢焦点）
7. 前端渲染工具条；若配置为"选中即译"则立即发起请求
8. `reqwest` + SSE 调用 DeepSeek，delta 流式推给前端
9. 用户可复制 / 关闭 / 换目标语言；关闭仅隐藏，不销毁

## 9. 里程碑

| 阶段 | 内容 | 预估 | 停止点 |
| --- | --- | --- | --- |
| 0 | 本设计文档 + 项目骨架 | 0.5 天 | 设计评审 |
| 1 | 最小闭环：托盘 + 全局热键 + 剪贴板取词 + DeepSeek 流式 + 置顶弹窗 | 1–2 天 | 管线是否通 |
| 2 | 选中即弹：钩子手势 + UIA 选区与段落 + 剪贴板兜底 + 按下快照判定 | 2–4 天 | 交互是否达标 |
| 3 | 工程化：helper 隔离、剪贴板事务还原、混合 DPI、单实例、非激活窗口、设置页、DPAPI | 3–5 天 | 全量验收 |
| 4 | 自主增强：自定义术语表、固定中→英提示词 | 待定 | — |

阶段 1 **刻意不碰 UIA 与钩子**，目的是用最低成本证明"取词之后的所有环节"是通的。

## 10. 验收标准

1. 四个场景各划一次中文短句，均出**中→英**：浏览器 / 记事本或 VS Code / WPS 或 Word / 微信
2. **剪贴板不被污染**：先在别处复制已知文本，划词后粘贴应仍是原文本
3. **译文确实来自 DeepSeek**：改错 Key 应报错，而非静默回落别的引擎
4. 连续划词 10 次不崩溃、不串词
5. 退出后划词无反应，无残留进程
6. 弹窗**不抢焦点**：划词后原应用的光标与输入焦点不变
7. 混合 DPI 双屏下弹窗位置正确
8. 终端 / 密码管理器里划词**不触发**剪贴板注入

## 11. 许可证与合规

- 本项目：**MIT**（见根目录 `LICENSE`）
- 依赖白名单：仅允许 MIT / Apache-2.0 / BSD / ISC。**禁止 AGPL / GPL**
- CI 必须跑许可扫描（Rust 侧 `cargo-deny`，前端侧 `license-checker`），不合规直接 fail
- 引入新依赖时同步更新 `THIRD_PARTY_NOTICES.md`（阶段 1 起建立）
- **禁止**从无许可证项目复制任何代码或文本片段

## 12. 来源与致谢（provenance）

本项目的设计要点来自对以下项目**公开文档**的学习，属思想层面，**未复制任何源码或文本片段**。此处如实记录来源，以便后续审查与致谢：

| 来源 | 许可 | 借鉴内容 |
| --- | --- | --- |
| TextLens | **无许可证**（`package.json` = `UNLICENSED`） | 架构分层思路、平台踩坑清单。**仅读文档，未使用任何代码** |
| `fyisgod/dsh-selection-tools` | MIT | Windows 取词坑清单（UIA 冷启动、剪贴板竞态、手势判定） |
| `0xfullex/selection-hook` | MIT | 兼容性行为参照（选区读取策略、应用兼容清单思路）。**未引入其 C++ 代码** |
| Cherry Studio | AGPL-3.0 | 仅作为交互形态的公开参考。**未使用任何代码或提示词文本** |

**未采用** `selection-hook` 的 C++ 核心：其构建为 node-gyp/prebuildify，无对外头文件与静态库 target，`worker.h` 强耦合 `napi_threadsafe_function`；接入需自建静态库 + C ABI + FFI，成本接近重写 Hook 层。

> 说明：这不是严格意义的 clean-room（撰写者读过上述文档），但已做到不复制受版权保护的表达。

## 13. 未决问题

1. 交互形态最终定稿："选中直接出译文" vs "先出工具条再点"——建议先做前者，工具条作为可选配置
2. 是否与外部知识库/记忆系统联动（需另行确认）
3. 代码签名：开源发布前是否购买证书，以规避 SmartScreen 警告
4. 两处平台细节待验证：`selection-hook` 的 `binding.gyp` 与 `selection_hook.cc` 全文未能获取（调研时 jsDelivr 返回 octet-stream、raw.githubusercontent 不可达）。**不阻塞本项目**，仅影响兼容性清单的完整度
