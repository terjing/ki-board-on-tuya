# 产品需求文档（PRD）— Agentic Keyboard for Kiro

| 项 | 内容 |
|----|------|
| 产品名 | Agentic Keyboard for Kiro（面向 Kiro 的 AI 小键盘） |
| 版本 | v1.3 |
| 日期 | 2026-08-21 |
| 状态 | 已定稿，供**电路设计审核**与**代码开发**使用 |
| 硬件基线 | Tuya T5AI-Core 开发板 + 自制扩展底板 |
| 受众 | 硬件设计/审核、固件开发、上位机开发 |

**关联文档**

| 文档 | 内容 |
|------|------|
| `hardware/pinmap/pinmap.md` | 引脚映射权威表（含 44P 排针、18P FPC 逐脚） |
| `hardware/pinmap/netlist.md` | 功能级网表 |
| `hardware/pinmap/bom.md` | BOM 与采购标记 |
| `shared/protocol/protocol.md` | 通信协议权威定义 |
| `docs/implementation-plan.md` | 16 任务 4 阶段实施计划 |
| `tuya-kb/` | TuyaOpen/T5 知识库（参考，不入库） |
| `ref_projects/ki-board/` | 参考项目（ESP32-S3 版，不入库） |

---

## 1. 产品概述

### 1.1 定位

一款便携 AI 小键盘外设，用于**提升 Kiro CLI / Kiro IDE 的开发体验**。它不是通用键盘，
而是面向"人与 AI Agent 协作编程"这一特定工作流的专用输入与状态显示设备。

### 1.2 核心差异化

| 差异点 | 说明 |
|-------|------|
| **语音输入直达 Kiro** | 按键触发语音，识别文本自动注入当前聚焦的 Kiro 窗口，免切换、免打字 |
| **Agent 状态上屏** | Kiro CLI hook 把 Agent 的 Idle/Running/Error 状态实时显示在设备 LCD 上 |

### 1.3 目标用户与场景

**用户**：使用 Kiro CLI/IDE 进行 agentic coding 的开发者（首个用户为作者本人）。

**核心场景**

| # | 场景 | 痛点 | 本产品解法 |
|---|------|------|-----------|
| S1 | 向 Agent 口述较长需求 | 打字慢、思路被打断 | 按语音键说话 → 文本自动进入 Kiro |
| S2 | Agent 长时间运行时想知道进展 | 需盯屏幕/切窗口 | LCD 常显 Agent 名与状态 |
| S3 | 高频操作（发送/中断/确认） | 记快捷键、手离键盘 | 物理键一键触发 |
| S4 | 多 Agent/多会话切换 | 窗口切换繁琐 | 摇杆导航切换 |

### 1.4 产品形态

外接 3.7V 锂电池供电的独立小盒子，桌面摆放：正面 2.8" LCD + 5 个机械键 + 1 个 5 向摇杆，
经 USB 线或 BLE 与 PC 相连。3D 打印外壳（独立子项目 `enclosure/`）。

---

## 2. 系统架构

### 2.1 硬件架构（路线 A：开发板 + 扩展底板）

```
┌─────────────────────────────────────────────┐
│  T5AI-Core 开发板（成品，不改板）              │
│  ─────────────────────────────────────────  │
│  T5-E1 模组: M33F@480MHz, 8MB Flash, 16MB RAM │
│  Wi-Fi 2.4G + BLE 5.4                        │
│  板载: Type-C / ETA6003 电池管理 / 总电源开关   │
│        模拟 MIC + codec(AEC) / 1W 功放+扬声器  │
│        CH342F 双路 USB 串口桥 / 用户 LED/按键   │
└────────────┬────────────────────────────────┘
             │  44P 排针 = J1(1×22) + J2(1×22)
┌────────────┴────────────────────────────────┐
│  扩展底板 PCB（自制，本项目设计范围）           │
│  ─────────────────────────────────────────  │
│  5× 机械键(CPG151101D06直焊) · 1× 5向摇杆(WS-1002) │
│  18P 0.5mm FPC 座 → 2.8" LCD(ST7789)         │
│  Type-C → CH340C(USB-UART,可烧录+通信,UART0; │
│    VBUS经LDO仅供CH340C,不供系统)             │
│  锂电池并联座(JST,同BAT网,带电池总开关) ·      │
│  3× WS2812(P4/SPI1,经 SN74LV1T34 电平转换)    │
│    · CH340C自动复位                          │
│  上拉/限流/去耦/ESD · 2× 1×22 母排座           │
└─────────────────────────────────────────────┘
```

**关键设计原则**：**充电管理(ETA6003)、音频、麦克风、扬声器由开发板负担**，扩展底板不重复。
但扩展底板**自带 Type-C(CH340C 桥,可烧录+通信) + 锂电池并联座 + 3×WS2812**，以支持最终产品形态
（外壳只露扩展板 Type-C）。扩展板 USB 与开发板 USB 共用 UART0，**同一时间只插一个 Type-C**；
电池座与开发板并联同一 BAT 网，**同一时间只插一节电池**。

### 2.2 软件架构

```
┌──────────────────────┐        ┌───────────────────────────┐
│ 固件 (TuyaOpen C)     │        │ Companion (Tauri)          │
│ ──────────────────── │        │ ─────────────────────────  │
│ 输入扫描+去抖         │        │ transport 后端(串口/BLE)    │
│   ↓                  │  JSONL │   ↓                        │
│ 事件与状态机          │◄──────►│ 协议解析/路由               │
│   ↓                  │ UART/  │   ↓                        │
│ LVGL @ ST7789 UI     │ BLE    │ CoreGraphics 注入 / 剪贴板  │
│ 可替换传输层          │        │ macOS ASR / 云 ASR 协同     │
└──────────────────────┘        └────────────┬──────────────┘
                                             │
                                   ┌─────────┴──────────┐
                                   │ Kiro CLI / IDE      │
                                   │ (焦点窗口)           │
                                   │ hook → Agent 状态    │
                                   └────────────────────┘
```

### 2.3 数据流

> **双链路架构（关键）**：Wi-Fi 与「设备↔PC」是两条**独立**链路。
> Wi-Fi **只用于云 ASR**（与 OTA），**不承担**设备到 PC 的数据传输；
> 设备↔PC 走 UART / BLE。按键事件与 ASR 文字都经后者送 PC。

