# Kindle Dashboard · KPW4 原型

此版本从原生书库启动 Slint，拉取并渲染局域网服务端发布的信息页。配置过 RTC 和供电属性的设备可以启用面板内自动休眠。完整设计见 [v0.1 规格](client-state-spec.md)。

## 许可证

- Kindle 客户端的原创代码和 `device/` 下的设备脚本使用 [GPL-3.0-only](LICENSE)，即仅 GNU GPL 第 3 版。分发 Kindle 客户端安装包时，请提供许可证文本和该版本的完整对应源码获取方式，包括构建与安装所需的脚本。
- 独立的 Python 服务端使用 [MIT](server/LICENSE)。
- `client/vendor/slint-backend-kindle/` 保留上游的 [MIT](client/vendor/slint-backend-kindle/LICENSE-MIT) 或 [Apache-2.0](client/vendor/slint-backend-kindle/LICENSE-APACHE) 双重授权。随客户端提供的 Atkinson Hyperlegible 和 Noto Sans SC 字体保留各自的 SIL OFL 1.1 声明，见 [Atkinson Hyperlegible](client/assets/OFL.txt) 和 [Noto Sans SC](client/assets/NotoSansSC-OFL.txt)。

## 局域网信息页

服务端只需要 Python 3。在项目目录运行以下命令，把示例发布到本地，再用电脑或服务器的局域网 IPv4 启动服务。

```powershell
python server/dashboard.py publish
python server/dashboard.py serve --host 192.168.114.5 --port 8765
```

在 Kindle 打开 **设置 → 服务器连接**，输入 IP、域名或 URL，填写服务端密钥后点 **保存并连接**。已有的 `server-url` 在首次保存连接设置前继续可用。服务端支持 `--key-file` 或 `DASHBOARD_API_KEY`。启用后页面和图片都需要匹配的密钥。

迁移到另一台设备、给 agent 的完整排版规则、密钥配置及 HTTPS 域名接入见 [部署与页面编排指南](server/DEPLOY-AND-LAYOUT.md)。

编辑 [server/draft.json](server/draft.json)，图片放进 [server/assets](server/assets)，改好后再执行 `publish`。示例包含「今日概览」和「日程」，可只用文字，也可按行列混排图片和按钮。发布时会校验整个页面和图片，只有通过后才替换已发布版本。刷新按钮拉取已发布内容，切页按钮可在本地缓存的页面之间切换。

协议版本 1 由 `GET /api/v1/dashboard` 提供。根对象使用 `version`、`defaultPage` 和 `pages`，每页有 `id` 和一个 `body` 积木。容器支持 `row` 和 `column`，可设 `gap`，行还可设 `weights`。文字支持 `role`、`align` 和 `maxLines`。图片使用 `src`、`alt`、`aspect` 和 `fit`。按钮使用稳定的 `id`，支持 `refresh` 和 `openPage`。完整示例和限制见服务端源码。

Kindle 优先显示最后一次成功发布的本地缓存。服务端通过 `ETag` 返回未变更结果，图片使用内容哈希地址，已下载图片可继续复用。内容拉取失败时旧页面保持可用，图片缺失时显示替代文字。顶部状态栏由 Kindle 绘制，休眠前显示「休眠中」和最后更新时间。

示例实测的页面描述约 1.5 KB，图标约 1.2 KB。中文字体随安装包提供。首次安装后，后续页面更新只传页面描述和客户端缺少的图片。

2026-11-19 在 KPW4 上验证了局域网直连、中文图文分栏、切页、刷新及离线缓存。重新发布单页内容，只改变该页的最后更新时间。RTC 唤醒时网络可能仍在恢复，客户端在单次刷新时限内重试，真机唤醒后成功请求到服务端并收到 `304`。

## 设置页

