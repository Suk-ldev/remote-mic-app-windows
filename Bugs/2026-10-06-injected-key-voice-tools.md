# Chatterfly 与豆包按语音键没有任何反应（注入的按键被丢弃）

- 发现日期：2026-10-06
- 状态：等待真机验证（RC003 报告层合成已实现，真机未验收）
- 影响范围：0.2.13 及之前全部版本、Windows、RC001/RC003、Chatterfly 与豆包输入法
- 功能点：按住说话快捷键（语音键 → 第三方语音工具）
- 现象：在连接页选「Chatterfly」预设后按住遥控器语音键，Chatterfly 毫无反应——
  连语音输入的窗口都不弹；豆包同样。语音会话本身正常（音频进了虚拟声卡）。
- 复现条件：选 Chatterfly 或豆包预设（右 Alt / 按住说话），按住遥控器语音键。
- 正常预期：按住期间该工具开始收音，松开后整理上屏。

## 根因

**这两个工具都丢弃程序注入的按键**，而本仓库此前只有 SendInput 一条路。

- 豆包（0.8.2.7）：ImeService 的全局低级钩子 `VoiceKeyHookProc` 回调首查
  `LLKHF_INJECTED`，命中即纯透传丢弃——字节级逆向 + 独立复现，四层闭环见
  `Bugs/2026-09-04-doubao-voice-hold-hotkey.md`。
- Chatterfly：上游 `GetSayAll` 2026-09-23 真机实测（分支
  `codex/chatterfly-hotkey-diagnosis`，记录
  `Bugs/2026-09-23-chatterfly-ime-activation-observability.md`）：同一文本框、
  焦点保持、TSF 已读回 `active_profile=chatterfly` 的前提下，**实体键盘**按它
  配置的左 Ctrl + 左 Win 能开麦，而同一组合键的 `SendInput` 表达（扫描码 /
  虚拟键 × 顺序 × 0/80ms 间隔）全部无反应，麦克风访问时间不更新；把快捷键临时
  改成左 Ctrl + 左 Alt 后结论一样（排除「只有 Win 参与的组合键才失败」）。
  旁路路线同时判死：`ITfKeystrokeMgr` 四种 Ctrl+Win preserved key 查询均
  `not_registered`、语言栏枚举无它的按钮、它的快捷键设置不接受 F5。

本仓库 2026-10-05 把 Chatterfly 填成预设（提交 `a4c27ca`）是错的：右 Alt 来自
上游 commit message 的转述，而上游那次观察到的「遥控器一按就弹 Chatterfly」用的
正是下面这条报告层路径，不是 SendInput；上游自己在 `6874311` 把 Chatterfly 相关
改动全部移除了。

## 修复

语音键不再经 SendInput，而是在 **HID 报告层**替换：RC003 的报告经 WUDFHost 的
`NtDeviceIoControlFile`（IOCTL `0x80018483`，9 字节报文）通过，钩子把语音键
usage `0x003E` 原地改成用户配置的单键 usage（右 Alt `0x00E6` / 左 Alt `0x00E2`）。
系统拿到的是**设备报告**，`injected=0`，与物理按键无从区分，于是检查注入标志的
工具照常响应。机制与上游 v0.5.0 的豆包支持同源（上游用注入 WUDFHost 的 Frida
agent；本仓库复用既有的 Detours C++ 钩子实现，见 ATTRIBUTION.md）。

- 配置单一事实源仍是「按住说话快捷键」：按住说话 + 恰好单键 + usage 在已实测
  白名单内才下发合成，其余配置继续走 SendInput（`rc003_hook::voice_synth_usage`）。
- 两条路径互斥，判据是注入器回执（`rc003_hook::voice_synth_active`）：合成生效
  期间 BLE 层不再注入同一个键（双写互扰）；子进程退出、卸钩、断连、改配置一律
  立即回落 SendInput（fail-open）。
- 成对性由报告状态语义保证：usage 从报告中消失 = 该键 UP，不需要也不应该补发
  结束边沿，粘键在结构上不可能。
- 连接页新增「按键送达方式」行（报告层 / 程序注入）与缺条件时的具体提示，
  让"按了没反应"一眼可归因，不必先去翻日志。

## 已知边界

- **只对 RC003 成立**。RC001 不经 WUDFHost 这条报告链，这两个工具在 RC001 上
  仍然无解；界面据此直接说明，不再让用户试。
- 和弦（≥2 键）无法用单个报告槽表达，白名单外的 usage 未逐键实测——两者都不
  下发合成。Chatterfly 的唤起键可自改，若用户改成和弦则该工具仍不可用。
- 替换发生在报告到达的瞬间，早于应用知情一个 BLE 往返，因此**不切输入法**：
  用豆包时需用户自己先把豆包设为当前输入法。上游对该窗口期加过"门内延迟"，
  2026-10-03 真机回归后默认关闭（上游 `4600128`），本仓库不实现。
- 替换位置（真实调用前 / 后）无法从代码静态判定，实现对两处都做幂等替换并分别
  计数（`synth_seen_before` / `synth_seen_after`），一次真机运行即可定论。

## 验证

- Rust 单测：`voice_synth_usage` 白名单与形态过滤、合成门禁只认匹配回执、
  注入器回执解析（`crates/sayall-windows/src/rc003_hook.rs`）。
- 前端单测：豆包/Chatterfly 预设写入右 Alt + 按住说话 + 不切输入法并标「需 RC003」、
  选中后未生效时给出具体缺条件提示（`src/pages/ConnectionPage.test.ts`）。
- 真机：**deferred**。需 RC003 + Chatterfly/豆包实测：按住即开麦、整段不被截断、
  松开即停、快速连按成对、断连/睡眠恢复后不残留按住的 Alt；并核对
  `rc003_hook synth` 与 `stage=synth` 日志里的 before/after 计数。
- 隐私：日志只记分类、计数与结果，不含设备身份、路径、窗口标题或语音内容。
