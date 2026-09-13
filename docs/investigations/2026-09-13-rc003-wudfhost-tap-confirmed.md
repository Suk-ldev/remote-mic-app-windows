# RC003 返回/音量± 经 WUDFHost 注入抓取——真机打通（2026-09-13）

承接 [2026-09-05 归档](2026-09-05-rc003-back-volume-buttons-invisible.md)（当时三键在所有已注册输入通道零事件，用户决定搁置）。本次重启调查并在真机端到端打通了抓取路径。

## 一句话结论

RC003 的**返回 / 音量+ / 音量-** 报文在本机（Windows 10 19045）唯一可达的用户态路径是：向承载该遥控器 HID-over-GATT 的 **WUDFHost.exe 注入 DLL**，挂钩 `ntdll!NtDeviceIoControlFile`，过滤 IOCTL **`0x80018483`**（BthLEEnum 读特征值），在 `STATUS_SUCCESS` 时读**输出缓冲区**（9 字节）。字节布局与 ZSTDJan 项目一致，已真机逐字节确认。

## 为什么别的路都不通（复核 + 新证据）

- **Raw Input**：设备只暴露一个 TYPE=1 键盘 collection，无 TYPE=0x02（HID）设备可注册厂商页 0xFF00（复核 2026-09-05 结论，仍然成立）。
- **直读 HID**：报文描述符显示 report 6/7/8（厂商页 0xFF00）与键盘 report 1 同处**一个** top-level collection（page 0x01/usage 0x06，inLen=121）。该 collection 被 kbdhid 独占打开，`CreateFile(GENERIC_READ)` 返回 err=5。因此无独立可读 collection（这也是与兄弟型号 32B9 的关键差异——`artchizhov/MiRemote_for_Windows` 能直读 32B9，是因为 32B9 把厂商页放在独立 collection）。
- **厂商 GATT 8A7A**：三个 Notify 特征（0102/0103/0112）订阅成功但按键期间零通知（需要未知的"开启上报"写入；`shammysha`/`artchizhov`/安卓 keylayout 均无此 opcode，8A7A 协议全网无公开资料）。HOGP（0x1812）与 ATVV（AB5E）对外部应用句柄 `AccessDenied`。

## 注入可行性（真机确认）

- 承载进程：`WUDFHost.exe`，以 **`NT AUTHORITY\LOCAL SERVICE`** 运行；服务 `mshidumdf` + 下层过滤 `WUDFRd`（`hidbthle.inf`）；进程内实际加载 `microsoft.bluetooth.profiles.hidovergatt.dll` + `Microsoft.Bluetooth.Proxy.dll`。
- 定位宿主 PID：注册表 `HKLM\SYSTEM\CurrentControlSet\Enum\BTHLEDevice\{00001812-…}_Dev_VID&012717_PID&32B8_…\<实例>\Device Parameters\WUDFDiagnosticInfo\HostPid`。**`HostPid` 是 `REG_QWORD`**（早先按 `REG_DWORD` 读会得到 `ERROR_UNSUPPORTED_TYPE=1630`）。
- 宿主是**共享设备池**（命令行 `DeviceGroupId=WudfDefaultDevicePoolPriorityHigh`），单设备 restart 不回收该进程；因此探针每次注入用**唯一 DLL 文件名**，便于反复运行。
- 缓解策略允许注入：`signature_flags=0`（非 MicrosoftSignedOnly）、`dynamic_flags=0`（非 ProhibitDynamicCode）。
- 注入方式：**已提权的管理员**即可（无需显式 SeDebugPrivilege）`OpenProcess`(PROCESS_VM_WRITE|VM_OPERATION|CREATE_THREAD) → `VirtualAllocEx` 写 DLL 路径 → `CreateRemoteThread(LoadLibraryW)`。
- 关键 ACL：LOCAL SERVICE 要能 **读+执行**暂存 DLL、**读写**共享映射，否则 `LoadLibraryW` 失败(err=5)/映射打不开。暂存目录/文件用 `D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FRFX;;;LS)`（SYSTEM/Admin 全权，LS 只读执行，普通用户无权）；映射用 `D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;LS)`。

