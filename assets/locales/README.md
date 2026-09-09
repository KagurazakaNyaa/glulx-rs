# Interface translations / 界面翻译

- `en.json`: English and the fallback catalog / 英文及回退文案。
- `zh.json`: Chinese / 中文文案。
- `diagnostics.json`: Maps existing VM/service diagnostics to catalog keys. Its `source` patterns are compatibility data, not text to translate / 将现有虚拟机及服务诊断映射到文案键；`source` 是兼容匹配规则，不应翻译。

Edit JSON **values** to translate; keep keys unchanged. The same key must mean the same thing in every language. When adding a message, add its key to both catalogs and refer to that key from the UI. Missing translations fall back to English.

翻译时只修改 JSON 的**值**，保持键名不变。同一个键在不同语言中应表达相同含义。添加文案时，在两个语言文件中增加同名键，并在界面代码中引用。缺失的翻译回退到英文。

Placeholders are zero-based: `{0}`, `{1}`, etc. Preserve their numbers; reorder them to suit the language. For example, `"Window {0}"` becomes `"窗口 {0}"`. Use JSON `\n` for line breaks. Arguments (paths, server responses, story data) are inserted verbatim.

占位符从零开始：`{0}`、`{1}` 等。保留编号，可按语序调整位置；例如 `"Window {0}"` 翻译为 `"窗口 {0}"`。换行使用 JSON 的 `\n`。路径、服务响应和故事数据等参数会原样插入。

Catalogs are embedded in the executable at build time, parsed once on first use, and require no extra files beside the installed application. Rebuild after editing translations. Adding another language also requires registering it in `src/app/i18n.rs` and the settings selector.

语言文件在构建时嵌入程序，首次使用时解析一次，安装后无需额外分发文件。修改翻译后需重新构建。新增语言还需在 `src/app/i18n.rs` 和设置选择器中注册。

Validation / 验证：`cargo test --lib app::i18n`
