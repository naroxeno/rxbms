//! Bevy 皮肤运行时：Lua 皮肤加载 → 图片资源 → 每帧帧求值 → sprite 实体池。
//!
//! `SkinRuntime` 为 **NonSend** 资源（`Lua` 非 `Send`，见 lua.rs 说明），
//! 系统在主线程访问。渲染流程：
//! 1. Startup：加载 `.luaskin`（header + main 描述表）→ `SkinDesc`，预加载全部 source 图片
//! 2. 每帧（Gameplay）：`sync_skin_state` 把 gameplay 状态写入 `PlayState`
//! 3. 每帧：`apply_skin_frame` 检查纹理就绪后建立槽池，`evaluate_frame` 生成指令并
//!    同步到槽实体（Transform / custom_size / Visibility / atlas）
//!
//! 槽池：每条指令一个 sprite 实体，按需增长、每帧先全隐藏再点亮活跃指令。
//! 纹理 UV：每个 source 图片建一个单区域 `TextureAtlasLayout`（区域 = 指令 uv）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use bevy::{
    asset::AssetServer,
    image::{Image, TextureAtlasLayout},
    prelude::*,
    sprite::Anchor,
};

use crate::core::settings::SettingsStore;
use crate::skin::lua::{LuaSkin, SkinConfigValues, SkinHeader};
use crate::skin::model::{Destination, SkinDesc, TextDef, resolve_wildcard};
use crate::skin::render::{DrawCmd, VirtualScreen, evaluate_frame, frame_at};
use crate::skin::state::{PlayState, install_main_state};

/// 皮肤运行时（NonSend resource）。
pub struct SkinRuntime {
    /// M3b/M5 使用（保持 Lua 存活并支持重新加载）。
    #[allow(dead_code)]
    pub lua: LuaSkin,
    #[allow(dead_code)]
    pub header: SkinHeader,
    pub config: SkinConfigValues,
    pub desc: SkinDesc,
    /// main_state 数据源（与 gameplay 每帧同步）。
    pub state: Arc<RwLock<PlayState>>,
    /// 皮肤目录绝对路径（通配符枚举用）。
    #[allow(dead_code)]
    pub dir: PathBuf,
    /// source id → 纹理句柄。
    pub textures: HashMap<String, Handle<Image>>,
    /// 字体 id → Font 句柄（skin.text 用）。
    pub fonts: HashMap<String, Handle<Font>>,
    /// 文本槽实体（与有 destination 的 text 对象对齐）。
    pub text_slots: Vec<Entity>,
    /// 槽位实体（按指令 id 寻址的实例池；同 id 复用，帧动画切帧只换 layout 句柄）。
    pub slots: Vec<Entity>,
    /// 槽 id → 槽索引（稳定身份，note 用全局谱面下标，避免窗口波动错配/共用）。
    pub slot_map: HashMap<u64, usize>,
    /// 每槽当前绑定的 uv（切帧时据此换 atlas index，layout 不变）。
    pub slot_uvs: Vec<URect>,
    /// 每槽是否特效槽（`blend=2` / BGA 帧 → Mesh2d + 自定义材质）。
    pub slot_fx: Vec<bool>,
    /// source id → (静态 atlas layout, uv→index 表)：同 source 合批、切帧只改 index。
    pub atlas: HashMap<
        String,
        (Handle<TextureAtlasLayout>, HashMap<(u32, u32, u32, u32), u32>),
    >,
    /// 特效材质缓存：(src, uv) → 材质句柄（同源同帧复用，切帧查/建）。
    pub fx_cache: HashMap<
        (String, u32, u32, u32, u32),
        Handle<crate::skin::material::SkinFxMaterial>,
    >,
    /// 纹理是否全部就绪（已建槽池）。
    pub ready: bool,
    /// source id → 源图尺寸（纹理就绪后填充；整图 w=-1 的图像与帧动画用）。
    pub src_sizes: HashMap<String, (u32, u32)>,
    /// 虚拟屏幕（窗口尺寸变化时重建）。
    pub screen: VirtualScreen,
}

