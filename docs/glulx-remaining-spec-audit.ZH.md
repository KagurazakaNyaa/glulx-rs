# 当前 Glulx 规范复核

[English](glulx-remaining-spec-audit.md) | [中文](glulx-remaining-spec-audit.ZH.md)

更新：2026-09-10。本文件描述当前实现状态 commit `1a27dd4` 的规范边界和风险，不记录历史实现过程。详细测试命令见[验收记录](glulx-validation.ZH.md)，实现清单见[规范清单](glulx-spec-checklist.ZH.md)。

## 来源与判定口径

复核依据为 [Glulx 3.1.3 正文](https://eblong.com/zarf/glulx/Glulx-Spec.html)、[Glk 0.7.6 正文](https://eblong.com/zarf/glk/Glk-Spec-076.html)、[Blorb 2.0.5 正文](https://eblong.com/zarf/blorb/Blorb-Spec.md) 和 Blorb 引用的 [AIFF-C 文档](https://eblong.com/zarf/ftp/aiff-c.9.26.91.ps)。只有在提供对应能力时，规范对该能力的格式/行为要求才适用；Glk 可选模块和 Blorb 可选格式不属于强制缺口。

## 当前状态

| 领域 | 当前实现 | 仍有的边界 |
| --- | --- | --- |
| Glulx 核心 | 文件头/checksum 校验、官方 opcode 分发、整数/单精度/双精度、字符串、heap、加速、IFZS、restart/protect 和 undo 已实现，并有专项回归。 | 不能仅凭分发表和单元测试证明所有合法/非法程序及所有宿主组合都完全符合规范。 |
| 无执行文件的 Blorb | 原始故事可以通过 API、CLI、GUI 和发现策略挂载外置无执行文件 Blorb 或散装资源目录。IFhd 身份、冲突、资源索引、会话保留和缓存清理已有覆盖。 | 资源替换采用重启故事流程；没有另行提供运行中替换已打开资源流或正在播放声音的 live-swap API。 |
| 资源流 | TEXT/BINA/FORM、编码字节位置、Unicode、元数据、封面、RDes 和资源优先级均已实现。 | 对部分非法容器仍采用宽容读取策略，见下文。 |
| SONG | 可选 SONG 支持 `SND<number>` AIFF 引用、SSND offset、MARK/INST sustain loop、样本转换、重复、偏移、暂停/停止、通知和会话恢复。 | 不声明覆盖全部历史编码变体或实体音频输出。 |
| 终端宿主 | 交互 TTY 支持网格/状态、预填编辑、定时取消、即时字符输入、回显/终止键、文件提示、多窗口选择和终端恢复。管道模式仍是稳定的自动化协议。 | Windows 控制台和 macOS 实机终端验收仍待完成。 |
| light 字重 | 有真实 light 字体时使用细字重；缺失时回退常规并如实报告能力。 | Windows/macOS 的字体和 DPI 行为仍未实机验证。 |
| Undo 预算 | 按保留页、栈字节和堆记录数据计费；共享故事映像和当前 VM 地址空间不计入。零值禁用保留，超预算候选失败。 | 预算判断前会先构造候选页，恶意的大量写入可能造成临时分配峰值。 |
| 容器严格性 | 校验 FORM/IFRS 边界、RIdx 结构、资源偏移、重复编号和身份。 | 允许非零 padding，也允许未索引 GLUL 的兼容回退；不宣称拒绝所有损坏容器。 |

## 可选项与宿主范围

- `glk_sound_load_hint` 只请求可选预加载，可以是 no-op。
- Blorb `Plte`、Z-machine 专用块和未支持的历史 tracker 方言不属于本 Glulx 播放器目标，除非宿主明确采用对应能力。
- 散装资源目录是播放器便利功能，不是必须新增的 Glk dispatch selector。
- 实体声卡波形、长篇游戏完整流程和跨平台 GUI 行为需要对应平台证据，不能从 Linux 或软件采样测试推断。

## 当前验收优先级

1. 修改 VM、资源或宿主行为时，按[验收记录](glulx-validation.ZH.md)重新运行当前 Linux 参考、GUI 和 TTY 工具。
2. 补充 Windows/macOS 实机的 GUI、字体/DPI、终端输入、文件提示、音频和会话恢复记录。
3. 扩展媒体矩阵，覆盖 codec、位深、采样率、声道数和历史 tracker 变体；成功和失败都要记录。
4. 在不改变 dirty-page 和恢复语义的前提下，降低超预算 undo 候选被拒绝前的临时分配峰值。

这些是工程和验收工作，不表示当前实现违反了某条强制 Glulx/Glk 规则。
