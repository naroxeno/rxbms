# rxbms TODO

Bevy 0.19 BMS 铺面播放器 — 未完成事项清单。
基线：74 测试全绿（2026-08），kira 音频 + beatoraja 皮肤移植完成。

## 优先级 P0（游玩体验关键）

- [ ] **judge_level 设置未应用**
  - 现状：`JudgeWindows::for_level(loaded.rank)` 用谱面 `#RANK` 头，设置里的"判定难度"
    （`judge_level`，Gameplay 类）被忽略
  - 目标：设置项生效（默认仍取谱面 rank；设置非默认时覆盖）
  - 位置：`src/gameplay.rs`（GameplaySession.judge_windows 构造处）

- [ ] **`Box::leak` 谱面泄漏**
  - 现状：`LoadedChart::load` 每次游玩 `Box::leak` 一份 `&'static Chart`（`src/gameplay/chart.rs:105`），
    供 `ChartPlayer` 借用；每玩一次泄漏一份谱面数据
  - 目标：改为 owned 播放器（ChartPlayer 持有 Chart 或生命周期管理），游玩结束释放
  - 影响：长会话反复游玩内存增长

- [ ] **y=0 事件被 bms-rs 排除**
  - 现状：`ChartPlayer.update` 用 `(Excluded(prev_y), Included(cur_y))`，start 时 `progressed_y=0`
    → 谱面第 0 小节首行的事件（首个 BGM/键音）不触发
  - 目标：谱面开头第一个事件能触发（或确认 bms-rs 语义后适配）

## 优先级 P1（功能补全）

- [ ] **结算界面 / 游玩记录**（`src/result.rs` / `src/record.rs` 空壳）
  - 结算：EX 分数、判定计数、血量结果、失败/通过展示（走皮肤或 UI）
  - 记录：`songs.db` 或独立表保存游玩历史（成绩、判定、时间）

- [ ] **主界面 BGM 未接入**
  - 现状：`AudioManager::play_menu_bgm/stop_menu_bgm` 已实现，`select.rs` 只调 `stop_menu_bgm`
  - 目标：进入选曲/标题时播放（曲目预览或菜单 BGM），来源待定（设置项/默认资源）

- [ ] **M5：皮肤切换 / skin_config 选项 UI**
  - 皮肤路径设置已有（`skin_path`），运行时切换需重载 SkinRuntime（`load_lua_skin` 已支持重建）
  - skin_config 选项（皮肤内 Lua 配置项）的 UI 调节界面

- [ ] **text destination 动画插值**
  - 现状：文本对象动画未做帧间插值（`text_frame` 取离散帧）
  - 目标：与图片对象一致的插值（位置/透明度渐变）

- [ ] **graph 数据图（M3）**
  - beatoraja skin 的 graph（成绩/进度曲线图）destination 未渲染

- [ ] **BGA `#STARTxx/#ENDxx` 时间窗**
  - 现状：视频从触发时刻播；`#START`（视频在谱面中的起始偏移）/`#END`（结束）未解析
  - 目标：bms-rs fork 解析 START/END，BGA 按时间窗裁剪

## 优先级 P2（体验增强）

- [ ] **等比例 1920×1080 缩放（2K/4K 适配）**
  - 现状：`VirtualScreen::fit` 已做等比信箱；验证 2K/4K 下皮肤/音符/文本清晰度
  - 目标：分辨率无关渲染（必要时提高贴图采样质量）

- [ ] **混音器/音频延迟可配置**
  - kira 内部缓冲（128 采样）→ 设置项暴露（延迟 vs 稳定性权衡）

- [ ] **BGA Overlay / Poor 层**
  - beatoraja 的 overlay 层（事件层叠加）、miss-layer（漏键时画面层）未实现

- [ ] **铺面数据库完善**（`src/database.rs`）
  - 现状：`songs.db` 已支持扫描/统计/去重；选曲排序、搜索、难度过滤、模式过滤未做

## 已知边界（暂不实现，记录备查）

- **DP（14K）**：双玩家模式不支持（当前 7k/5k 单玩家）
- **pop（9K）**：9 键玩法解析不做
- **mod.rs**：项目约定不用 `mod.rs` 模块组织