```
物理按键(GPIO 扫描) ──┐
                       ├──→ [按键事件 / ASR 文字] ──→ UART 或 BLE ──→ Companion ──→ Kiro 焦点窗口
板载 MIC → Wi-Fi 云 ASR ┘        (AI_USER_EVT_ASR_OK)
```

**上行（设备→PC）**：按键/摇杆/语音事件 → JSONL over UART/BLE → Companion → 映射 → macOS CoreGraphics/粘贴 → Kiro 窗口

**下行（PC→设备）**：Kiro CLI hook → Companion → JSONL → 设备 → LCD 显示 Agent 状态

**语音方式②（一期）**：设备语音键 → `voice:start` → Companion 调 macOS Dictation / Apple Speech（**PC 麦克风**）→ 文本注入
（此路径设备不采音，仅作触发器）

**语音方式①**：设备语音键 → 板载 MIC 采音 → Wi-Fi 云 ASR → `AI_USER_EVT_ASR_OK` 取文本 → 经协议给 Companion → 注入

---

## 3. 硬件规格（供电路设计审核）

### 3.1 器件规格

| 部件 | 型号 | 关键参数 | 库存 |
|------|------|---------|------|
| 主控板 | T5AI-Core（T5-E1） | M33F@480MHz, 8MB Flash, 16MB RAM, Wi-Fi2.4G+BLE5.4, 73×29×11mm | ✅ **已有若干** |
| LCD | CL28CK230-18A | ST7789, 320×240, 4线SPI + 电阻触摸(RTP), 18P 0.5mm FPC | ✅ 有 1 个 |
| 机械键 | CPG151101**D06 茶轴**（直焊） | 5×（建议备 10+）；**轴体直焊 PCB，非热插拔**；MX 段落手感；立创 C49234237 现货 ￥0.69/个(5+) | ⚠️ 需采购，见 `bom.md`（青轴 D91 无立创现货，已定 D06 茶轴） |
| 摇杆 | WS-1002 | 6 脚, 4向+中心, 0.1A/DC12V, 操作力 160±50gf(方向)/250±50gf(中心), 寿命 5万次 | ✅ 有 2 个 |
| FPC 座 | 18P 0.5mm 下接/翻盖 | 匹配 LCD 排线 | ❌ 需采购 |
| 母排座 | 2× 1×22 2.54mm | 匹配开发板 HDR254-1x22-M-H890 | ❌ 需采购 |
| 电池 | 3.7V 锂电（聚合物/18650） | 带保护板，1C 放电 | 用户自备 |
| USB-UART 桥 | CH340C（SOP-16） | 扩展板 Type-C 的 USB→串口，接 UART0 | ✅ 库存 C84681 ×5 |
| Type-C 母座 | 16P 卧贴（含 CC） | 扩展板 USB 口 | ✅ 库存多款 |
| 电平转换 | SN74LV1T34（SOT-23-5） | WS2812 3.3→5V | ✅ 库存 C100024 ×5 |
| 复位三极管 | S8050（SOT-23,NPN） | CH340C DTR/RTS 自动复位 | ✅ 库存 ×50 |
| RGB LED | WS2812（XL-3528RGBW） | 3× 菊花链，P4/SPI1（v1.2：P13→P4） | ✅ 库存多款 / 自营现货 244K+ |
| 电源指示 LED | 普通红色 0603 LED | 2×（EDA 位号 LED4/LED5）：LED4=Type-C(VBUS)供电指示、LED5=电池(BAT_P)供电指示；各串限流电阻 | ✅ 库存/易采 |
| 电池座 | JST PH 1.25mm 2P | 与开发板并联同 BAT 网 | ✅ 库存 ×100 |
| 电池总开关 | 拨动开关 SK12D07VG5 | 串于电池 BAT+ | ✅ 库存 C431548 ×20 |
| CH340C 供电 LDO | HE9073A33M5R（SOT-23-5,3.3V） | VBUS 5V→3.3V 专供 CH340C | ✅ 库存 C723789 ×5 |

### 3.2 引脚分配（权威表）

**开发板已占用（扩展底板不可复用）**

| 引脚 | 用途 | 有效电平 |
|------|------|---------|
| P5 | SPI1_MISO（v1.2 保留给 WS2812 的 SPI1 组；原 LCD_BL 已移走） | — |
| P6 | LCD 复位 RST | — |
| P7 | LCD 电源使能 PWR | 高 |
| P8 | LCD 背光 BL（v1.2：P5→P8, J2.16/PWM2） | 高 |
| P9 | 用户 LED | 高 |
| P10 / P11 | UART0 RX/TX（串口桥共用，烧录+日志+一期通信） | — |
| P14 / P16 | LCD SPI0 SCK / MOSI | — |
| P15 / P17 | LCD CS / DC | — |
| P29 | 用户按键 | 低（上拉） |
| P39 | 扬声器使能 | 低 |

**扩展底板新增（全部内部上拉、按下接地、低有效）**

| 逻辑名 | T5 引脚 | 排针 | 器件脚 |
|--------|--------|------|-------|
| KEY1 | P24 | J2.13 | SW1 |
| KEY2 | P25 | J2.8 | SW2 |
| KEY3 | P26 | J2.9 | SW3 |
| KEY4 | P27 | J2.12 | SW4 |
| KEY5 | P23 | J2.7 | SW5 |
| NAV_UP | P32 | J2.14 | 摇杆① (A) |
| NAV_DOWN | P20 | J2.4 | 摇杆③ (B) |
| NAV_LEFT | P21 | J2.5 | 摇杆⑥ (C) |
| NAV_RIGHT | P22 | J2.6 | 摇杆④ (D) |
| NAV_CENTER | P30 | J2.11 | 摇杆② |
| GND | — | J1/J2 GND | 摇杆⑤ (Com) |
| WS2812_DIN | P4 | J2.20 | 3× WS2812 菊花链，经 SN74LV1T34 电平转换（v1.2：P13→P4=SPI1_MOSI，官方 SPI 驱动） |

> ⚠️ **审核要点**：P28 / P33 / P35 **未从 44P 排针引出**，禁止使用。
> 引脚冲突已用 `hardware/pinmap/check_pins.py` 校验通过（12 板载占用 + 11 新增，无重叠；11 = 5 键 + 5 摇杆方向 + WS2812_DIN）。

### 3.3 LCD 连接（18P FPC 逐脚）

