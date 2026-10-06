# 第三方组件与许可证

本文件记录 WordRay 的**直接依赖**及其许可证。

数据来源：**读取本机已安装包的实际元数据**（前端读 `node_modules/<pkg>/package.json` 的 `license` 字段；
Rust 读 `cargo metadata` 的 `license` 字段），不是凭记忆填写。
核对日期：2026-10-04。

## 合规要求（见 docs/design.md 第 11 节）

- 本项目自身：**MIT**（见根目录 `LICENSE`）
- 依赖白名单：仅允许 **MIT / Apache-2.0 / BSD / ISC**
- **禁止 AGPL / GPL** —— 会传染到本项目自身的开源许可
- 本文件随直接依赖变更同步更新

## 前端直接依赖

| 组件 | 版本 | 许可证 | 用途 |
| --- | --- | --- | --- |
| @tabler/icons-react | 3.48.0 | **MIT** | 界面图标（`src/components/icons.tsx` 统一封装） |
| @tauri-apps/api | 2.12.1 | Apache-2.0 OR MIT | 前端与 Rust 侧通信 |
| react | 18.3.1 | MIT | 界面框架 |
| react-dom | 18.3.1 | MIT | 界面渲染 |
| @tauri-apps/cli | 2.12.1 | Apache-2.0 OR MIT | 构建工具（开发期） |
| @types/react | 18.3.31 | MIT | 类型（开发期） |
| @types/react-dom | 18.3.7 | MIT | 类型（开发期） |
| @vitejs/plugin-react | 4.7.0 | MIT | 构建工具（开发期） |
| typescript | 5.9.3 | Apache-2.0 | 类型系统（开发期） |
| vite | 6.4.3 | MIT | 构建工具（开发期） |
| vitest | 5.0.3 | MIT | 单元测试（开发期，测 `src/selectionAlign.ts` 的纯逻辑） |

## Rust 直接依赖

| 组件 | 版本 | 许可证 | 用途 |
| --- | --- | --- | --- |
| tauri | 2.12.1 | Apache-2.0 OR MIT | 桌面框架、窗口、IPC |
| tauri-build | 2.7.1 | Apache-2.0 OR MIT | 构建脚本 |
| tauri-plugin-global-shortcut | 2.4.0 | Apache-2.0 OR MIT | 全局热键 |
| serde | 1.0.229 | MIT OR Apache-2.0 | 序列化 |
| serde_json | 1.0.151 | MIT OR Apache-2.0 | JSON 与 SSE 解析 |
| arboard | 3.6.1 | MIT OR Apache-2.0 | 剪贴板读写 |
| tokio | 1.53.2 | MIT | 异步运行时 |
| reqwest | 0.12.28 | MIT OR Apache-2.0 | DeepSeek HTTP / SSE |
| futures-util | 0.3.34 | MIT OR Apache-2.0 | 流式读取 |
| windows | 0.58.0 | MIT OR Apache-2.0 | UI Automation、Win32、DPAPI |

**结论：全部为 MIT 或 Apache-2.0，无 copyleft 依赖。**

## 尚未完成的部分（阶段 3）

本文件目前只覆盖**直接依赖**。公开发布前还需要：

1. 从 `pnpm-lock.yaml` 与 `src-tauri/Cargo.lock` 生成**完整传递依赖**清单并复核许可证
2. 在 CI 中加入自动扫描：
   - Rust 侧 `cargo-deny`（检出禁用许可证与已知漏洞）
   - 前端侧 `license-checker`
   - 许可证不合规直接 fail
3. 二进制分发时随附本文件与各许可证原文

## 设计要点来源（非代码依赖）

本项目的若干设计纪律与平台坑位记录来自对第三方项目**公开文档**的学习。
这些项目**不构成代码依赖**，本项目未复制其源码，来源与授权情况详见
[docs/design.md](docs/design.md) 第 12 节。
