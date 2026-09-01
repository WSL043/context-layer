# Windows-first 基础架构提案

## 决策结论

采用 **Rust 共享核心 + Windows 原生适配器 + Tauri 2 桌面壳 + SQLite 本地事件/投影库**。运行时由一个当前用户后台代理、一个按需打开的 UI、一个浏览器 Native Messaging 桥组成。首版不需要服务器、账号、同步、Windows 服务或管理员权限。

这不是为了“现在少写代码”，而是为了固定四个以后不能轻易推翻的契约：事件、身份、证据、权限。Windows、macOS、Linux 后续只替换平台适配器和安装包，不替换任务图、评分、存储与 UI 查询协议。

## 本地、开源仍然没有消除的难点

本地运行解决的是数据外发问题；开源解决的是可审查性。它们都不能让 Windows 自动提供不存在的语义：

- 文件事件能证明“发生了写入或重命名”，不能单独证明“这是 Word 中某文档的另存为”。
- 浏览器能直接给出下载 URL，但文件系统只能看到新文件；两条事件要靠稳定身份与时间证据连接。
- Windows 安全边界仍会阻止未授权目录、受保护进程和部分应用信息。
- 同一文件可能被后台同步器、防病毒、索引器和用户应用共同触碰，简单按时间邻近会误关联。
- 开源源码不等于用户安装的二进制一定对应源码；发布仍需要签名、可复现构建、哈希和供应链记录。

因此最小权限不是为了做“云端式合规”，而是为了让采集结果可预测、用户敢于常驻、安装不要求管理员权限。

## 三种可行结构比较

| 结构 | 优点 | 长期问题 | 判断 |
|---|---|---|---|
| C#/.NET + WinUI 3 全 Windows 原生 | Windows API 接入快、调试体验成熟 | 核心迁移到 macOS/Linux 时需要重写或增加跨语言桥 | 不选作长期基础 |
| Electron + Node 原生模块 | UI 开发最快、生态大 | 常驻内存较高，原生模块与 Node ABI/签名/更新形成额外变化压力 | 适合快速演示，不适合基础层 |
| Rust 核心 + Tauri 2 + 平台适配器 | 核心可跨系统复用；低常驻资源；原生能力可在 Rust 内封装；UI 可快速迭代 | Rust/前端双栈，早期需要认真定义 IPC | **推荐** |

