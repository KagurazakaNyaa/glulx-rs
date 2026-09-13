# Windows 界面卡顿诊断

Debug 版默认在当前工作目录生成 `glulx-debug.log`，用于收集闪烁和无响应的线索。
`--diagnostics LOG` 仅用于改用其他文件名或路径；release 版仍需此参数才能启用日志。
Debug 版同时默认监听 `127.0.0.1:6060` 的 profiling HTTP 接口；release 版需要显式传入
`--profile-http ADDRESS`。打开 `/debug/pprof/` 可查看入口，`/debug/metrics` 返回可供脚本
读取的 JSON，包括 VM 状态、时间片、解码缓存和 opcode 计数。Linux/macOS 可用
`/debug/pprof/profile` 下载 pprof protobuf 或用 `/debug/pprof/flamegraph` 下载 SVG；追加
`?seconds=N`（最多 300 秒）可采集新的有限时间窗口；
Windows 因 pprof-rs 的 POSIX 信号采样限制改用 ETW provider；`/debug/etw` 返回 provider
GUID 和事件说明，可用 WPR/WPA 采集 CPU 栈并按进程查看 VM 时间片标记。JSON 指标仍然可用。
分发的诊断版使用 `diagnostic` 构建配置：开启优化，保留调试信息、断言和默认日志。
日志首行的 `opt_level=3` 表示已开启优化。Windows 测试包使用 GNU 工具链；正式版使用 MSVC，
两者运行速度和平台细节可能不同。当前测试版采用游戏画布、日志＋输入、翻译三个原生窗口；游戏画布和日志＋输入窗口都能提交命令，但只有获得焦点的窗口提交；辅助窗口不改变画布尺寸。
游戏画面在事件边界统一发布，避免呈现执行片段中的半成品。硬件 OpenGL 使用 GPU 图片缩放和混合；软件驱动自动采用缓存 CPU 画布。日志中的 `graphics_gpu` 和 `renderer` 标明实际路径。
Settings 支持分别选择比例/等宽字体的原生字体选择器（Windows、macOS、Linux GTK 3）和共享回退字体文件；桌面组件不可用时可填写字体文件路径。
工具栏 Log、Translation 控制两个辅助窗口的显示，Settings 打开独立设置窗口。
设置保存在可执行文件同目录的 `glulx-settings.json`，首次自动迁移旧设置。
关闭辅助窗口后，也可从 View 菜单重新打开；在画布窗口按 Ctrl+Shift+L 打开日志＋输入，
按 Ctrl+Shift+T 打开翻译窗口。
故事、内存、画布等原始数据超过 16 MiB 时跳过自动会话快照。
大型游戏请用游戏内 Save 命令保存进度；小型游戏仍可自动恢复会话。

解压后，在 PowerShell 中执行（修改游戏路径）：

```powershell
.\glulx-rs.exe "<游戏文件路径>"
```

如需指定其他日志位置：

```powershell
.\glulx-rs.exe --diagnostics "<log-path>" "<游戏文件路径>"
```

启用 HTTP 指标（release 构建也适用）：

```powershell
.\glulx-rs.exe --profile-http 127.0.0.1:6060 "<游戏文件路径>"
```

`ADDRESS` 会直接绑定到指定接口。非回环地址必须同时提供 `--profile-token TOKEN`，
或设置 `GLULX_PROFILE_TOKEN`；客户端使用 `Authorization: Bearer TOKEN` 请求。
回环地址可以不认证。Token 是 bearer 凭据，不要放入共享命令历史或日志。

运行期间可在另一个 PowerShell 窗口执行：

```powershell
Invoke-WebRequest http://127.0.0.1:6060/debug/metrics -OutFile "<output-dir>\metrics.json"
```

返回的 `opcode_counts` 按累计执行次数排序；`vm_ms`、`ui_ms` 和
`decode_cache_hit_rate` 可用来区分 VM、界面和指令解码成本。服务可以绑定任意地址；非回环地址
没有 token 时会拒绝启动。涉及 ETW/UAC 的采集仍应只暴露给可信网络。

Windows 原生 CPU 采样可在另一个 PowerShell 窗口执行：

```powershell
wpr -start CPU -filemode
```

