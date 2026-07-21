# PCB 设计（V2，未实施）

## 板规格

| 项 | 选择 | 理由 |
|---|---|---|
| 尺寸 | **100 × 100 mm** | 卡 JLC 经济档 |
| 层数 | **2 层** | V1 不需要 4 层，省 50% 打样费 |
| 厚度 | 1.6mm | 标准 |
| 表面 | 无铅 HASL | 手焊友好 |
| 板材 | FR4 | 标准 |
| 阻焊 | 黑色或深灰 | 美观 |

## 物理布局

板框 100×100mm | MX 中心距 19.05mm | 边距 ≥3mm

```
              USB-C（顶边居中）
        ┌─────────────────────────────┐
        │  [A1] [A2] [A3] [A4] [A5]   │ Row0 状态键（每键底 RGB）
        │  [OK ] [NO ] [STP] [NEW][PTT]│ Row1 主操作
        │  [FN ] [MOD] [ENC] [JY][WF] │ Row2 功能层
        │                              │
        │  [BOOTSEL]      [RESET]     │
        └─────────────────────────────┘
```

V1 焊接最小集：**A1-A5 + OK/NO/STOP + EC11 = 9 元件**

PCB 全 15 个热插拔座都画上，V1 不焊的标 DNP（不贴）。

## 电气架构

```
USB-C (CC + Rp 或 5.1k)
  → ESD (USBLC6-2SC6)
  → 5V
  → LDO 3V3 (AP2112K-3.3)
  → RP2040 + W25Q16JV Flash + 12MHz 晶振
  → 键矩阵 3×5（含二极管）
  → EC11 A/B/SW
  → 摇杆 VX/VY → ADC（V2）
  → SK6812MINI-E 数据链（5V 供电）
```

### USB-C 电流协商（V1 关键修正）

原计划 5.1k×2 = 500mA 不够（12 颗 RGB 满亮 720mA）。两个方案：

- **方案 A（推荐）**：用 **CH224K** 自动协商 1.5A/3A，CC 电阻改 Rp
- **方案 B**：固件硬限 RGB 总电流 ≤ 200mA（约 28% 亮度）

V1 仅焊 5 颗 RGB，5×60mA=300mA 在 500mA 内，可暂不协商。V2 全焊时必须协商。

### RGB 数据线电平

SK6812 数据高电平要求 ≥ 0.7×Vdd = 3.5V（Vdd=5V 时）。RP2040 GPIO 输出 3V3。
- 多数 SK6812 实测 3V3 能工作
- 保险做法：数据线串一个 SN74AHC1G125 或专用的电平移位
- 简化：直接 3V3 驱动，预留 100Ω 串阻 + 上拉焊盘

### 去耦

- RP2040 每组电源：100nF
- 大宗：10µF × 2
- RGB 5V 总线：100µF + 100nF（每个 LED 之间）

### USB D+/D- 差分

2 层板做 90Ω 差分：
- D+ D- 走线平行，间距匹配
- 走线短（< 50mm）
- 底面铺地，避免跨分割
- JLC 板厚 1.6mm，铜厚 1oz

## 预留（V2 贴）

- JST 电池焊盘（1.25mm 间距）
- 充电 IC 焊盘（TP4054 等 LCSC 基础库）
- BLE 模块焊盘或 u.FL 天线位

## EDA

KiCad 8（git 文本，agent 友好）。

```
hardware/
└── kicad/
    ├── agent-deck.kicad_pro
    ├── agent-deck.kicad_sch
    ├── agent-deck.kicad_pcb
    ├── agent-deck.pretty/      # 自定义封装
    ├── agent-deck.3dshapes/    # 自定义 3D
    └── fp-lib-table / sym-lib-table
```

## 生产（JLCPCB）

| 项 | 选择 |
|---|---|
| PCB | 2 层 1.6mm HASL（黑油） |
| SMT | 优先基础库料 |
| 手焊 | 轴体、键帽、EC11 帽、摇杆帽 |
| 导出 | Gerber + bom.csv + cpl.csv |

V1 不打样（先做软件 + simulator）。