Tauri 在 Windows 使用系统 WebView2，不需要随应用打包完整浏览器；官方可生成 NSIS `setup.exe` 或 WiX MSI，并提供必须经过签名验证的更新机制。[Tauri Windows installer](https://v2.tauri.app/distribute/windows-installer/)、[WebView2 前提](https://v2.tauri.app/start/prerequisites/)、[Updater](https://v2.tauri.app/plugin/updater/)。Tauri 只是 UI 和打包壳，核心不依赖它；未来更换 UI 不影响事件图。

## 运行时边界

### `context-agent.exe`

当前用户后台代理，登录后启动。它是唯一可以写本地数据库的进程，负责采集器生命周期、事件归一化、身份解析、图投影、评分、权限策略和本地 IPC。UI 关闭后它继续运行。

不做 Windows Service，因为首版只处理当前用户明确授权的目录和活动。服务会引入管理员安装、跨用户隔离、Session 0、权限提升和更大的攻击面，却不增加 MVP 所需价值。

### `context-ui.exe`

Tauri 桌面应用，只通过版本化本地 API 查询代理和发送命令，绝不直接读写 SQLite。它可以崩溃、升级或重做而不影响采集和数据库一致性。

### `context-native-host.exe`

浏览器扩展通过 stdin/stdout Native Messaging 启动的短生命周期桥。它只验证扩展来源、限制消息大小、添加接收时间并转发给代理；不自己写库。也可以由同一 Rust 二进制的 `native-host` 子命令实现，减少发布物。

Edge/Chrome 的 Native Messaging Host 由安装器写入当前用户注册表，微软文档支持 `HKEY_CURRENT_USER`，因此不需要管理员权限。[Microsoft Edge Native Messaging](https://learn.microsoft.com/en-us/microsoft-edge/extensions-chromium/developer-guide/native-messaging)。

## 稳定依赖方向

```text
Windows / Browser adapters
          │  RawEvent
          ▼
Ingest ─► Identity ─► Provenance ─► Task Context ─► Ranking
   │          │            │              │             │
   └──────────┴────────────┴──────────────┴──────► Storage ports
                                                       ▲
UI / Native Host ─► versioned local API ─► Agent runtime│
```

约束如下：

1. 平台适配器只能产生 `RawEvent`，不能直接创建图边、修改评分或写数据库。
2. 身份模块不知道 Windows 窗口或 Tauri，只接收标准身份证据。
3. 图模块只保存观察、推断、确认关系，不负责 UI 排序。
4. 排名模块只能读取图特征和反馈，不能改写原始证据。
5. UI 不依赖平台 API，也不依赖数据库表结构。
6. 基础能力通过 Rust trait/port 连接；只有出现第二个真实实现时才抽象，不预建万能插件系统。

## 推荐仓库布局

```text
context-layer/
  apps/
    desktop/                 # Tauri shell + TypeScript UI
    browser-extension/       # Chromium Manifest V3
  crates/
    event-contract/          # 版本化 EventEnvelope、命令与查询 DTO
    ingest/                  # 校验、去重、背压、顺序与检查点
    identity/                # Artifact/FileVersion/Location 身份解析
    provenance/              # 证据边、置信度、确认/否定
    task-context/            # 显式任务和成员关系
    ranking/                 # 任务局部相关性、热度、滞回
    privacy-policy/          # 目录/应用/域名范围和保留策略
    storage-sqlite/          # 事件日志、投影、迁移、FTS
    platform-windows/        # 文件、前台窗口、启动与 DPAPI 适配
    agent-runtime/           # 生命周期与本地 IPC
    native-host/             # 浏览器桥
  schemas/                   # JSON Schema / fixture 兼容样例
  tests/
    contract/                # 跨版本契约
    fixtures/                # rename/copy/save-as/download/overflow
    end-to-end/              # 真实临时目录和浏览器桥
  docs/
    adr/                     # 不可逆架构决定
    threat-model.md
    data-model.md
  packaging/windows/
  .github/workflows/
```

`crates` 不是按 controller/service/repository 机械分层，而是按会独立变化和独立测试的业务能力划界。

## 第一份不能轻易改坏的事件契约

```rust
struct EventEnvelope {
    event_id: UuidV7,
    schema_version: u16,
    source: SourceId,
    source_sequence: Option<u64>,
    observed_at: Timestamp,
    ingested_at: Timestamp,
    scope_id: ScopeId,
    correlation_id: Option<Uuid>,
    sensitivity: SensitivityClass,
    payload: EventPayload,
    evidence: EvidenceDescriptor,
}
```

规则：

- 事件至少一次投递，以 `event_id` 幂等；不假设适配器永不重复。
- `observed_at` 与 `ingested_at` 分开，避免休眠、缓冲和浏览器桥延迟破坏时间推断。
- 适配器自己的序号和检查点必须保存；丢事件时进入 `gap_detected`，不能默默继续。
- `payload` 使用封闭枚举并版本化，不把任意 JSON 直接传播到核心。
- 原始事件追加保存；身份、图和排名都是可删除、可重建的投影。
- 任何推断都引用生成它的事件 ID、算法版本和特征摘要。

## Windows 文件采集的正确基础

### 默认模式：无管理员权限

用户在 UI 选择一个或多个根目录。`platform-windows` 使用异步 `ReadDirectoryChangesExW` 监控，并对每个进入范围的文件通过句柄取得卷序列号与 File ID。微软说明 `GetFileInformationByHandle` 返回的卷序列号和文件索引可以判断两个路径是否对应同一对象。[文件身份 API](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfileinformationbyhandle)。

目录监控缓冲区可能溢出；微软规定溢出时返回零字节，应用应重新枚举目录。因此协议必须内建：检查点、`gap_detected` 事件、范围内增量重扫和状态对账，而不是把 watcher 当可靠消息队列。[ReadDirectoryChangesExW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-readdirectorychangesexw)。

### 可选模式：USN 加速器

USN Journal 是卷级持久变更日志，但微软明确说明变更日志操作需要管理员权限，而且记录只说明对象和变化原因，不包含足够的应用/任务语义。[USN 权限](https://learn.microsoft.com/en-us/windows/win32/fileio/using-the-change-journal-identifier)、[记录语义](https://learn.microsoft.com/en-us/windows/win32/fileio/change-journal-records)。

所以 USN 必须是可选能力：单独请求提升、能力探测后启用、关闭后回到目录 watcher；事件契约完全相同。它不能成为应用启动和核心逻辑的前置条件。

## 本地数据库与可重建投影

SQLite 由代理单写，UI 通过 IPC 读取。初始表只分四类：

- `raw_event`：追加事件、来源序列和校验结果。
- `identity_*`：逻辑对象、版本、文件身份、位置历史。
- `edge`：类型化关系、状态、置信度、证据、检测器版本。
- `task_*` / `feedback` / `ranking_projection`：显式任务、反馈和可重建视图。

使用 WAL 允许代理写入时并发查询；SQLite 官方说明 WAL 是其正式事务机制。[SQLite WAL 文件格式](https://www.sqlite.org/walformat.html)。迁移必须前向兼容一个稳定版本，投影表带 `projection_version`，升级失败时回滚二进制并从原始事件重建投影。

Windows 使用 DPAPI 保护数据库密钥或敏感字段密钥；默认只能由同一机器的同一用户解密。[CryptProtectData](https://learn.microsoft.com/en-us/windows/win32/seccrypto/example-c-program-using-cryptprotectdata)。它主要防离线磁盘读取，不承诺抵御已经在同一用户会话中运行的恶意程序。

## 本地 API：UI 永远不接触表结构

首版命令：

- `GrantScope` / `RevokeScope`
- `StartTask` / `SwitchTask` / `StopTask`
- `PinArtifact` / `UnpinArtifact`
- `ConfirmEdge` / `RejectEdge`
- `ExcludeApp` / `ExcludePath` / `ExcludeDomain`
- `DeleteTaskData` / `DeleteAllData`

首版查询：

- `GetCurrentTaskView`
- `ExplainPlacement`
- `GetArtifactHistory`
- `SearchArtifacts`
- `GetCollectorHealth`
- `GetPrivacyAudit`

使用 Windows Named Pipe，协议消息采用长度前缀 + MessagePack/CBOR 或 JSON；无论编码如何，都必须由 `event-contract` 生成兼容测试。Named Pipe 限制为当前用户 SID，并进行客户端版本握手。

## 排名基础：先确定性，后模型

MVP 使用规则化、可解释评分：显式固定 > 已确认关系 > 精确下载关系 > 同 File ID > 当前任务期间编辑 > 打开/前台重叠 > 内容相似。每个展示结果返回证据列表，而不是只返回一个浮点数。

自动变化受三条硬约束：稳定桶、滞回阈值、单位时间换位预算。评分器可以以后替换，`TaskView` 合同和用户反馈不变。模型绝不能读取被排除事件，也不能把 `INFERRED` 边升级成 `OBSERVED`。

## 安装和用户使用方式

普通用户应当 **下载安装软件**，而不是克隆仓库或运行脚本。

### Alpha 推荐

发布一个已签名的、当前用户范围的 NSIS `setup.exe`：

1. 安装到 `%LOCALAPPDATA%\Programs\ContextLayer`，不弹 UAC。
2. 安装 `context-agent.exe`、`context-ui.exe` 和 Native Messaging manifest。
3. 在 HKCU 注册 Edge/Chrome Native Messaging Host。
4. 注册当前用户登录启动；首次启动仍由用户选择是否启用。
5. 数据写到 `%LOCALAPPDATA%\ContextLayer\data`，日志只保留结构化诊断，不记录正文。
6. 卸载时明确询问是否删除本地索引和设置。

便携 ZIP 只给开发者诊断，因为浏览器 Host 注册、登录启动和卸载清理天然需要安装器。Tauri 官方直接支持 NSIS `setup.exe`；MSI 当前依赖 WiX v3，构建还可能涉及正被逐步弃用的 VBSCRIPT，因此 Alpha 优先 NSIS。

### 稳定版分发

- GitHub Releases 承载版本化安装包、签名、SHA-256、SBOM 和更新 JSON。
- Tauri Updater 校验单独的更新签名；官方实现不允许关闭签名验证。
- 提交 WinGet manifest，让技术用户可以 `winget install`；WinGet 是发现/安装入口，不替代原安装包。
- 面向普通用户稳定发布时考虑 Microsoft Store/MSIX。微软推荐 Store 作为多数应用的起点，Store 提供签名和更新；直接 MSIX 分发则需要可信签名。[Windows 分发路径](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/choose-distribution-path)、[发布 Windows 应用](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/publish-first-app)。

代码开源不意味着安装包可以不签名。签名解决“这个二进制来自谁、更新是否被替换”，源码审查解决“代码做了什么”，两者不是替代关系。

## 首个可验证纵向切片

只实现一条完整链路：

1. 用户创建任务并选择一个测试目录。
2. 浏览器扩展下载一个文件，Native Host 发送 URL 与最终路径事件。
3. 文件 watcher 观察创建事件，身份模块取得 File ID。
4. Ingest 幂等落入原始事件表。
5. Provenance 生成一个 `OBSERVED downloaded_from` 边。
6. 当前任务视图显示文件和 URL，并能解释两条原始证据。
7. 用户重命名文件，逻辑对象不变，位置历史新增。
8. 代理、UI 或扩展任一崩溃后重启，事件不重复、关系可重建。

这条链路通过前，不实现自动任务推断、OCR、全文嵌入、邮件、USN、跨平台或复杂插件系统。

## 架构准入测试

- 契约：旧版本事件 fixture 能被当前版本读取；未知新字段不会破坏旧消费者。
- 幂等：同一事件重复 100 次，只产生一份原始事件和一条投影关系。
- 身份：创建、重命名、移动仍是同一 Artifact；复制产生新 File ID 和候选关系。
- 缺口恢复：人为制造 watcher 缓冲区溢出，系统产生 `gap_detected` 并重扫收敛。
- 权限：标准用户完整运行；拒绝一个目录不会让整个代理退出。
- 生命周期：UI 关闭后继续采集；代理重启后从检查点继续。
- 隐私：被排除的应用、路径和域名没有载荷进入数据库。
- 可解释：每个任务视图项目都能返回来源事件和评分组成。
- 卸载：二进制、启动项和 Native Messaging 注册清理；数据保留/删除严格按用户选择。

## 明确非目标

首个基础版本不做：跨设备同步、云端账号、团队共享、物理移动文件、管理员常驻服务、内核驱动、全盘默认采集、屏幕截图、键盘记录、自动删除、自动任务切换、插件市场、微服务或远程数据库。

## GitHub 公共仓库建议字段

在创建外部仓库前需要确认所有者。建议值：

- Owner：`WSL043`。
- Repository：`context-layer`
- Visibility：`Public`
- Description：`Local-first task context and provenance layer for desktop operating systems. Windows-first.`
- Default branch：`main`
- License：`Apache-2.0`，原因是允许商业使用，同时包含明确专利授权；比 MIT 更适合未来存在技术专利和生态 SDK 的项目。
- 初始化文件：`README.md`、`LICENSE`、`SECURITY.md`、`CONTRIBUTING.md`、`CODE_OF_CONDUCT.md`、`.gitignore`
- 首个标签：不创建；第一个能跑通纵向切片的版本再标 `v0.1.0-alpha.1`

不要先建立大量 Issue、Roadmap 或插件目录。首个公开提交应只有 ADR、事件契约、威胁模型、最小 workspace 和一条可运行测试，避免公开仓库从第一天就积累虚假架构。