| FPC Pin | 信号 | 连接 |
|---------|------|------|
| 1 | GND | GND |
| 2 | RST | P6（J2.18） |
| 3 | SCL | P14（J1.14），可选串 22Ω 阻尼 |
| 4 | RS (DC) | P17（J1.12） |
| 5 | CS | P15（J1.15） |
| 6 | SDA (MOSI) | P16（J1.13），可选串 22Ω 阻尼 |
| 7 | SDO (MISO) | 不接（单向写屏） |
| 8 | GND | GND |
| 9 | VCC | 3.3V（受 P7 使能控制），就近 1µF + 100nF 去耦 |
| 10 | LEDA | 背光阳极 → 3.3V |
| 11–14 | LEDK1–4 | 背光阴极 4 路 → 限流电阻 → GND；P8 做开关（v1.2：P5→P8） |
| 15–18 | XL/YU/XR/YD | 电阻触摸，一期**不接**，PCB 预留焊盘 |

> **审核要点**：背光限流电阻阻值需按实测亮度确定（接线图注明 R1 可调）；
> 4 路 LEDK 应各自串限流或并联后统一限流，需确认屏内 LED 是否已并联。
> 脚序以 LCD 规格书为准：Pin 1=GND、Pin 2=RST；旧版文档曾整体错位一位，现已修正。

### 3.4 电源

| 项 | 实现 | 说明 |
|----|------|------|
| 输入① | Type-C USB 5V | 开发板自带，外壳需开孔 |
| 输入② | JST PH 1.25mm 锂电 3.7V | 开发板自带电池座 |
| 充电管理 | ETA6003 | 开发板自带，**4.2V / 500mA** |
| 总电源开关 | 开发板拨动开关 | 开发板自带（切整机主电源），**外壳需开孔可拨动** |
| 扩展板电池总开关 | 拨动开关 SK12D07VG5（串 BAT+） | 扩展板自带，**仅通断扩展板这节电池**；Type-C 插入时系统仍通电/充电（见 §3.6） |
| 电源域 | LN2220PAR 升压 5V（音频）+ RY3408 降压 3.3V（T5/数字） | 开发板自带 |
| 扩展底板取电 | 经排针取 3.3V（J2.1）与 GND | LCD 与输入器件供电 |

> **审核要点**
> 1. 充电电流固定 500mA：若用 18650（2000–3500mAh），充满需 4–7h，建议选 500–2000mAh 聚合物电池。
> 2. 电池须带过充/过放/短路保护板。
> 3. **两级电源开关（已定稿）**：整机主电源开关由**开发板自带拨动开关**满足（外壳开孔即可）；
>    扩展板另加一个**电池总开关**（SK12D07VG5，串于扩展板电池座 BAT+），语义 = **仅通断扩展板这节电池**
>    （关断=该电池断供；Type-C 插入时系统仍通电/充电）。⚠️ 受 44P 排针限制，扩展板**无法**实现
>    “一关全断（含 USB）”的真总开关——ETA6003 系统输出节点在开发板内部、排针未引出。详见 §3.6。
> 4. 烧录固件时必须打开电源开关。

### 3.5 输入电路

- 每个按键/摇杆方向：一端接 GPIO，另一端接 GND；使用 **T5 内部上拉**，无需外置上拉电阻。
- 可选加固（长走线/金属外壳）：串 100Ω 限流 + 对地 100nF 去抖 + ESD（LESD5D5.0，库存有）。
- 摇杆⑤(Com) 接 GND。

### 3.6 扩展板 Type-C / 电池 / WS2812（新增）

**Type-C + CH340C（USB→UART，可烧录+通信）**
- T5 无 USB Device 能力 → 用 **CH340C** 提供 CDC；串口接 **UART0（P10/P11）**，与开发板板载 CH342F 共用。
- **同一时间只插一个 Type-C**（TX 串 2kΩ 隔离降低总线争用；库存 C60488，460800 烧录 RC 影响可忽略）。
- **自动复位/下载**（照抄开发板 SCH，Q2 S8050 单管）：RTS0→Q2基极(直连)；DTR0→R22(4.7k)→Q2基极；
  Q2集电极→R14(100Ω)→T5_RST(J2.2)；发射极→GND。T5_RST 上拉 R47(10k)+C51(0.1µF) 开发板已有。`tos.py flash` 一键烧录。
- Type-C CC1/CC2 各 5.1kΩ 下拉（UFP/设备角色）；USB D+/D- 串 ESD。
- **CH340C 供电**：Type-C VBUS(5V) → 小 LDO(HE9073A33M5R,3.3V) → CH340C VCC=3.3V。仅供 CH340C 一颗芯片，
  **不给系统灌电**；IO=3.3V 直接匹配 T5；**不依赖电池** → 电池空了插 Type-C 也能烧录/救砖。V3 脚与 VCC 短接。
- **Type-C 只做数据、不供电系统**（VBUS 不接排针 5V）。系统供电靠电池 + 开发板 Type-C。
- 烧录 vs 通信区分：复位时下载握手进 bootloader；PC 端串口独占（TyuTool vs Companion 互斥）。

**锂电池并联座 + 电池总开关**
- JST PH 1.25mm 2P 座，BAT+ 经**电池总开关(SK12D07VG5)** 后与开发板 JST 并联同一 BAT 网；充电由开发板 ETA6003 管。
- ⚠️ **同一时间只插一节电池**（两座各插一节会电池直接并联倒灌，危险）。
- **电池总开关**：串在扩展板电池 BAT+ 路径，关断=切断电池供电（关机）；**Type-C 插入时系统仍照常通电/充电**（开关只管电池路径）。

**3× WS2812 RGB LED**
- 单线菊花链，数据脚 **P4（J2.20，SPI1_MOSI；v1.2 由 P13 迁入）**，由官方 SPI WS2812 驱动，1 根数据线控 3 颗。
- 3.3V GPIO → **SN74LV1T34** 电平转换 → 5V DIN；DIN 串 100-330Ω；每颗 100nF 去耦；5V 供电（来自排针，非 Type-C）。

**2× 电源指示灯（普通红色 LED，EDA v1.0 实现，2026-08-24 补录）**
- 除 3 颗 WS2812 外，扩展板另有 **2 颗普通红色 0603 指示灯**（EDA 位号 LED4/LED5，非 WS2812）：
  - **LED4**：`VBUS → 限流电阻(R6) → LED4 → GND`，指示 **Type-C(VBUS 5V) 供电**（插 Type-C 亮）。
  - **LED5**：`BAT_P → 限流电阻(R10) → LED5 → GND`，指示 **电池(BAT_P) 供电**。