- **前光亮度**读取当前档位，使用两侧按钮逐档调节。KPW4 的范围为 0–24。
- **Wi-Fi** 显示开关、连接状态与 IPv4 地址，可切换 Wi-Fi，也可让 Kindle 重连已保存的网络。
- **休眠与唤醒**支持开关自动休眠。点 **闲置休眠**可分别输入分钟和秒，支持 15 秒到 99 分 59 秒。唤醒计划可选 15 / 30 / 60 分钟、本地整点或自定义 Cron。可开启本地时间 00:00–07:00 免打扰，这段时间内的 RTC 唤醒顺延到 07:00。电池供电时也可点 **立即休眠**。
- 信息页的 **更新** 位于 **设置** 旁边，点击后立即由 Kindle 拉取。电池模式下从休眠恢复后自动拉取，插电保持清醒时按唤醒计划到点拉取。

### 自定义唤醒时间

在 **设置 → 休眠与唤醒 → 自定义 Cron** 中输入规则，点 **预览** 查看按 Kindle 本地时间计算的接下来三次唤醒，再点 **保存计划**。规则有五个字段，依次是 `分钟 小时 日 月 星期`。支持数字、`*`、逗号列表、范围和步长。星期日可以写 `0` 或 `7`。

| 表达式 | 效果 |
| --- | --- |
| `10 * * * *` | 每小时第 10 分钟 |
| `10,40 * * * *` | 每小时第 10 和第 40 分钟 |
| `10 8-18 * * 1-5` | 周一至周五的 08:10 到 18:10，每小时一次 |
| `10 8 * * 1-5; 40 18 * * 1-5` | 周一至周五的 08:10 和 18:40 |

用 `;` 分隔最多四条规则。多条规则落在同一分钟只拉取一次。日期与星期都限定时，任一匹配即可。分钟字段的 `*/45` 匹配每小时的 `:00` 和 `:45`。启用免打扰时，预览会反映顺延到 07:00 的结果。旧版 `sleep-settings` 会继续读取，启用 Cron 后才写入新版配置。

前光和 Wi-Fi 开关只在面板会话内改变，退出时恢复原值。休眠设置保存在 Kindle 的 `/mnt/us/kindle-dashboard/sleep-settings`，修改后立即应用。没有此文件的设备沿用旧版 `auto-sleep` 和 `wake-seconds` 配置。

2026-11-19 在 KPW4 固件 5.18.1 上验证了前光调节、Wi-Fi 开关和重连、退出恢复及休眠设置持久化。整点和免打扰使用 Kindle 的本地时间接口，凌晨启用后设置页显示 07:00 唤醒。以 45 秒验证周期分别测试「立即休眠」及随后的一轮自动休眠，两轮均在 RTC 目标时刻唤醒。退出后，原生 `powerd` 进程未重启，RTC 闹钟与面板会话均已清理。测试配置已移除。

## 构建与复制

安装 Rust 1.92+、Zig、`cargo-zigbuild` 和 Python 3，再运行下列命令。Windows 的 GNU 主机工具链还需要能链接主机侧 Rust 构建脚本的 MinGW 工具链。Windows 上可用 `python` 代替 `python3`。

```sh
rustup target add armv7-unknown-linux-musleabihf
cargo zigbuild --manifest-path client/Cargo.toml --locked --release --target armv7-unknown-linux-musleabihf
python3 tools/package.py
```

打包脚本只用 Python 标准库，会校验 ARMv7 硬浮点静态可执行文件，生成 `dist/kpw4-dashboard.zip` 并输出 SHA-256。可用 `--binary` 和 `--output-dir` 指定其他路径。

本次 Windows 构建使用 Rust 1.100.0 的 GNU 主机工具链、MinGW GCC 和 Rust 随附的 `rust-lld`。HTTPS 依赖含有 C 源码，也需要能编译 ARMv7 musl 的 C 编译器和归档工具。如果 Zig 在 Windows 上无法链接，可在工具均已加入 PATH 的 PowerShell 中使用以下命令。将 `CC`、`AR` 的值换成对应的交叉工具。