## 挂钩点与字节布局（真机逐字节确认）

- 函数：`ntdll!NtDeviceIoControlFile`（**不是** `NtReadFile`——本机 UMDF 走该 IOCTL；探针实测 `NtReadFile` 全程 0 次）。
- 过滤：`IoControlCode == 0x80018483` 且返回 `STATUS_SUCCESS`。
- 读法：数据在**调用方输出缓冲区**（arg9，长度 = arg10 `OutputBufferLength` = 9）；`IO_STATUS_BLOCK.Information` 在本机读回 0，不能用它当长度（这是首轮误判"matched=0/无样本"的根因之一，另一根因是远端用户没在窗口内按到键）。
- 布局：9 字节 = `01 00 00` 前缀 + 3 个小端 `uint16` usage 槽。松开为全零。

真机抓到的报文（2026-09-13，本项目 `native/rc003-tap-probe` 探针 `--discover-rc003`）：

| 报文 | 键 | usage |
|---|---|---|
| `01 00 00 00 00 00 00 00 00` | 松开 | — |
| `01 00 00 F1 00 00 00 00 00` | 返回 | 0x00F1 |
| `01 00 00 80 00 00 00 00 00` | 音量+ | 0x0080 |
| `01 00 00 81 00 00 00 00 00` | 音量- | 0x0081 |
| `01 00 00 28 00 00 00 00 00` | 确定 | 0x0028 |

确定键（能用键）也走这条路，说明**所有**键都在此 IOCTL 出现；但能用键 Windows 已作为键盘输入投递,生产只从这里取**返回/音量±**,避免重复。完整 usage 表见 `crates/sayall-windows/src/raw_input.rs::button_for_usage`。

## 探针（里程碑 1 工件）

`native/rc003-tap-probe/`（`scripts/build-rc003-tap-probe.ps1` 用 pinned + 哈希校验的 Microsoft Detours 源码构建，不安装任何东西）：

- `tap.cpp`：Detours 只读挂钩 `NtDeviceIoControlFile`（+ `NtReadFile` 作对照），逐线程事务更新，DLL 自钉住（钩子仅读、绝不改/拦调用）。发现模式把每个 IOCTL 码计数并采样输出缓冲区，承载 IOCTL 的报文进环形缓冲。
- `probe.cpp`：注册表定位宿主 → 校验 WUDFHost 身份 + 缓解策略 + HID 驱动模块 → ACL 暂存 DLL → 远线程 `LoadLibraryW` 注入 → 共享映射读计数/环形缓冲。模式：`--self-test`（进程内验证挂钩机制）/ `--preflight`（只定位不注入）/ `--discover-rc003`（注入+采集）。
- `protocol.h`：仅计数/样本的共享内存 ABI（诊断期采少量本机按键报文字节以确认布局，不外传）。

`--self-test` / `--preflight` / `--discover-rc003` 均 passed（2026-09-13）。

## 下一步（生产化）

1. 生产钩子 DLL：解析 9 字节报文 → 对返回/音量± 做 press/release 边沿 → 经共享内存环喂给应用。
2. Rust 注入器（sayall-windows）：移植 probe.cpp 的定位/校验/暂存/注入；应用以管理员运行(登录计划任务，方案 C"默认管理员启动")。
3. 边沿灌入现有 `ButtonMappingRuntime`（`EngineMessage::GateEdge`），复用单击/双击/长按 + 原生透传，让三键与其它键一致可映射。
4. 生命周期（方案 3）：连接时注入,睡眠/断连时卸钩;宿主随遥控器断连消失即天然最小驻留。
5. 安装期嵌入 DLL + CI 构建 + 管理员登录任务。