pub fn load_lua_skin(world: &mut World) {
    // 清理旧皮肤（重进 Gameplay 时按新模式重建，避免槽实体泄漏）
    if let Some(rt) = world.remove_non_send::<SkinRuntime>() {
        for e in rt.slots.into_iter().chain(rt.text_slots) {
            world.despawn(e);
        }
    }
    let asset_server = world.resource::<AssetServer>().clone();
    let skin_path = world
        .resource::<SettingsStore>()
        .get_string("skin_path", "test_skin/Play");
    let dir = PathBuf::from("assets").join(&skin_path);
    // 按谱面模式选入口（5K → Play5.luaskin，7K → Play7.luaskin）
    let Some(session) = world.get_resource::<crate::gameplay::GameplaySession>() else {
        return; // setup_gameplay 失败（谱面加载错误）时无皮肤
    };
    let entry_name = match session.mode {
        crate::core::keybind::PlayMode::FiveKey => "Play5.luaskin",
        crate::core::keybind::PlayMode::SevenKey => "Play7.luaskin",
    };
    let lua = match LuaSkin::new(&dir) {
        Ok(l) => l,
        Err(e) => {
            spawn_fallback(world, &format!("Lua 皮肤加载失败: {e}"));
            return;
        }
    };
    // main_state 真 API 必须先于首次执行注入（对齐 beatoraja 构造器时机）
    let state = Arc::new(RwLock::new(PlayState::default()));
    if let Err(e) = install_main_state(lua.lua(), state.clone()) {
        spawn_fallback(world, &format!("main_state 安装失败: {e}"));
        return;
    }
    let entry = dir.join(entry_name);
    let header = match lua.load_header(&entry) {
        Ok(h) => h,
        Err(e) => {
            spawn_fallback(world, &format!("header 加载失败: {e}"));
            return;
        }
    };
    let config = SkinConfigValues::from_header(&header);
    let desc = match lua.load_skin(&entry, &header, &config) {
        Ok(t) => match SkinDesc::from_table(lua.lua(), &t, header.w, header.h) {
            Ok(d) => d,
            Err(e) => {
                spawn_fallback(world, &format!("描述表解析失败: {e}"));
                return;
            }
        },
        Err(e) => {
            spawn_fallback(world, &format!("main 加载失败: {e}"));
            return;
        }
    };
    // 预加载全部 source 纹理（通配符按选中名解析）
    let mut textures = HashMap::new();
    for (id, src) in &desc.sources {
        let selected = config.file_path.first().map(|(_, p)| p.as_str());
        let rel = match resolve_wildcard(&dir, &src.path, selected) {
            Ok(r) => r,
            Err(e) => {
                warn!("[skin] 通配符解析失败 {}: {e}", src.path);
                continue;
            }
        };
        let handle = asset_server.load(format!("{skin_path}/{rel}"));
        textures.insert(id.clone(), handle);
    }
    // 预加载字体（skin.font）
    let mut fonts = HashMap::new();
    for f in &desc.fonts {
        let handle = asset_server.load(format!("{skin_path}/{}", f.path));
        fonts.insert(f.id.clone(), handle);
    }
    info!(
        "[skin] Lua 皮肤已加载: {}（{} 源图, {} 图像, {} 数字, {} destination）",
        header.name,
        textures.len(),
        desc.images.len(),
        desc.values.len(),
        desc.destinations.len()
    );
    world.insert_non_send(SkinRuntime {
        lua,
        header,
        config,
        desc,
        state,
        dir,
        textures,
        fonts,
        text_slots: Vec::new(),
        slots: Vec::new(),
        slot_map: HashMap::new(),
        slot_uvs: Vec::new(),
        slot_fx: Vec::new(),
        atlas: HashMap::new(),
        fx_cache: HashMap::new(),
        ready: false,
        src_sizes: HashMap::new(),
        screen: VirtualScreen::fit(1920.0, 1080.0, 1280.0, 720.0),
    });
}