- 纯硬件电源状态指示，不占用 GPIO；限流电阻阻值以 EDA 原理图为准。

### 3.7 电路审核检查清单

| # | 检查项 | 期望 |
|---|-------|------|
| H1 | 所有使用的 GPIO 均在 J1/J2 排针引出清单内 | 无 P28/P33/P35 |
| H2 | 无引脚重复分配、无与板载占用冲突 | `check_pins.py` exit 0 |
| H3 | 母排座排布与开发板排针一致（2× 1×22, 2.54mm, **行中心距 25.502mm 实测**） | 机械可插合 |
| H4 | FPC 座极性/方向与 LCD 排线触点朝向匹配 | 下接 vs 上接确认 |
| H5 | FPC VCC 就近去耦（1µF + 100nF） | 已布 |
| H6 | 背光限流电阻在位、阻值可调（先用较大值） | 已布 |
| H7 | 触摸脚 14–18 预留但不连 | 焊盘预留 |
| H8 | SPI 走线尽量短、等长、远离电源/音频 | 48MHz 完整性 |
| H9 | 3.3V 取自 J2.1，GND 多点回流 | 电源完整性 |
| H10 | 外壳开孔：**扩展板 Type-C**、电源开关、扬声器、麦克风、复位 | 与 `enclosure/` 对齐 |
| H11 | CH340C TX→T5 RX(P10) 串隔离电阻；DTR/RTS→S8050→T5_RST(J2.2) 复位电路 | 照抄开发板 SCH |
| H12 | Type-C CC1/CC2 各 5.1kΩ 下拉；D+/D- 串 ESD | UFP 角色 |
| H13 | 扩展板电池座 BAT+/- 与开发板 JST 同网；丝印标注"只插一节" | 防倒灌 |
| H14 | WS2812 经 SN74LV1T34 电平转换；5V 供电；DIN 串阻 + 每颗去耦 | P4/SPI1（v1.2：P13→P4） |
| H15 | 两个 Type-C 共用 UART0 — 丝印/文档标注"只插一个" | 防总线争用 |
| H16 | CH340C 供电 = Type-C VBUS→LDO(3.3V)，**不接系统电**；V3 与 VCC 短接 | 独立供电、IO 3.3V |
| H17 | 电池总开关串于扩展板 BAT+；Type-C 插入不受开关影响 | 电池路径通断 |

---

## 4. 功能需求

优先级：**P0** 必须（一期）· **P1** 重要（一期内尽力）· **P2** 二期

### FR-1 物理按键输入 · P0

| 项 | 内容 |
|----|------|
| 描述 | 5 个机械键产生按下/抬起/长按事件，经协议上报 PC，由 Companion 映射为按键/宏注入 Kiro |
| 去抖 | 10–20ms（固件侧） |
| 长按阈值 | 默认 800ms（可配置） |
| 事件 | `key`: `{id: k1..k5, action: down|up|long, held_ms}` |
| 验收 | 每键按下/抬起/长按均产生正确事件，无抖动重复；映射后在 Kiro 窗口产生预期输入 |

### FR-2 摇杆导航 · P0

| 项 | 内容 |
|----|------|
| 描述 | 5 向摇杆（4 方向 + 中心按下）产生导航事件 |
| 事件 | `nav`: `{dir: up|down|left|right|center, action: down|up|long, held_ms}` |
| 方向标定 | 物理 A/B/C/D ↔ 上/下/左/右 在装配后由固件常量标定 |
| 验收 | 5 个方向均可识别，无串扰（同时按多向时行为明确） |

### FR-3 LCD 显示 · P0

| 项 | 内容 |
|----|------|
| 描述 | 2.8" 320×240 显示：连接状态、当前按键提示、语音录制状态、Agent 状态区 |
| 技术 | LVGL @ ST7789，SPI0 48MHz，RGB565 |
| 分区 | 顶部：连接/电量；中部：Agent 名 + 状态；底部：按键功能提示/语音指示 |
| 验收 | 上电显示正常；操作与语音时状态实时反映；无花屏/撕裂 |

### FR-4 语音输入 · P0 / P1

> 依据 `tuya-kb/11-product-design/01-feasibility-matrix.md`（源码级核实）与
> `tuya-kb/08-ai-audio-lcd/02-voice-dictation-and-events.md`。
> **架构澄清**：Wi-Fi **只用于云 ASR**，不承担设备↔PC 的数据传输；两者是独立链路。

| 方式 | 优先级 | 阶段 | 实现 | 关键 API / 前置条件 |
|------|-------|------|------|-------------------|
| ② PC 端收声 + 本机/3P ASR | **P0** | 一期 | 设备发 `voice:start` → Companion 用 **PC 麦克风** + macOS Dictation / Apple Speech (SFSpeechRecognizer) 听写 → 注入 | 纯 PC 侧，无前置条件。⚠️ **此方式下设备在语音链路中无角色**（不采音），仅作触发器 |
| ① 板载 MIC + 云 ASR | **P1** | 一期 | 板载 MIC 采音（16kHz/16bit，含 AEC）→ Wi-Fi 上云 → 取文字 → 经协议给 Companion → 注入 | `ai_chat_init` + `ai_user_event_notify_register(cb)` → 事件 `AI_USER_EVT_ASR_OK` → 从 `AI_NOTIFY_TEXT_T.data` 取文本。**需 Wi-Fi 配网 + 涂鸦云 AI 授权（UUID/AuthKey）** |
| ③ 板载 MIC → 传 PC → PC ASR | P2 | 二期 | 板端取 PCM 经 UART/BLE 传 PC，PC 侧 ASR | `ai_audio_input` 的 `AI_AUDIO_OUTPUT output_cb` 或 `AI_USER_EVT_MIC_DATA` 取 PCM。**带宽**：16k×16bit ≈ **256kbps** → USB 串口（高波特）可行；**BLE 需压缩**（ADPCM/Opus） |

> **约束**：设备未配网时仅方式②③可用（无网 = 无云 ASR，方式①出局；②用 PC 麦克风、③设备只传音频，均不依赖联网）。
> **验收**：聚焦 Kiro 窗口，触发语音后说话，识别文本自动出现在输入位置；处理无结果/超时/权限异常。