复现路线后停止并保存 ETL：

```powershell
wpr -stop "<output-dir>\glulx-cpu.etl"
```

在 WPA 中按 `glulx-rs.exe` 进程过滤 CPU 栈；配置 ETW provider GUID 后，VM 时间片事件可用于对齐
`vm-slice`、`ui` 等阶段。

也可以通过 HTTP 让播放器代为执行同样的有界采集，并直接下载 ETL：

```powershell
Invoke-WebRequest "http://127.0.0.1:6060/debug/etw/profile?seconds=10" -OutFile "<output-dir>\glulx-cpu.etl"
```

该接口依赖系统中的 `wpr.exe`，缺少 WPR 或权限不足时会返回错误。
接口会先用当前令牌启动 WPR；仅当 WPR 报告缺少系统性能权限（如 `0xc5585011`）时，才通过
UAC 启动短时采集 helper。该 helper 只执行最多 60 秒的 CPU 采集，输出上限 256 MiB；拒绝 UAC
会返回 HTTP 403。由于请求可能触发 UAC 提示，请勿把未认证的 profiling HTTP 服务暴露给不可信网络。

仍然会打开图形界面。重现闪烁或无响应后再等约 10 秒，然后退出；
如果无法退出，可在任务管理器结束该诊断进程。将 `glulx-debug.log` 发回即可。
日志独立写入并及时刷新，无需重定向控制台，也无需开启翻译。
每次运行会覆盖指定日志，重试前请保留需要的那一份。

日志包括启动信息、状态转换、窗口尺寸变化，以及每两秒一次的心跳：
当前操作、操作持续时间、累计界面帧数、VM 执行片段数、指令地址、
待处理事件和输入请求数。超过 250 毫秒的操作会记录耗时；自动保存会记录开始和结束。
不记录游戏正文、输入文本或翻译凭据。Rust panic 会记录错误信息。

若没有生成日志或 Windows 提示缺少运行库，请提供错误信息。
此构建已在 Linux 交叉编译并检查导入依赖，尚未在原生 Windows 运行验证。

本地构建命令：

```sh
rustup target add x86_64-pc-windows-gnu
cargo build --locked --profile diagnostic --target x86_64-pc-windows-gnu
```

Linux 交叉编译需要 MinGW-w64 工具链。原生 Windows MSVC 环境也可直接
`cargo build --locked --profile diagnostic`，生成带同样诊断功能的可执行文件。
普通 `cargo build` 仍为未优化开发构建，也会默认记录日志，不用于游戏性能测试。

翻译开关仅控制后续新文本，历史不补译也不清空。当前原文和译文直接显示，历史默认折叠；同屏追加，清屏后切换。成功结果按原文和配置缓存，相同在途请求合并。

点击延迟诊断（VM 在主事件循环线程执行）：

```powershell
.\glulx-rs.exe --diagnostics .\performance.log
```

打开游戏并复现“输入指令已出现在日志，但画面稍后才更新”，再退出并查看该日志。
每次心跳包含 `interval_ms`、VM 时间片的 `vm_ms`、界面的 `ui_ms`，以及 `frame_delta`／`slice_delta`。
VM 摘要累计记录 `instructions`、`polls` 和真正触发画面边界的 `poll_yields`。
这些字段记录经过时间而不是 CPU 时间；心跳的 `stage` 和 `stage_ms` 表示当前仍在执行的阶段。

`vm_ms` 接近心跳间隔表示主要在 VM 工作；它是经过时间，不等于 CPU 时间。
`ui_ms` 高则需要检查文本布局、图片上传或软件渲染。`stage=vm-slice` 的 `stage_ms` 长时间增加
表示单个指令／宿主调用尚未返回。低 CPU 总占用也可能是单核受限：例如 16 个逻辑核中
一个核满载，任务管理器的总占用也只有约 6%。需要结合单核曲线与日志判断。
连续空轮询原本每次都让出到下一帧；回归用例中的 16 次轮询已从 17 个时间片减少为 1 个。

`window-layout` 在窗口几何、字体外观或样式提示变化时记录尺寸及 hints，不记录正文。正文缓存和定时器唤醒修复后的日志可用来区分剩余的游戏计算时间与排版／事件调度时间。