/// 皮肤层渲染：帧求值 → 槽池同步 + 文本槽更新。
#[allow(clippy::type_complexity)]
pub fn apply_skin_frame(
    mut commands: Commands,
    mut runtime: Option<NonSendMut<SkinRuntime>>,
    mut atlas_layouts: ResMut<Assets<TextureAtlasLayout>>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut fx_materials: ResMut<Assets<crate::skin::material::SkinFxMaterial>>,
    fonts: Res<Assets<Font>>,
    bga: Res<crate::gameplay::bga::BgaPlayer>,
    window_q: Query<&Window>,
    // 注意：三个 Query 都改 Transform/Visibility，须用 Without 拆开（B0001）
    mut q: Query<
        (Entity, &mut Sprite, &mut Transform, &mut Visibility),
        Without<Text2d>,
    >,
    // 特效槽（Mesh2d + 自定义材质：black-key 抠像 / RGB 重排）
    mut mq: Query<
        (
            Entity,
            &mut MeshMaterial2d<crate::skin::material::SkinFxMaterial>,
            &mut Transform,
            &mut Visibility,
        ),
        (Without<Sprite>, Without<Text2d>),
    >,
    mut tq: Query<
        (
            Entity,
            &mut Text2d,
            &mut TextColor,
            &mut TextFont,
            &mut Transform,
            &mut Visibility,
            &mut Anchor,
        ),
        Without<Sprite>,
    >,
) {
    let Some(runtime) = runtime.as_deref_mut() else { return };
    let Ok(win) = window_q.single() else { return };
    runtime.screen =
        VirtualScreen::fit(runtime.desc.vw, runtime.desc.vh, win.width(), win.height());

    // 纹理就绪检查（全部加载后才建槽池）
    if !runtime.ready {
        let all_ready = runtime.textures.values().all(|h| images.get(h).is_some());
        if !all_ready {
            return;
        }
        // 记录源图尺寸（帧动画裁剪 / 特效槽 uv_rect 换算用）。
        // blend=2 黑底特效图不再做 CPU 抠像——已改由 GPU shader
        // （SkinFxMaterial FLAG_BLACK_KEY，beatoraja layer.frag 语义）处理。
        for (id, h) in &runtime.textures {
            if let Some(img) = images.get(h) {
                let s = img.size();
                runtime.src_sizes.insert(id.clone(), (s.x, s.y));
            }
        }
        // 构建每 source 静态 atlas（同 source 合批 + 切帧只改 index，不重建 layout）
        for (id, _) in &runtime.textures {
            let size = runtime.src_sizes.get(id).copied().unwrap_or((1, 1));
            let mut layout = TextureAtlasLayout::new_empty(UVec2::new(size.0, size.1));
            let mut map = HashMap::new();
            for uv in crate::skin::render::collect_source_uvs(&runtime.desc, &runtime.src_sizes, id) {
                let key = (uv.min.x, uv.min.y, uv.max.x, uv.max.y);
                if !map.contains_key(&key) {
                    let idx = layout.add_texture(uv) as u32;
                    map.insert(key, idx);
                }
            }
            runtime
                .atlas
                .insert(id.clone(), (atlas_layouts.add(layout), map));
        }
        runtime.ready = true;
    }

    // 文本槽（M4 动态字体）：先全部隐藏
    for &e in &runtime.text_slots {
        if let Ok((_, _, _, _, _, mut vis, _)) = tq.get_mut(e) {
            *vis = Visibility::Hidden;
        }
    }
    // 文本内容就绪检查（字体加载完才显示）
    let text_ready = runtime.fonts.values().all(|h| fonts.get(h).is_some());
    if text_ready {
        let vs = runtime.screen;
        apply_texts(runtime, &mut commands, &mut tq, &vs);
    }

    let cmds = evaluate_frame(&runtime.desc, &runtime.config, &runtime.state, &runtime.src_sizes);
    let vs = runtime.screen;

    // 1) 先隐藏全部槽
    for &e in &runtime.slots {
        if let Ok((_, _, _, mut vis)) = q.get_mut(e) {
            *vis = Visibility::Hidden;
        }
    }

    // 2) 逐指令：槽按稳定 id 寻址（同 id 复用同一实体），帧动画切帧只换 layout 句柄
    for cmd in &cmds {
        // BGA destination：取 BGA 当前帧（皮肤 `skin.bga`），否则查普通 source 纹理
        let handle = if cmd.src == "__bga__" {
            match bga.current_image() {
                Some(h) => h,
                None => continue,
            }
        } else {
            let Some(h) = runtime.textures.get(&cmd.src) else { continue };
            h.clone()
        };
        // 就绪保证纹理存在（atlas 已构建）
        if images.get(&handle).is_none() {
            continue;
        }

        // 槽按 id 查找/创建
        let slot_idx = if let Some(&i) = runtime.slot_map.get(&cmd.id) {
            i
        } else {
            // 特效槽（blend=2 黑底图 / BGA 帧）→ Mesh2d + 自定义材质（GPU 抠像/重排）
            let is_fx = cmd.blend == 2 || cmd.src == "__bga__";
            let e = if is_fx {
                let fx_handle = fx_material_for(
                    cmd,
                    &handle,
                    &runtime.src_sizes,
                    &mut runtime.fx_cache,
                    &mut fx_materials,
                );
                commands
                    .spawn((
                        Mesh2d(meshes.add(Rectangle::new(
                            cmd.w * vs.scale,
                            cmd.h * vs.scale,
                        ))),
                        MeshMaterial2d::<crate::skin::material::SkinFxMaterial>(fx_handle),
                        Transform::from_xyz(
                            vs.world_x(cmd.x + cmd.w / 2.0),
                            vs.world_y(cmd.y + cmd.h / 2.0),
                            cmd.z as f32 * 0.01,
                        ),
                        Visibility::Hidden,
                    ))
                    .id()
            } else {
                commands
                    .spawn((
                        Sprite {
                            image: handle.clone(),
                            custom_size: Some(Vec2::new(cmd.w * vs.scale, cmd.h * vs.scale)),
                            ..default()
                        },
                        Transform::from_xyz(
                            vs.world_x(cmd.x + cmd.w / 2.0),
                            vs.world_y(cmd.y + cmd.h / 2.0),
                            cmd.z as f32 * 0.01,
                        ),
                        Visibility::Hidden,
                    ))
                    .id()
            };
            let i = runtime.slots.len();
            runtime.slots.push(e);
            runtime.slot_map.insert(cmd.id, i);
            runtime.slot_fx.push(is_fx);
            // 哨兵：强制首次更新时绑定 atlas（否则无裁剪显示整图）
            runtime
                .slot_uvs
                .push(URect::new(u32::MAX, u32::MAX, u32::MAX, u32::MAX));
            i
        };

        let e = runtime.slots[slot_idx];
        // 特效槽：更新材质（切帧换 handle）+ 位置；普通槽：Sprite 更新
        if runtime.slot_fx[slot_idx] {
            if let Ok((_, mut mat, mut tf, mut vis)) = mq.get_mut(e) {
                mat.0 = fx_material_for(
                    cmd,
                    &handle,
                    &runtime.src_sizes,
                    &mut runtime.fx_cache,
                    &mut fx_materials,
                );
                tf.translation = Vec3::new(
                    vs.world_x(cmd.x + cmd.w / 2.0),
                    vs.world_y(cmd.y + cmd.h / 2.0),
                    cmd.z as f32 * 0.01,
                );
                *vis = Visibility::Visible;
            }
        } else if let Ok((_, mut sprite, mut tf, mut vis)) = q.get_mut(e) {
            // uv 变化（帧动画切帧）→ 换 atlas index（layout 固定，同 source 合批）
            if runtime.slot_uvs[slot_idx] != cmd.uv {
                if let Some((layout, index_map)) = runtime.atlas.get(&cmd.src) {
                    let key = (cmd.uv.min.x, cmd.uv.min.y, cmd.uv.max.x, cmd.uv.max.y);
                    let index = index_map.get(&key).copied().unwrap_or(0);
                    sprite.texture_atlas = Some(TextureAtlas {
                        index: index as usize,
                        layout: layout.clone(),
                    });
                    runtime.slot_uvs[slot_idx] = cmd.uv;
                }
            }
            sprite.image = handle.clone();
            sprite.custom_size = Some(Vec2::new(cmd.w * vs.scale, cmd.h * vs.scale));
            sprite.color = Color::srgba_u8(cmd.r, cmd.g, cmd.b, cmd.a);
            tf.translation = Vec3::new(
                vs.world_x(cmd.x + cmd.w / 2.0),
                vs.world_y(cmd.y + cmd.h / 2.0),
                cmd.z as f32 * 0.01,
            );
            *vis = Visibility::Visible;
        }
    }
}


