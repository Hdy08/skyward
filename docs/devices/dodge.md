# OnePlus 13（dodge）本地基底适配

此适配使用用户提供且已验证能解密 Data 的
`OrangeFox-R11.3-dodge.img`。2026-11-14，Skyward 已在本机启动；机主确认显示、
触摸正常，并在手机上输入锁屏密码成功解密 Data、浏览内部存储。直接播放三组
双振动也已由机主确认。将修正固化进镜像、再次刷写 `recovery_a` 并重新启动后，
机主再次确认显示、触摸、触摸振动和 Data 解密正常。MTP 实际上传、下载通过，
ADB 同时在线。WLAN 扫描、连接、DHCP 和联网探测已通过实机检查；OnePlus
OTA 已接入安装系统使用的组件协议，官方完整包接入原有下载、断点续传、校验和
A/B 安装流程。
2026-11-15，集成版重新刷入并启动后，机主确认 WLAN 首次开启成功、OTA 版本
显示正确，触摸、振动和 Data 解密再次通过。
同日更新安装版本显示与 WLAN 后台准备后，机主确认 WLAN 开启明显变快，
安装确认页显示 `PJZ110_16.0.10.501(CN01)`。
2026-11-15，机主另行完成启动分区备份、启动分区恢复和内核刷入验证，均未发现异常。

最终镜像 `Skyward-0.3.1-dodge-ota-wifi-fast.img` 大小为 104857600 字节，SHA-256：

```text
a9a76312302705fc1ade768532cd345dd6a25a6c912debbed92254232128a9a8
```

## 基底与设备记录

| 项目 | 记录 |
| --- | --- |
| 基底 SHA-256 | `0c3f2fa01bf53f648ceb13e2ad19f7b9e10b68ca61f10cafbad10f1cd835afce` |
| recovery 分区 | 104857600 字节，Android boot header v4，无内核 |
| 实机型号与系统 | PJZ110，`PJZ110_16.0.5.703(CN01)`，Android 16 |
| 系统安全补丁 | 2026-03-01 |
| 屏幕 | DSI，1440 × 3168；实机报告 60/90/120 Hz 模式 |
| 本机触摸驱动 | `oplus_bsp_synaptics_tcm2`，输入设备 `touchpanel` |
| Gatekeeper | AIDL v1；通过安装系统的 `init.qti.keymaster.sh` 选择 QTI/SPU 实现 |
| Weaver | NXP，64 槽，16 字节密钥、16 字节值；通过 OMAPI 使用 eSE1 |
| firmware / persist | 按安装系统 fstab 的路径挂载 modem 和 persist，modem 去掉不兼容的 SELinux context |
| 振动 | `qcom-hv-haptics`，自定义 FIFO 波形；`t_lra_us` 为 7692 µs，约 130 Hz |
| WLAN | Peach 2.0，PCI 设备 `0x110e`，`qca_cld3_peach_v2`；接口 `wlan0` |
| OTA 通道 | 中国区 OnePlus，NV `10010111`（`NV97`），`ALLNET`；`/update/v6` |

在 Skyward 中，QSEE、SSGTZ、HLOSMink、KeyMint、Gatekeeper、eSE、OMAPI、
NXP Weaver 和 boot control 已实际启动。OMAPI 连接到 eSE1，安全服务完成共享
签名密钥协商，Gatekeeper 接口版本和 Weaver 配置均可读取，随后实际 Data 解密成功。

## 适配方式

`devices/dodge.toml` 的 `[recovery]` 精确固定本地基底的格式和散列值。
`skyward-pack recovery --base` 会校验完整文件，拒绝替代基底。
保留基底的 init、adbd、SELinux 策略和运行库，替换 recovery 程序，移除 OrangeFox
资源和会自动启动 HAL 的 init 定义。安装系统的安全服务由 Skyward 挂载分区并
采用其系统版本和安全补丁后启动。

基底的 libbinder 使用 `SYST` 通信头，打包时写入
`ro.skyward.binder.header=SYST`。其他基底继续使用默认的 `RECO`。
OnePlus 的 NXP Weaver 文件名以 `.nxp` 结束。Gatekeeper 的运行时选择属性需要在
执行原厂脚本后重新读取。系统版本号来自 `my_manifest/build.prop`。