### FR-5 PC 连接方式 · P0 / P2

> 依据 `tuya-kb/11-product-design/00-wireless-ai-keyboard.md` 与 `01-feasibility-matrix.md`（源码级核实）。
> **三种方式的板端前半段（按键 GPIO 扫描、取 ASR 文字）完全复用，只换传输/协议层。**

| 方式 | 优先级 | 阶段 | 板端实现 | PC 端 | 即插即用 | 工程量 |
|------|-------|------|---------|-------|:---:|:---:|
| ① USB 串口 + 助手 | **P0** | 一期 | `tkl_uart_*` 写 → 板载 CH342F → PC COM | Companion 读串口注入 | ❌ | 最低 |
| ② BLE 自定义 GATT + 助手 | **P0** | 一期 | `tkl_ble_gatts_service_add` 私有服务 + `tkl_ble_gatts_value_notify` | Companion（btleplug）订阅注入 | ❌ | 中低 |
| ③ BLE HID | P2 | 二期 | 原生 NimBLE 手搓 HID GATT 0x1812 | 无需软件（系统认作蓝牙键盘） | ✅ | 高 |

**方式② 实现清单（一期无线）**

1. 私有服务用 **128-bit UUID**；一个 **Notify** 特征推「按键事件 / ASR 文字」给 PC；
   可选一个 **Write** 特征供 PC → 板（配置/回执）。
2. 发送用 `tkl_ble_gatts_value_notify(conn, char_handle, data, len)`。
3. **TKL 限制**：每服务 **≤ 6 个特征**；无描述符字段（私有协议足够）。

**方式③ 实现清单（二期，端口级开发）**

1. `tuya_ble_cfg.h` 开 SM：`TY_HS_BLE_SM_LEGACY`（或 `_SC`）= 1（**默认 0**；HID 强制加密+绑定）。
2. **绕过 TAL/TKL**，用原生 NimBLE `ble_gatts_register_svcs` 定义：
   - HID Service **0x1812**：Report Map、Input Report + CCCD(0x2902) + **Report Reference(0x2908)**、
     HID Info、HID Control Point、Protocol Mode
   - DIS **0x180A**（有现成 `ble_svc_dis`）、Battery **0x180F**
3. 广播 Appearance = 键盘；GAP 可连接可绑定；bonding 持久化（默认存 3 个）。
4. **字符→HID Usage 映射表**（含 Shift 修饰）+ 按键→HID Usage 映射。
5. ⚠️ **无现成 `ble_svc_hid`**，HID 服务需手写（可移植 NimBLE 上游 HoGP 例子）。

**各方式下按键与文字的表现**
- **③ BLE HID**：按键 → HID 按键报文；文字 → 拆成按键序列逐字敲。系统直接当键盘。
- **①② 助手模式**：按键 → 事件消息；文字 → 文本消息；Companion 统一转键盘注入。

> **技术约束（已核实）**：T5 的 USB **仅 Host 模式**，SDK 无 USB device 栈 → **USB HID 不可行**，
> "直连"只能走 BLE HID。
> **验收**：①拔插自动识别端口，事件不丢；②拔掉 USB 后 BLE 下功能等价；③系统识别为蓝牙键盘。

### FR-6 Wi-Fi 配网与云激活 · P1

> 依据 `tuya-kb/11-product-design/01-feasibility-matrix.md`（源码级核实）。
> **重要区分**：「连上路由器上网」≠「涂鸦云已激活」。云 ASR（FR-4 方式①）需要**两者都成立**。

| 方式 | 实现 | 关键 API | 提醒 |
|------|------|---------|------|
| ① 官方默认 BLE 配网 | 内置 `NETCFG_TUYA_BLE` + SmartLife App 扫描添加 | `netmgr_conn_set(..., NETCFG_TUYA_BLE)` | ⚠️ 与「BLE 当键盘」**共用蓝牙**，见风险 R8 |
| ② USB + 助手配网 | Companion 经串口下发 SSID/密码 | `tkl_wifi_station_connect(ssid, passwd)` 或 netmgr `NETCONN_CMD_SSID_PSWD` | ⚠️ **只解决联网**，不等于云激活 |

**云激活链路（悬点已解，源码核实）**

激活请求强制带 **token**，region 从 token 解析（`atop_service.c` / `tuya_endpoint.h`）。token 有两种来源：

1. App 配网（BLE/AP）时由 App 生成；
2. **MQTT bind（免 App）**：设备用 **UUID + AuthKey** 连涂鸦 MQTT broker，云端直接下发
   token + region + regist_key。相关实现：`mqtt_bind.c` 的 `mqtt_bind_token_get_thread`、
   协议号 `PRO_MQ_ACTIVE_TOKEN_ON = 46`（cloud→device 直接下发激活 token）、
   `tuya_endpoint_update_auto_region()` 自动定区。

**结论**：持有 UUID+AuthKey（设备 License）+ Wi-Fi（可经 USB 配网设置）
→ 设备可**免 App、走 MQTT bind 自取 token 完成云激活**。即 **方式② + 云 ASR 组合可行**，
不必依赖官方 BLE/App 配网。

> **仅剩待实测项**：首次**账号绑定**（把设备关联到某用户账号）是否为涂鸦云 AI 服务的必需前置——
> 取决于产品在平台的配置。若必需，首次用官方 BLE 配网（方式①）绑一次即可，之后 `fast_connect` 复用。
>
> **约束**：设备只连 **2.4GHz** Wi-Fi。
> **验收**：两种方式均可成功连网；LCD 显示连接状态；云 ASR 可用（激活成功）。

### FR-7 Agent 状态上屏 · P0

| 项 | 内容 |
|----|------|
| 描述 | Kiro CLI hook 把 Agent 生命周期事件转 JSONL，经 Companion 下发设备，LCD 显示 Agent 名与状态 |
| Hook 事件 | `agentSpawn` → Idle；`userPromptSubmit` → Running；`stop` → Idle；`postToolUse`(错误) → Error |
| 协议 | `display`: `{agent_name, state: idle|running|error, text}` |
| 回退 | Companion 不可用时，hook 可直连串口写入设备 |
| 验收 | Kiro 运行 agent 时 LCD 实时反映状态切换；Companion 重启不丢状态 |

### FR-8 无弹窗与打包 · P1

- 纳入 `powershell_shim`（Rust GUI 子系统）消除 Kiro hook 触发的控制台黑框。
- Companion 经 `tauri build` 产出 macOS 安装包（DMG / .app bundle）。

