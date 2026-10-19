# 实现计划 - Tuya T5AI-Core AI 小键盘（面向 Kiro 的开发外设）

> 状态：已定稿，进入执行前尚有待确认事项（见文末「待确认事项」）。
> 本文档为设计/规格说明，中文优先维护。

## 重要约束（个人开发规则）
- **禁止**自动修改项目下的 `MyMemo.md`（由开发者自行维护）。
- 交互与设计/规格文档用**中文**优先；README 提供中英双版（`README.md` 英文主版 + `README.zh-CN.md` 中文）；代码注释用**英文**；commit message 用英文。
- ~~TuyaOpen 项目不要放 C 盘，路径不能含中文/空格~~（已作废）：本机实测当前路径 `C:\@MyWorks\...` 可正常构建，无需规避 C 盘 / 中文 / 空格。

## 问题陈述
基于 Tuya T5AI-Core 开发板打造便携 AI 小键盘：2.8" LCD + 5 茶轴按键 + 1 个 5 向摇杆 + 板载麦克风，用于提升 Kiro CLI/IDE 的开发体验。核心差异化是"语音输入到 Kiro"与"Agent 状态上屏"。分阶段交付，每阶段独立可演示。

## 最终需求与决策
- 硬件：T5AI-Core（T5-E1 模组）+ CL28CK230-18A LCD（ST7789，320×240，SPI）+ WS-1002 5向开关 + 5× 茶轴（CPG151101D06）+ 板载 Mic/Speaker
- 固件框架：原生 TuyaOpen SDK + `tos.py`，**原生 Windows 编译**（Git Bash，无需 WSL；本机实测当前仓库路径可正常构建，无需规避 C 盘 / 中文 / 空格）
- 连接分阶段：阶段1 有线（UART/USB-CDC + Companion）→ 阶段2 BLE GATT + Companion → 阶段3 BLE HID 直连
- 语音分阶段：阶段1 macOS Dictation / Apple Speech（PC 端，笔记本麦克风）→ 阶段2 板端唤醒词 + 在线 ASR → 阶段3（可选）板端本地 ASR
- Companion：macOS 原生，**Tauri (Rust + React/TypeScript)**，对照 `ref_projects/ki-board/` 的 `transport`/`protocol`/ASR 结构复用
- 从 `ref_projects/ki-board/` 借鉴纳入：Kiro CLI hook → Agent 状态上屏、`powershell_shim` 消弹窗、串口 VID/PID 自动发现、可替换传输层、`ble_probe`/`asr_probe`/`ota_flash` 探针工具

## 技术结论（已核实）
- T5：M33F@480MHz，8MB Flash/16MB PSRAM，Wi-Fi6 + BLE 5.4，48× GPIO，2×SPI+2×QSPI，3×UART，2×I2C，3×I2S，2×音频ADC+1×DAC，1× HS USB2.0。
- **USB HID Device 高风险**：TuyaOpen 对 T5 USB 仅公开 Host 模式；"直连"落到 BLE HID（BLE5.4 原生），不是 USB HID。
- **GPIO 充足**：需求约 16-17 信号 vs 48 GPIO；仅 P9(用户LED)、P29(用户按键) 已占用。
- **有线最稳**：T5 UART ↔ 板载串口桥 ↔ PC COM 口，无需 T5 原生 USB Device 支持。
- **jlceda-mcp**：当前环境未挂载，无法实测；由固件侧产出网表/连接清单作为其输入，用户在具备该 MCP 的环境验证。
- 参考项目 ki-board 已克隆到 `agentic_keyboard_with_tuya/ki-board`，companion 为 Tauri(Rust)，transport 层含 `usb_serial.rs` + `ble_gatt.rs`，可直接对照复用。

