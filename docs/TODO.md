# rxbms TODO

Bevy 0.19 BMS 铺面播放器 — 未完成事项清单。
基线：88 测试全绿（+2 个 `#[ignore]` 手动基准，2026-10）。

## 最近完成（2026-08-14 ~ 2026-10-17）

- **铺面导入（重写 `src/database.rs` + 新增 `src/library.rs`）**：
  - 扫描改后台线程（`SongScanner`，`Update` 中只排空事件），启动不再阻塞界面
  - `ignore::WalkBuilder::build_parallel()` 并行枚举 + `MAX_PARSE_THREADS = 12`
    并行解析（`std::thread::scope` + 原子派发），单文件 `catch_unwind` 隔离 panic
  - 增量判据 `(size, mtime_ns, parse_version)`；`PARSE_VERSION` 提升即全库重解析
  - 扫描触发：**只按需扫描**（设置页「重新扫描」/ 添加目录 / 选曲页 F5），无后台周期扫描；曾尝试 notify 热重载，因会自我触发反复扫描已移除
  - 健壮性：二进制/非法编码拒收（Shift_JIS 替换字符比例）、`scan_errors` 表记录
    失败原因、内容哈希去重（同内容只展示一条）、改名/搬移「复活」原记录保留身份、
    删除标 inactive（可恢复）、嵌套根目录去重
  - SQLite 换 WAL + `synchronous=NORMAL` + `busy_timeout`，批量 upsert（512/批）
  - 实测：500 个合成铺面首扫 42ms / 增量 12ms；真实库（68 铺面 15MB BMS 文本）
    首扫 1.47s（bms-rs 解析为主）、增量 10ms

- **ffmpeg 依赖**：`ffmpeg-next` 8.1.0 → 9.0.0，适配系统 ffmpeg 9.0.2 / libavcodec 63
  （此前 14 个编译错误全在 crate 内部，整仓库无法 `cargo test`）
- **铺面加载内存**（用户报告"加载铺面时内存过大"）：
  1. 音频不再整谱一次性解码 → `pump_preload` 按谱面时间 ±10s 滚动窗口预加载；
  2. LRU 淘汰对"已租约"项生效（旧实现只在 `refs == 0` 里找候选 → 上限从未生效），
     首批音频 pinned 不可淘汰，miss 时同步解码补回；
  3. BGA 帧缓冲 16 → 4 且复用 `img.data`，解码通道改有界 `sync_channel(4)`；
  4. 事件读取改 `events().as_events()` 切片，去掉整份事件 Vec 的深拷贝
- **BGM 丢音**：BGM 拆三条子轨（`bgm` / `bgm_sample` / `keysound`），一次性采样走
  `bgm_sample` 并有 256 声部软上限；BGM 角色改**相对判定**（时长 ≥ 铺面时长 × 25%
  才算连续音频 Bed），修掉"1.0s 绝对阈值把 1.32s 鼓组采样判成 BGM 而丢 42% 事件"
- **谱面资源统一释放**：`src/gameplay/resources.rs` 的 `cleanup_gameplay_resources`
  成为 `OnExit(Gameplay)` 唯一释放入口（音频租约/皮肤槽位/资源统计）
- **谱面结束判定**：改用每个音符实体的 `consumed`（LN 会 record 两次，
  `judged() >= note_count` 会让长音谱面提前结束）

## 优先级 P0（游玩体验关键）

- [x] **judge_level 设置未应用**
  - ✅ 已实现（2026-08）：设置非默认（VeryHard/Hard/Easy）时覆盖谱面 `#RANK`；
    默认 Normal/未知 → 用谱面自带 rank（`src/gameplay.rs` setup_gameplay）

- [x] **`Box::leak` 谱面泄漏**
  - ✅ 已实现（2026-08）：`LoadedChart.chart` 改 `Arc<Chart>`，播放头用
    `self_cell`（`ChartPlayback`）绑定 (Arc<Chart>, ChartPlayer)，随
    GameplaySession 整体释放，无泄漏（`src/gameplay.rs`）

- [x] **y=0 事件被 bms-rs 排除**
  - ✅ 已在 bms-rs fork 分支 `fix-stop-duration-ms` 修复（cee4348：首帧用闭区间）
  - ⚠️ 仓内无回归覆盖：fork 里对应测试被删（137805f）。建议在 rxbms 侧补一个
    "谱面第 0 小节首行事件能触发"的集成测试

## 优先级 P1（功能补全）

- [ ] **重新设计 Lua API（去 beatoraja 化）**
  - 现状：皮肤描述语言与运行时 API（`main_state`、timer 模型、`SkinSourceImageSet`
    语义、destination/object 约定）整体移植自 beatoraja `Skin` 体系
    （`src/skin/lua.rs` / `model.rs` / `state.rs` / `render.rs` / `runtime.rs`）
  - 目标：按 rxbms 自身需求重新设计——更清晰的描述语法、精简/重构 `main_state`
    接口、独立的对象/动画/文本/图形模型；旧皮肤（beatoraja 格式）不承诺兼容，
    以可维护性与可扩展性为优先
  - 范围：描述表解析 → 运行时状态 → 渲染求值全链路；涉及 skin 模块五个文件

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

- [ ] **音频类设置项**：BGM 角色判定比例 `BED_MIN_CHART_RATIO`（当前常量 0.25）
  与"多长算连续音频"开关（Auto / 全部叠音 / 全部去重）暴露到设置，供异常谱面自救

- [ ] **BGA Overlay / Poor 层**
  - beatoraja 的 overlay 层（事件层叠加）、miss-layer（漏键时画面层）未实现

- [ ] **选曲界面重做**（`src/select.rs`，仍是 121 行原型）
  - 现状：铺面库已就绪（按需扫描/去重/热重载已移除），界面只有一列纯文本按钮
  - 待做：可滚动列表、选择框、音频预览（`play_menu_bgm` 已有但无调用方）、
    排序（难度/BPM/标题）、搜索、难度与模式过滤、历史最佳（依赖下方游玩记录）

## 已知边界（暂不实现，记录备查）

- **`-fuse-ld=wild` 实际未生效**：写在 `Cargo.toml` 的
  `[target.'cfg(target_os = "linux")'] rustflags` 不是有效 Cargo 键（构建警告
  `unused manifest key`），需挪到 `.cargo/config.toml` 才生效；`resolver` 键同理

- **DP（14K）**：双玩家模式不支持（当前 7k/5k 单玩家）
- **pop（9K）**：9 键玩法解析不做
- **mod.rs**：项目约定不用 `mod.rs` 模块组织
