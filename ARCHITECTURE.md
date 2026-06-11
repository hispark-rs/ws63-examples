# ws63-examples 架构

本仓库是 [ws63-rs](https://github.com/hispark-rs/ws63-rs) monorepo 的子模块。

`ws63-examples` 收录应用示例，目前仅 `blinky`（GPIO 点灯）。

完整架构与评审（集中维护于主仓库）：
- 组件文档：<https://github.com/hispark-rs/ws63-rs/blob/main/docs/architecture/ws63-examples.md>
- 总体架构：<https://github.com/hispark-rs/ws63-rs/blob/main/docs/architecture/overview.md>
- 整改排期：<https://github.com/hispark-rs/ws63-rs/blob/main/ROADMAP.md>

> 已知问题：blinky 当前因 hisi-riscv-rt 链接脚本不传播而无法链接；且唯一示例不足以验证 31 个驱动。
> 见 ROADMAP 阶段 1 / 5。