## 架构总览
固件（TuyaOpen C）：输入扫描+去抖 → 事件与状态机 → LVGL@ST7789 UI + 可替换传输层（UART / BLE GATT / BLE HID）。
Companion（Tauri: Rust + React/TS）：传输后端+协议解析 → 事件路由/映射 → macOS CoreGraphics 按键注入 / arboard 粘贴 / macOS Dictation + Apple Speech → 注入 Kiro 焦点窗口；Kiro CLI hook JSONL → 回写板子状态。

## 传输协议（阶段1 定稿、跨阶段复用）
- 阶段1 载体 UART 115200/8N1；阶段2 BLE 特征值；协议内容不变
- 帧：行分隔 JSON（开发期易调试），字段 `v`/`seq`/`type`/`payload`
- 板→PC 事件：`key`、`nav`、`voice`(start/stop)、`macro`
- PC→板 命令：`led`、`display`(Agent 状态/文本)、`config`、`ack`
- 参照 ki-board `shared/protocol/serial-protocol.md` 的 JSONL 约定

## 任务分解（测试驱动、增量、每步可演示）

### 阶段 0：脚手架与硬件定义
- **Task 1: 仓库骨架与文档基线**。建 `firmware/`、`companion/`、`hardware/`、`docs/`、`shared/protocol/`；README 中英双版；协议草案 `docs/protocol.md`（中文）。companion 用 create-tauri-app（React+TS）初始化；firmware 预留 TuyaOpen app 结构；代码注释英文。测试：Tauri 空窗可起、Rust `cargo test` 占位单测通过。Demo：结构可浏览 + 空壳窗口启动 + 协议草案可读。
- **Task 2: 引脚映射与硬件连接清单**。基于 T5 Pinmux + T5AI-Core SCH，产出 5 按键/5向开关/ST7789 SPI/背光/(预留)音频的确定 GPIO 分配，避开 P9/P29；输出 `hardware/pinmap.md` + jlceda-mcp 可用网表。LCD 用硬件 SPI；按键直连 GPIO 带内部上拉。测试：脚本校验引脚无冲突。Demo：pinmap + 网表 + 校验脚本通过。

### 阶段 1：有线 MVP（串口 + Tauri Companion + macOS Dictation / Apple Speech）
- **Task 3: 固件可编译烧录（心跳 + 板级 Config）**。`tos.py` 基于 `TUYA_T5AI_CORE.config` 建最小 app，串口打印心跳。记录原生 Windows 环境搭建步骤到 docs。测试：串口见周期心跳。Demo：上电串口输出心跳。
- **Task 4: 输入扫描——按键与 5 向开关**。带去抖(~10-20ms)与长按阈值，产生 key/nav 按下/抬起/长按事件；状态机抽成可单测模块。测试：host 侧状态机单测。Demo：按键/拨摇杆串口实时打印事件。
- **Task 5: 固件传输层——UART 事件上报（协议 v1）**。事件按 JSON 行经 UART 发出，seq 递增 + ack；传输层做成接口，UART 为首个后端。测试：host 协议编解码单测 + 串口抓包。Demo：操作输入→串口见规范帧。
- **Task 6: Companion 串口后端与协议解析（Rust）**。Tauri Rust 侧用 serialport 连 COM、解析协议、事件经 Tauri event 推前端；VID/PID+product 自动发现端口。解析器与 IO 解耦、坏帧容错；对照 ki-board `usb_serial.rs`+`protocol.rs`。测试：cargo test 字节流回放 + 坏帧用例。Demo：板操作→Tauri 窗口实时显示事件。
- **Task 7: Companion 键盘注入到焦点窗口**。key/nav/macro 按可配置映射转真实按键，macOS CoreGraphics 注入；长文本用 arboard 粘贴。映射表 JSON，支持组合键与宏；注入层封装可测。测试：注入层 mock 断言 INPUT 序列 + 记事本/终端手动验收。Demo：Kiro 终端聚焦下按板键触发预设命令。
- **Task 8: 语音触发→macOS 系统 ASR→文本注入（阶段1 核心）**。板上语音键发 voice:start；Companion 调 Apple Speech（SFSpeechRecognizer，经 ApplicationServices/Speech framework，用笔记本麦克风）听写，文本经注入层进入焦点窗口。先做一句话听写，处理无结果/超时/权限；先写独立探针 example 验证 Apple Speech 链路。测试：ASR 封装 mock 单测 + 真实语音验收。Demo：聚焦 Kiro 说话，识别文本自动出现。
- **Task 9: LCD UI（LVGL @ ST7789）与状态显示**。接 ST7789+LVGL，显示按键提示/语音录制态/连接态，预留 Agent 状态区；显示逻辑与数据解耦。测试：host 侧 UI 状态映射单测。Demo：操作与语音时屏上实时反映状态。
- **Task 10: Kiro CLI hook → Agent 状态上屏**。移植/改写 kiro_board_hook（Python 或 Rust），把 agentSpawn/userPromptSubmit/stop/postToolUse 转 JSONL，经 Companion 下发到板，LCD 显示 Agent 名与 Idle/Running/Error；hook 优先交 Companion，不可用回退直连 CDC；PC→板走协议 display。测试：mock hook 事件断言 display 命令 + 端到端验收。Demo：Kiro 跑 agent，LCD 实时显示状态。
- **Task 11: hook 噪声消除 + 阶段1 打包**。消除 Kiro hook 触发的终端噪声（macOS 下静默运行 hook 子进程）；tauri build 出 macOS 安装包（DMG / .app bundle）。测试：hook 运行无弹窗 + 安装包可运行。Demo：完整有线闭环（按键/语音/Agent 状态全通、无弹窗、安装包可分发）。

