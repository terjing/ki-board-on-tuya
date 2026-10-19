# 面向 Kiro 的 AI 小键盘（Tuya T5AI-Core）

一款基于 **Tuya T5AI-Core** 开发板的便携 AI 小键盘，用于提升 Kiro CLI / IDE 的
开发体验。核心差异化功能是**语音输入直达 Kiro**与 **Agent 状态上屏显示**。

> 状态：**阶段 0 已完成，阶段 0.5 硬件 Bring-up 进行中**。扩展底板 v1.0
> 已完成设计与审核；6 个使用本项目 IO 的独立 Bring-up 固件均已构建，等待在实板上
> 依次验收。产品固件和上位机仍为骨架。

*(English version: [README.md](README.md))*

## 硬件

| 部件 | 型号 |
|------|------|
| 主控 | T5AI-Core（T5-E1 模组，Cortex-M33F @ 480 MHz，Wi-Fi6 + BLE 5.4） |
| 屏幕 | CL28CK230-18A LCD（ST7789，320×240，SPI） |
| 按键 | 5× 茶轴机械开关（CPG151101D06） |
| 导航 | WS-1002 5 向摇杆 |
| 音频 | 板载麦克风 / 扬声器 |

## 架构

- **固件（TuyaOpen C）**：输入扫描 + 去抖 → 事件与状态机 →
  LVGL @ ST7789 UI + 可替换传输层（UART / BLE GATT / BLE HID）。
- **上位机（Tauri: Rust + React/TS）**：传输后端 + 协议解析 → 事件路由/映射 →
  Win32 SendInput 注入 / 剪贴板粘贴 / Windows 系统 ASR → 注入到焦点 Kiro 窗口；
  Kiro CLI hook（JSONL）→ 把 Agent 状态回写到板子。

## 路线图（分阶段，每阶段独立可演示）

1. **阶段 0** — 脚手架与硬件定义（仓库骨架、引脚映射）。
2. **阶段 1** — 有线 MVP：串口 + Tauri 上位机 + Windows 系统 ASR。
3. **阶段 2** — 无线化：BLE GATT + 板端唤醒词 + 在线 ASR。
4. **阶段 3** — BLE HID 直连 + 可选板端本地 ASR。

完整任务分解见 [docs/implementation-plan.md](docs/implementation-plan.md)。

## 仓库结构

```text
.
├── firmware/            # TuyaOpen 产品骨架 + 独立 bring-up 程序
├── companion/           # Tauri（Rust + React/TS）上位机
├── hardware/
│   └── pinmap/          # GPIO 分配 + 网表（Task 2）
├── enclosure/           # 3D 打印外壳设计（独立子项目）
├── shared/protocol/     # 权威协议规范
├── docs/                # 设计/规格文档（中文优先）
└── ref_projects/        # 外部参考项目（已 git-ignore，不属于本仓库）
    ├── ki-board/
    └── signal_light_demo/
```

## 构建（骨架）

前置：Node 22+、Rust 1.96+。macOS 构建与权限说明见
[docs/companion-macos.md](docs/companion-macos.md)（英文）。

```bash
cd companion
npm ci
npm run dev              # 启动骨架窗口
cd src-tauri && cargo test   # 运行协议/传输层单元测试
```

## 文档

| 文档 | 内容 |
|------|------|
| [prd.md](docs/prd.md) | **产品需求文档（供电路审核与代码开发）** |
| [implementation-plan.md](docs/implementation-plan.md) | 需求、决策、16 任务计划 |
| [protocol.md](docs/protocol.md) | 协议导读（指向权威源） |
| [shared/protocol/protocol.md](shared/protocol/protocol.md) | 权威协议规范 |
| [hardware/pinmap/](hardware/pinmap/) | 引脚映射、网表、BOM、冲突校验 |

## 约定

- 交互与设计文档：中文优先。README：中英双版（英文为主版）。
- 代码注释：英文。Commit message：英文。
- `ref_projects/` 仅存放**参考项目**，不纳入版本管理。

## 许可

供学习与个人使用。所用第三方库各自遵循其原始许可证。