Android 16 的 vendor 程序需要较新的 libc++ 符号。Skyward 在启动这些程序前，
将安装系统的 `/vendor/lib64/libc++.so` 原子复制到 RAM 中的 `/system/lib64`；
基底的 Binder 运行库继续使用。原厂命名脚本的 `/vendor/bin/sh` 会重置 PATH，
因此这些脚本改由 recovery 的 `/system/bin/sh` 执行。忽略被 `//` 注释掉的 init
定义，避免 Oplus 固件中的停用片段使整个读取过程失败。

显示正常亮度上限从 Oplus 的 `oplus,dsi-brightness-normal-max-level` 读取，
本机为 3515。USB 初始化清除基底的 `sys.usb.config_disable`，让文件传输切换后
ADB 能由 init 重新启动。振动读取 `t_lra_us`，以 `period-us` 转换为频率；
不使用尚未恢复校准时返回 0、且单位不同于 Xiaomi 的 `lra_frequency_hz`。

模块列表覆盖基底支持的触摸硬件变体，缺失或探测失败的可选模块会记入日志，
依赖关系来自相应来源的 `modules.dep`。加载后触发 ADSP 启动。
GitHub Actions 的下载构建只列出含 `[stock]` 的机型；本机型需要本地基底。

WLAN 配置使用 `peach_v2/WCNSS_qcom_cfg.ini`。模块加载器允许依赖分布在不同
分区：本机 `cfg80211` 依赖的 `rfkill.ko` 在 `/system_dlkm/lib/modules`，
支持这个路径和其他机型的 `flatten/lib/modules`。芯片首次加载需约 22 秒：
本机日志中硬件冷启动校准约 8.3 秒，随后等待 Recovery 中不可用的基带 DMS
MAC 服务约 11.2 秒。`prepare_on_boot = true` 让 WLAN 工作线程在启动时提前
加载驱动、准备固件并完成这次初始化，与菜单和 Data 解密并行；准备完成后
保持 `wlan0` 关闭，不启动 supplicant、不扫描或连接网络。用户开启 WLAN 时
复用已准备的驱动。后台准备失败会记录日志，用户开启时仍可重试；其他机型
默认保留按需初始化。刚进入 Recovery 就立即开启 WLAN，仍可能等待后台准备
剩余的时间，硬件校准和驱动自身的 DMS 重试仍照常执行。创建网络接口的等待
上限保持 60 秒。日志记录后台准备、用户开启、就绪和首次扫描结果的时间，便于
比较实际等待。联网探测地址取自本机中国区
NetworkStack overlay，使用 `conn-service-cn-05.allawntech.com/generate204`。

`[update] provider = "oplus"` 启用 OnePlus 查询；未指定 provider 的已有 Xiaomi
配置继续使用原协议。查询时重新读取安装系统的属性、组件版本以及
`my_stock/etc/extension/com.oplus.app-features.xml` 中的服务器、公钥和公钥版本，
核对配置的服务器与当前系统地区一致。使用每次新生成的 AES-256-CTR 会话密钥和
IV，按安装更新器的 RSA-OAEP 协议交换密钥。设备 ID 使用更新器在 OpenID
不可用时的全零默认值，不读取账号、IMEI 或 Android 的私有标识。

只识别单个 `my_manifest` 的完整聚合 OTA 包，拒绝将增量、拆分组件或其他型号
的响应作为完整系统包。查询失败会显示错误，不会当成“暂无更新”。下载时先从
同地区服务器的 `/ts` 读取时间戳，附带原厂更新器使用的设备配置请求头，通过
`GET /downloadCheck` 和 `Range: bytes=0-0` 获取 HTTPS 官方 CDN 地址，再交给
原有下载器。未附带这些请求头的请求会返回 `responseCode=2306`；此前将其解释
为 Recovery 无法获得下载授权，是不准确的。设备配置请求头只发给同地区的 OTA
服务器，不转发到 CDN。下载保存在 `/sdcard/downloaded_rom`，支持暂停、断点
续传、过期地址刷新和整包 MD5 校验。

安装确认页和安装进度页优先显示包内 `META-INF/com/android/metadata` 的
`version_name_show`，其次读取 `version_name`，最后回退到
`post-build-incremental`。本机官方包提供的显示版本为
`PJZ110_16.0.10.501(CN01)`，内部构建号为 `V.3205195-ced00f-d0523d`。
显示版本直接来自待安装包，离线打开本地文件也适用，不依赖在线查询结果；
设备、时间戳、安全补丁、签名和是否清除 Data 的检查继续使用原有元数据。

