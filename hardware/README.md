# Hardware

KiCad 工程（V2，未实施）。

V1 不打样。先做软件 + simulator，硬件文档并行沉淀。

## 目录

```
hardware/
├── kicad/              # KiCad 8 工程
│   ├── agent-deck.kicad_pro
│   ├── agent-deck.kicad_sch
│   └── agent-deck.kicad_pcb
├── mechanical/         # 外壳 STEP/STL（V2）
└── preview/            # 布局 SVG 预览
    └── layout.svg
```

## 设计依据

详见：
- [docs/pcb.md](../docs/pcb.md) — 板规格、电气、布局
- [docs/bom.md](../docs/bom.md) — BOM 选型
