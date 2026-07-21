# Firmware（V2，未实现）

V1 不需要固件 —— simulator 完全替代硬件。

V2 真硬件出来后，固件职责：

## MCU

RP2040（双核 Cortex-M0+，133MHz，264KB SRAM）

## 技术栈

- pico-sdk 1.5+（C/C++）
- TinyUSB（USB 协议栈）
- 自己实现 CDC ACM 虚拟串口

## 模块（计划）

```
firmware/
├── CMakeLists.txt
├── src/
│   ├── main.c              # 入口、初始化
│   ├── usb_cdc.c           # TinyUSB CDC，JSON Lines 收发
│   ├── matrix.c            # 5×3 矩阵扫描 + 软件消抖
│   ├── encoder.c           # EC11 旋转编码器（GPIO 中断）
│   ├── joystick.c          # ADC 采样 + 阈值 → 方向事件
│   ├── rgb.c               # SK6812 单线协议驱动
│   ├── protocol.c          # 与 packages/protocol 字段对齐
│   └── effect.c            # solid / breathe / blink_fx 本地实现
└── pico_sdk_import.cmake
```

## 行为契约（与 host 协议对齐）

### 上电

1. 灯自检跑马（5 颗依次亮 100ms）
2. 进 idle：5 颗灯慢呼吸白
3. 等 host 连接

### host 连接后

- 收 `leds` 帧 → 应用 rgb/br/fx
- 收 `focus` → 焦点槽高亮

### host 断开

- 灯回到 idle 状态（dim 白）
- 按键仍可扫描上报（缓存），重连后补发

### USB 协议

CDC JSON Lines，与 simulator 同协议（见 docs/protocol.md）。

## 亮度限制（USB 电流管理）

USB-C 默认 500mA（CC 5.1k）。12 颗 SK6812 满亮 720mA 超标。两个方案：

- **方案 A（推荐）**：加 CH224K 协商 1.5A/3A
- **方案 B**：固件硬限全局亮度 ≤ 30%

V1 焊 5 颗，5×60mA=300mA 在 500mA 内，可暂时不协商。

## 编译

```bash
cd firmware/build
cmake ..
make -j8
# 产物：firmware.uf2
# 按住 BOOTSEL 插 USB，拷贝 .uf2 到 RPI-RP2 卷
```