/// 特效槽材质：按 (src, uv) 缓存查/建 `SkinFxMaterial`。
///
/// - BGA 帧（`__bga__`）：`FLAG_SWAP_RGB`（3 通道 RGB 上传，GPU 重排）；
/// - `blend=2` 黑底特效图：`FLAG_BLACK_KEY`（纯黑抠像），帧动画裁剪经 `uv_rect`。
fn fx_material_for(
    cmd: &DrawCmd,
    handle: &Handle<Image>,
    src_sizes: &std::collections::HashMap<String, (u32, u32)>,
    fx_cache: &mut std::collections::HashMap<
        (String, u32, u32, u32, u32),
        Handle<crate::skin::material::SkinFxMaterial>,
    >,
    fx_materials: &mut Assets<crate::skin::material::SkinFxMaterial>,
) -> Handle<crate::skin::material::SkinFxMaterial> {
    use crate::skin::material::{FLAG_BLACK_KEY, FLAG_SWAP_RGB, SkinFxMaterial, SkinFxUniform};
    let uv = (cmd.uv.min.x, cmd.uv.min.y, cmd.uv.max.x, cmd.uv.max.y);
    let key = (cmd.src.clone(), uv.0, uv.1, uv.2, uv.3);
    if let Some(h) = fx_cache.get(&key) {
        return h.clone();
    }
    let flags = if cmd.src == "__bga__" {
        FLAG_SWAP_RGB
    } else {
        FLAG_BLACK_KEY
    };
    let uniform = if uv == (0, 0, 0, 0) {
        SkinFxUniform::full(flags)
    } else {
        let (iw, ih) = src_sizes.get(&cmd.src).copied().unwrap_or((1, 1));
        SkinFxUniform {
            flags,
            _pad: [0; 3],
            uv_rect: Vec4::new(
                uv.0 as f32 / iw as f32,
                uv.1 as f32 / ih as f32,
                uv.2 as f32 / iw as f32,
                uv.3 as f32 / ih as f32,
            ),
        }
    };
    let h = fx_materials.add(SkinFxMaterial {
        uniform,
        texture: handle.clone(),
    });
    fx_cache.insert(key, h.clone());
    h
}