### 阶段 2：无线化（BLE GATT + Companion；板端唤醒 + 在线 ASR）
- **Task 12: 固件 BLE GATT 自定义服务**。新增 ble_gatt 传输后端，复用协议 v1，经 GATT 特征值收发；与 UART 后端并存可切换；处理配对/重连。测试：host 协议层复用测试 + BLE 抓包。Demo：拔 USB，BLE 下按键/状态同步照常。
- **Task 13: Companion BLE 后端（btleplug）**。Rust 侧加 transport/ble_gatt.rs，btleplug 扫描/连接/收发，与串口后端统一接口，UI 可选传输；对照 ki-board ble_gatt.rs，写 ble_probe example 先验证。测试：cargo test mock 传输层 + 真实设备验收。Demo：Companion 经 BLE 收事件并注入 Kiro。
- **Task 14: 板端唤醒词 + 在线 ASR**。板端 KWS 唤醒后采音，经在线 ASR（TuyaOpen 语音能力或 3P WebSocket 流式，参照 ki-board asr_probe）转文本，经 BLE/串口交 Companion 注入；音频 16kHz/16bit；密钥用 keyring 存储。测试：ASR 适配层 mock 单测 + 端到端验收。Demo：对板说唤醒词+指令，文本进入 Kiro，无需碰 PC 麦克风。

### 阶段 3：BLE HID 直连 + 可选板端本地 ASR
- **Task 15: 固件 BLE HID over GATT**。实现标准 BLE HID keyboard，基础按键/导航直达系统（免 Companion）；与 GATT 高级通道并存（HID 管按键、GATT 管状态/语音）；映射固件内可配。测试：多系统识别验证（Win 为主）。Demo：不装 PC 软件，板作为蓝牙键盘直接输入。
- **Task 16（可选）: 板端本地 ASR**。采音质量达标前提下上受限词表本地 ASR，实现离线固定指令；先评估采音质量与词表规模，不达标则维持阶段2 在线方案。测试：词表命中率评测。Demo：断网下板端识别固定指令并注入。

## 执行注意
- 每个 Task 完成后运行对应构建/测试（Rust `cargo test`、前端 `tsc`、固件构建），验证通过再进入下一个。
- 传输层与协议层跨阶段复用，务必保持接口稳定。
- 优先对照已克隆的 `ref_projects/ki-board/` 项目结构复用，减少重复设计。

## 待确认事项
> 执行前需与开发者确认（占位，待补充）。
