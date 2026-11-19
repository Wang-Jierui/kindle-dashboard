# Kindle Dashboard 部署与页面编排指南

这份指南可直接交给负责在另一台设备上运行服务或生成页面的 agent。服务端用 Python 3 标准库，支持 Linux、macOS 和 Windows。Kindle 只读取已经发布的页面和图片。

## 1. 在另一台设备上部署

复制项目里的 `server/` 目录到目标设备，保留 `dashboard.py`、`draft.json` 和 `assets/`。在可持久保存文件的位置启动服务。以下命令假设当前目录下有 `server/`。Windows 可将 `python3` 换成 `python`。

```sh
python3 server/dashboard.py publish
python3 server/dashboard.py serve --host 192.168.1.20 --port 8765
```

把 `192.168.1.20` 换成服务设备的实际局域网 IPv4。局域网直连时放通这一地址的 TCP 8765。只供同机反向代理或隧道读取时，绑定 `127.0.0.1`。服务默认读取和写入 `server/published/`，可用 `--state` 改为独立目录。指定时，发布和启动应使用同一个目录，选项放在子命令前。

```sh
python3 server/dashboard.py --state ./dashboard-state publish
python3 server/dashboard.py --state ./dashboard-state serve --host 127.0.0.1 --port 8765
```

需要长期运行时，可由目标设备的服务管理器托管上述 `serve` 命令。发布命令可独立反复运行，服务会读取最新的已发布版本。

### 启用服务密钥

生成一次随机密钥，在 POSIX 系统上可以这样保存：

```sh
umask 077
mkdir -p .secrets
python3 -c 'import secrets; print(secrets.token_urlsafe(24))' > .secrets/dashboard.key
python3 server/dashboard.py serve --host 127.0.0.1 --port 8765 \
  --key-file .secrets/dashboard.key --require-key
```

在 Windows PowerShell 中可以这样生成密钥并启动：

```powershell
New-Item -ItemType Directory -Force .secrets | Out-Null
python -c 'import secrets; print(secrets.token_urlsafe(24))' |
    Set-Content -Encoding ascii .secrets/dashboard.key
python server/dashboard.py serve --host 127.0.0.1 --port 8765 --key-file .secrets/dashboard.key --require-key
```

生成密钥的命令只运行一次。后续重启时直接使用已有密钥文件。使用 Kindle 局域网直连时，把上述启动命令的 `127.0.0.1` 换成服务设备的局域网 IP。

`--key-file` 优先于 `DASHBOARD_API_KEY` 环境变量。`--require-key` 使服务在缺少密钥时拒绝启动。启用后，页面接口和图片接口都检查 `Authorization: Bearer <密钥>`，还会先认证再考虑返回 `304`。缺少或错填密钥返回 `401`。更换密钥后重启服务，并更新 Kindle 中的密钥。有效字符为英文字母、数字及 `.`、`_`、`~`、`-`，最多 128 位。

密钥只留在连接配置或服务进程环境，页面和图片清单里只写公开的显示字段。HTTP 直连的密钥会在局域网明文传输。使用域名或跨不可信网络访问时，请使用 HTTPS。Kindle 的密钥字段默认遮住，连接配置保存在设备存储中，应和设备本身一起保管。

### 在 Kindle 上连接

打开 **设置 → 服务器连接**，填地址和密钥，点 **保存并连接**。地址示例为 `192.168.1.20:8765`、`http://nas.lan:8765` 或 `https://kindle.example.com`。省略协议时，IP 和 `localhost` 默认用 HTTP，域名默认用 HTTPS。已保存的地址和密钥即时生效，换地址后会加载该地址对应的页面。启用密钥的服务器要求填写原密钥，开放的局域网服务可留空。

批量配置时，可在 Kindle 的 `/mnt/us/kindle-dashboard/connection.json` 写入以下结构，再打开面板。旧版 `server-url` 在没有 `connection.json` 时仍可用。

```json
{
  "url": "https://kindle.example.com",
  "key": "replace_with_your_own_key"
}
```

## 2. 页面结构

