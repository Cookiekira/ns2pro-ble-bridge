# Rust 重写 Spec（第 3 版）

完整实现规格已发布为 [GitHub issue #15](https://github.com/Cookiekira/ns2pro-ble-bridge/issues/15)，标签为 `ready-for-agent`。issue 包含用户故事、实现决策和用户确认的会话级测试边界；本文保留详细设计背景，执行与验收以 issue 为准。

状态：Rust 实现已在 feat/rust-rewrite 集成；实际验证及未完成的硬件验收见 docs/rust-integration-validation.md。历史设计背景保留，GitHub issue #15 仍是权威规格。
第 3 版的决定（2026-10-22）：连接间隔尽量匹配 Switch 2 的 5ms；只有已验证的适配器路径才启用，通用路径回退到标准的 7.5ms 请求。其它采用第 2 版推荐方案：延迟优先、原生接口、Linux 去掉 BlueZ D-Bus（bluer/libdbus）。

## 0. 目标与非目标

**目标**

- 一次性用 Rust 替换整个项目：删除 C#/.NET、Go、`vendor/VIIPER`，不保留过渡兼容层。
- **延迟第一**：bridge 自身不应引入可感知的延迟；输入路径上不加任何定时器等待（目标 p99 < 0.5ms，见 §1）。
- **原生接口**：
  - Linux：直接使用内核接口——原始 L2CAP、Bluetooth mgmt socket、vhci sysfs；不经过 D-Bus，也不调用 `usbip` 命令。
  - Windows：WinRT Bluetooth + usbip-win2 IOCTL。
- CLI 参数和缓存文件格式与现版本一致。
- Linux 在实机上验收；Windows 只要求 `cargo build` 和 `cargo test` 在 CI 通过。

**非目标**

- 只支持 NS2Pro，不做通用的虚拟 USB 框架。
- 不写内核驱动或签名驱动。

## 1. 延迟模型（设计依据）

端到端的输入路径：

```
手柄 ──BLE 连接事件──▶ 内核 L2CAP ──▶ [bridge: 解析 → 完成挂起的 IN URB] ──▶ vhci ──▶ usbhid/hidraw ──▶ 游戏/SDL
      ①                               ②                                     ③
```

| 段 | 量级 | 控制手段 |
|---|---|---|
| ① BLE 连接间隔 | **目标 5ms，标准回退请求 7.5ms；实际值取决于协商** | 5ms 仅用于已验证的适配器路径；Linux 通用路径用 mgmt `LOAD_CONN_PARAM` 请求 7.5ms（interval 6、latency 0）。Windows 请求 `ThroughputOptimized`，不承诺具体间隔。参见 §1.3 |
| ② bridge 内部 | 目标 µs 级 | 事件驱动，零额外延迟（见 §1.1） |
| ③ vhci → 主机 | µs 级 | 走 socketpair；不经过 TCP，也没有批量 flush |

现有实现在 ② 段有可观的延迟：VIIPER 的写批处理器每 1ms 才 flush 一次；中断 IN 靠主机轮询；回调链上还要经过 Go 运行时、cgo 和 .NET 线程池。① 段 Linux 没有主动请求连接参数；Windows 已请求 `ThroughputOptimized`，但请求不代表实际协商值。

### 1.1 bridge 内部的热路径规则

1. **单线程运行时**：用 tokio `current_thread`。热路径上的唤醒都发生在同一线程，没有 work-stealing 和跨线程 futex。阻塞调用（Windows IOCTL）放到 `spawn_blocking`。
2. **输入路径两跳**：ATT reader 收到 notification 后，在原地解析并写入 `watch<InputSample>`，随即唤醒 URB 任务，由它完成挂起的 IN URB 并写 socket。中间没有 input 泵任务，也没有 mpsc。
3. **热路径零分配**：
   - 接收用复用的固定缓冲区；
   - notification 通过回调 `sink(&[u8])` 交付，不生成 `Vec`；
   - report 直接构造到 `[u8; 64]`；
   - 应答拼到复用的 `Vec`，每批只调用一次 `write`。
4. **中断 IN 由事件驱动**：
   - 有新样本、且有挂起的 URB 时，立即完成；
   - 有新样本但没有挂起的 URB 时，标记为脏，下一个 IN 提交上来就立即完成；
   - 只有在**没有新样本**的情况下，才按 bInterval（4ms）发保活 report。
5. **socket 选项**：TCP 模式设 `TCP_NODELAY`，不做写批处理。
6. **线程优先级**（尽力而为，失败只在 debug 日志里记录）：
   - Linux 以 root 运行时，把运行时线程设为 `SCHED_FIFO` 优先级 10；
   - Windows 对运行时线程调用 `AvSetMmThreadCharacteristicsW("Games")`。
7. **内置测量**：在 `--log-level debug` 下每 5s 输出一次：
   - BLE report 速率和到达间隔抖动；
   - bridge 内部延迟的 p50/p99（从 notification 到达到对应 URB 完成，用 `Instant` 计时）；
   - URB 完成速率和保活次数；
   - rumble 写入速率。

### 1.2 输出路径（rumble / LED）

- 主机发出的 OUT 会立即 ACK，然后写入 `watch<Option<Rumble>>`。
- rumble 任务采用前沿节流：距上次写入已满 `RUMBLE_MIN_INTERVAL` 时立即写，否则到期后只写最新值。这样第一个包不额外等待，后续不堆积过期的包。
- 初始值 20ms，与现在相同。实机验收时测一下改成 10ms 和连接间隔对齐是否稳定，再决定最终值。
- LED 走命令通道（串行，超时 2s），不在热路径上。

### 1.3 “NS 默认间隔”的依据与回退策略

- 这里的 NS 指 Switch 2 与 NS2Pro。抓包研究记录主机使用厂商专用 HCI 命令设置 **5ms**，不是之前推荐的 7.5ms；这属于逆向研究结果，不是 Nintendo 官方保证的规格。[原始研究](https://github.com/ndeadly/switch2_controller_research/blob/master/bluetooth_interface.md)
- 普通 PC 的系统默认值不是主机的 5ms。Linux 上游初始化的 LE interval 范围是 `0x18..0x28`（30–50ms）；已有设备参数、用户设置和后续协商都可能改变它。[Linux 源码](https://github.com/torvalds/linux/blob/master/net/bluetooth/hci_core.c)
- 用户选择：**尽可能匹配主机的 5ms，不支持时标准回退**。不把 5ms 作为建立连接的必要条件，也不更改适配器全局默认值。
- Linux 通用 mgmt 路径不能承诺设置 5ms：传统 LE 参数以 1.25ms 为单位，最小合法值为 6（7.5ms）；不发送 interval 4 来碰运气。较新 Bluetooth 的短间隔功能也必须由主机栈、适配器及对端共同支持，不能仅凭版本号启用。
- 5ms 路径须有明确的适配器/固件匹配、已核实的命令或能力、结果校验及失败后的恢复办法；不能仅因 Switch 2 上有厂商命令，就向任意 PC 适配器发送。当前尚未验证本机有这种路径，因此通用实现先具备 7.5ms 回退，5ms 支持在受支持硬件上单独验收。
- Linux 在连接前对目标设备请求 7.5ms（6/6、latency 0、timeout 200），只有通过上述验证时才走 5ms 路径。参数请求被拒绝则记录原因，仍尝试以系统允许的参数连接；不得因此无限重试或阻止输入。
- Windows 使用 WinRT `ThroughputOptimized` 作为系统允许的低延迟请求；它不提供指定 5ms 或 7.5ms 的保证。不修改驱动或注册表来强制间隔。[Microsoft API](https://learn.microsoft.com/en-us/uwp/api/windows.devices.bluetooth.bluetoothlepreferredconnectionparameters.throughputoptimized)
- 日志分别记录目标、请求结果和可观测到的实际连接间隔；读不到实际间隔时明确记为 unknown。BLE report 到达速率单独统计，不能当成连接间隔的证明。5ms/7.5ms 分别约为 200/133 个连接事件每秒，但不保证每个事件产生一个 report。

## 2. 设计原则（借鉴 Effect-TS，按 Rust 习惯落地）

| Effect-TS 概念 | 落地方式 |
|---|---|
| `Effect<A, E, R>` 带类型的错误 | 每个模块用 `thiserror` 定义错误枚举；不用 `anyhow`；`main` 把错误映射成退出码 |
| `R`、`Context.Tag`、`Layer` | 服务用 trait 定义（`Bluetooth`、`GattLink`），泛型静态分发；只在 `app.rs` 组装 |
| `Scope`、`acquireRelease` | RAII + 显式幂等的 `async fn close()`（BLE 断开、vhci detach、恢复本进程改动过的设备 conn param；不能覆盖其它管理进程的新设置） |
| Fiber、结构化并发 | `CancellationToken` + `JoinSet` |
| `Schedule` | `backon` 指数退避，只包住"连接手柄" |
| `Schema` | `zerocopy`（二进制）、`serde`（JSON 缓存） |
| 纯核心 + 外壳 | **sans-IO**：协议、设备、URB 调度、ATT/mgmt 编解码都是纯状态机；时间和随机数由外部注入 |

明确不做：Effect monad/HKT、fp 风格的库、`pipe` 组合子链。

## 3. Workspace 与依赖

```
crates/
├─ ns2pro-proto/   纯：NS2Pro BLE 协议、配对、校准；bt::att（ATT + GATT 发现）；bt::mgmt（mgmt 报文 + EIR 解析）
├─ ns2pro-usb/     纯：描述符、设备状态机、report、USB/IP 编解码、UrbEngine
└─ ns2pro-bridge/  外壳（bin：ns2pro-ble-bridge）
fixtures/viiper/   从现有 Go 实现导出的基准字节（已生成）
```

纯 crate 只依赖 `aes`、`uuid`、`zerocopy`、`thiserror`，拿不到 tokio。

外壳的依赖：
- 通用：`tokio` 1.54（current_thread）、`tokio-util`、`futures`、`backon`、`clap` 4.6、`tracing`、`tracing-subscriber`、`serde`/`serde_json`、`dirs` 7、`getrandom` 0.4。
- Linux：`libc`，用于 AF_BLUETOOTH socket、`sched_setscheduler`，配合 tokio `AsyncFd`。
- Windows：`windows` 0.62，用于 WinRT Bluetooth、`IBufferByteAccess`、SetupDi、DeviceIoControl、Avrt。

和第 1 版相比**去掉了 `bluer` 和 `dbus`（vendored libdbus）**：不再依赖 libdbus 或 Go/cgo；系统接口仍通过 `libc` / Windows API 调用，arm64 构建也变得简单。

## 4. `ns2pro-proto`（纯）

| 模块 | 内容 |
|---|---|
| `address` | `Address`（48 位），可解析和格式化，提供 BE/LE 字节序转换 |
| `advertisement` | 识别 Nintendo 0x0553 / VID 0x057E / PID 0x2069，mode 0x00 和 0x81 |
| `gatt` | 5 个特征及其 UUID；`INIT_PAYLOAD` |
| `input` | `Buttons`、`Stick`、`InputState`；`COMMON_BUTTON_LAYOUT` 一张表同时用于 BLE 解码和 USB 编码；`parse_common_report` 与 C# 逐条一致 |
| `calibration` | 出厂区和 user 区（magic `A2 B2`）的校准 |
| `command` | 命令帧、应答校验、SPI 读、LED、feature、42 字节 rumble 包 |
| `pairing` | 配对状态机：`next_command()`/`on_response()`；host_key 和 challenge 由外部注入 |
| `bt::att` | PDU 构造与解析；`classify()`；`Discovery` 状态机（MTU 247 → services → characteristics → 通知特征的 CCCD），按 service 正确计算特征的结束句柄 |
| `bt::mgmt` | mgmt 报文编解码（见 §6.5 用到的命令和事件）；EIR/AD 解析，提取 manufacturer data |

## 5. `ns2pro-usb`（纯）

### 5.1 描述符

设备、配置、字符串、HID report、MS OS 1.0 描述符都与 VIIPER 逐字节一致（由 fixture 断言）。速度为 Full speed，VID/PID 为 0x057E/0x2069。

### 5.2 设备状态机 `Ns2ProDevice`

- 接口：`control()`、`interrupt_in(&InputSample) -> [u8; 64]`、`interrupt_out()`、`bulk_out()`、`pop_bulk_in()`。
- 输出事件 `Output::{Rumble, PlayerLeds}` 通过返回值交付，不用回调。
- bulk 命令、report 0x05/0x09、flash 假数据都与 VIIPER 一致。
- `InputSample { state: InputState, motion_timestamp_us: u32, seq: u64 }`：motion 时间戳由外部注入，见 §5.3。

### 5.3 与 VIIPER 不同的地方（有意为之）

| 场景 | VIIPER | 新实现 |
|---|---|---|
| 未知的控制请求或描述符 | status 0、0 字节 | **STALL**（−EPIPE），与真设备一致 |
| SET_IDLE、SET_INTERFACE、CLEAR/SET_FEATURE | 空应答 | ACK |
| 中断 IN | 收到就立即回包，主机端因此一直紧密轮询 | 事件驱动，见 §1.1 第 4 条 |
| bulk IN 队列为空 | 回 0 字节 | 挂起 URB，等应答产生时立即完成 |
| UNLINK | 总是回 −ECONNRESET | 在挂起的 URB 中找到就移除并回 −ECONNRESET，找不到回 0 |
| report 0x05 的 motion 时间戳 | 每个 report +4000（虚构的 4ms 节拍） | **真实时间**：如果 BLE report 自带手柄的 IMU 时间戳，就原样透传（实机先确认偏移）；否则用收到 notification 时的单调时钟（µs）。保活 report 沿用上一个样本的时间戳 |

### 5.4 USB/IP

- 编解码：管理报文（devlist/import，失败时 status=1）；48 字节大端的 URB 头；RET_SUBMIT/RET_UNLINK。
- `UrbEngine` 的接口：`submit()`、`unlink()`、`on_sample()`（有新样本）、`on_keepalive_due()`。产出的 `Effects { replies, outputs }` 由外壳负责写出。
- 保活的时间点由引擎根据注入的时间计算（`next_keepalive_at()`），外壳只负责 `sleep_until`。
- 导出信息：busid `1-1`。

## 6. `ns2pro-bridge`（外壳）

### 6.1 模块

```
main.rs / cli.rs / app.rs / cache.rs / channels.rs / rt.rs（线程优先级）
usb/   session.rs（URB 循环）· server.rs（TCP 管理协议）· vhci.rs [linux] · usbip_win.rs [windows] · host.rs
ble/   mod.rs（trait）· linux/{mgmt.rs, att_link.rs, mod.rs} · windows.rs
controller.rs（初始化、命令通道、配对驱动、会话任务）· session.rs（来源解析、缓存语义、supervisor）
```

### 6.2 并发拓扑（单线程运行时）

```
ATT reader ──sink: 解析──▶ watch<InputSample> ──▶ URB 会话任务 ──write──▶ vhci socket
                                                     │
            ◀── rumble 任务 ◀── watch<Option<Rumble>> ◀┤
            ◀── LED 任务    ◀── watch<Option<u8>>    ◀┘
```

- 会话结束时把输入重置为默认值。现在会保留最后一帧，导致断线后按键一直处于按下状态。
- 重连后重放主机设置的 LED，未设置过时用默认 0x06。进入新会话时丢弃旧的 rumble 值。

### 6.3 trait

```rust
trait Bluetooth {
    type Link: GattLink;
    fn scan(&self) -> impl Future<Output = Result<Discovered, BleError>>;
    fn connect(&self, target: &Discovered /* 或缓存地址 */) -> impl Future<Output = Result<Self::Link, BleError>>;
    fn adapter_address(&self) -> impl Future<Output = Result<Address, BleError>>;
}
trait GattLink: 'static {
    fn write(&self, ch: Characteristic, data: &[u8]) -> impl Future<Output = Result<(), LinkError>>;
    /// 通知在 reader 上下文里同步调用 sink，避免拷贝和任务切换。
    fn subscribe(&self, ch: Characteristic, sink: Box<dyn FnMut(&[u8]) + Send>) -> impl Future<Output = Result<(), LinkError>>;
    fn closed(&self) -> CancellationToken;
    fn close(&self) -> impl Future<Output = ()>;
}
```

运行时是单线程，所以 future 不要求 `Send`（Windows 的 sink 会在 WinRT 线程池上调用，因此 sink 本身要求 `Send`）。

### 6.4 会话流程（与现有行为一致，改动处单独标出）

1. 设备来源的优先级为 `--device-address` > 缓存 > 扫描。
2. 连接后初始化：Init → 订阅 CommandResponse → 设置 LED → 读校准（失败时只告警）→ `set_feature_mask(0xFF)` → `enable_features(--feature-flags)` → 订阅 Input。
3. 来源为扫描或指定了 `--pair-host` 时，执行 host 配对。**改动**：`--pair-host` 在一个进程内只配对一次。
4. 成功后写缓存。来源是缓存且连接失败（不是因为取消）时删除缓存，下次改为扫描。
5. 第 1–4 步用 `backon` 指数退避重试：1s → 10s，带 jitter，次数不限。现在是固定 3s。
6. 会话正常结束后等 1s 再重新开始。

### 6.5 Linux BLE：只用内核接口

**mgmt 控制通道**（`AF_BLUETOOTH`/`BTPROTO_HCI`，`HCI_CHANNEL_CONTROL`）：

| 用途 | mgmt 操作 |
|---|---|
| 适配器地址、上电 | `READ_INDEX_LIST`、`READ_INFO`；未上电时 `SET_POWERED` |
| 扫描 | `START_DISCOVERY`（LE），监听 `DEVICE_FOUND` 事件中 EIR 里的 manufacturer data。`DEVICE_FOUND` 只对实际收到的广播触发，天然满足"只认新广播"（9ee85ba 的问题从根本上消失）。如果别的进程已经在扫描（返回 Busy），就只被动监听事件 |
| 连接前 | `STOP_DISCOVERY`（同步完成，不像 bluer 是在 drop 时异步停止）；如果设备已经连接，先 `DISCONNECT` |
| 低延迟 | 遵循 §1.3：仅已验证的适配器路径尝试 5ms；通用 `LOAD_CONN_PARAM` 对目标设备请求 interval 6/6（7.5ms）、latency 0、timeout 200（2s）；失败时保留系统允许的参数并记录原因 |
| 缓存设备重连 | 先做一次 LE 扫描，等目标地址的新广播出现。这样能拿到准确的地址类型，并且在手柄确实在广播时才发起连接。最多等 30s，超时按连接失败处理（参见 §6.4 第 4 条） |

**原始 ATT 链路**：
- `socket(AF_BLUETOOTH, SOCK_SEQPACKET, BTPROTO_L2CAP)`，安全级别 `BT_SECURITY_LOW`；bind 本地（LE public，cid 4），connect 对端（cid 4，用广播报告的地址类型），非阻塞 connect 加 `AsyncFd`，超时 10s。
- 不调用 BlueZ 的 `Device1.Connect`，不触发 SMP。
- reader 的处理：
  - notification 在原地调用 sink；
  - 应答写入单个 oneshot 槽位；
  - 对端发来的 MTU 请求回 247；
  - indication 回 confirmation；
  - 其它请求回 Request Not Supported。
- EOF 或出错时取消 `closed`。

**权限**：mgmt 控制命令需要 `CAP_NET_ADMIN`。
- 自动 attach 本来就需要 root，所以这一点没有额外要求。
- 以非 root 运行 `--no-auto-attach` 时，需要先执行一次 `sudo setcap cap_net_admin+ep ./ns2pro-ble-bridge`（README 会写明）。这是相对现版本的一个变化。

**和 bluetoothd 共存**：不依赖 bluetoothd，bluetoothd 在运行也能正常工作；在没有 bluetoothd 的系统上同样可用。

### 6.6 Windows BLE

- 扫描：`BluetoothLEAdvertisementWatcher`（Active、AllowExtended）。
- 连接：`BluetoothLEDevice::FromBluetoothAddressAsync` → `RequestPreferredConnectionParameters(ThroughputOptimized)`（尽力而为）→ GATT 发现（Uncached）。
- `ThroughputOptimized` 不等于设置固定间隔；实际参数与降级行为按 §1.3 记录。
- 写入用 WriteWithoutResponse。`ValueChanged` 在 WinRT 线程上直接调用 sink，通过 `IBufferByteAccess` 零拷贝读取 notification 数据。
- `ConnectionStatusChanged` 触发 `closed`。

### 6.7 USB 主机侧

| 平台 / 模式 | 行为 |
|---|---|
| Linux 默认 | `socketpair(AF_UNIX, SOCK_STREAM)` → 在 `vhci_hcd.0/status*` 中找空闲 hs 端口 → 写 `"<port> <fd> <devid> 2"` 到 `attach`，然后关闭 kernel 端的 fd。不开 TCP，也不需要 `usbip` 命令。退出时写 `detach`。内核主动断开时视为致命错误，进程退出 |
| Linux `--no-auto-attach` | 在 `--usb-addr` 上开 TCP server（`TCP_NODELAY`），供手动或远程 `usbip attach` |
| Windows 默认 | 在 `--usb-addr` 上开 TCP server，再通过 `spawn_blocking` 调 IOCTL `PLUGIN_HARDWARE`（busid `1-1`） |
| Windows `--no-auto-attach` | 只开 TCP server |

socketpair 方案还没有实测。如果内核拒绝 AF_UNIX socket，就退回"本地 TCP socket 的 fd 写 sysfs"，依然不需要 `usbip` 命令。

## 7. CLI（与现版本一致）

`--usb-addr`（默认 `localhost:3241`）、`--device-address`、`--pair-host`、`--host-address`、`--forget-device`、`--cache-file`（`dirs::data_local_dir()/Ns2Pro.BleBridge/controller-cache.json`）、`--no-auto-attach`、`--feature-flags`（默认 `0x07`，接受十六进制或十进制）、`--log-level`（默认 `info`，`RUST_LOG` 优先）、`-h`。

另外：
- 隐藏参数 `--bt-smoke-test`（替换原来的 `--bluez-smoke-test`），执行 mgmt `READ_INDEX_LIST` 并做一次 0.5s 扫描。
- 删除 `--verify-native-library`。
- 缓存格式不变：`{"address": "AA:BB:CC:DD:EE:FF"}`。
- 退出码：0 正常；1 运行期错误；2 USB 设置失败或参数错误。

## 8. 错误类型

- 纯 crate：`ShortReport`、`ResponseError`、`PairingError`、`AttError`/`DiscoveryError`、`MgmtError`、`UsbipError`。
- 外壳：`LinkError`、`BleError`、`ControllerError`、`AttachError`、`UsbHostError`、`CacheError`。
- 顶层 `AppError` 用 `#[from]` 汇总，提供 `hint()`（modprobe、root/setcap、usbip-win2）和 `exit_code()`。

## 9. 构建与发布

- release profile：`lto = "fat"`、`codegen-units = 1`、`panic = "abort"`、`strip = true`、`opt-level = 3`。不设 `target-cpu`，保证可移植。
- 删除：C# 全部源码与测试、csproj、`bin/`、`obj/`、空的 `packaging/`、`vendor/VIIPER`、`.gitmodules`。
- CI：Linux 跑 `fmt --check`、`clippy -D warnings`、`test`，再跑 smoke（runner 上有适配器时才执行）；Windows 跑 `build` 和 `test`。
- Release：linux-x64、linux-arm64（在 arm runner 上原生构建）、win-x64，资产命名为 `ns2pro-ble-bridge-<tag>-<rid>`。
- README：更新构建方式（只需要 Rust），Linux 不再需要 `usbip` 和 BlueZ 工具（`vhci-hcd` 仍然需要），补充 setcap 说明。
- 版本号改为 0.2.0；`.gitignore` 增加 `target/`。

## 10. 测试与验收

1. `fixtures/viiper/`（已生成）：逐字节断言描述符、控制应答、bulk 命令、report 和 USB/IP 握手。§5.3 的有意差异单独标注；motion 时间戳在测试里注入 +4000 节拍来对齐 fixture。
2. 纯 crate 的单元测试：地址、广播、按键表往返、摇杆、校准、命令帧、配对（确定性的 key 加 AES 校验）、ATT 发现（模拟 GATT 数据库）、mgmt/EIR 解析。
3. 把 C# 的测试翻成 Rust，使用 `FakeBluetooth`/`FakeLink`：初始化流程、断线信号、LED 和 rumble 路由、扫描后配对并写缓存、`--host-address` 跳过适配器查询、缓存设备连接失败时删除缓存。
4. URB 会话集成测试（`tokio::io::duplex` + 暂停时间）：
   - 枚举；
   - **有新样本时立即完成 IN，断言不经过定时器**；
   - 4ms 保活；
   - bulk IN 挂起；
   - UNLINK 和 STALL。
5. 延迟基准：在 `ns2pro-usb` 上用 bench 测量 sample → RET_SUBMIT 编码的耗时；在外壳上用 duplex 测量 sink → 字节写出的 p99。
6. 实机验收，需要你配合：
   - 用 `sudo` 运行，验证 socketpair attach，并用 `lsusb` 确认设备；
   - 分别验证 5ms 支持路径（硬件支持时）、7.5ms 标准回退和请求被拒绝后仍能连接；用连接参数事件或 HCI 抓包确认实际间隔，不能仅凭 BLE report 速率判断；
   - 在 debug 日志里分别查看 BLE report 速率、实际连接间隔（不可读时为 unknown）和 bridge 内部延迟的 p99；p99 < 0.5ms 是 bridge 内部目标，不包含 BLE 等待和主机消费延迟；
   - 确认 BLE report 里 IMU 时间戳的偏移；
   - 按 `docs/linux-hardware-checklist.md` 全部走一遍（前置条件改为 `vhci-hcd` 和 root/setcap）。
7. Windows：确认 CI 的 build 和 test 通过即可。

## 11. 已收敛的决定与实现边界

1. **Linux 去掉 BlueZ D-Bus，改用 mgmt socket**（§6.5）。不依赖 libdbus，可请求每设备的连接参数。代价是非 root 的 `--no-auto-attach` 需要先执行一次 setcap。
2. **尽量匹配 Switch 2 的 5ms，不支持时回退到标准参数**（§1.3）。Linux 通用回退请求 7.5ms；Windows 用 ThroughputOptimized。两者都不把请求成功等同于实际间隔，参数优化失败不阻止正常连接。
3. **中断 IN 事件驱动、空闲时 4ms 保活；motion 时间戳改用真实时间**（§1.1、§5.3）。
4. **SCHED_FIFO / MMCSS 尽力提升线程优先级**（§1.1 第 6 条）。
5. 第 1 版就列出的行为改动：未知请求 STALL、bulk IN 挂起、UNLINK 语义、断线清空输入、重连后重放 LED、`--pair-host` 只配对一次、指数退避、内核断开即退出、release 资产改名。
6. **本轮交付为 spec**。此前提前生成的 workspace 骨架、部分协议代码和 fixtures 保留原状；后续明确开始实现时，按本版本一次替换 C# / Go / VIIPER，不做临时兼容层。Windows 验收要求仍为编译和测试通过；Linux 硬件验收单独记录实测结果。