```powershell
$sysroot = rustc --print sysroot
$hostTriple = ((rustc -vV | Select-String '^host: ').Line -split ' ')[1]
$env:LIBRARY_PATH = Join-Path $sysroot "lib/rustlib/$hostTriple/lib/self-contained"
$env:CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER = Join-Path $sysroot "lib/rustlib/$hostTriple/bin/rust-lld.exe"
$env:CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_RUSTFLAGS = '-C linker-flavor=ld.lld -C link-self-contained=yes'
$env:CC_armv7_unknown_linux_musleabihf = 'arm-linux-musleabihf-gcc'
$env:AR_armv7_unknown_linux_musleabihf = 'arm-linux-musleabihf-ar'
$compat = (New-Item -ItemType Directory -Force 'client/target/musl-compat').FullName
ar cr "$compat/libdl.a"
cargo rustc --manifest-path client/Cargo.toml --locked --release --target armv7-unknown-linux-musleabihf -- -L "native=$compat"
python tools/package.py
```

musl 的 `dl*` 符号在 `libc` 中，空的 `libdl.a` 用于解析依赖声明里的 `-ldl`。

将 `dist/kpw4-dashboard.zip` 中的 `documents/` 和 `kindle-dashboard/` 合并复制到 Kindle USB 存储的根目录。弹出 USB 存储后拔线，在原生书库中打开 **Kindle Dashboard**。Scriptlet 支持依赖固件上的脚本集成。面板固有文字使用随包分发的 Atkinson Hyperlegible 字体。

KPW4 5.18.1 保留原生书库进程运行。启动器短暂暂停窗口管理器，保存原生帧缓冲，并让 Slint 后端独占触摸。退出或崩溃后恢复画面、窗口管理器和原生弹窗，再确认书库在前台且没有模态弹窗。启动器在 `/var/tmp` 或 `/dev/shm` 中选择可执行的 tmpfs，并在那里保存程序副本和恢复记录。本阶段请避免在应用运行期间插电脑 USB 数据线。

## 首轮真机验收

1. 先准备不依赖面板触屏的恢复通道，例如 Wi-Fi SSH。记下固件版本。确认 `initctl status framework`（或确认的替代服务）及 `lipc-get-prop com.lab126.powerd preventScreenSaver` 的输出。
2. 从书库打开 **Kindle Dashboard**。确认信息页和顶部状态栏可见，点 **设置**，点 **返回信息页**，再进设置点 **退出回书库**。确认书库恢复，能打开一本书。核对服务状态及 `preventScreenSaver` 恢复到启动前的值。
3. 再开启面板，通过 SSH 运行 `/bin/sh /mnt/us/kindle-dashboard/supervisor.sh recover`。确认它也能结束面板并恢复书库。程序启动失败、退出或心跳停止时，监督脚本应自动恢复书库。检查 `/mnt/us/kindle-dashboard/session.log` 的 `Dashboard ready`、`Restored preventScreenSaver=`、`Native library window confirmed` 和 `Dashboard session cleaned up`。若还有残留会话，可再次执行恢复命令。
4. 如果书库显示但无法操作，仍判为验收失败。实际画面与触控需人工验证。

## 第二轮真机验收 · RTC

独立探针通过 SSH 单次启动，不启动 Slint。设备在接管期间保留进入前的墨水屏画面，因此试验中可能看起来停在书库首页。试验结束后恢复原生电源设置并退出。

KPW4 固件 5.18.1 真机记录（2026-11-17）。目标节点为 `rtc0`，供电状态读取 `/sys/class/power_supply/bd71827_ac/online`，并与 `powerd` 的充电状态交叉核对。

| 场景 | 实测结果 |
| --- | --- |
| 连续三轮 RTC 自动唤醒 | 内核每轮均挂起约 45–46 秒，三轮都在目标时间后 1 秒内返回。每轮均恢复书库，原 `powerd` 和书库进程保持运行，原电源设置恢复，RTC 闹钟为空 |
| 电源键提前唤醒 | 内核记录实际按键，提前 18 秒返回。探针清除尚未到期的自有闹钟，恢复电源设置并回到书库 |
| 清醒时插拔墙充 | sysfs 与 `powerd` 都读到插电，探针拒绝挂起，未设置 RTC。拔线后两个读数都恢复为 `0` |
| 挂起时插墙充 | 内核在挂起期间检测到充电器并提前 32 秒唤醒。探针清除了尚未到期的闹钟，恢复原生电源设置并回到书库 |

