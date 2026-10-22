<div align="center">

  <br>
  <img src="./resources/assets/logo.svg" width="230" alt="GlideWM logo" />
  <br>

# GlideWM

**一个受 i3wm 启发的 Windows 平铺窗口管理器。**

[![Discord invite][discord-badge]][discord-link]
[![Downloads][downloads-badge]][downloads-link]

GlideWM 让您可以通过键盘驱动的命令轻松组织窗口并即时调整其布局。

[安装](#安装) •
[新增功能](#新增功能) •
[从 GlazeWM 迁移](#从-glazewm-迁移) •
[默认快捷键](#默认快捷键) •
[配置文档](#配置文档) •
[常见问题](#常见问题) •
[贡献 ↗](CONTRIBUTING.md)

![Demo video][demo-video]

</div>

> 演示视频沿用自 GlazeWM，因此展示的是原版的平铺布局。

> [!NOTE]
> **GlideWM 是 [GlazeWM](https://github.com/glzr-io/glazewm) 的分支**，这是 Glzr Software 出品的
> Windows 平铺窗口管理器。
>
> 它最初只是个人的分支，后来逐渐长成了自己的样子：可插拔的布局系统、niri 风格的滚动布局，
> 以及基于实时合成器缩略图的动画。GlideWM 与 GlazeWM 的作者无隶属关系，也未获得其认可。
>
> GlideWM 构建于 GlazeWM 之上，因此上游能用的功能这里都能用。如果您遇到问题，请先确认它
> 在上游 GlazeWM 上是否同样存在。
>
> **GlideWM 仅支持 Windows。** 上游项目同时支持 macOS，但这个分支不构建、不打包、也不
> 支持 macOS。macOS 的代码仍然保留在仓库中（继承自上游），未作改动，以便日后合并上游
> 的变更。

## 新增功能

### 布局

GlazeWM 只有一种布局：若干分割容器共享视口。GlideWM 把它换成了可插拔的布局系统，
因此一个工作区可以有多种不同的排布方式。

- **滚动布局**（灵感来自 [niri](https://github.com/YaLTeR/niri) 和
  [PaperWM](https://github.com/paperwm/PaperWM)）。窗口沿一条轴排成一条带，视口会滚动以
  保持焦点窗口可见。新窗口插入到焦点窗口旁边，并可通过 `alt+r` 再按 `+` / `-` 在一组预设
  尺寸之间循环切换。
- **BSP 布局**（类似 [bspwm](https://github.com/baskerville/bspwm)）。窗口构成一棵二叉分割
  树；插入窗口时会把焦点窗口的空间一分为二。
- **每个工作区可选布局。** 各工作区自行选择布局，并且只覆盖需要不同的那些选项。
- **统一的全局 `layout` 配置块。** 布局选项只配置一次，所有工作区继承。配置九个布局相同
  的工作区时，不必把滚动选项重复写九遍。
- **运行时切换布局。** `set-layout` 设置工作区布局，`cycle-layout` 在可用布局之间循环。
  切换仅在本次会话中生效，工作区启动时使用哪种布局由配置文件决定。
- **布局切换行为。** 已有窗口是重新排列到新布局（`reflow`），还是保持原样（`keep`），
  现在可以配置了。

新增命令：`set-layout`、`cycle-layout`、`scroll`、`cycle-preset-size`、`center-focused`、
`toggle-tiling-direction`。

### 动画

GlazeWM 直接移动真实窗口，因此窗口动画要么缺失，要么会卡顿。GlideWM 改为动画
**实时合成器缩略图**，在动画期间把真实窗口隐藏起来。这样无论应用本身有多繁忙，动画都
能保持流畅，因为合成器只需要绘制一张缩略图。

- **工作区切换动画**：`slide`、`slide_vert` 或 `fade`，可配置方向、时长和三次贝塞尔缓动曲线。
- **窗口打开动画**：`slide`、`slide_vert`、`popin` 或 `fade`。
- **窗口移动动画**：位置或大小发生变化的窗口会平滑地移动到位，而不是突然跳过去。正在
  动画中的窗口会平滑地转向新位置，而不是重新开始动画。

动画需要 `hide_method: 'cloak'`。由外部工具负责动画的窗口（例如 Windhawk 的
"Hyprland Animations" 模组）会交由该工具处理。

### 其他

- **停靠在视口外的窗口。** 使用 `offscreen_windows: 'park'` 时，滚动出视口的窗口会被移动到
  所有显示器边缘之外，而不是被隐藏，因此它们仍会出现在 Alt+Tab 和任务栏中。在 Alt+Tab 中
  选中它，窗口就会滚动回视野内。
- **单一的布局配置。** `general.layout_switch`、`workspaces[].scroll` 和 `workspaces[].bsp`
  被整合进全局的 `layout` 配置块，各工作区通过 `layout_overrides` 覆盖。旧键仍然可用，但
  已废弃。
- **更合理的默认值。** 滚动布局、垂直轴、居中对齐，以及 `keep` 的布局切换行为都是开箱
  即用的默认设置。
- **本地安装脚本。** `resources/scripts/install-local.ps1` 会构建一个已签名的 UIAccess
  版本，将它安装、注册为开机自启，并启动它。

## 安装

GlideWM **仅支持 Windows**，通过源码构建和安装。需要 Rust 工具链，请通过
[rustup](https://rustup.rs) 安装，因为构建需要 nightly。

```sh
git clone https://github.com/JASSIM-ALHUMAID/glazewm.git
cd glazewm
./resources/scripts/install-local.ps1
```

就这些。脚本会构建 release 二进制文件，创建一个自签名代码签名证书，用它给二进制文件
签名，把它们复制到 `C:\Program Files\GlideWM`，注册 GlideWM 开机自启，然后启动它。
脚本需要管理员权限，因此会弹出一次 UAC 提示。

构建是增量的，因此在 `git pull` 之后重新运行脚本只会重新构建发生变化的部分。传入
`-SkipBuild` 可以直接安装 `target\release` 中已有的二进制文件。

`ui_access` 特性让 GlideWM 无需以管理员身份运行就能管理提权窗口（例如任务管理器）。
它需要一个*受信任的*签名，这正是自签名证书的作用。这也是脚本要把二进制文件安装到
Program Files 并在计算机上信任该证书的原因。

### 命令行工具

CLI 安装在 `C:\Program Files\GlideWM\cli\glidewm.exe`。把该目录加入 `PATH` 即可在终端
中使用：

```sh
glidewm query workspaces
glidewm command set-layout --layout scrolling --workspace 1
glidewm command scroll --direction down --amount 10%
```

## 从 GlazeWM 迁移

GlideWM 沿用了 GlazeWM 的配置格式，只做了一些重命名。如果您已经在使用 GlazeWM，
变化如下：

| GlazeWM                        | GlideWM                                  |
| ------------------------------ | ---------------------------------------- |
| `%userprofile%\.glzr\glazewm\` | `%userprofile%\.glidewm\`                |
| `GLAZEWM_CONFIG_PATH`          | `GLIDEWM_CONFIG_PATH`                    |
| `glazewm.exe`                  | `glidewm.exe`                            |
| `general.layout_switch`        | `layout.switch_behavior`                 |
| `workspaces[].scroll`          | `layout.scrolling`（+ `layout_overrides`）|
| `workspaces[].bsp`             | `layout.bsp`（+ `layout_overrides`）     |

旧键仍然可以解析：它们会被记录为已废弃，并且新键优先。但建议尽快迁移，因为旧键最终
会被移除。

## 默认快捷键

在 GlideWM 首次启动时，可以选择性地生成默认配置。

以下是所有可用命令及其默认快捷键的速查表。

![Infographic](/resources/assets/cheatsheet_cn_ZH.png)

> 速查表沿用自 GlazeWM，因此没有列出这里新增的布局与动画命令。完整列表见
> [`resources/assets/sample-config.yaml`](resources/assets/sample-config.yaml)。

## 配置文档

[默认配置](resources/assets/sample-config.yaml) 文件生成在
`%userprofile%\.glidewm\config.yaml`。

要使用不同的配置文件位置，您可以使用 CLI 参数 `--config="..."` 启动 GlideWM 可执行文件，
如下所示：

```sh
./glidewm.exe start --config="C:\<配置文件路径>\config.yaml"
```

或者为 `GLIDEWM_CONFIG_PATH` 环境变量传递一个值：

```sh
setx GLIDEWM_CONFIG_PATH "C:\<配置文件路径>\config.yaml"
```

使用自定义路径的好处是您可以为配置文件选择不同的名称，例如 `glidewm.yaml`。

### 配置：常规

```yaml
general:
  # WM 启动时运行的命令（例如运行脚本或启动另一个应用程序）。
  startup_commands: []

  # WM 关闭前运行的命令。
  shutdown_commands: []

  # WM 配置重新加载后运行的命令。
  config_reload_commands: []

  # 是否自动聚焦光标下方的窗口。
  focus_follows_cursor: false

  # 当聚焦当前工作区时，是否在先前聚焦的工作区之间来回切换。
  toggle_workspace_on_refocus: false

  cursor_jump:
    # 是否在指定触发器上自动移动光标。
    enabled: true

    # 光标跳转的触发器：
    # - 'monitor_focus': 当焦点在显示器之间切换时跳转。
    # - 'window_focus': 当焦点在窗口之间切换时跳转。
    trigger: "monitor_focus"
```

### 配置：快捷键

可用的键盘快捷键可以通过 `keybindings` 选项自定义。快捷键由一个或多个按键组合和按下时运行的一个或多个命令组成。

建议使用 alt 键作为快捷键。不幸的是，Windows 键很难重新映射，因为操作系统保留了某些快捷键（例如 `lwin+l`）。

```yaml
keybindings:
  # 要运行的命令。
  - commands: ["focus --workspace 1"]

    # 触发快捷键的按键组合。
    bindings: ["alt+1"]

  # 可以按顺序运行多个命令（例如将窗口移动到工作区 + 聚焦工作区）。
  - commands: ["move --workspace 1", "focus --workspace 1"]
    bindings: ["alt+shift+1"]
```

**可用于快捷键的完整按键列表：**

<details>
<summary>按键列表</summary>

| 按键                  | 描述                                                           |
| --------------------- | -------------------------------------------------------------- |
| `a` - `z`             | 字母键                                                         |
| `0` - `9`             | 数字键                                                         |
| `numpad0` - `numpad9` | 数字小键盘键                                                   |
| `f1` - `f24`          | 功能键                                                         |
| `shift`               | 左或右 SHIFT 键                                                |
| `lshift`              | 左 SHIFT 键                                                    |
| `rshift`              | 右 SHIFT 键                                                    |
| `control`             | 左或右 CTRL 键                                                 |
| `lctrl`               | 左 CTRL 键                                                     |
| `rctrl`               | 右 CTRL 键                                                     |
| `alt`                 | 左或右 ALT 键                                                  |
| `lalt`                | 左 ALT 键                                                      |
| `ralt`                | 右 ALT 键                                                      |
| `lwin`                | 左 ⊞ Windows 徽标键                                            |
| `rwin`                | 右 ⊞ Windows 徽标键                                            |
| `space`               | 空格键                                                         |
| `escape`              | ESCAPE 键                                                      |
| `back`                | BACKSPACE 键                                                   |
| `tab`                 | TAB 键                                                         |
| `enter`               | ENTER 键                                                       |
| `left`                | ← 方向键                                                       |
| `right`               | → 方向键                                                       |
| `up`                  | ↑ 方向键                                                       |
| `down`                | ↓ 方向键                                                       |
| `num_lock`            | NUM LOCK 键                                                    |
| `scroll_lock`         | SCROLL LOCK 键                                                 |
| `caps_lock`           | CAPS LOCK 键                                                   |
| `page_up`             | PAGE UP 键                                                     |
| `page_down`           | PAGE DOWN 键                                                   |
| `insert`              | INSERT 键                                                      |
| `delete`              | DELETE 键                                                      |
| `end`                 | END 键                                                         |
| `home`                | HOME 键                                                        |
| `print_screen`        | PRINT SCREEN 键                                                |
| `multiply`            | `*` 键（仅限数字小键盘）                                       |
| `add`                 | `+` 键（仅限数字小键盘）                                       |
| `subtract`            | `-` 键（仅限数字小键盘）                                       |
| `decimal`             | DEL 键（仅限数字小键盘）                                       |
| `divide`              | `/` 键（仅限数字小键盘）                                       |
| `volume_up`           | 音量增加键                                                     |
| `volume_down`         | 音量减少键                                                     |
| `volume_mute`         | 静音键                                                         |
| `media_next_track`    | 媒体下一曲键                                                   |
| `media_prev_track`    | 媒体上一曲键                                                   |
| `media_stop`          | 媒体停止键                                                     |
| `media_play_pause`    | 媒体播放/暂停键                                                |
| `oem_semicolon`       | 美式标准键盘上的 `;`/`:` 键（因键盘而异）                      |
| `oem_question`        | 美式标准键盘上的 `/`/`?` 键（因键盘而异）                      |
| `oem_tilde`           | 美式标准键盘上的 `` ` ``/`~` 键（因键盘而异）                  |
| `oem_open_brackets`   | 美式标准键盘上的 `[`/`{` 键（因键盘而异）                      |
| `oem_pipe`            | 美式标准键盘上的 `\`/`\|` 键（因键盘而异）                     |
| `oem_close_brackets`  | 美式标准键盘上的 `]`/`}` 键（因键盘而异）                      |
| `oem_quotes`          | 美式标准键盘上的 `'`/`"` 键（因键盘而异）                      |
| `oem_plus`            | 美式标准键盘上的 `=`/`+` 键（因键盘而异）                      |
| `oem_comma`           | 美式标准键盘上的 `,`/`<` 键（因键盘而异）                      |
| `oem_minus`           | 美式标准键盘上的 `-`/`_` 键（因键盘而异）                      |
| `oem_period`          | 美式标准键盘上的 `.`/`>` 键（因键盘而异）                      |

</details>

如果某个按键不在上述列表中，如果您在快捷键中使用其字符，它很可能仍然受支持（例如挪威语 Å 字符的 `alt+å`）。

> 德语和美式国际键盘对右侧 alt 键的处理不同。对于这些键盘布局，请使用 `ralt+ctrl` 而不是 `ralt` 来绑定右侧 alt 键。

### 配置：间隙

窗口之间的间隙可以通过配置文件中的 `gaps` 属性更改。内部和外部间隙分别设置。

```yaml
gaps:
  # 相邻窗口之间的间隙。
  inner_gap: "20px"

  # 窗口与屏幕边缘之间的间隙。
  outer_gap:
    top: "20px"
    right: "20px"
    bottom: "20px"
    left: "20px"
```

### 配置：布局

布局选项在配置文件中的 `layout` 属性里统一配置，并由所有工作区共享。下面的值即为内置默认值。

```yaml
layout:
  # 未设置自身 `layout` 的工作区所使用的布局。
  # - 'tiling': 窗口平分视口。
  # - 'scrolling': 窗口沿滚动轴排成一条带，视口会滚动以保持焦点窗口可见
  #   （灵感来自 niri/PaperWM）。
  # - 'bsp': 窗口构成一棵二叉分割树；插入窗口时会把焦点窗口的空间一分为二
  #   （类似 bspwm）。
  default: "scrolling"

  # 当工作区的布局被切换时（通过 `set-layout`、`cycle-layout` 或配置重载），
  # 已有窗口会如何处理。
  # - 'reflow': 将已有窗口重新排列到新布局中。
  # - 'keep': 保持已有窗口的排列；只有之后新增的窗口才遵循新布局。
  switch_behavior: "keep"

  # 平铺布局的选项。平铺布局目前没有选项。
  tiling: {}

  # 滚动布局的选项。
  scrolling:
    # 窗口带滚动的轴。同时也是滚动布局工作区的初始平铺方向：
    # `toggle-tiling-direction` 会在运行时翻转它，该改动不会写回这里。
    axis: "vertical"

    # 新窗口的默认宽度，占视口宽度的比例。
    # 滚动轴为水平时生效。
    window_width: 0.7

    # 新窗口的默认高度，占视口高度的比例。
    # 滚动轴为垂直时生效。
    window_height: 0.7

    # `cycle-preset-size` 循环切换焦点窗口大小时使用的尺寸
    # （占视口的比例），分别对应水平轴和垂直轴。
    preset_widths: [0.3, 0.5, 0.7, 1.0]
    preset_heights: [0.5, 1.0]

    # 焦点窗口如何滚动到视口内：
    # - 'minimal': 只滚动到刚好可见的程度（类似 niri）。
    # - 'center': 将焦点窗口居中于视口。
    align: "center"

    # 新窗口插入到窗口带中的位置：
    # - 'start': 插入到窗口带开头（顶部/左侧）。
    # - 'before_focused': 插入到焦点窗口之前。
    # - 'after_focused': 插入到焦点窗口之后。
    # - 'end': 追加到窗口带末尾（底部/右侧）。
    insert_position: "after_focused"

    # 滚动出视口的窗口如何被隐藏：
    # - 'hide': 使用配置的隐藏方式隐藏（不会出现在 Alt+Tab 中）。
    # - 'park': 移动到所有显示器边缘之外但保持显示，因此仍会出现在
    #   Alt+Tab 和任务栏中。
    offscreen_windows: "park"

  # BSP 布局的选项。
  bsp:
    # 新分割方向如何选择：
    # - 'longest_side': 分割焦点窗口较长的一边（类似 bspwm）。
    # - 'alternate': 按树的深度交替分割方向。
    automatic_scheme: "longest_side"
```

工作区可以使用不同的布局，也可以单独覆盖部分布局选项。覆盖项会逐字段合并，因此只有列出的选项会被更改。

```yaml
workspaces:
  # 使用全局的 `layout.default`。
  - name: "1"

  # 使用滚动布局，并沿用全局的滚动选项。
  - name: "2"
    layout: "scrolling"

  # 使用滚动布局，但新窗口的默认宽度更小。
  - name: "3"
    layout: "scrolling"
    layout_overrides:
      scrolling:
        window_width: 0.5
```

### 配置：动画

动画在 `general.animations` 属性中配置。动画需要 `general.hide_method: 'cloak'`，
因为真实窗口会在其缩略图播放动画期间被隐藏。

```yaml
general:
  hide_method: "cloak"

  animations:
    # 是否启用窗口动画。
    enabled: true

    # 显示器上显示的工作区发生变化时播放的动画。
    workspace_switch:
      # - 'none': 不播放动画。
      # - 'slide': 窗口水平滑动。
      # - 'slide_vert': 窗口垂直滑动。
      # - 'fade': 工作区原地交叉淡入淡出。
      style: "slide"

      # 新的工作区从哪一侧滑入。'auto' 会根据工作区切换的方向自动推导。
      # - 'auto' | 'left' | 'right' | 'up' | 'down'。
      direction: "auto"

      # 动画持续时间，单位为毫秒。
      duration_ms: 250

      # 缓动曲线，格式为三次贝塞尔控制点 [x1, y1, x2, y2]。
      curve: [0.22, 1.0, 0.36, 1.0]

    # 窗口首次显示时播放的动画。
    window_open:
      # - 'none': 不播放动画。
      # - 'slide': 窗口从显示器右边缘滑入。
      # - 'slide_vert': 窗口从显示器底边滑入。
      # - 'popin': 窗口在淡入的同时放大到目标大小。
      # - 'fade': 窗口淡入。
      style: "slide"

      # 动画持续时间，单位为毫秒。
      duration_ms: 200

      # 缓动曲线，格式为三次贝塞尔控制点 [x1, y1, x2, y2]。
      curve: [0.22, 1.0, 0.36, 1.0]

    # 可见窗口的位置或大小发生变化时播放的动画。
    window_move:
      # 是否启用该动画。
      enabled: true

      # 动画持续时间，单位为毫秒。
      duration_ms: 150

      # 缓动曲线，格式为三次贝塞尔控制点 [x1, y1, x2, y2]。
      curve: [0.22, 1.0, 0.36, 1.0]
```

### 配置：工作区

工作区需要通过配置文件中的 `workspaces` 属性预定义。启动时，每个显示器会自动分配一个工作区。

```yaml
workspaces:
  # 这是工作区的唯一 ID。它用于快捷键命令，如果未提供 `display_name`，
  # 它也是第三方应用程序（例如 Zebar）中显示的标签。
  - name: "1"

    # 第三方应用程序中使用的工作区标签的可选覆盖。
    # 不需要是唯一的。
    display_name: "工作"

    # 如果存在，可选择强制工作区在特定显示器上。
    # 0 是您最左边的屏幕，1 是右边的下一个，依此类推。
    bind_to_monitor: 0

    # 可选择防止工作区在空时被停用。
    keep_alive: false

    # 可选择使用与 `layout.default` 不同的布局。
    # - 'tiling': 窗口平分视口。
    # - 'scrolling': 窗口沿滚动轴排成一条带。
    # - 'bsp': 窗口构成一棵二叉分割树。
    layout: "scrolling"

    # 可选择覆盖单个布局选项。只有列出的选项会被覆盖，
    # 其余选项均继承自 `layout`。
    layout_overrides:
      scrolling:
        window_width: 0.5

    # 可选择为该工作区覆盖 `layout.switch_behavior`。
    # - 'reflow': 将已有窗口重新排列到新布局中。
    # - 'keep': 保持已有窗口的排列。
    layout_switch: "keep"
```

### 配置：窗口规则

可以在窗口首次启动时运行命令。这对于添加特定于窗口的行为很有用，比如始终以全屏模式启动窗口或分配到特定工作区。

窗口可以通过其进程、类和标题进行定位。可以一起使用多个匹配条件来更精确地定位窗口。

```yaml
window_rules:
  - commands: ["move --workspace 1"]
    match:
      # 将浏览器移动到工作区 1。
      - window_process: { regex: "msedge|brave|chrome" }

  - commands: ["ignore"]
    match:
      # 忽略任何 Zebar 窗口。
      - window_process: { equals: "zebar" }

      # 忽略浏览器的画中画窗口。
      # 注意标题和类都必须匹配才能运行规则。
      - window_title: { regex: "[Pp]icture.in.[Pp]icture" }
        window_class: { regex: "Chrome_WidgetWin_1|MozillaDialogClass" }
```

### 配置：窗口效果

可以通过 `window_effects` 选项对窗口应用视觉效果。目前，彩色边框是唯一可用的效果，未来会有更多效果。

> 注意：窗口效果仅适用于 Windows 11。

```yaml
window_effects:
  # 应用于聚焦窗口的视觉效果。
  focused_window:
    # 用彩色边框突出显示窗口。
    border:
      enabled: true
      color: "#0000ff"

  # 应用于非聚焦窗口的视觉效果。
  other_windows:
    border:
      enabled: false
      color: "#d3d3d3"
```

### 配置：窗口行为

`window_behavior` 配置选项用于自定义窗口可以处于的状态（`tiling`、`floating`、`minimized` 和 `fullscreen`）。

```yaml
window_behavior:
  # 新窗口在可能的情况下以此状态创建。
  # 允许的值：'tiling'、'floating'。
  initial_state: "tiling"

  # 设置创建新窗口时的默认选项。这也会更改状态更改命令
  # （如 `set-floating`）在不使用任何标志时的默认值。
  state_defaults:
    floating:
      # 是否默认居中浮动窗口。
      centered: true

      # 是否将浮动窗口显示为始终在顶部。
      shown_on_top: false

    fullscreen:
      # 如果可能，最大化窗口。如果窗口没有最大化按钮，
      # 则会正常全屏显示。
      maximized: false
```

### 配置：绑定模式

绑定模式用于在 GlideWM 运行时修改快捷键。

可以使用 `wm-enable-binding-mode --name <名称>` 启用绑定模式，使用 `wm-disable-binding-mode --name <名称>` 禁用。

```yaml
binding_modes:
  # 启用时，可以通过方向键或 HJKL 调整聚焦窗口的大小。
  - name: "resize"
    keybindings:
      - commands: ["resize --width -2%"]
        bindings: ["h", "left"]
      - commands: ["resize --width +2%"]
        bindings: ["l", "right"]
      - commands: ["resize --height +2%"]
        bindings: ["k", "up"]
      - commands: ["resize --height -2%"]
        bindings: ["j", "down"]
      # 让聚焦窗口在滚动布局的预设尺寸之间循环。
      # 只有滚动布局的工作区才有预设尺寸。
      - commands: ["cycle-preset-size"]
        bindings: ["shift+oem_plus"]
      - commands: ["cycle-preset-size --reverse"]
        bindings: ["oem_minus"]
      # 按 enter/escape 返回默认快捷键。
      - commands: ["wm-disable-binding-mode --name resize"]
        bindings: ["escape", "enter"]
```

> `+` 键在多数键盘布局上都需要按 Shift，因此写作 `shift` 加 `=` 键（`oem_plus`）。快捷键
> 字符串会按 `+` 分割，所以不能直接写 `+`；而需要 Shift 的按键也无法作为字面字符解析。

## 常见问题

**问：如何在启动时运行 GlideWM？**

右键单击系统托盘中的 GlideWM 图标，选择 `开机自启`。本地安装脚本也会替您注册。

**问：如何在工作区之间切换布局？**

`set-layout --layout <tiling|scrolling|bsp>` 设置聚焦工作区（或 `--workspace` 指定的工作区）
的布局，`cycle-layout` 在可用布局之间循环。该切换仅在本次会话中生效；要让它固定下来，
请在配置中设置该工作区的 `layout`。

默认情况下，切换布局时已有窗口保持原排列（`switch_behavior: 'keep'`）。设为 `'reflow'`
则会把它们重新排列到新布局中。

**问：如何滚动一个滚动布局的工作区？**

`scroll --direction <left|right|up|down> --amount <值>` 会沿滚动轴滚动窗口带，而不移动焦点。
`center-focused` 会把焦点窗口居中到视口，`cycle-preset-size` 则会在配置的预设尺寸之间调整
焦点窗口的大小。它绑定在 `resize` 绑定模式（`alt+r`）的 `+` 和 `-` 上，因此 `alt+r` 再按
`+` 会跳到下一个预设，按 `-` 会回到上一个。

**问：如何创建 `<插入布局>`？**

在 GlideWM 中，布局是一等公民。在 `layout` 属性中选择 `tiling`、`scrolling` 或 `bsp`，
也可以用 `set-layout` 在运行时切换工作区。在平铺布局内部，`alt+v` 仍然用于改变平铺方向，
这决定了下一个窗口相对于当前窗口的放置位置。

社区制作的脚本如 [Dutch-Raptor/GAT-GWM](https://github.com/Dutch-Raptor/GAT-GWM) 和
[burgr033/GlazeWM-autotiling-python](https://github.com/burgr033/GlazeWM-autotiling-python)
可用于自动更改平铺方向。目前不支持自动布局的原生支持。

**问：如何为 `<插入应用程序>` 创建规则？**

要匹配特定应用程序，您需要一个要执行的命令以及窗口的进程名称、标题或类名称。例如，如果您使用 Flow-Launcher 并希望设置窗口浮动，您可以执行以下操作：

```yaml
window_rules:
  - commands: ["set-floating"]
    match:
      - window_process: { equals: "Flow.Launcher" }
        window_title: { equals: "Settings" }
```

像 Winlister 或 AutoHotkey 的 Window Spy 这样的程序对于获取窗口信息很有用。

**问：当 `<插入应用程序>` 聚焦时，如何忽略 GlideWM 的快捷键？**

目前不支持此功能，但是，默认配置中的快捷键 `alt+shift+p` 用于禁用所有其他快捷键，直到再次按下 `alt+shift+p`。

**问：为什么我的动画没有播放？**

动画需要 `general.hide_method: 'cloak'` 和 `general.animations.enabled: true`，并且仅
支持 Windows。如果某个窗口由外部工具（例如 Windhawk）负责动画，GlideWM 不会去动它。

## 致谢

GlideWM 站在 [GlazeWM](https://github.com/glzr-io/glazewm)（Glzr Software 出品）的肩膀上，
也离不开更广泛的平铺窗口管理器社区：niri、bspwm、PaperWM、i3 和 Polybar。感谢你们。

[discord-badge]: https://img.shields.io/discord/1041662798196908052.svg?logo=discord&colorB=7289DA
[discord-link]: https://discord.gg/ud6z3qjRvM
[downloads-badge]: https://img.shields.io/github/downloads/JASSIM-ALHUMAID/glazewm/total?logo=github&logoColor=white
[downloads-link]: https://github.com/JASSIM-ALHUMAID/glazewm/releases
[demo-video]: resources/assets/demo.webp