OnePlus 的官方包使用 `pre-device=OP5D0DL1`，而本地 Recovery 使用 `dodge`。
安装器从已安装系统读取准确的 `ro.product.device`；没有安装系统时才使用
Recovery 的产品名。其他设备、增量包、系统时间戳和安全补丁降级检查继续执行。
旧基底携带的证书缺少本机当前的 `online_OTA` 公共证书，因此 `[recovery]
ota_certificates` 指向 `/mnt/my_engineering/etc/security/otacerts.zip`。启动时先
解析并校验该 ZIP 中的公共证书，再原子复制到 RAM 中的
`/system/etc/security/otacerts.zip`，供现有签名检查和 update_engine 使用。
不读取私钥，不关闭签名验证；源文件不可读或无效时保留基底证书并记录错误。
安装仍经过现有的检查、确认页和 `update_engine_sideload`，下载完成不自动安装。

## Windows 构建

需要 Rust（版本由 `rust-toolchain.toml` 固定）、Android ARM64 target、Android
NDK 和 `lz4.exe`。本次使用 NDK `28.2.13676358`；仓库 CI 使用 NDK r29。
在仓库根目录的 PowerShell 执行：

```powershell
rustup target add aarch64-linux-android
.\scripts\build-dodge.ps1 `
  -Base 'D:\Flash\dodge_OnePlus13\Rec\OrangeFox-R11.3-dodge.img' `
  -Ndk 'C:\Android\Sdk\ndk\28.2.13676358' `
  -Lz4 'D:\Documents\Code\skyward\target\dodge\tools\lz4.exe'