/// 文本对象帧求值（与图片 `evaluate_frame` 的 timer/frame_at 规则一致）：
/// 显式 timer 关闭 → None；无 timer 且首帧时间未到 → None；
/// `loop=-1` 播完（t > 末帧）→ None；否则返回插值帧。
fn text_frame(d: &Destination, s: &PlayState) -> Option<crate::skin::model::KeyFrame> {
    let timer_val = match d.timer {
        Some(id) if (id as usize) < 256 => {
            let v = s.timers[id as usize];
            if v == crate::skin::state::TIMER_OFF {
                return None;
            }
            (s.scene_time_ms - v as f64).max(0.0)
        }
        Some(_) => crate::skin::state::TIMER_OFF as f64,
        None => s.scene_time_ms,
    };
    if d.timer.is_none() {
        let first_time = d.frames.first().map_or(0.0, |f| f.time as f64);
        if timer_val < first_time {
            return None;
        }
    }
    frame_at(d, timer_val)
}

/// 文本对象内容：constant > value 回调 > ref（main_state text）。
fn text_content(runtime: &SkinRuntime, t: &TextDef) -> String {
    if let Some(c) = &t.constant {
        return c.clone();
    }
    if let Some(f) = &t.value {
        if let Ok(v) = f.call::<String>(()) {
            return v;
        }
    }
    if let Some(id) = t.ref_id
        && let Ok(s) = runtime.state.read()
    {
        return crate::skin::state::text(&s, id);
    }
    String::new()
}