修改 `server/draft.json`。顶层对象包含 `version: 1`、`defaultPage` 和 `pages`。每页有稳定且唯一的 `id` 和一个 `body` 积木。`body` 可以是一段文字，也可以是包含多种子积木的容器。顶部时间、电量、充电状态、最后更新时间及设置入口由 Kindle 绘制。

这个例子用左右分栏显示图文，下面加两个按钮。第二页只使用文字，展示两种正文可以在同一面板中共存。

```json
{
  "version": 1,
  "defaultPage": "today",
  "pages": [
    {
      "id": "today",
      "body": {
        "type": "column",
        "gap": 28,
        "children": [
          { "type": "text", "role": "title", "text": "今日概览" },
          {
            "type": "row",
            "gap": 20,
            "weights": [1, 2],
            "children": [
              { "type": "image", "src": "weather.png", "aspect": 1.3, "alt": "晴间多云" },
              {
                "type": "column",
                "gap": 8,
                "children": [
                  { "type": "text", "role": "value", "text": "22°C" },
                  { "type": "text", "role": "body", "text": "下午有短阵雨", "maxLines": 2 }
                ]
              }
            ]
          },
          {
            "type": "row",
            "children": [
              {
                "type": "button",
                "id": "refresh-weather",
                "label": "刷新",
                "style": "primary",
                "action": { "type": "refresh" }
              },
              {
                "type": "button",
                "id": "open-agenda",
                "label": "查看日程",
                "action": { "type": "openPage", "page": "agenda" }
              }
            ]
          }
        ]
      }
    },
    {
      "id": "agenda",
      "body": {
        "type": "text",
        "role": "body",
        "text": "今日日程\n10:00 周会\n14:30 整理阅读清单",
        "maxLines": 3
      }
    }
  ]
}
```

如果第二页还需要返回入口，可以把它的 `body` 改成 `column`，放入原文字和 `action: { "type": "openPage", "page": "today" }` 的按钮。正式页面建议给每个可切入的辅助页提供返回按钮。

### 容器

| 字段 | 取值与作用 |
| --- | --- |
| `type` | `column` 从上到下排列，`row` 从左到右分栏 |
| `children` | 子积木数组。列有 1–12 项，行有 1–4 项 |
| `gap` | 可选，整数 0–48，默认 16 |
| `weights` | 仅用于行。每项 1–10 的整数，数量等于子项数。省略时等宽 |

例如 `weights: [1, 2]` 把可用宽度按 1:2 分。列放在行里可以做「左图、右侧多段文字」。分栏过窄时，Kindle 会将该行的子项改成从上到下排列。先用浅层容器、短段落和较少的分栏，让按键和关键数据容易看见。

### 文字

`text` 是普通 UTF-8 字符串，1–600 个字符。用 JSON 转义 `\n` 显式换行。`align` 可用 `left`（默认）、`center` 或 `right`。可选 `maxLines` 为 1–6，超过上限的文字会截断。

| `role` | 用途 | 默认行数 |
| --- | --- | ---: |
| `title` | 页面标题 | 1 |
| `heading` | 区块标题 | 1 |
| `value` | 温度、价格等重点数值 | 1 |
| `body` | 正文，省略角色时使用 | 3 |
| `caption` | 来源或补充说明 | 2 |

字号和字体由客户端角色决定。要突出一段内容时，可以把它拆成对应角色的独立文字积木。生成当前时间、电量和「更新于」字段的工作已由客户端处理。

### 图片

把图片放在 `server/assets/`。编辑稿的 `src` 写文件名，例如 `weather.png`。文件名使用英文字母、数字、下划线或连字符，加小写 `.png`、`.jpg` 或 `.jpeg`。单图最大 **800 × 800 像素、192 KiB**，整个面板最多 6 张不同图片。优先使用大小合适、黑白对比清晰的图。

- `alt` 必填，1–100 字。图片丢失或加载失败时 Kindle 会显示这段文字。
- `aspect` 可选，是占位框宽高比，范围 0.5–4，默认 1。
- `fit` 可选，`contain` 默认完整显示，`cover` 填满并裁切。

发布时程序会检查大小并生成 `/assets/<内容哈希>.png` 或 `.jpg`。agent 继续在编辑稿中引用原文件名即可，图片改变后会得到新地址，旧的内容哈希仍可用来重用缓存。

### 按钮