```

`-Lz4` 可以指向已安装的另一份 `lz4.exe`，在 PATH 中时可省略。
脚本构建 ARM64 recovery、静态链接的 system-init 和 Windows 打包器，输出：

```text
target/dodge/skyward-dodge-experimental.img
target/dodge/skyward-dodge-experimental.img.sha256
```

脚本只在电脑生成文件。重复构建前请保留要比较的镜像与散列记录。

## 设备验证

先从当前 recovery 保存实际的 recovery_a、recovery_b 和当前槽位 boot 分区，
记录各自的散列；当前已安装的镜像可能和提供的基底有自定义资源差异。
首次检查包括：ADB、DSI 显示、触摸坐标、模块加载、modem/persist 挂载、
KeyMint/Gatekeeper/eSE/Weaver 的注册，然后在手机上输入锁屏密码验证 Data。
密码只在手机本地输入。

集成镜像验证结果：

- 新启动无需 RAM 手动修正：约 14 秒内安全服务已就绪，界面启动，电量可读取。
- 机主确认显示、触摸、触摸振动正常；重新输入密码解密 Data 成功。
- Windows 识别 OnePlus 13 内部存储。192 KiB 随机测试文件通过 MTP 上传并下载，
  两次散列均与源文件一致，ADB 全程在线，手机测试文件随后删除。
- WLAN 已扫描并连接到密码保护的网络，DHCP 获取地址，HTTP 204 联网探测通过。
- 电脑端按当前手机固件查询官方 OTA 成功，服务返回
  `PJZ110_16.0.10.501(CN01)`、OTA 版本 `PJZ110_11.C.93_1930_202608031342`，
  完整包大小 8975091784 字节，MD5 `6bd2956979795c267a062462ad7fc50f`。
  这是该机型、地区和已安装版本当前获推送的结果，未下载或刷入系统包。
- 2026-11-15 的集成版实机查询返回相同版本和精确大小，日志与机主的界面反馈
  一致。首次开启 WLAN 成功，机主重新核对触摸、振动和 Data 解密通过。
- 回读分区散列：`recovery_a` 与最终镜像一致；`recovery_b`、`boot_a` 与原始
  备份一致。没有切槽、格式化 Data 或刷写其他分区。
- 74 项相关单元与回归测试通过；Windows 打包器与 Android recovery 的 Clippy
  检查通过。另核对镜像的 AVB 散列、嵌入二进制、设备配置、静态 system-init
  以及原始备份散列。
- WLAN/OTA 集成另通过 43 项 Windows 配置、模块、打包与设备回归测试，
  46 项直接在手机运行的网络测试（含 6 项 OnePlus 协议测试），以及 3 项更新
  界面测试。Android recovery 的 Clippy（`-D warnings`）通过。
- 2026-11-15 的完整 OTA 适配版本在手机上直接从官方 CDN 下载
  `PJZ110_11.C.93_1930_202608031342_all.zip`，在约 32 MiB 处暂停并继续，
  532 秒下载完成，大小为 8975091784 字节，整包 MD5 与官方值一致。
  使用已安装系统的公共证书验证整包签名为 `Signed`，耗时 63 秒。
  解析出未压缩 A/B payload，偏移 61、长度 8975086380；产品名、时间戳和安全
  补丁检查通过，`wipe=false`。包内版本为 `V.3205195-ced00f-d0523d`，对应查询
  显示的 `PJZ110_16.0.10.501(CN01)`。
- 完整 OTA 版本通过 218 项相关测试：43 项 Windows 配置、模块和打包测试，
  48 项 Android 网络测试，124 项 Android 安装器测试，以及 3 项更新界面测试。
  Android Clippy（`-D warnings`）通过；镜像 AVB、嵌入配置和二进制检查通过。
  冷启动时从已安装系统加载了 2 个 OTA 公共证书，RAM 副本与源文件 SHA-256
  相同。`recovery_a` 回读散列与当时的完整 OTA 镜像一致，`recovery_b` 和 `boot_a`
  与原始备份相同。
- 机主在新启动的完整 OTA 版本中解密 Data、连接 WLAN，更新页面识别已下载的
  完整包，点按“安装”后成功进入安装确认页。机主确认尚未开始安装。实际系统
  升级保持待确认状态，本次未执行 update_engine 写入、切槽或清除数据。
- 安装版本显示与 WLAN 优化版本再次通过 171 项测试：43 项 Windows 配置、
  模块和打包测试，125 项手机上的安装器测试（包括完整包显示版本的优先顺序和
  空值回退），3 项更新界面测试。Android Clippy（`-D warnings`）通过。
  该次冷启动在 13.906683 秒开始后台准备驱动，35.653822 秒完成，耗时
  21747 毫秒；准备完成时接口标志 `0x1002`，未启动 supplicant。
  73.009928 秒收到开启请求，73.138833 秒可扫描（128 毫秒），
  74.211781 秒返回 3 个网络（距开启约 1.202 秒）。联网探测通过，查询仍返回
  `PJZ110_16.0.10.501(CN01)`。107.079188 秒安装器从实际完整包读出同一显示
  版本，机主确认界面正确且尚未开始安装。最终镜像 AVB、嵌入配置和二进制
  检查通过，`recovery_a` 回读与最终镜像一致，另两份原始分区备份仍匹配。
- 2026-11-15，机主确认启动分区备份、启动分区恢复及内核刷入均已实机验证，
  未发现异常。此项结果来自机主反馈；上述 boot 分区散列为此前制作和验证
  Recovery 镜像时的记录，不代表内核刷入后的分区内容。
- 提交前通过 Android 全工作区、全部 targets 的 Clippy（`--locked --workspace
  --all-targets --target aarch64-linux-android -- -D warnings`），包括新增测试代码。

测试范围为表中这台 PJZ110 及其固件。启动分区的备份与恢复已由机主验证；
完整系统刷写、Data 备份与还原、其他触摸硬件变体和其他地区固件尚未进行实机测试。

此 recovery 镜像无内核。本次曾使用当前 `boot_a` 的内核合成临时启动镜像，
bootloader 接受 `fastboot boot` 后却进入正常 Android，未使用 Skyward ramdisk。
因此本机未验证出可用的临时启动方式。机主随后明确授权直接刷分区，适配过程中
由电脑执行的 Recovery 镜像刷写均指向当前槽位 `recovery_a`：

```powershell
adb reboot bootloader
fastboot getvar current-slot
fastboot flash recovery_a target\dodge\skyward-dodge-experimental.img
fastboot reboot recovery
```

这些刷写命令是本次已授权测试的记录。其他设备应先核对槽位并保存自己的备份。
本机 `recovery_a`、`recovery_b`、`boot_a` 的原始备份保存在
`target/dodge/device/backup/`，散列记录在同目录的 `manifest.json`。
恢复当前槽位时使用该槽位的实际备份，例如：

```powershell
fastboot flash recovery_a target\dodge\device\backup\recovery_a.img
fastboot reboot recovery
```

测试日志应保存在电脑的 `target/dodge/device/` 下。日志可能含设备标识，提交代码
时只保留最小回归用例；该目录和镜像均由仓库的 target 忽略规则排除。
