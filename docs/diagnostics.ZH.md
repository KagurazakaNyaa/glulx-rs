# Windows 界面卡顿诊断

Debug 版默认在当前工作目录生成 `glulx-debug.log`，用于收集闪烁和无响应的线索。
`--diagnostics LOG` 仅用于改用其他文件名或路径；release 版仍需此参数才能启用日志。
分发的诊断版使用 `diagnostic` 构建配置：开启优化，保留调试信息、断言和默认日志。
日志首行的 `opt_level=3` 表示已开启优化。Windows 测试包使用 GNU 工具链；正式版使用 MSVC，
两者运行速度和平台细节可能不同。当前测试版采用游戏画布、日志＋输入、翻译三个原生窗口；辅助窗口不改变画布尺寸。
游戏画面在事件边界统一发布，避免呈现执行片段中的半成品。硬件 OpenGL 使用 GPU 图片缩放和混合；软件驱动自动采用缓存 CPU 画布。日志中的 `graphics_gpu` 和 `renderer` 标明实际路径。
Settings 支持原生字体选择器（Windows、macOS、Linux GTK 3）和字体文件；桌面组件不可用时可填写字体文件路径。
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
.\glulx-rs.exe --diagnostics "D:\Logs\player.log" "<游戏文件路径>"
```

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
