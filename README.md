# rxbms

基于 **Bevy 0.19** 的 BMS 铺面播放器（BMS = 节奏游戏音乐数据格式，源自 BM98/beatoraja 生态）。

> ⚠️ **极早期开发阶段**：本项目仍处于高度开发中，功能可能随时变化或损坏，
> 请勿作为日常工具依赖。欢迎在开发过程中共同完善。

`rust` 实现的现代 BMS 播放器（音游）：beatoraja 风格皮肤系统、低延迟音频、BGA 视频、完整游玩闭环
（选曲 → 游玩 → 结算）。

## 功能

- **游玩**：7K / 5K 单玩家；判定（LR2 表：PG/GR/GD/BD/POOR）、长音（LN）、血量条（EX-NORMAL 模型）、Auto 模式（F2）
- **皮肤**：beatoraja 风格 Lua 皮肤（`assets/test_skin/`，Play5/Play7 按模式加载）——音符/判定/打击特效/血条/HUD 全部由皮肤驱动；BGA 作为皮肤 destination 渲染
  - 皮肤素材来源：[FAm_Renderer（space.bilibili.com/2585574）](https://space.bilibili.com/2585574/dynamic) 制作的皮肤素材，仅供本地测试与开发使用，感谢作者
- **音频**：kira（cpal 后端）低延迟混音——BGM 流式播放 + 谱面时钟精确对齐；键音静态缓存多路并发；**后台解码池**渐进预加载（开玩秒进，游玩中不卡）
- **BGA**：ffmpeg 后台线程解码 + beatoraja 时间模型（触发时刻校准），与音乐同步
- **铺面数据库**：配置界面指定铺面文件夹，`songs.db`（rusqlite）扫描/统计/去重
- **统一设置**：注册表驱动设置系统（判定难度/下落速度/音量/皮肤/键位…），持久化 `~/.rxbms/config.json`
- **多模式键位**：5K/7K 分别配置按键（默认 盘 A/S/D/F/Space + J/K/L）

## 构建

```bash
# 依赖（Linux）
#   ffmpeg 开发库（BGA 视频；默认动态链接）
sudo pacman -S ffmpeg          # Arch
sudo apt install libavcodec-dev libavformat-dev libswscale-dev  # Debian/Ubuntu

cargo build --release
```

- 无 ffmpeg 环境可用 `--features static-ffmpeg` 静态编译（自包含，需 C 工具链，编译较慢）
- Windows：提供 ffmpeg DLL 即可；macOS：`brew install ffmpeg`

## 运行

```bash
cargo run --release
```

首次进入需要配置铺面目录：
1. 主界面 → 设置 → 添加铺面文件夹（如 `~/lr2oraja/songs/`）
2. 保存 → 返回选曲，选择铺面进入游玩

测试铺面可放在任意目录（如 lr2oraja 的 `songs/rainbow_ogg/`）。

## 测试

```bash
cargo test          # 74 测试（判定/音频缓存/皮肤求值/BMS 解析等）
cargo clippy        # 保持零警告（除既有 manifest key ×2）
```

## 操作

| 按键 | 功能 |
|---|---|
| 1-5 | 调试：状态机切换 |
| F2 | Auto 模式开关（游玩中） |
| Esc | 退出游玩 / 设置返回 |

## 配置

- 设置界面（注册表驱动）：判定难度、下落速度、全局音量、皮肤路径、键位（5K/7K 分模式）等
- 持久化：`~/.rxbms/config.json`
- 铺面数据库：`songs.db`（在配置的铺面文件夹旁自动维护）

## 目录结构

```
src/
├── main.rs          # 入口（LogPlugin 过滤、禁用 Bevy AudioPlugin）
├── core/            # 状态机 + 统一设置系统（SettingsRegistry 大表）
├── audio.rs         # kira 音频：分层轨道 + 谱面时钟 + 后台解码池
│   └── audio/       # 节拍器
├── database.rs      # songs.db（rusqlite）扫描/统计
├── select.rs        # 选曲界面
├── settings.rs      # 设置界面（文件夹管理 + 设置项渲染）
├── gameplay.rs      # 游玩核心：判定/长音/血量/同步
│   └── gameplay/    # chart 解析、bga 播放、judge、data、lane
├── skin/            # Lua 皮肤：lua/model/state/render/runtime
├── result.rs        # 结算界面（TODO）
└── record.rs        # 游玩记录（TODO）
```

## 技术栈

- Bevy 0.19（ECU/渲染/UI）
- bms-rs（BMS 解析，git fork：`https://github.com/naroxeno/bms-rs.git`）
- kira 0.12 + cpal（音频输出）+ Symphonia（解码）
- ffmpeg-next（BGA 视频）
- mlua（LuaJIT，皮肤脚本）
- rusqlite（铺面数据库）

## 已知限制

- 仅 7K/5K 单玩家；DP（14K）/ pop（9K）暂不支持
- 皮肤 API 目前移植自 beatoraja，计划重新设计（见 `docs/TODO.md`）

## 许可证

GPLv3 — 见 [LICENSE](LICENSE)。