---

## 5. 通信协议

权威定义见 `shared/protocol/protocol.md`。摘要：

- **编码**：UTF-8 JSON Lines，每帧以 `\n` 结尾，未知 `type` 必须忽略。
- **载体**：一期 UART 115200 8N1 → 二期 BLE GATT → 三期 BLE HID（HID 不走本协议）。
- **帧字段**：`v`（版本=1）/ `seq`（递增）/ `type` / `payload`。
- **设备→PC**：`key`、`nav`、`voice`、`macro`
- **PC→设备**：`led`、`display`、`config`、`ack`
- **握手/心跳**：`hello` / `hello_ack`、`ping` / `pong`
- **信任模型**：UART 物理可信（免配对）；BLE 需配对 token 门控。

---

## 6. 交互设计

### 6.1 按键功能映射（初始方案，可配置）

> 映射表以 JSON 配置存于 Companion，用户可改。以下为默认建议。

| 输入 | 普通态 | 语音录制中 | 语音编辑态 |
|------|-------|-----------|-----------|
| KEY1 | 语音开始 | 停止并发送 | 发送 |
| KEY2 | 发送（Enter） | — | 发送 |
| KEY3 | 中断（Esc） | 取消录音 | 退出编辑 |
| KEY4 | 自定义宏 1 | — | Backspace |
| KEY5 | 自定义宏 2 | — | — |
| NAV 上/下 | 历史/列表导航 | — | 光标移动 |
| NAV 左/右 | 切换 Agent/会话 | — | 光标移动 |
| NAV 中心 | 确认 | — | 确认 |

> **待确认**：具体映射需在一期实机试用后固化。参考 `ki-board` 的 3 键方案经验。

### 6.2 语音状态机

```
[普通] --KEY1--> [录制中] --KEY1--> [普通]（转写+粘贴+Enter）
                    |
                    +--KEY3--> [普通]（取消，丢弃）
                    |
                    +--(停止但不发)--> [编辑态] --KEY2--> [普通]（Enter）
                                          |
                                          +--KEY4--> [编辑态]（Backspace）
                                          |
                                          +--KEY3--> [普通]（退出）
```

**规则**：录音态禁止切换 Agent（避免文本落到错误窗口）。

### 6.3 LCD UI 布局（320×240）

```
┌────────────────────────────────┐
│ USB/BLE  Wi-Fi  🔋            │ 状态条 (24px)
├────────────────────────────────┤
│                                │
│      Agent: planner            │ Agent 区
│      ● Running                 │ (名称+状态+表情)
│                                │
├────────────────────────────────┤
│ K1语音 K2发送 K3中断 …          │ 按键提示 (40px)
└────────────────────────────────┘
```

录音时中部切换为录音指示（电平/计时）。

---

## 7. 软件开发规格

### 7.1 固件模块（TuyaOpen C）

| 模块 | 职责 | 关键 API | 可测性 |
|------|------|---------|-------|
| `input_scan` | GPIO 扫描、去抖、长按判定 | `tkl_gpio_*`（input + pullup） | 状态机可 host 侧单测 |
| `event_fsm` | 按键/摇杆/语音事件与状态机 | — | host 单测 |
| `transport/uart` | 一期传输 | `tkl_uart_*` | 接口稳定 |
| `transport/ble_gatt` | 二期传输（私有服务，≤6 特征） | `tkl_ble_gatts_service_add`、`tkl_ble_gatts_value_notify` | mock |
| `transport/ble_hid` | 三期直连 | 原生 NimBLE `ble_gatts_register_svcs`（绕过 TAL/TKL） | 实机 |
| `protocol` | JSONL 编解码、seq/ack | — | host 单测 |
| `display` | LVGL @ ST7789，UI 状态映射 | `tdd_disp_spi_st7789_register` | 状态映射可单测 |
| `audio` | 板载 MIC 采集、云 ASR 取文字 | `ai_chat_init`、`ai_user_event_notify_register` → `AI_USER_EVT_ASR_OK` / `AI_NOTIFY_TEXT_T.data`；PCM 取流 `AI_USER_EVT_MIC_DATA` | 适配层 mock |
| `network` | 配网与云激活 | `tkl_wifi_station_connect`、`netmgr_conn_set(NETCFG_TUYA_BLE)`、MQTT bind（UUID+AuthKey 自取 token） | mock |

**板级配置关键宏**（`boards/T5AI/TUYA_T5AI_CORE/tuya_t5ai_core.c`）

```c
#define BOARD_LCD_WIDTH        240   // 短边
#define BOARD_LCD_HEIGHT       320   // 长边（官方 demo 为 240，本项目改 320）
#define BOARD_LCD_PIXELS_FMT   TUYA_PIXEL_FMT_RGB565
#define BOARD_LCD_SPI_PORT     TUYA_SPI_NUM_0
#define BOARD_LCD_SPI_CLK      48000000
#define BOARD_LCD_SPI_CS_PIN   TUYA_GPIO_NUM_15
#define BOARD_LCD_SPI_DC_PIN   TUYA_GPIO_NUM_17
#define BOARD_LCD_SPI_RST_PIN  TUYA_GPIO_NUM_6
#define BOARD_LCD_BL_PIN       TUYA_GPIO_NUM_8   // v1.2: P5->P8 (J2.16, PWM2), 高有效
#define BOARD_LCD_POWER_PIN    TUYA_GPIO_NUM_7   // 高有效
// 新增输入
#define BOARD_KEY1_PIN  TUYA_GPIO_NUM_24
#define BOARD_KEY2_PIN  TUYA_GPIO_NUM_25
#define BOARD_KEY3_PIN  TUYA_GPIO_NUM_26
#define BOARD_KEY4_PIN  TUYA_GPIO_NUM_27
#define BOARD_KEY5_PIN  TUYA_GPIO_NUM_23
#define BOARD_NAV_UP_PIN      TUYA_GPIO_NUM_32
#define BOARD_NAV_DOWN_PIN    TUYA_GPIO_NUM_20
#define BOARD_NAV_LEFT_PIN    TUYA_GPIO_NUM_21
#define BOARD_NAV_RIGHT_PIN   TUYA_GPIO_NUM_22
#define BOARD_NAV_CENTER_PIN  TUYA_GPIO_NUM_30
// 全部 input + TUYA_GPIO_PULLUP，低有效
```

