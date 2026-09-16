# Google TV / Chromecast 遥控器支持：已落地部分与待真机验证项

状态：**未验收**。代码已接入，`remote_profile::GOOGLE_TV.verified = false`，
在真机核对完下面每一条之前，不要把该机型作为已支持对外发布。

## 已落地

- `crates/sayall-windows/src/remote_profile.rs`：机型档案表（VID/PID、按键集合、
  HID usage → RemoteButton）。小米档案的 usage 表与重构前的全局解码表逐条一致，
  由 `xiaomi_usage_table_matches_the_shipped_decoder` 锁定。
- `RemoteButton` 追加 `Youtube` / `Netflix`（追加在末尾，既有 ordinal 不变，
  已保存的门控掩码与映射文件不受影响）。两者无原生 Windows 动作。
- `ButtonStateMerger` 按连接中的档案解 usage；切换档案时清空 HID 侧按下状态。
- Raw Input 枚举与 key_suppressor 的设备归属判定改走档案表（原先硬编码小米 VID/PID）。
- `RawInputSnapshot.profile_id` / 诊断摘要 `rawInput.profileId` 暴露识别结果。
- 按键页在示意图之外补出"该遥控器的其他按键"，配置该机型独有的键。

## 待真机验证

| # | 待验证 | 现在的依据 | 怎么验 |
|---|---|---|---|
| 1 | VID `0x18D1` / PID `0x9450` | vibe-mote `keys.py` 的 `find_wudfhost_pid` 默认值 | 配对后在设备管理器看 HID 设备的硬件 ID；或跑 `Testing/list-all-rawinput.ps1` 打印路径 |
| 2 | HID usage 表 14 条 | vibe-mote `keys.py` `USAGE_TO_KEY`，该项目注明"受控采集，14/14 全命中" | 逐键按下，看诊断日志的 `map_edges` 是否解出预期按键 |
| 3 | Raw Input 报文形状 | **无依据**。`decode_report_usages` 目前只认小米的 9/7/6 字节形状；vibe-mote 是用 frida 从 WUDFHost 读报文，没有 Raw Input 路径的证据 | 先看 `RawInputPhase` 是否 ready、`rawEventCount` 是否增长；报文形状不符会以 `UnsupportedReportShape` 落进日志 |
| 4 | 语音链路 | ATVV + IMA/DVI ADPCM 与小米同协议，`sayall-core` 应可直接复用 | 按住语音键，看 `StreamStarted` 与 `decodedSamples` |
| 5 | GATT 型号识别 | **未实现**。`RemoteModel` 仍只有 Rc001/Rc003/Unknown，ble.rs 的 2A24 型号匹配没有加 Google 分支 | 连接后读 Model Number (2A24) 的实际字符串，再决定加什么匹配 |
| 6 | 按键是否需要吞键 | 未知。小米这边 key_gate 要处理原始键泄漏；Google 遥控器的按键在 Windows 上会产生什么原生动作没测过 | 先不配映射，逐键按下看系统有无反应 |

第 3、5 条是接上真机后最可能要改代码的两处。

## 不适用

vibe-mote 用 frida 注入 WUDFHost 读 HID 报文；本项目已有自建的
`native/rc003-hook` 注入方案，不引入 frida。

---

# 附：默认麦克风临时切换（同批次落地，同样未验收）

`crates/sayall-windows/src/default_capture.rs` 是本项目**唯一**走未公开 COM 接口
的地方：`IPolicyConfig`（CLSID `870af99c-…`、IID `f8679f50-…`），
`SetDefaultEndpoint` 在虚拟表里排在 IUnknown 三项之后第 11 位。该布局来自
参考项目 QL-4/RemoteMapper 的可用实现，微软从未承诺它跨 Windows 版本稳定。

默认关闭。开启前请在真机核对：

| # | 验什么 | 判据 |
|---|---|---|
| 1 | 接口能创建 | 开启开关后按一次语音键，日志无 `default_capture action=borrow ... terminal_result=failed` |
| 2 | 三个角色都切了 | 语音期间 Windows 声音设置里录制默认设备是 CABLE Output，且通讯默认也是 |
| 3 | 松开即还原 | 松开语音键后默认设备立刻变回原来的麦克风 |
| 4 | 异常路径还原 | 语音中拔掉遥控器 / 让电脑睡眠，恢复后默认设备是原来的 |
| 5 | 崩溃自检 | 任务管理器强杀应用后重启，连接页顶部出现"上次异常退出没还原干净"提示 |
| 6 | 副作用 | 语音期间正在录音的会议软件会被切走——这是设计上的取舍，需在说明里讲清楚 |

第 1 条失败（返回 E_NOTIMPL 或崩溃）说明虚拟表布局在该 Windows 版本上不适用，
应保持开关关闭并考虑移除该功能，而不是调整偏移试错。