/// 文本对象渲染（M4 动态字体）：遍历 destinations 匹配 text 对象，
/// 用 Text2d 实体显示（内容/字体/字号/对齐/位置/颜色）。
///
/// 可见性求值与图片对象一致（`evaluate_frame`）：op 条件 + timer 驱动 +
/// `frame_at` 动画插值（`loop=-1` 播完消失、显式 timer 关闭不显示、
/// 无 timer 且首帧时间未到不显示）。
///
/// 槽池对齐：先按 text destination 总数建满槽（隐藏/跳过分支不破坏
/// `idx ↔ 槽` 的对应），循环内 `idx` 只随 destination 位置推进。
#[allow(clippy::type_complexity)]
fn apply_texts(
    runtime: &mut SkinRuntime,
    commands: &mut Commands,
    tq: &mut Query<
        (
            Entity,
            &mut Text2d,
            &mut TextColor,
            &mut TextFont,
            &mut Transform,
            &mut Visibility,
            &mut Anchor,
        ),
        Without<Sprite>,
    >,
    vs: &VirtualScreen,
) {
    // 槽数量固定 = text destination 总数（占位槽隐藏，内容逐帧覆盖）
    let total = runtime
        .desc
        .destinations
        .iter()
        .filter(|d| runtime.desc.text(&d.id).is_some())
        .count();
    while runtime.text_slots.len() < total {
        let e = commands
            .spawn((
                Text2d::new(""),
                TextFont {
                    font_size: FontSize::Px(24.0),
                    ..default()
                },
                TextColor(Color::WHITE),
                Anchor::CENTER,
                Transform::default(),
                Visibility::Hidden,
            ))
            .id();
        runtime.text_slots.push(e);
    }
    let mut idx = 0;
    for d in &runtime.desc.destinations {
        let Some(t) = runtime.desc.text(&d.id) else { continue };
        // op 条件
        let visible = d.op.iter().all(|&id| {
            if id > 0 {
                runtime.config.is_option_enabled(id)
            } else {
                !runtime.config.is_option_enabled(-id)
            }
        });
        // 可见性求值：op 条件 + timer/frame_at（与图片对象一致）
        let Some(frame) = (match runtime.state.read() {
            Ok(s) => text_frame(d, &s),
            Err(_) => {
                idx += 1;
                continue;
            }
        }) else {
            idx += 1;
            continue;
        };
        let content = text_content(runtime, t);
        let font = runtime.fonts.get(&t.font).cloned();

        let e = runtime.text_slots[idx];
        if let Ok((_, mut t2d, mut tc, mut tf, mut tfm, mut vis, mut anchor)) = tq.get_mut(e) {
            if !visible || content.is_empty() {
                *vis = Visibility::Hidden;
                idx += 1;
                continue;
            }
            *t2d = Text2d::new(content);
            if let Some(f) = font {
                tf.font = f.into();
            }
            tf.font_size = FontSize::Px(t.size as f32);
            // destination 帧颜色（含 alpha 淡入淡出）
            *tc = TextColor(Color::srgba_u8(
                frame.r.clamp(0, 255) as u8,
                frame.g.clamp(0, 255) as u8,
                frame.b.clamp(0, 255) as u8,
                frame.a.clamp(0, 255) as u8,
            ));
            // 对齐（beatoraja SkinText：align=1 中心在 region.x、2 右缘在 region.x、
            // 0 左缘在 region.x——见 SkinTextImage.draw `x = region.x - w/2 / -w / 0`）
            let (new_anchor, x) = match t.align {
                1 => (Anchor::CENTER, frame.x as f32),
                2 => (Anchor::CENTER_RIGHT, frame.x as f32),
                _ => (Anchor::CENTER_LEFT, frame.x as f32),
            };
            *anchor = new_anchor;
            tfm.translation = Vec3::new(
                vs.world_x(x),
                vs.world_y(frame.y as f32 + frame.h as f32 / 2.0),
                5.0,
            );
            *vis = Visibility::Visible;
        }
        idx += 1;
    }
}