**工具链**：`tos.py`（非 `tos`）；改 `.config` 后必须 `tos.py clean -f` 再编译；
Windows 走**云编译 + 本地烧录**（见 `tuya-kb/03-toolchain-setup/01-cloud-build-local-flash-windows.md`）；
调试串口 Uart1 / 460800。

### 7.2 Companion 模块（Tauri: Rust + React/TS）

| 模块 | 职责 | 现状 |
|------|------|------|
| `transport/mod.rs` | `BoardTransport` trait + `TransportKind` | ✅ 骨架完成 |
| `transport/usb_serial.rs` | serialport 连接、VID/PID 自动发现、行读取线程 | 🔲 骨架（Task 6 实现） |
| `transport/ble_gatt.rs` | btleplug 扫描/连接/通知 | 🔲 骨架（Task 13 实现） |
| `protocol.rs` | JSONL 编解码、坏帧容错 | ✅ 骨架 + 3 单测通过 |
| 注入层 | macOS CoreGraphics 按键注入 / arboard 粘贴 | 🔲 Task 7 |
| ASR 层 | Apple Speech (SFSpeechRecognizer) / macOS Dictation | 🔲 Task 8 |
| hook 服务 | 接收 Kiro hook JSONL，转 `display` 下发 | 🔲 Task 10 |
| 前端 | React/TS，事件显示、映射配置、传输选择 | ✅ 骨架窗口 |

**依赖锁定**：`Cargo.lock` 已对齐 ki-board 已验证版本（tauri 2.11.3），避免依赖解析拿到不兼容新版。

### 7.3 代码开发检查清单

| # | 检查项 |
|---|-------|
| C1 | 传输层与协议层解耦，协议跨阶段复用不改接口 |
| C2 | 协议解析对坏帧/截断/未知 type 容错，不 panic |
| C3 | 输入状态机可 host 侧单测（不依赖硬件） |
| C4 | 注入层可 mock 断言 INPUT 序列 |
| C5 | ASR 适配层可 mock，处理无结果/超时/权限 |
| C6 | 密钥（涂鸦 UUID/AuthKey、ASR key）用 keyring，**不入库** |
| C7 | 代码注释英文；commit message 英文 |
| C8 | 每个 Task 完成后跑 `cargo test` / `tsc` / 固件构建 |

---

## 8. 非功能需求

| 类别 | 要求 |
|------|------|
| 输入延迟 | 按键到 PC 注入 < 100ms（有线）/ < 200ms（BLE） |
| 语音延迟 | 说完到文本出现 < 2s（方式②本机 ASR） |
| 显示刷新 | UI 状态变更 < 200ms 反映；LVGL 无可见撕裂 |
| 去抖 | 10–20ms，无重复触发 |
| 音频规格 | 16kHz / 16bit / mono（板载 codec 默认，含 AEC） |
| 功耗 | 便携可用；空闲可降背光/休眠（二期优化） |
| 电池续航 | 目标 ≥ 4h 连续使用（依电池容量，实测确定） |
| 可靠性 | 断连自动重连；Companion 崩溃后 hook 可回退直连串口 |
| 安全 | BLE 需配对 token；UART 物理可信；密钥不入库、不出现在日志 |
| 兼容性 | PC 端 macOS 原生（一期）；BLE HID 阶段兼容主流系统 |

---

## 9. 分阶段交付与验收

| 阶段 | 范围 | Demo 验收标准 |
|------|------|--------------|
| **阶段 0** 脚手架与硬件定义 | 仓库骨架、引脚映射、BOM、协议草案 | ✅ 已完成：结构可浏览、协议可读、`check_pins.py` 通过、BOM 标记采购 |
| **阶段 1** 有线 MVP | 固件心跳→输入扫描→UART 协议→Companion 串口→注入→macOS ASR→LCD→hook 状态→打包 | 完整有线闭环：按键触发命令、说话出文本、LCD 显示 Agent 状态、无弹窗、安装包可分发 |
| **阶段 2** 无线化 | BLE GATT 固件+Companion、板端唤醒词+在线 ASR | 拔掉 USB，BLE 下功能等价；对板说话文本进 Kiro |
| **阶段 3** 直连与本地化 | BLE HID、（可选）板端本地 ASR | 不装 PC 软件即可作蓝牙键盘；断网可识别固定指令 |

任务级分解见 `docs/implementation-plan.md`（16 个 Task）。

---

## 10. 风险与待确认项

### 10.1 风险

| # | 风险 | 影响 | 缓解 |
|---|------|------|------|
| R1 | T5 无 USB Device HID（SDK 无 device 栈） | "直连"方案受限 | 已确认走 BLE HID（BLE5.4 原生），二期实现 |
| R2 | 云 ASR 需配网 + 涂鸦云激活 | 语音方式①受阻 | 一期以方式②（PC ASR）保证可用；持 UUID+AuthKey 可走 MQTT bind 免 App 激活 |
| R3 | 本机 C 盘空间不足 | Tauri 完整编译（3–5GB）无法跑通 | 已用隔离 crate 验证纯逻辑；需清理磁盘后跑完整测试 |
| R4 | BLE HID 需手搓 NimBLE（无现成 `ble_svc_hid`） | 二期工作量大 | 参考 ki-board 实现 + 移植 NimBLE 上游 HoGP 例子 |
| R5 | 库存无 D06 茶轴，需采购 | 采购项 | **轴体直焊**（不用热插拔）；已定 **D06 茶轴**（立创 C49234237，现货 ￥0.69/个）。库存 D91 青轴无立创自营现货，且茶轴段落感更适合桌面小设备，故不选青轴 |
| R6 | 18650 充电慢（500mA 固定） | 充电体验差 | 建议选 500–2000mAh 聚合物电池 |
| R7 | LCD 背光限流阻值未定 | 亮度不合适 | 打样时先用较大阻值，实测后调整 |
| **R8** | **BLE 配网与 BLE 键盘共用同一蓝牙** | 二期 BLE HID 与配网可能冲突 | **需分阶段（先配网激活，再切 HID/GATT 角色）或做共存管理**；设计时明确角色切换时序 |
| **R9** | 云 AI 服务是否要求"已绑定账号" | 免 App 激活可能不足 | 首次真机验证；若必需，首次用 BLE 配网绑定一次，之后 `fast_connect` 复用 |
| R10 | 方式③ PCM 经 BLE 带宽不足（256kbps） | 二期语音方式③受限 | BLE 需压缩（ADPCM/Opus）；或仅在 USB 下支持方式③ |