按钮的 `id` 在同一页内唯一，标签长度为 1–30 字。样式 `primary` 为实底，`secondary` 为描边且是默认值。尺寸可设 `normal` 或 `large`，最小触控高度由客户端控制。

| `action` | 效果 |
| --- | --- |
| `{"type":"refresh"}` | 重新拉取已发布版，按钮显示刷新反馈 |
| `{"type":"openPage","page":"agenda"}` | 打开同一份面板中 `id` 为 `agenda` 的页，可从缓存切换 |

按钮动作通过这两个枚举选择。多个按钮可组成一行。设定页面跳转后检查目标页存在，通常也需要能切回默认页。

### 整体限制

- 每份面板 1–6 页，共至多 80 个积木，含容器和子项。
- 最多 6 层，`body` 算第 1 层。
- 页面 ID 和按钮 ID 允许 1–40 个英数字、下划线、连字符。
- 草稿和发布后的 JSON 各最多 64 KiB。静态图片另行下载和缓存。

## 3. 给 agent 的发布流程

1. 从现有 `draft.json` 保留页面 ID 和按钮 ID，只改需要变化的文本、布局或图片。持续生成内容时，避免把每次生成时间写进正文，否则内容未变也会被当作新版。
2. 使用 UTF-8 写入草稿文件，把新图片写入 assets 目录。需要指定输入路径时，用 `python3 server/dashboard.py publish --draft <草稿> --assets <图片目录>`。
3. 等发布命令成功。校验失败时修好草稿再发布。运行中的服务继续提供上次通过校验的版本。
4. 确认 `GET /api/v1/dashboard` 返回 `200`，有 `ETag`，页面数和默认页正确。启用密钥时，可用下面的命令读接口。通过 stdin 把认证头交给 curl，避免把密钥放进参数列表。

```sh
printf 'Authorization: Bearer %s\n' "$(cat .secrets/dashboard.key)" |
  curl -fsS -H @- http://127.0.0.1:8765/api/v1/dashboard
```

5. 从 Kindle 点「刷新」看页面。重复请求同一版本时会收到 `304`，页面最后更新时间保持原值。网络故障、认证失败或新内容校验失败时，Kindle 可继续显示同一地址之前已经缓存的内容。

## 4. 使用域名

**局域网域名**可以在路由器、本地 DNS 或其他设备的 DNS 服务里添加地址记录，使名字解析到运行服务的局域网 IP。用 HTTP 时，在 Kindle 显式填 `http://nas.lan:8765`。如果用 HTTPS，证书要与域名匹配，并由客户端信任的 CA 签发。

**Cloudflare Tunnel** 可以把独立子域名（如 `kindle.example.com`）映射到部署机器上的 `http://127.0.0.1:8765`。如果 `cloudflared` 在容器或另一台机器上，把目标改成它实际能访问的源站地址。通过 Cloudflare 管理面板添加公共主机名路由时，会创建指向隧道的 DNS 记录。Kindle 里填 `https://kindle.example.com`，源站用 `--require-key` 启动。完整路由包含页面和图片路径。[Cloudflare Tunnel 路由文档](https://developers.cloudflare.com/tunnel/concepts/routing/)

客户端现在使用应用的 Bearer 密钥。如果这个域名另外启用了需要交互登录的 Cloudflare Access，Kindle 会在到达应用前被拦住。采用 Access 服务令牌需要另外支持它的请求头。为认证内容配置代理缓存时应遵守源站的 `Cache-Control: private`，并避免给此域名套用强制共享缓存规则。[Cloudflare Access 服务令牌](https://developers.cloudflare.com/cloudflare-one/access-controls/service-credentials/service-tokens/) · [Cloudflare 缓存控制](https://developers.cloudflare.com/cache/concepts/cache-control/)

使用自己的 Nginx、Caddy 或其他反向代理时，也可把具备公共信任证书的 HTTPS 域名转发到本地服务。确保代理保留 `Authorization` 和 `If-None-Match`。如果 Kindle 地址带子路径，例如 `https://example.com/kindle/`，代理需去掉 `/kindle/` 前缀再转发，因为 Python 服务本身在其根路径提供 `/api/v1/dashboard` 和 `/assets/`。