/// 加载失败兜底：全屏黑色背景 + 错误提示（不插入 SkinRuntime）。
fn spawn_fallback(world: &mut World, msg: &str) {
    warn!("[skin] 回退到内置提示渲染: {msg}");
    world.spawn((
        Sprite::from_color(Color::BLACK, Vec2::new(1920.0, 1080.0)),
        Transform::from_xyz(0.0, 0.0, -100.0),
    ));
    world.spawn((
        Text2d::new(format!("[skin] {msg}\n请检查 skin_path 设置或 assets/test_skin 目录")),
        TextFont {
            font_size: FontSize::Px(28.0),
            ..default()
        },
        TextColor(Color::srgb(1.0, 0.3, 0.3)),
        Anchor::CENTER,
        Transform::from_xyz(0.0, 0.0, 100.0),
    ));
}

/// 退出游玩：隐藏全部皮肤槽（避免残留到其他界面）。
pub fn hide_skin_slots(
    mut runtime: Option<NonSendMut<SkinRuntime>>,
    mut q: Query<&mut Visibility>,
) {
    let Some(runtime) = runtime.as_deref_mut() else { return };
    for &e in &runtime.slots {
        if let Ok(mut v) = q.get_mut(e) {
            *v = Visibility::Hidden;
        }
    }
    for &e in &runtime.text_slots {
        if let Ok(mut v) = q.get_mut(e) {
            *v = Visibility::Hidden;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    /// 构造一段两帧的淡出文本动画（模拟 Play5 loading-title 的游玩中组：
    /// timer=40、loop=-1、a 255→0）。
    fn fade_out_dest(timer: Option<i64>, loop_: i32) -> Destination {
        Destination {
            id: "t".into(),
            timer,
            loop_,
            op: vec![],
            offsets: vec![],
            blend: 0,
            filter: 0,
            frames: vec![
                crate::skin::model::KeyFrame {
                    time: 0,
                    x: 100,
                    y: 100,
                    w: 50,
                    h: 20,
                    a: 255,
                    ..Default::default()
                },
                crate::skin::model::KeyFrame {
                    time: 250,
                    a: 0,
                    ..Default::default()
                },
            ],
        }
    }

    #[test]
    fn text_frame_visibility() {
        use crate::skin::state::TIMER_READY;
        // 游玩中组（timer=40 READY，loop=-1）：timer 开启时显示插值帧
        let d = fade_out_dest(Some(TIMER_READY as i64), -1);
        let mut s = PlayState {
            scene_time_ms: 1100.0,
            timers: {
                let mut t = [crate::skin::state::TIMER_OFF; 256];
                t[TIMER_READY] = 1000; // 开启时刻 1000 → 动画 100ms
                t
            },
            ..Default::default()
        };
        let f = text_frame(&d, &s).expect("timer 开启应显示");
        assert!(f.a < 255, "100ms 处 alpha 应已开始淡出");
        // 开始游玩后 timer 关闭（TIMER_OFF）→ 消失（曲目信息不再残留）
        s.timers[TIMER_READY] = crate::skin::state::TIMER_OFF;
        assert!(text_frame(&d, &s).is_none(), "timer 关闭应隐藏");
        // loop=-1 播完（动画超过末帧）→ 消失
        let s2 = PlayState {
            scene_time_ms: 5000.0,
            timers: {
                let mut t = [crate::skin::state::TIMER_OFF; 256];
                t[TIMER_READY] = 0; // 开启时刻 0 → 动画 5000ms > 250ms
                t
            },
            ..Default::default()
        };
        assert!(text_frame(&d, &s2).is_none(), "loop=-1 播完应消失");
        // 无 timer（加载中组）：首帧时间未到 → 不显示
        let mut d2 = fade_out_dest(None, 500);
        d2.frames[0].time = 250;
        let s3 = PlayState {
            scene_time_ms: 100.0,
            ..Default::default()
        };
        assert!(text_frame(&d2, &s3).is_none(), "首帧时间未到应不显示");
        // 已过首帧 → 显示
        let s4 = PlayState {
            scene_time_ms: 300.0,
            ..Default::default()
        };
        assert!(text_frame(&d2, &s4).is_some(), "已过首帧应显示");
    }
}
