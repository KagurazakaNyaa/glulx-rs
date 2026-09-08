# Glulx 剩余规范项复核

日期：2026-09-08。代码基线 `5816d37`；本轮先复核规范、补文档，再实现。这里记录已读一手正文与当前代码的差异；待办不是实现完成记录。本轮覆盖 Glulx 3.1.3 核心、Blorb 2.0.5 资源与声音格式、Glk 0.7.6 宿主输入/呈现；以下同时记录已确认缺口及没有发现新缺口的边界。

## 一手来源与判定口径

| 来源 | 本次实际核对范围 |
| --- | --- |
| [Blorb 2.0.5 正文](https://eblong.com/zarf/blorb/Blorb-Spec.md)，维护方 [IFTF](https://github.com/iftechfoundation/ifarchive-if-specs) | Overall Structure、Resource Index、Picture/Sound/Data/Executable Resource Chunks、Game Identifier、Fspc、RDes、Z-machine 专用块、IFF、Other Resource Arrangements |
| [Glk 0.7.6 正文](https://eblong.com/zarf/glk/Glk-Spec-076.md) | [Resource Streams](https://eblong.com/zarf/glk/Glk-Spec-076.html#resource_streams)、[Sound Resources](https://eblong.com/zarf/glk/Glk-Spec-076.html#sound_resources)、[Playing Sounds](https://eblong.com/zarf/glk/Glk-Spec-076.html#sound_playing)、[Blorb Layer](https://eblong.com/zarf/glk/Glk-Spec-076.html#blorblayer) |
| [Glulx 3.1.3 正文](https://eblong.com/zarf/glulx/Glulx-Spec.md) | Save-Game Format → Associated Story File：`IFhd` 为 ROM 前 128 字节 |
| [Apple AIFF-C 原始文档](https://eblong.com/zarf/ftp/aiff-c.9.26.91.ps)，由 Blorb 的 AIFF Sounds 直接链接 | §6 Marker Chunk、§9 Instrument Chunk：marker 使用 sample-frame 位置，`sustainLoop` 的模式和端点。文档自述为 1991-08-26 draft；不将其中 SAXEL 提案列入本项目目标。 |

前三份正文使用本次任务已下载的本地副本，SHA-256 分别为：

```text
Blorb-Spec.md     ce642b0875a4ed61e199b3f0d1c0e54f22171bdbb6f26cd518de5f4cc5c41b4d
Glk-Spec-076.md   974d4c63539521a6e57efa967418e3dde463f53e45871de8abaed961b9003eea
Glulx-Spec.md     ccd5e8aacff3cbd7906e5fcac055d7421dd9d9297634a9c86eff54f5405f36e7
aiff-c.9.26.91.ps e7a905a06cd8b60b67ac45a7f66bffe7f1f9e9fc8ba31d68f2639f46a36ea524
```

“规范要求”指选择支持该能力之后的格式或行为规则；Glk 的图形、声音等模块本身可以不提供。“可选扩展”指规范明确允许不实现的功能。“播放器扩展”指用户入口、发现策略和桌面会话等宿主行为。“验证缺口”表示已有功能缺少指定验收证据，不能直接写成尚未实现。

## 独立 Blorb 与故事身份

| 项目 | 规范与当前证据 | 分类及待办 |
| --- | --- | --- |
| 无执行文件的资源包 | Blorb “Executable Resource Chunks” 明确允许无 Exec 的资源包，与独立 executable 一起使用；只交资源包无法运行。[Story::from_bytes](../src/story.rs) 对所有 FORM 都调用 `extract_glul_chunk`，raw `.ulx` 的资源索引为空，尚无挂载 API。[CLI](../src/main.rs) 与 [GUI](../src/app.rs) 只加载一个故事路径。 | **合法 Blorb 使用方式缺失**。增加独立资源解析/挂载及 API、CLI、GUI 入口；无 Exec 不能成为资源挂载失败原因。不要因此让资源包独自作为故事执行。 |
| 参数冲突 | 同一章节要求对“另给 executable，同时资源包内也有 executable”的矛盾参数作出诊断。 | **规范建议诊断**。挂载入口明确拒绝或提示这类组合，不能默默替换用户指定的故事。若提供额外覆盖模式，需明确这是播放器策略。 |
| `IFhd` 匹配 | Blorb “The Game Identifier Chunk” 说明 IFhd 可选，存在时可检查关联故事，不匹配应报错；内嵌 executable 与 IFhd 也应匹配。Glulx “Associated Story File” 定义内容为故事前 **128 字节**，包括长度、checksum 和编译器数据。[story.rs](../src/story.rs) 未检查 Blorb IFhd；[save.rs](../src/vm/save.rs) 已为 IFZS 使用前 128 字节，但不能证明 Blorb 路径也做了检查。 | **缺少规范建议的一致性检查**。无 IFhd 允许挂载；存在时检查长度和内容，覆盖匹配/不匹配/截断及内嵌 executable。不能套用 Z-machine 的 13 字节 release/serial/checksum/Initial PC 布局。 |
| 资源映射统一 | Glk Blorb Layer 中注册的资源文件供资源调用查找。[Story](../src/story.rs) 的图片、音频、Data、Fspc、RDes 均读取当前 container/index。 | **实现边界**。选中的资源包应对所有这些入口生效，保留故事执行映像和 VM 身份；先校验再替换，挂载失败保留原状态。 |
| 资源来源与会话 | [session.rs](../src/vm/session.rs) 从 `container.unwrap_or(image)` 重建 Story，默认 container 含 executable；资源索引被 serde 跳过。 | **播放器功能缺失**。独立资源必须保存来源和内容，恢复时分别验证执行映像与资源包、重建索引。不能在恢复时重新发现另一个同名文件。可移植 IFZS 仍只负责规范中的 VM 状态。 |
| 同名发现 | Blorb “File Suffixes” 定义 `.blorb`、含 Glulx 的 `.gblorb` 及较短后缀；没有指定同名发现算法。Glk “What the Program Does” 把启动时找文件交给宿主。 | **播放器扩展**。为 raw `.ulx` 查找同名候选时记录后缀/大小写/多候选优先级、显式指定优先、坏包诊断和无候选行为。不能标成遗漏的 Glk opcode。 |

Glk 明确说 Blorb Layer “is not part of the Glk API per se”。`giblorb_set_resource_map` 是 C 宿主与资源库的集成入口；Rust 播放器需要等价的资源映射能力，不需要凭空添加一个标准 Glulx dispatch selector，也无需为了采用这套格式而改变现有 Rust 依赖方案。[依据](https://eblong.com/zarf/glk/Glk-Spec-076.html#blorblayer)

独立资源验收应补齐：

- [ ] 将固定版本 `resstreamtest.gblorb` 拆成 `.ulx` 与无 Exec 资源包；原包与拆包的 Data/Unicode 输出一致，复用已有参考转录。
- [ ] 合成资源包同时提供 PNG/JPEG、AIFF/MOD、TEXT/BINA/FORM、Fspc 和 RDes，验证所选资源映射覆盖这些调用；FORM 资源保留自身八字节头。
- [ ] 检查无 IFhd、匹配的 128 字节 IFhd、错误长度/内容、损坏 FORM/RIdx、重复资源编号及参数冲突，验证失败后的故事/资源保持一致。
- [ ] CLI GUI/headless 显式参数与 GUI 选择入口；同名发现的优先级和错误提示；切换故事后不沿用上个故事的外置资源。
- [ ] 保存桌面会话后移动或删除原始资源文件，恢复后的图像、Data、描述仍可读取，声音按会话记录继续；内嵌包、独立包、旧会话三种路径均有覆盖。

若允许运行中替换资源，还应定义已经打开的资源流、正在播放的声音、已绘制图像的行为，并清理 [VM 图像尺寸缓存](../src/vm/presentation.rs) 与 [GUI 图片/封面缓存](../src/app.rs)。这属于动态替换功能的验收，不能仅靠启动前挂载测试勾选。

## SONG：可选格式与实现后必须满足的规则

Blorb “Song Sounds” 明确写道：“The song file format is deprecated, as of Blorb 2.0.”，且 “Its support in interpreters should be considered optional.” 因此 `SONG` 未实现不等于缺少 Glulx 必需指令，也不否定已有 MOD/XM/S3M/IT 支持。当前 [sound.rs](../src/vm/sound.rs) 仅将 `MOD ` 交给 tracker，其他格式走采样解码器；[tracker.rs](../src/vm/sound/tracker.rs) 没有资源解析回调、SND 引用或 AIFF sustain-loop 装配。现有 [验收记录](glulx-validation.md) 也没有 SONG 样本。[规范依据](https://eblong.com/zarf/blorb/Blorb-Spec.md)

如果实现 SONG，以下是格式规则和所需验收，不能以普通 MOD 能播放替代：

- [ ] 解析原 MOD 的 22 字节 sample-name 字段中的 `SND<number>`，从同一资源映射查找 AIFF。引用目标必须是 AIFF，不能是 MOD 或另一个 SONG；覆盖共享样本、不同编号、缺少资源、错误类型、截断名称/文件和编号溢出。Blorb 没有定义跨 SONG 递归引用。
- [ ] 忽略 SONG 自带的 sample length、repeat start、repeat length，改用 AIFF 的样本长度和 instrument `sustainLoop`；保留 MOD sample record 的 finetune 和 volume。使用故意错误的旧长度/循环字段验收，防止恰好相等的 fixture 掩盖遗漏。
- [ ] 无 INST 或 `sustainLoop.playMode == NoLooping` 时，repeat start/length 视为零；有 releaseLoop 不意味着它能代替 sustainLoop。依据 Blorb “Song Sounds”。
- [ ] 依据 AIFF 的 MARK/INST 解析 loop：`beginLoop/endLoop` 是 **marker ID**，需要映射为 **sample-frame 位置**，不是直接的字节偏移；覆盖 NoLooping(0)、ForwardLooping(1)、ForwardBackwardLooping(2)。begin 不小于 end 时按 AIFF 文档忽略该 loop；损坏/缺失 marker 必须有有界、明确的失败或回退行为。[AIFF §6/§9](https://eblong.com/zarf/ftp/aiff-c.9.26.91.ps)
- [ ] 使用非零 SSND offset、不同合法位深和样本长度的 AIFF，验证装配后的音高、音量和 loop 边界；记录立体声等多声道样本转换策略。Blorb 明确允许将 AIFF 样本裁剪/填充到 8 位后装配成 MOD，不要求保留高位深；若采用该策略应如实记录。[Blorb AIFF/Song Sounds](https://eblong.com/zarf/blorb/Blorb-Spec.md)
- [ ] 接入 Glk 的播放返回值、0/1/有限/无限重复、暂停/恢复、停止/替换、完成通知及 play_multi；完成通知只在最后一次有限重复结束后发送，0 次、无限或中止不发送。桌面声音恢复偏移另作播放器验收。[Glk Playing Sounds](https://eblong.com/zarf/glk/Glk-Spec-076.html#sound_playing)
- [ ] 对合成 SONG 与等价装配后模块比较 PCM/帧数和重复边界，并提供一次 GUI 播放与桌面恢复验证；有限采样预算检查恶意长度不能引发无界分配。

Blorb 的 `Loop` chunk 是 Z-machine 范围的整段声音重复提示，不能拿它替代 SONG 所引用 AIFF 的 `INST.sustainLoop`。[Blorb “Chunks Specific to the Z-machine”](https://eblong.com/zarf/blorb/Blorb-Spec.md)

## 已实现、允许忽略与验证缺口

| 项目 | 复核结论 | 来源与代码 |
| --- | --- | --- |
| ResourceStream | 已有按 Data 编号打开的只读 TEXT/BINA/FORM、Latin-1/UTF-8/大端 word、FORM 包含头和 LF 终止规则，官方资源流样本已有精确转录比较。独立包挂载不等于重写这些接口。 | [Glk Resource Streams](https://eblong.com/zarf/glk/Glk-Spec-076.html#resource_streams)；[streams.rs](../src/vm/streams.rs)、[Story::resource_file](../src/story.rs)、[验收记录](glulx-validation.md) |
| 散装 `PIC1/SND1/DATA1` 文件 | 未实现；Glk 使用 “may”，Blorb 明确是 platform-specific arrangement。属于另一个可选开发便利功能，不能把只支持 Blorb 的实现标成资源流不符合规范。 | [Glk Resource Streams](https://eblong.com/zarf/glk/Glk-Spec-076.html#resource_streams)、[Blorb Other Resource Arrangements](https://eblong.com/zarf/blorb/Blorb-Spec.md)；[streams.rs](../src/vm/streams.rs) |
| `glk_sound_load_hint` | 允许无操作；它只影响可选预加载，不改变实际播放语义，不应列为缺失功能。 | [Glk 声音章节](https://eblong.com/zarf/glk/Glk-Spec-076.md)，`glk_sound_load_hint` 定义；[sound.rs](../src/vm/sound.rs) |
| 标准采样/tracker 格式 | Blorb 定义 AIFF、OGGV、MP3 和共用 `MOD ` 标签的 MOD/XM/S3M/IT；现有解码器和合成测试覆盖这些格式。SONG 是单独可选项。 | [Blorb Sound Resource Chunks](https://eblong.com/zarf/blorb/Blorb-Spec.md)；[sound.rs](../src/vm/sound.rs)、[tracker.rs](../src/vm/sound/tracker.rs)、[sampled.rs](../src/vm/sound/sampled.rs) |
| 格式变体与声卡输出 | 所有类别有基本支持不代表每种编码位深/采样率/通道/历史 tracker 变体已验收。现有软件采样同步也不能替代实体声卡波形检查；这些应保持为验证待办。 | [Blorb MOD Sounds](https://eblong.com/zarf/blorb/Blorb-Spec.md) 不穷举四种 tracker 内部方言；[验收矩阵与范围](glulx-validation.md) |
| `Plte` 与 Z-machine 提示 | Plte 可以完全忽略；RelN/Reso/APal/Loop 对 Z-code 有定义；Rect 明确可选，Glulx 行为未定义。不是本 Glulx 播放器必需实现清单。现有文档的 Z-machine 范围说明可补列 RelN。 | [Blorb Color Palette、Placeholder Pictures、Chunks Specific to the Z-machine](https://eblong.com/zarf/blorb/Blorb-Spec.md) |
| Fspc、RDes、IFmd | 已有封面、资源文字描述和元数据展示。RDes 不是必需 chunk；独立包验收应覆盖相同能力，不能把它们再次列成未实现。 | [Blorb Frontispiece、Resource Description、Metadata](https://eblong.com/zarf/blorb/Blorb-Spec.md)；[story.rs](../src/story.rs)、[app.rs](../src/app.rs) |

现有清单称“严格 FORM/IFRS chunk 边界及 RIdx 校验”，其已验证范围是边界、RIdx 首块/唯一、索引长度/偏移/重复编号。复核发现 [blorb_chunks](../src/story.rs) 跳过奇数长度块的 padding，却未检查其值必须为零；`extract_glul_chunk` 还允许未进入 Exec 索引的 GLUL 作为兼容回退。Blorb 的 IFF 规则要求生成文件的 padding 为零、Resource Index 要求资源建索引，但没有要求解释器必须拒绝所有非法输入。因此应记录这是**宽容读取策略/校验覆盖边界**，不把它误报为合法游戏缺失功能；若继续声明完整严格容器校验，则应增加对应检查和损坏 fixture。[规范依据](https://eblong.com/zarf/blorb/Blorb-Spec.md)

本轮只做规范阅读与源码核对，未重新运行现有测试，也未把上面的待验收项记为通过。已有 158 项测试及参考/GUI 结果的依据仍是 [先前验收记录](glulx-validation.md)。


## Glk 终端宿主与字体

| 项目 | 规范与实现证据 | 待办与验收 |
| --- | --- | --- |
| 终端网格/状态显示 | [Text Grid Windows](https://eblong.com/zarf/glk/Glk-Spec-076.html#window_textgrid) 定义可见网格和原位输入；[main.rs](../src/main.rs) 的 headless 循环只打印 take_output，未显示 grid/status/window_views，而 [windows.rs](../src/vm/windows.rs) 仍允许创建网格。 | **真实终端宿主缺口**。通过 PTY 检查成功创建窗口可见、网格更新/关闭、多窗口布局；明确管道驱动模式支持范围，不能宣称其已有完整终端呈现。 |
| 终端行/字符输入 | [Line Input Events](https://eblong.com/zarf/glk/Glk-Spec-076.html#line_events) 要求预填视为已输入、取消返回当前组成；[Text Buffer Windows](https://eblong.com/zarf/glk/Glk-Spec-076.html#window_textbuf) 说明字符输入不回显。现有 main 使用 stdin.read_line，未显示 initial_input 或持续 update_line_input，字符请求也需要 Enter。 | **输入宿主语义缺口**。真实 TTY 覆盖预填编辑、输入中定时取消、单键立即交付及无回显；保留现有管道转录回归，并对 EOF 和终端恢复验证。 |
| light 字重 | [Style Hints](https://eblong.com/zarf/glk/Glk-Spec-076.html#stream_style_hints) 明确允许忽略提示；[presentation.rs](../src/vm/presentation.rs) 将 light 回退为 regular，查询如实返回 0；[fonts.rs](../src/app/fonts.rs) 目前只筛选常规字体。 | **可选显示增强**。加载真实 light face，验证呈现、style_measure=-1 和 style_distinguish；没有对应字体时报告实际回退值。不能把当前合法回退写成 Glk 强制要求未满足。 |
| 样式测量单位 | [Testing Styles](https://eblong.com/zarf/glk/Glk-Spec-076.html#stream_style_check) 允许平台自定缩进及字号单位，查询实际外观。当前返回像素缩进、实际字号，headless 返回无法测量/区分，符合该范围要求。 | 未发现新缺口。 |
| 媒体 capability | [Sound Capabilities](https://eblong.com/zarf/glk/Glk-Spec-076.html#sound_testing)、[Hyperlink Capabilities](https://eblong.com/zarf/glk/Glk-Spec-076.html#link_testing)；[vm.rs](../src/vm.rs) 按宿主、窗口类型及音频设备报告。Hyperlinks 表示函数可用，与各窗口 HyperlinkInput 分开。 | 未发现新的确定缺口；不要把无设备时如实返回不支持误报成实现遗漏。 |

## Glulx 核心与文档精度

对调用/输出续体、Null/filter/Glk I/O、Float/Double、IFZS、heap/undo、acceleration 的有界重核未发现新的确定功能缺失或有效程序语义错误。该结论来自正文与实现核对，并非用 150 条分发表齐全代替语义证据；完整符合性仍不能由这次审阅证明。

- `setmemsize/malloc` 的上限与预留失败有规范失败路径，[memory.rs](../src/memory.rs)；不能将其泛化为所有 Rust 分配/OOM 都能恢复。栈超限是 VM 错误。[内存指令](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_memory)、[malloc](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_malloc)。
- undo 的 64 MiB 是 [vm.rs](../src/vm.rs) 对 memory/stack/image 的估算记账，不含 heap_blocks 索引和分配器开销；应称估算预算。[保存状态](https://eblong.com/zarf/glulx/Glulx-Spec.html#saveformat)。这些是文档精度修正，未新增核心实现待办。

## 验收边界与实施顺序

Windows/macOS 实机、长篇游戏完整路线、更多媒体编码变体和实体音频输出属于验收待办。现有 [release workflow](../.github/workflows/release.yml) 配置了三平台测试/构建，但工作流存在不等于已经运行本次版本，更不等于 GUI 实机验收。应记录实际平台、构建版本、输入路线及结果，逐项更新清单。

先提交本次文档；再实现独立资源挂载/身份检查/会话，真实终端宿主，SONG 和实际 light 字重。散装资源发现为可选播放器便利项，单独登记，不冒充 Glk 标准 selector。实现后按上述验收条件更新状态；没有运行的平台和场景继续保留未勾选。