### 10.2 待确认项

| # | 项 | 影响 | 责任 |
|---|-----|------|------|
| Q1 | ✅ 已定 **D06 茶轴**（直焊，立创 C49234237）；确认采购数量（10–20） | BOM 采购 | 用户 |
| Q2 | FPC 座下接/上接方向 | 采购与布线 | 核对 LCD 排线触点朝向 |
| Q3 | 按键功能映射最终方案 | 交互设计 | 一期实机试用后固化 |
| Q4 | 方向 A/B/C/D ↔ 上下左右 | 固件常量 | 装配后标定 |
| Q5 | ✅ 已定：扩展板加**电池总开关**（SK12D07VG5 串 BAT+，仅通断电池）；整机主开关用开发板自带 | 电路 | 用户已确认 |
| Q6 | SPI 阻尼电阻是否需要 | 信号完整性 | 打样实测（先 0Ω 跳线） |
| Q7 | 电池容量与外壳尺寸 | 结构设计 | 与 `enclosure/` 协同 |
| **Q8** | **BLE 配网与 BLE 键盘的角色切换时序 / 共存方案** | 二期架构 | 二期设计前明确（见 R8） |
| **Q9** | 云 AI 服务是否要求已绑定账号 | 云 ASR 可用性 | 首次真机激活时验证（见 R9） |
| Q10 | 方式③ 的 PCM 压缩方案（ADPCM/Opus）或限定 USB | 二期语音 | 二期评估 |

---

## 11. 附录

### 11.1 术语

| 术语 | 含义 |
|------|------|
| Companion | PC 端上位机助手程序（Tauri 应用） |
| JSONL | JSON Lines，每行一个 JSON 对象 |
| AEC | 声学回声消除 |
| ASR / KWS | 自动语音识别 / 唤醒词检测 |
| RTP | 电阻式触摸屏 |
| DP | 涂鸦 Data Point（数据点） |
| 注入 | 用 macOS CoreGraphics/剪贴板把输入送进焦点窗口 |

### 11.2 参考资料

| 资料 | 位置 |
|------|------|
| **连接模式最终设计（4 模式对比+实现清单）** | `tuya-kb/11-product-design/00-wireless-ai-keyboard.md` |
| **系统级功能可行性矩阵（源码级核实）** | `tuya-kb/11-product-design/01-feasibility-matrix.md` |
| 开发环境 | `tuya-kb/11-product-design/02-dev-environment.md` |
| 蓝牙 / BLE HID | `tuya-kb/07-network-provisioning/01-bluetooth-and-ble-hid.md` |
| 语音听写与事件（取 ASR 文字） | `tuya-kb/08-ai-audio-lcd/02-voice-dictation-and-events.md` |
| LCD ST7789 + LVGL | `tuya-kb/08-ai-audio-lcd/03-lcd-st7789-lvgl.md` |
| T5AI-Core 原理图 / 丝印 / 3D | `tuya-kb/datasheets/t5-ai-core/` |
| T5 芯片手册 | `tuya-kb/datasheets/t5-e1/` |
| T5 引脚复用表 | `tuya-kb/02-t5ai-core-board/01-peripheral-pin-mapping.md` |
| LCD 规格书 / 接线图 / ST7789 手册 | `datasheets/CL28CK230-18A_RTP&WithoutTP_230801/` |
| 摇杆规格书 | `datasheets/C53207708_多向开关_WS-1002_规格书_WS-1002.PDF` |
| 机械键手册 | `datasheets/CPG151101D06_茶轴开关.pdf` |
| 库存 BOM | `datasheets/BOM_Stocks_20260810.xlsx` |
| 官方 ST7789 接线与板级代码 | developer.tuya.com/cn/docs/developer/t5-chatbot |
| 参考项目 ki-board | `ref_projects/ki-board/`（ESP32-S3 版，USB HID + 5 屏） |

### 11.3 变更记录

| 版本 | 日期 | 变更 |
|------|------|------|
| v1.0 | 2026-08-21 | 首版。基于阶段 0 全部产出（引脚映射、BOM、协议、架构决策）编写 |
| v1.1 | 2026-08-21 | 整合 `tuya-kb/11-product-design/` 源码级核实结论：连接模式实现清单（方式②/③ 的 TKL/NimBLE 细节）、ASR 关键 API 与带宽、云激活 MQTT bind 链路、双链路架构澄清；新增风险 R8–R10、待确认 Q8–Q10 |
| v1.2 | 2026-08-21 | 扩展板新增：Type-C+CH340C(USB→UART,可烧录+通信,共用 UART0,TX隔离,DTR/RTS→S8050 自动复位)、锂电池并联座(同 BAT 网/单节)、3×WS2812(P13,经 SN74LV1T34 电平转换)；更新架构框图/器件表/引脚表/§3.6-3.7/检查清单 H11-H15；BOM 新增器件对照库存(基本零采购)+实价查证 |
| v1.3 | 2026-08-21 | 电源方案定稿+全文档一致性审核：① Type-C 只做数据不供电系统；② CH340C 供电改为 VBUS→LDO(HE9073A33M5R,3.3V) 独立供电(不依赖电池,IO 3.3V,V3 与 VCC 短接)；③ 新增电池总开关(SK12D07VG5,串 BAT+)；修正 netlist 重复/矛盾电源段、bom 青轴过时表述与编号冲突；检查清单加 H16/H17 |
| v1.4 | 2026-08-24 | **对齐 EDA v1.0（原理图+PCB，只读核对，未改 EDA）**：① 补录 2 颗电源指示灯 LED4(VBUS)/LED5(BAT) 及限流电阻 R6/R10（§3.1/§3.6）；② 澄清 WS2812 仍为 3 颗（文档原记载正确）；③ 记录 EDA 实际位号与旧文档功能命名的对应（母排=U1、摇杆=SW2、按键=K1-5、CH340C=U3、电平转换=U2、LDO=U5、复位管=Q2、背光管=Q1、WS2812 DIN 串阻=R3、SPI 阻尼=R11/R12）；④ 阻容实际值统一标注"以 EDA 原理图为准"，不臆测。详见 netlist.md/bom.md/component-notes.md 与 hardware/eda/pcb_review_report_2026-08-24.md |