完成书库验收后，从 Wi-Fi SSH 读取当前设备能力。用同一台设备分别在插墙充和纯电池时检查供电值，确认所选属性在两种情况下分别读为 `1` 和 `0`。RTC 需要有可写的 `wakealarm`、可读的 `since_epoch`，且与 `date +%s` 基本一致。如果同时有多个可用 RTC，按目标固件的实测结果指定节点。

```sh
cat /sys/power/state
date +%s
for f in /sys/class/rtc/rtc*/since_epoch /sys/class/rtc/rtc*/wakealarm /sys/class/power_supply/*/online; do
    [ -r "$f" ] || continue
    printf '%s = ' "$f"
    cat "$f"
    printf '\n'
done
```

把实际节点名 `rtc0` 或 `rtc1` 写入 `/mnt/us/kindle-dashboard/rtc-device`，把已验证的 `/sys/class/power_supply/…/online` 完整路径写入同目录的 `external-power-path`。探针的两次预检都要求读到电池模式。现有 RTC 闹钟非空时会退出并保留它。

1. 退出面板并拔掉所有 USB 线。从 Wi-Fi SSH 执行 `/bin/sh /mnt/us/kindle-dashboard/supervisor.sh launch rtc`。
2. 探针在与面板相同的原生接管条件下设置约 **45 秒**的单次 RTC 闹钟。运行时没有触屏界面。看设备是否自动返回书库，随后读取 `session.log`。超过约一分钟仍未恢复时，短按电源键，记录为手动唤醒，并检查日志和恢复结果。
3. 检查 `Armed`、`Calling mem suspend`、`Kernel returned` 及书库恢复记录。目标时刻附近返回仅记为候选通过。同设备的内核睡眠记录（如 `dmesg` 中实际出现的 suspend entry/exit）或独立功耗观测可用来确认设备确实挂起。重复测试 RTC 自动唤醒及提前按电源键唤醒各一次，确认遗留 RTC 闹钟已经清理。后者显示 `EARLY RETURN` 符合预期。若没有自动恢复或休眠证据，保持面板休眠关闭。

## 面板内自动休眠

验证硬件后，在设置页开启 **Auto sleep**。电池模式默认闲置 25 秒后显示 **Standby**，等墨水屏完成更新再请求挂起。墙充保持面板清醒，拔线后重新计算闲置时间。默认 RTC 周期为 30 分钟。

休眠策略随面板进程退出。退出时由启动器恢复接管前的 `preventScreenSaver`，只清除仍属于本次会话的 RTC 闹钟。书库出现后，再核对原生 `powerd` 仍在运行且面板会话目录已经清理。

2026-11-18 在 KPW4 固件 5.18.1 上，以 45 秒周期实测同一面板会话。电池供电时连续三轮挂起并在 RTC 目标时刻唤醒。第四轮插入墙充后提前 5 秒恢复，面板切换到插电模式。用户从设置退出后，原生屏保可正常显示，电源键可回到书库。设备端确认原生 `powerd` 进程未重启，`preventScreenSaver` 恢复为 `0`，RTC 与面板会话均已清理。测试配置已移除，下次启动使用默认 30 分钟周期。

初版写入 `wakeup_count` 后，Wi-Fi 驱动在挂起过程中产生的事件会使内核拒绝挂起。修正版保留 RTC 读回校验和挂起前的供电检查，沿用已通过探针验证的挂起入口。

上游后端把 PW4 列为支持机型。当前固件所用的服务、RTC、供电节点及 USB 行为以真机验收为准。[后端文档](https://github.com/sverrejb/slint-kindle-backend) · [Scriptlet 文档](https://kindlemodding.org/kindle-dev/scriptlets.html)
