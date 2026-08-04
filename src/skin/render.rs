//! 皮肤渲染：虚拟屏幕变换 + destination 求值 → 绘制指令。
//!
//! 坐标模型：皮肤为虚拟分辨率（FAm Breeze 1920×1080）、**y 向下**；
//! Bevy 2D 世界原点在窗口中心、y 向上。`VirtualScreen` 做等比缩放居中映射。
//!
//! M2 范围：静态求值——destination 无 `timer` 或 timer 关闭时取首帧位置，
//! 支持 `op` 选项条件与 image/数字对象；timer 动画插值、text/slider/graph
//! 在 M3 后半 / M4。
//!
//! `#![allow(dead_code)]`：渲染指令尚未被 gameplay 引用（仅测试使用），
//! M3 接入 Bevy 渲染后移除。

#![allow(dead_code)]

use std::sync::{Arc, RwLock};

use bevy::math::URect;

use crate::skin::lua::SkinConfigValues;
use crate::skin::model::{Destination, SkinDesc, ValueDef};
use crate::skin::state::{PlayState, number};

/// 虚拟屏幕 → 窗口的等比缩放居中映射。
///
/// 坐标模型：beatoraja（libGDX）**y 向上**、原点左下（皮肤坐标，
/// 如判定线 `judge-line` 在 y=200 = 屏幕底部 200px，title 在 y=1022 = 顶部）。
/// Bevy 2D 世界原点在窗口中心、y 向上——两者同向，只需平移缩放。
#[derive(Debug, Clone, Copy)]
pub struct VirtualScreen {
    pub vw: f32,
    pub vh: f32,
    pub scale: f32,
    /// 虚拟坐标原点在窗口中的像素偏移（左上原点）。
    ox: f32,
    oy: f32,
    win_w: f32,
    win_h: f32,
}

impl VirtualScreen {
    /// 按窗口尺寸等比缩放适配（letterbox）。
    pub fn fit(vw: f32, vh: f32, win_w: f32, win_h: f32) -> Self {
        let scale = (win_w / vw).min(win_h / vh);
        let ox = (win_w - vw * scale) / 2.0;
        let oy = (win_h - vh * scale) / 2.0;
        Self {
            vw,
            vh,
            scale,
            ox,
            oy,
            win_w,
            win_h,
        }
    }

    /// 虚拟 x → Bevy world x（原点窗口中心）。
    pub fn world_x(&self, x: f32) -> f32 {
        self.ox + x * self.scale - self.win_w / 2.0
    }

    /// 虚拟 y（向上，原点左下）→ Bevy world y（向上，原点中心）。
    pub fn world_y(&self, y: f32) -> f32 {
        self.oy + y * self.scale - self.win_h / 2.0
    }

    /// 虚拟尺寸 → 世界尺寸（同一缩放系数）。
    pub fn world_size(&self, v: f32) -> f32 {
        v * self.scale
    }
}

/// 一次绘制指令（对应一个 sprite；数字对象每位一条）。
#[derive(Debug, Clone, PartialEq)]
pub struct DrawCmd {
    /// 稳定身份（destination/judge 用 1000000+索引，note 用 2000000+全局下标；
    /// 槽按此寻址，帧动画切帧不换槽）。
    pub id: u64,
    /// 混合模式（beatoraja blend：0=正常，2=加色 additive——黑底特效图）。
    pub blend: i32,
    /// source id（纹理来源）。
    pub src: String,
    /// 源图裁剪区域（像素，已按 divx/divy 单帧计算）。
    pub uv: URect,
    /// 虚拟坐标与尺寸（y 向下）。
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
    /// 绘制顺序（destination 数组索引）。
    pub z: i32,
}

impl DrawCmd {
    /// 生成图像类指令（destination 首帧位置 + 图像帧动画纹理区域）。
    fn from_image(
        desc: &SkinDesc,
        d: &Destination,
        id: u64,
        z: i32,
        frame: &crate::skin::model::KeyFrame,
        state: &Arc<RwLock<PlayState>>,
        src_sizes: &std::collections::HashMap<String, (u32, u32)>,
    ) -> Option<Self> {
        let img = desc.image(&d.id)?;
        let src = desc.sources.get(&img.src)?;
        let (iw, ih) = (img.w as f32, img.h as f32);
        // destination 给定了显示尺寸则用之；否则用裁剪区域尺寸（w<0 需源纹理尺寸，M2 暂跳过）
        let (dw, dh) = if frame.w > 0 && frame.h > 0 {
            (frame.w as f32, frame.h as f32)
        } else if iw > 0.0 && ih > 0.0 {
            (iw, ih)
        } else {
            return None;
        };
        // 帧动画（divx×divy 网格 + timer/cycle）
        let (timers, now) = match state.read() {
            Ok(s) => (s.timers, s.scene_time_ms),
            Err(_) => return None,
        };
        let f = frame_index(now, img.timer, img.divx * img.divy, img.cycle, &timers);
        Some(Self {
            id,
            blend: d.blend,
            src: src.id.clone(),
            uv: frame_uv(img, f, src_sizes),
            x: frame.x as f32,
            y: frame.y as f32,
            w: dw,
            h: dh,
            r: frame.r as u8,
            g: frame.g as u8,
            b: frame.b as u8,
            a: frame.a as u8,
            z,
        })
    }
}

/// 数字对象当前值：value 回调优先；否则 ref → main_state number。
fn value_number(
    desc: &SkinDesc,
    v: &ValueDef,
    state: &Arc<RwLock<PlayState>>,
) -> Result<i64, mlua::Error> {
    if let Some(f) = &v.value {
        return f.call::<i64>(());
    }
    if let Some(ref_id) = v.ref_id {
        let s = state.read().map_err(|_| mlua::Error::external("state lock"))?;
        return Ok(number(&s, ref_id));
    }
    let _ = desc;
    Ok(0)
}

/// 数字对象 → 每位一条指令（align：0=右 1=左 2=中，beatoraja SkinNumber；
/// 前导空位不画，zeropadding=0 语义）。
fn emit_value(
    desc: &SkinDesc,
    v: &ValueDef,
    _d: &Destination,
    id: u64,
    state: &Arc<RwLock<PlayState>>,
    z: i32,
    frame: &crate::skin::model::KeyFrame,
    out: &mut Vec<DrawCmd>,
) -> Result<(), mlua::Error> {
    let src = desc.sources.get(&v.src);
    let Some(src) = src else { return Ok(()) };
    let val = value_number(desc, v, state)?;
    // 各位（对齐 beatoraja currentImages：digits[0]=最高位 … digits[digit-1]=个位）
    let digits = (0..v.digit)
        .rev()
        .map(|i| (val / 10_i64.pow(i as u32)) % 10)
        .collect::<Vec<_>>();
    let sw = v.w as f32 / v.divx as f32;
    let step = sw + v.padding as f32;
    // 前导空位数（从最高位去 0；至少保留 1 位）
    let mut shiftbase = 0u32;
    while shiftbase + 1 < v.digit && digits[shiftbase as usize] == 0 {
        shiftbase += 1;
    }
    // beatoraja SkinNumber：
    //   - zeropadding=0：前导空不画，align 决定前导偏移（0=右 shift=0、1=左 shift=全长、2=中）
    //   - zeropadding=1/2：前导补 0（image[0]）/灰色 0（image[10]），占满 digit 位 → shift=0
    // 数字块左边界恒为 destination.x（region.x）。
    let filled = v.zeropadding >= 1;
    let shift = if filled {
        0.0
    } else {
        match v.align {
            0 => 0.0,
            1 => step * shiftbase as f32,
            _ => step * 0.5 * shiftbase as f32,
        }
    };
    // 单格绘制（前导与有效位共用）
    let emit_digit = |j: u32, idx: i64, out: &mut Vec<DrawCmd>| {
        if idx < 0 || idx as u32 >= v.divx * v.divy {
            return;
        }
        let col = idx as u32 % v.divx;
        let row = idx as u32 / v.divx;
        let cell_w = (v.w as u32 / v.divx) as i32;
        let cell_h = (v.h as u32 / v.divy) as i32;
        let sx = v.x + (col as i32) * cell_w;
        let sy = v.y + (row as i32) * cell_h;
        let x = frame.x as f32 + step * j as f32 - shift;
        out.push(DrawCmd {
            id: id + j as u64,
            blend: _d.blend,
            src: src.id.clone(),
            uv: URect::new(
                sx as u32,
                sy as u32,
                (sx + cell_w) as u32,
                (sy + cell_h) as u32,
            ),
            x,
            y: frame.y as f32,
            w: sw,
            h: cell_h as f32,
            r: frame.r as u8,
            g: frame.g as u8,
            b: frame.b as u8,
            a: frame.a as u8,
            z,
        });
    };
    // 前导填充（zeropadding=1/2）
    if filled {
        let pad = if v.zeropadding == 2 { 10 } else { 0 };
        for j in 0..shiftbase {
            emit_digit(j, pad, out);
        }
    }
    // 有效位
    for j in shiftbase..v.digit {
        emit_digit(j, digits[j as usize], out);
    }
    Ok(())
}

/// 可见音符 → 下落绘制（皮肤虚拟坐标，**y 向上**）。
///
/// 轨道区域 `note.dst[lane]`：判定线在区域底边（`region.y`，即 beatoraja 的 hl）；
/// note 皮肤 y = 判定线 y + (note.position − now_y) × (区域高 / visible_y)，
/// 未来音符 y 更大（更靠上），随时间推进向下落到判定线。
fn emit_notes(
    desc: &SkinDesc,
    state: &Arc<RwLock<PlayState>>,
    out: &mut Vec<DrawCmd>,
    src_sizes: &std::collections::HashMap<String, (u32, u32)>,
) {
    let Some(nd) = &desc.note else { return };
    let s = match state.read() {
        Ok(s) => s,
        Err(_) => return,
    };
    let base_z = desc.destinations.len() as i32 + 10;
    for n in &s.notes {
        if n.consumed {
            continue;
        }
        // 轨道映射：rxbms lane 0=scratch（最左），皮肤轨道最后一位=scratch（FAm Breeze 惯例）
        let skin_lane = if n.lane == 0 {
            nd.note.len().saturating_sub(1)
        } else {
            n.lane - 1
        };
        let Some(region) = nd.dst.get(skin_lane) else { continue };
        // 判定线 = 区域底边（y 向上坐标系里 region.y 是最小 y = 底部）
        let judge_y = region.y as f32;
        // beatoraja 逐段：像素 = scroll 加权 measure 差 × region.h × hispeed
        // （hispeed=1；BPM/#SPEED 已由 progressed_y 推进体现）
        let sm = scroll_measure(s.now_y, n.position, &s.scroll_timeline);
        // 音符判定点 y（底边）；未来音符更靠上
        // px = scroll 加权 measure × region.h × 玩家 hispeed（settings scroll_speed）
        let y = judge_y + sm as f32 * region.h as f32 * s.hispeed as f32;
        // 可见范围过滤（区域上下留一点余量）。
        // 长音（kind=1）命中后 head 固定在判定线、body 从判定线延伸到 tail，
        // 可见性在下方 LN 分支按 tail_y 判断（head 过线不消失）。
        let region_top = (region.y + region.h) as f32;
        if n.kind != 1 && (y < judge_y - 60.0 || y > region_top + 60.0) {
            continue;
        }
        let h = nd.size.get(skin_lane).copied().unwrap_or(30.0);
        // 图像选择：长音 body / 地雷 / 普通
        let (img_id, is_ln_body) = match n.kind {
            1 => {
                let active = nd.lnactive.get(skin_lane).map(|s| s.as_str());
                if n.ln_active && active.is_some() {
                    (active.unwrap(), true)
                } else {
                    (nd.lnbody.get(skin_lane).map(String::as_str).unwrap_or(""), true)
                }
            }
            2 => (nd.mine.get(skin_lane).map(String::as_str).unwrap_or(""), false),
            _ => (nd.note.get(skin_lane).map(String::as_str).unwrap_or(""), false),
        };
        let Some(img) = desc.images.get(img_id) else { continue };
        let Some(src) = desc.sources.get(&img.src) else { continue };
        let f = frame_index(s.scene_time_ms, img.timer, img.divx * img.divy, img.cycle, &s.timers);
        let uv = frame_uv(img, f, src_sizes);
        let z = base_z + n.lane as i32;
        if is_ln_body {
            // 长音 body：从 head 底向上延伸到 tail 底（tail_y > head_y）。
            // 命中后（ln_active）head 固定在判定线（beatoraja），body 从判定线
            // 延伸到仍在下落的 tail——避免 head 过线后整条 body 消失。
            let len = n.length.unwrap_or(0.0) as f64;
            let sm_tail = scroll_measure(s.now_y, n.position + len, &s.scroll_timeline);
            let tail_y = judge_y + sm_tail as f32 * region.h as f32 * s.hispeed as f32;
            let body_base = if n.ln_active { judge_y } else { y };
            // 可见性按 body 顶部（tail_y）判断；底部（body_base）不得低于区域下缘
            if tail_y < judge_y - 60.0 || body_base > region_top + 60.0 {
                continue;
            }
            let body_h = (tail_y - body_base).max(1.0);
            out.push(DrawCmd {
                id: 2_000_000u64 + n.idx as u64,
                blend: 0,
                src: src.id.clone(),
                uv,
                x: region.x as f32,
                y: body_base,
                w: region.w as f32,
                h: body_h,
                r: 255,
                g: 255,
                b: 255,
                a: 255,
                z,
            });
            // 头（head 底在 body_base，向上 h）
            if let Some(head_id) = nd.lnstart.get(skin_lane).filter(|s| !s.is_empty()) {
                if let Some(head) = desc.images.get(head_id) {
                    if let Some(hs) = desc.sources.get(&head.src) {
                        out.push(DrawCmd {
                            id: 2_100_000u64 + n.idx as u64,
                            blend: 0,
                            src: hs.id.clone(),
                            uv: frame_uv(head, 0, src_sizes),
                            x: region.x as f32,
                            y: body_base,
                            w: region.w as f32,
                            h,
                            r: 255,
                            g: 255,
                            b: 255,
                            a: 255,
                            z: z + 1,
                        });
                    }
                }
            }
            // 尾（tail 底在 tail_y，向上 h）
            if let Some(end_id) = nd.lnend.get(skin_lane).filter(|s| !s.is_empty()) {
                if let Some(end) = desc.images.get(end_id) {
                    if let Some(es) = desc.sources.get(&end.src) {
                        out.push(DrawCmd {
                            id: 2_200_000u64 + n.idx as u64,
                            blend: 0,
                            src: es.id.clone(),
                            uv: frame_uv(end, 0, src_sizes),
                            x: region.x as f32,
                            y: tail_y,
                            w: region.w as f32,
                            h,
                            r: 255,
                            g: 255,
                            b: 255,
                            a: 255,
                            z: z + 2,
                        });
                    }
                }
            }
        } else {
            out.push(DrawCmd {
                id: 2_000_000u64 + n.idx as u64,
                blend: 0,
                src: src.id.clone(),
                uv,
                x: region.x as f32,
                y,
                w: region.w as f32,
                h,
                r: 255,
                g: 255,
                b: 255,
                a: 255,
                z,
            });
        }
    }
}

/// 按时间 t 取 destination 当前帧（线性插值，beatoraja `prepareRegion` 语义）。
///
/// 循环规则（对齐 `SkinObject.prepareRegion`）：
/// - `loop_ == -1`：播放完（t > 末帧）后消失（`None`）
/// - `lasttime(末帧) > 0 && t > dstloop(loop_)`：
///   - `lasttime == dstloop`：固定在末帧（入场动画只播一次，如 lane-bg/gauge 的 loop=900）
///   - 否则：`(t - dstloop) % (lasttime - dstloop) + dstloop` 循环（如 artist 淡入 loop=0）
pub(crate) fn frame_at(d: &Destination, t: f64) -> Option<crate::skin::model::KeyFrame> {
    let frames = &d.frames;
    let Some(first) = frames.first() else { return None };
    if frames.len() <= 1 {
        return Some(first.clone());
    }
    let (t0, tn) = (frames[0].time as f64, frames.last().unwrap().time as f64);
    // 播完消失（loop == -1）
    if d.loop_ == -1 && t > tn {
        return None;
    }
    let dstloop = d.loop_ as f64;
    let t = if tn > 0.0 && t > dstloop {
        if tn == dstloop {
            dstloop // 入场动画播完固定末帧
        } else {
            (t - dstloop) % (tn - dstloop) + dstloop
        }
    } else {
        t
    };
    if t <= t0 {
        return Some(frames[0].clone());
    }
    for pair in frames.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let (ta, tb) = (a.time as f64, b.time as f64);
        if t >= ta && t <= tb {
            let k = if tb == ta { 0.0 } else { (t - ta) / (tb - ta) };
            let lerp = |x: i32, y: i32| (x as f64 + (y - x) as f64 * k).round() as i32;
            return Some(crate::skin::model::KeyFrame {
                time: 0,
                x: lerp(a.x, b.x),
                y: lerp(a.y, b.y),
                w: lerp(a.w, b.w),
                h: lerp(a.h, b.h),
                a: lerp(a.a, b.a),
                r: lerp(a.r, b.r),
                g: lerp(a.g, b.g),
                b: lerp(a.b, b.b),
            });
        }
    }
    Some(frames.last().unwrap().clone())
}

/// 数字网格裁剪区域（divx×divy 网格，行优先展平；与 `emit_value` 一致）。
pub fn value_uv(v: &crate::skin::model::ValueDef, f: u32) -> URect {
    let total = (v.divx * v.divy).max(1);
    let f = f.min(total - 1);
    let col = f % v.divx;
    let row = f / v.divx;
    let cw = (v.w as u32 / v.divx).max(1);
    let ch = (v.h as u32 / v.divy).max(1);
    let sx = v.x as u32 + col * cw;
    let sy = v.y as u32 + row * ch;
    URect::new(sx, sy, sx + cw, sy + ch)
}

/// 收集 source 下所有可能出现的裁剪区域（构建静态 atlas 用）：
/// images（含帧动画/整图）、values 数字网格、sliders 直接裁剪。
pub fn collect_source_uvs(
    desc: &SkinDesc,
    src_sizes: &std::collections::HashMap<String, (u32, u32)>,
    source: &str,
) -> Vec<URect> {
    let mut uvs: Vec<URect> = Vec::new();
    for img in desc.images.values() {
        if img.src != source {
            continue;
        }
        let total = (img.divx * img.divy).max(1);
        for f in 0..total {
            uvs.push(frame_uv(img, f, src_sizes));
        }
    }
    for v in &desc.values {
        if v.src != source {
            continue;
        }
        let total = (v.divx * v.divy).max(1);
        for f in 0..total {
            uvs.push(value_uv(v, f));
        }
    }
    for sl in &desc.sliders {
        if sl.src != source {
            continue;
        }
        uvs.push(URect::new(
            sl.x as u32,
            sl.y as u32,
            (sl.x + sl.w.max(1)) as u32,
            (sl.y + sl.h.max(1)) as u32,
        ));
    }
    uvs
}

/// scroll 加权 measure 距离（beatoraja LaneRenderer 逐段累积）：
/// 从 now_y 到 note_y 逐段累加 `段 measure × 段 scroll`，返回带符号（未来为正）。
/// 像素 = 结果 × region.h × hispeed（scroll 在 bms-rs 为绝对值，不含 BPM/speed——
/// BPM/#SPEED 已由 progressed_y 推进体现，STOP 由 now_y 停滞体现）。
fn scroll_measure(now_y: f64, note_y: f64, tl: &[(f64, f64)]) -> f64 {
    let sign = if note_y >= now_y { 1.0 } else { -1.0 };
    let (lo, hi) = if sign > 0.0 { (now_y, note_y) } else { (note_y, now_y) };
    let mut px = 0.0;
    let mut cur = lo;
    let mut scroll = 1.0;
    for (m, s) in tl {
        if *m <= cur {
            scroll = *s;
            continue;
        }
        if *m >= hi {
            break;
        }
        px += (*m - cur) * scroll;
        cur = *m;
        scroll = *s;
    }
    px += (hi - cur) * scroll;
    sign * px
}

/// 帧动画索引（beatoraja `SkinSourceImageSet.getImageIndex`）：
/// `(t × total / cycle) % total`；timer 开启则 t 相对 timer 开始；关闭 → 0。
fn frame_index(
    t: f64,
    timer: Option<i64>,
    total: u32,
    cycle: i32,
    timers: &[i64; 256],
) -> u32 {
    if cycle <= 0 || total <= 1 {
        return 0;
    }
    let mut t = t;
    if let Some(id) = timer {
        let tv = timers[id as usize];
        if tv == crate::skin::state::TIMER_OFF {
            return 0;
        }
        t -= tv as f64;
    }
    if t < 0.0 {
        return 0;
    }
    ((t * total as f64 / cycle as f64) as u32) % total
}

/// 按帧索引计算图像裁剪区域（divx×divy 网格，行优先展平）。
/// `w/h <= 0`（整图，如 bg/bomb）用源图尺寸 `src_sizes[img.src]`。
fn frame_uv(
    img: &crate::skin::model::ImageDef,
    frame: u32,
    src_sizes: &std::collections::HashMap<String, (u32, u32)>,
) -> URect {
    let (iw, ih) = if img.w <= 0 || img.h <= 0 {
        src_sizes
            .get(&img.src)
            .copied()
            .unwrap_or((img.w.max(1) as u32, img.h.max(1) as u32))
    } else {
        (img.w as u32, img.h as u32)
    };
    let total = (img.divx * img.divy).max(1);
    let f = frame.min(total - 1);
    let col = f % img.divx;
    let row = f / img.divx;
    let cell_w = (iw / img.divx).max(1);
    let cell_h = (ih / img.divy).max(1);
    let sx = img.x as u32 + col * cell_w;
    let sy = img.y as u32 + row * cell_h;
    URect::new(sx, sy, sx + cell_w, sy + cell_h)
}

/// judge 弹字 destination id → 判定类型（0=PG 1=GR 2=GD 3=BD 4=PR 5=MS/空POOR）。
/// 匹配 `judge-pg` / `judge-num-pg` 等（beatoraja judge images/numbers 命名）。
/// 其余 id（如 judge-line）返回 None（不受弹字过滤）。
fn judge_pop_id(d: &Destination) -> Option<u8> {
    let id = d
        .id
        .strip_prefix("judge-num-")
        .or_else(|| d.id.strip_prefix("judge-"))?;
    match id {
        "pg" => Some(0),
        "gr" => Some(1),
        "gd" => Some(2),
        "bd" => Some(3),
        "pr" => Some(4),
        "ms" => Some(5),
        _ => None,
    }
}

/// 帧求值：destination（op 条件 + timer 动画插值 + 数字读 state）+ 判定弹字 +
/// 血量条 + note 下落。text/slider/graph 暂跳过（M4 / M3 后半）。
///
/// 填充到调用方提供的缓冲（`out.clear()` 后 push，避免每帧分配）；
/// 测试可走 [`evaluate_frame`] 包装。
pub fn evaluate_into(
    desc: &SkinDesc,
    config: &SkinConfigValues,
    state: &Arc<RwLock<PlayState>>,
    src_sizes: &std::collections::HashMap<String, (u32, u32)>,
    out: &mut Vec<DrawCmd>,
) {
    out.clear();
    let base_z = desc.destinations.len() as i32;
    for (z, d) in desc
        .destinations
        .iter()
        .chain(desc.judge_objects.iter())
        .enumerate()
    {
        let id = 1_000_000u64 + (z as u64) * 64; // 每 destination 独立 64 槽（数字位/血条粒不侵入相邻对象 id）
        let z = if z < base_z as usize { z as i32 } else { base_z + (z as i32 - base_z) };
        // op 条件（正 = 选中才显示，负 = 未选中才显示）
        let visible = d.op.iter().all(|&id| {
            if id > 0 {
                config.is_option_enabled(id)
            } else {
                !config.is_option_enabled(-id)
            }
        });
        if !visible {
            continue;
        }
        // judge 弹字（judge_objects）：所有 judge-pg/gr/gd/bd/pr 共用 TIMER_JUDGE_1P（46），
        // 必须按最近判定的类型过滤——否则一次判定后全部弹字图重叠显示（GREAT 与 POOR 混在一起）。
        if z >= base_z
            && let Some(expect) = judge_pop_id(d)
        {
            let s = match state.read() {
                Ok(s) => s,
                Err(_) => {
                    out.clear();
                    return;
                }
            };
            let hit = s.judge_pops.iter().any(|p| p.judgement == expect);
            if !hit {
                continue;
            }
        }
        // timer 驱动：显式 timer 关闭 → 不绘制（beatoraja `prepareRegion` draw=false）；
        // timer 值 = 开启时刻 → 动画时间 = scene_time - 开启时刻（beatoraja `time - timer.get`）
        let timer_val = match d.timer {
            Some(id) if (id as usize) < 256 => {
                let s = match state.read() {
                    Ok(s) => s,
                    Err(_) => {
                    out.clear();
                    return;
                }
                };
                let v = s.timers[id as usize];
                if v == crate::skin::state::TIMER_OFF {
                    continue;
                }
                (s.scene_time_ms - v as f64).max(0.0)
            }
            Some(_) => crate::skin::state::TIMER_OFF as f64,
            None => {
                let s = match state.read() {
                    Ok(s) => s,
                    Err(_) => {
                    out.clear();
                    return;
                }
                };
                // 无 timer：用场景时间（含加载）——入场动画在 playstart 前完成
                s.scene_time_ms
            }
        };
        // 无 timer 且首帧时间未到（beatoraja `starttime > time` → draw=false）
        if d.timer.is_none() {
            let first_time = d.frames.first().map_or(0.0, |f| f.time as f64);
            if timer_val < first_time {
                continue;
            }
        }
        let Some(frame) = frame_at(d, timer_val) else { continue };
        // 数字对象优先（多位指令）
        if let Some(v) = desc.value(&d.id) {
            if emit_value(desc, v, d, id, state, z as i32, &frame, out).is_ok() {
                continue;
            }
        }
        // 血量条（beatoraja SkinGauge）：条分 parts 粒，每粒按状态选
        // `type×6 + [满, 满border, 空, 空border, 当前, 当前border]` 节点。
        // type/max/border 由 gameplay 血条类型（GaugeState）每帧同步到 PlayState。
        if let Some(g) = &desc.gauge {
            if g.id == d.id && !g.nodes.is_empty() && g.nodes.len() >= 6 {
                let s = match state.read() {
                    Ok(s) => s,
                    Err(_) => {
                    out.clear();
                    return;
                }
                };
                let gauge_type = s.gauge_type;
                let max = s.gauge_max.clamp(1.0, 1000.0);
                let border = s.gauge_border;
                let parts = g.parts.max(1);
                // beatoraja SkinGauge.draw：CLASS(6)/EXCLASS(7)/EXHARDCLASS(8) 折回 3/4/5 组
                // （段位血条共用 HARD 系外观组），其余类型按索引直接乘 6
                let group = if gauge_type >= 6 { gauge_type - 3 } else { gauge_type };
                let exgauge = (group * 6) as usize;
                let value = s.gauge.clamp(0.0, max);
                let notes = if value > 0.0 {
                    ((value * parts as f64 / max).max(1.0)) as i32
                } else {
                    0
                };
                let z = z as i32;
                for i in 1..=parts {
                    let border_at = i as f64 * max / parts as f64;
                    let state_idx = if notes == i {
                        4
                    } else if notes > i {
                        0
                    } else {
                        2
                    };
                    let idx = state_idx + if border_at < border { 1 } else { 0 };
                    let Some(node_id) = g.nodes.get(exgauge + idx) else { continue };
                    let Some(img) = desc.images.get(node_id) else { continue };
                    let Some(src) = desc.sources.get(&img.src) else { continue };
                    let x = frame.x as f32 + frame.w as f32 * (i - 1) as f32 / parts as f32;
                    let w = frame.w as f32 / parts as f32;
                    out.push(DrawCmd {
                        id: id + i as u64,
                        blend: d.blend,
                        src: src.id.clone(),
                        uv: frame_uv(img, 0, src_sizes),
                        x,
                        y: frame.y as f32,
                        w,
                        h: frame.h as f32,
                        r: frame.r as u8,
                        g: frame.g as u8,
                        b: frame.b as u8,
                        a: frame.a as u8,
                        z,
                    });
                }
                continue;
            }
        }
        // slider：沿 angle 方向按进度填充（type=6 进度条 / type=4 轨道遮挡）。
        // 进度 = value 回调（无 → 时长比例）
        if let Some(sl) = desc.sliders.iter().find(|s| s.id == d.id) {
            let s = match state.read() {
                Ok(s) => s,
                Err(_) => {
                    out.clear();
                    return;
                }
            };
            let value = if sl.value.is_some() {
                0.0 // 自定义回调值 M3 余量后补
            } else if s.total_sec > 0.0 && (sl.type_ == 6 || sl.type_ == 102) {
                (s.duration_sec / s.total_sec).clamp(0.0, 1.0)
            } else {
                0.0
            };
            // beatoraja SkinSlider.draw：**固定大小图像**（region.w/h 不变）沿 direction
            // 移动 value×range（不是拉伸！song-progress 进度点 12×24 不变、随歌曲移动）
            let off = value as f32 * sl.range.max(0) as f32;
            // direction：0=y+（上移）1=x+（右移）2=y-（下移）3=x-（左移），libGDX y 向上
            let (dx, dy) = match sl.angle {
                1 => (off, 0.0),
                2 => (0.0, -off),
                3 => (-off, 0.0),
                _ => (0.0, off),
            };
            // src 是 source id，裁剪区域 = 对象自身 x/y/w/h（图集内）
            if let Some(src) = desc.sources.get(&sl.src) {
                out.push(DrawCmd {
                    id,
                    blend: d.blend,
                    src: src.id.clone(),
                    uv: URect::new(
                        sl.x as u32,
                        sl.y as u32,
                        (sl.x + sl.w.max(1)) as u32,
                        (sl.y + sl.h.max(1)) as u32,
                    ),
                    x: frame.x as f32 + dx,
                    y: frame.y as f32 + dy,
                    w: frame.w as f32,
                    h: frame.h as f32,
                    r: frame.r as u8,
                    g: frame.g as u8,
                    b: frame.b as u8,
                    a: frame.a as u8,
                    z: z as i32,
                });
            }
            continue;
        }
        // graph：按 type 取 0-1 值画填充条（beatoraja SkinGraph：
        // direction=1 垂直（从底向上 h×value），其他水平（从左 w×value）。
        // type 110/111 = 当前/最终分数比率；best/target 无数据 → 0。
        if let Some(g) = desc.graphs.iter().find(|g| g.id == d.id) {
            let s = match state.read() {
                Ok(s) => s,
                Err(_) => {
                    out.clear();
                    return;
                }
            };
            let value = match g.type_ {
                110 | 111 => (s.rate_x100() as f64 / 10000.0).clamp(0.0, 1.0),
                _ => 0.0, // best/target 无 rival/best 数据
            };
            if value <= 0.0 {
                continue;
            }
            // beatoraja SkinGraph：src 是 source id，裁剪区域 = 对象自身 x/y/w/h（图集内）
            let Some(src) = desc.sources.get(&g.src) else { continue };
            let (dw, dh) = if g.angle == 1 {
                (frame.w as f32, frame.h as f32 * value as f32)
            } else {
                (frame.w as f32 * value as f32, frame.h as f32)
            };
            if dw <= 0.0 || dh <= 0.0 {
                continue;
            }
            out.push(DrawCmd {
                id,
                blend: d.blend,
                src: src.id.clone(),
                uv: URect::new(
                    g.x as u32,
                    g.y as u32,
                    (g.x + g.w.max(1)) as u32,
                    (g.y + g.h.max(1)) as u32,
                ),
                x: frame.x as f32,
                y: frame.y as f32,
                w: dw,
                h: dh,
                r: frame.r as u8,
                g: frame.g as u8,
                b: frame.b as u8,
                a: frame.a as u8,
                z: z as i32,
            });
            continue;
        }
        // BGA 特殊 destination（id="bga"，beatoraja `skin.bga`）：无 image 定义，
        // 绘制当前 BGA 帧（stretch 到 dst 区域；uv=(0,0,0,0) 整图标记）。
        if d.id == "bga" {
            out.push(DrawCmd {
                id,
                blend: d.blend,
                src: "__bga__".into(),
                uv: URect::new(0, 0, 0, 0),
                x: frame.x as f32,
                y: frame.y as f32,
                w: frame.w as f32,
                h: frame.h as f32,
                r: frame.r as u8,
                g: frame.g as u8,
                b: frame.b as u8,
                a: frame.a as u8,
                z: z as i32,
            });
            continue;
        }
        if let Some(cmd) = DrawCmd::from_image(desc, d, id, z as i32, &frame, state, src_sizes) {
            out.push(cmd);
        }
    }
    emit_notes(desc, state, out, src_sizes);
}

/// 包装：返回新分配的 DrawCmd 列表（测试用）。
pub fn evaluate_frame(
    desc: &SkinDesc,
    config: &SkinConfigValues,
    state: &Arc<RwLock<PlayState>>,
    src_sizes: &std::collections::HashMap<String, (u32, u32)>,
) -> Vec<DrawCmd> {
    let mut out = Vec::new();
    evaluate_into(desc, config, state, src_sizes, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skin::lua::{LuaSkin, SkinConfigValues};
    use crate::skin::model::SkinDesc;
    use crate::skin::state::NoteState;

    fn load() -> (LuaSkin, SkinDesc, SkinConfigValues, Arc<RwLock<PlayState>>) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/test_skin/Play");
        let skin = LuaSkin::new(&dir).unwrap();
        let entry = dir.join("Play5.luaskin");
        let header = skin.load_header(&entry).unwrap();
        let cfg = SkinConfigValues::from_header(&header);
        let t = skin.load_skin(&entry, &header, &cfg).unwrap();
        let desc = SkinDesc::from_table(skin.lua(), &t, header.w, header.h).unwrap();
        let state = Arc::new(RwLock::new(PlayState::default()));
        (skin, desc, cfg, state)
    }

    /// 测试用状态：非空曲目 + 一个可见音符。
    fn sample_state() -> PlayState {
        PlayState {
            now_time_ms: 5000.0, // play 时间（main_state.time()）
            scene_time_ms: 5000.0, // 场景时间（含加载，驱动入场动画）
            now_y: 10.0,
            visible_y: 100.0,
            total_notes: 500,
            ex_score: 700,
            bpm_now: 180.0,
            bpm_min: 100.0,
            bpm_max: 200.0,
            gauge: 75.0,
            title: "Test".into(),
            artist: "Artist".into(),
            genre: "Genre".into(),
            notes: vec![
                NoteState {
                    idx: 0,
                    lane: 0,
                    position: 10.4,
                    length: None,
                    kind: 0,
                    consumed: false,
                    ln_active: false,
                },
                NoteState {
                    idx: 1,
                    lane: 1,
                    position: 10.25,
                    length: Some(0.2),
                    kind: 1,
                    consumed: false,
                    ln_active: true,
                },
                NoteState {
                    idx: 2,
                    lane: 5,
                    position: 10.5,
                    length: None,
                    kind: 2,
                    consumed: false,
                    ln_active: false,
                },
                NoteState {
                    idx: 3,
                    lane: 2,
                    position: 10.2,
                    length: None,
                    kind: 0,
                    consumed: true,
                    ln_active: false,
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn virtual_screen_fit() {
        let vs = VirtualScreen::fit(1920.0, 1080.0, 1280.0, 720.0);
        assert!((vs.scale - 0.6666667).abs() < 1e-5);
        // 皮肤 y 向上、原点左下：虚拟 (0,0) 左下角 → world 左下方
        assert!((vs.world_x(0.0) - (-640.0)).abs() < 1e-4);
        assert!((vs.world_y(0.0) - (-360.0)).abs() < 1e-4);
        // 虚拟 (1920,1080) → 右上方（y 向上）
        assert!((vs.world_x(1920.0) - 640.0).abs() < 1e-4);
        assert!((vs.world_y(1080.0) - 360.0).abs() < 1e-4);
        // 中心不变
        assert!(vs.world_x(960.0).abs() < 1e-4);
        assert!(vs.world_y(540.0).abs() < 1e-4);
        // 信箱：16:9 窗口等比
        let vs2 = VirtualScreen::fit(1920.0, 1080.0, 1600.0, 720.0);
        assert!((vs2.scale - 0.6666667).abs() < 1e-5);
        // 判定线 y=200（屏幕底部 200px）应在窗口下方
        assert!(vs.world_y(200.0) < 0.0, "judge-line y=200 应在下方");
        // title y=1022（顶部）应在窗口上方
        assert!(vs.world_y(1022.0) > 0.0, "title y=1022 应在上方");
    }

    #[test]
    fn evaluate_frame_background_frame() {
        let (_skin, desc, cfg, state) = load();
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        // 默认配置（Pure Mode Off 选中）：bg + frame 可见
        let bg = cmds.iter().find(|c| c.src == "src-bg").expect("缺少 bg 指令");
        assert_eq!((bg.x, bg.y, bg.w, bg.h), (0.0, 0.0, 1920.0, 1080.0));
        let frame = cmds.iter().find(|c| c.src == "src-frame").expect("缺少 frame 指令");
        assert_eq!((frame.x, frame.y, frame.w, frame.h), (0.0, 0.0, 1920.0, 1080.0));
        assert_eq!(frame.uv, URect::new(0, 0, 1920, 1080));
        // judge-line：776×15
        let jl = cmds
            .iter()
            .find(|c| c.uv == URect::new(0, 0, 776, 15))
            .expect("缺少 judge-line 指令");
        assert_eq!(jl.src, "src-judgeline");
        // z 顺序：bg 在前
        let bg_z = cmds.iter().position(|c| c.src == "src-bg").unwrap();
        assert_eq!(bg_z, 0);
    }

    #[test]
    fn evaluate_frame_op_conditions() {
        let (_skin, desc, cfg, state) = load();
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        // artist op=-1008（未选中显示）：应有 artist 文本……但 text 对象 M2 跳过，
        // 这里验证 op 条件影响 image 类对象。取任意 op>0 的对象验证被过滤：
        // judge-pg op={32} 未选中 → 不生成指令
        assert!(
            cmds.iter().all(|c| c.src != "src-judge"),
            "judge op=32 未选中不应显示"
        );
        // 2P 分支：frame op 无条件，但 lane-bg-2p 在 is1p 分支……检查 bg 仍存在
        assert!(cmds.iter().any(|c| c.src == "src-bg"));
    }

    #[test]
    fn number_block_left_edge_is_destination_x() {
        // beatoraja SkinNumber：数字块左边界 = destination.x（region.x），align 只影响
        // 块内前导排布。此前 align=0 被实现成"右对齐向左延伸"，导致数字块左移
        // 半个块宽、被旁边图标盖住。验证首位数字 x = destination 末帧 x。
        let (_skin, desc, cfg, state) = load();
        *state.write().unwrap() = sample_state();
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        let d = desc
            .destinations
            .iter()
            .find(|d| d.id == "gauge-num")
            .expect("gauge-num destination");
        let z = desc
            .destinations
            .iter()
            .position(|d| d.id == "gauge-num")
            .expect("gauge-num destination");
        let dest_x = desc.destinations[z].frames.last().expect("末帧").x as f32;
        let base = 1_000_000u64 + (z as u64) * 64;
        let gnums: Vec<_> = cmds
            .iter()
            .filter(|c| c.id >= base && c.id - base < 5)
            .collect();
        assert_eq!(
            gnums.len(),
            2,
            "gauge-num（75）应画 2 位（前导空不画）: {:?}",
            gnums.iter().map(|c| (c.id, c.x)).collect::<Vec<_>>()
        );
        let max_x = gnums.iter().map(|c| c.x).fold(f32::MIN, f32::max);
        // align=0（右对齐，无前导填充）：数字右缘 = 块右边界 dest_x + digit×36
        assert!(
            (max_x + 36.0 - (dest_x + 3.0 * 36.0)).abs() < 0.5,
            "gauge-num 右缘应在块右边界 dest_x+108（实际 {max_x}）"
        );
        // 前导空占 1 格：首位 x = dest_x + 36
        assert!(
            gnums.iter().any(|c| (c.x - (dest_x + 36.0)).abs() < 0.5),
            "gauge-num 首位应在 dest_x+36（前导空占 1 格）"
        );
        assert!(
            gnums.iter().all(|c| c.x >= dest_x - 0.5),
            "数字不得向左超出 destination（块左边界）"
        );
    }

    #[test]
    fn graph_objects_emit_fill() {
        // beatoraja SkinGraph：type 110/111（当前/最终分数比率）画水平填充条
        let (_skin, desc, cfg, state) = load();
        *state.write().unwrap() = sample_state();
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        let z = desc
            .destinations
            .iter()
            .position(|d| d.id == "graph-you")
            .expect("graph-you destination");
        let dest = &desc.destinations[z];
        let dest_w = dest.frames.last().expect("末帧").w as f32;
        let dest_h = dest.frames.last().expect("末帧").h as f32;
        let base = 1_000_000u64 + (z as u64) * 64;
        let g = cmds
            .iter()
            .find(|c| c.id == base)
            .expect("graph-you 应有填充条指令");
        // sample：ex=700, notes=500 → rate = 0.7。
        // graph-you（angle 默认 1 = 垂直）：w=满宽，h = frame.h × rate（从底向上填充）
        let rate = 700.0 / (500.0 * 2.0);
        assert!(
            (g.w - dest_w).abs() < 0.5,
            "graph-you 垂直条宽应 = dest_w（{}），实际 {}",
            dest_w,
            g.w
        );
        assert!(
            (g.h - dest_h * rate as f32).abs() < 0.5,
            "graph-you 填充高应 = dest_h×rate（{}），实际 {}",
            dest_h * rate as f32,
            g.h
        );
        // graph-best（type=113，无 best 数据）→ 不生成
        let zb = desc
            .destinations
            .iter()
            .position(|d| d.id == "graph-best")
            .expect("graph-best destination");
        let baseb = 1_000_000u64 + (zb as u64) * 64;
        assert!(
            !cmds.iter().any(|c| c.id == baseb),
            "graph-best（无数据）不应生成指令"
        );
    }

    #[test]
    fn number_digits_ordered_left_to_right() {
        // gauge-num（ref=107，值 75）→ 2 位 "75"：十位（7）在最左、个位（5）在最右。
        // 此前 digits 索引反了（个位画在最左），数字显示成倒序。
        let (_skin, desc, cfg, state) = load();
        *state.write().unwrap() = sample_state();
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        let z = desc
            .destinations
            .iter()
            .position(|d| d.id == "gauge-num")
            .expect("gauge-num destination");
        let base = 1_000_000u64 + (z as u64) * 64;
        let mut nums: Vec<_> = cmds
            .iter()
            .filter(|c| c.id >= base && c.id - base < 5)
            .collect();
        assert_eq!(nums.len(), 2, "gauge-num(75) 应画 2 位（前导空不画）");
        nums.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
        // src-number divx=10 每格 36px：数字 7 → x=252，5 → x=180
        assert_eq!(nums[0].uv.min.x, 252, "最高位（十位）应最左且为 7");
        assert_eq!(nums[1].uv.min.x, 180, "个位应最右且为 5");
        assert!(nums[0].x < nums[1].x, "高位在左、低位在右");
    }

    #[test]
    fn evaluate_frame_value_digits() {
        let (_skin, desc, cfg, state) = load();
        *state.write().unwrap() = sample_state();
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        // gauge/score 区（y=124）：gauge-num（3 位）+ gauge-dnum（1 位）+ ex-score（4 位）
        // （gauge-num 入场动画 loop 推进后可能滑到正 x，不再按 x 过滤）
        let y124: Vec<_> = cmds
            .iter()
            .filter(|c| c.y == 124.0 && c.h == 46.0 && c.w == 36.0 && c.uv.min.y == 0)
            .collect();
        // score 区（y=886 = exscore_y）：ex-score（700 → 3 位，前导空不画）；
        // ex-score-5d 仅在 long chart 分支（main_state.number(74) 骨架 0 → 不生成）
        let score: Vec<_> = cmds
            .iter()
            .filter(|c| c.y == 886.0 && c.h == 46.0 && c.w == 36.0 && c.uv.min.y == 0)
            .collect();
        assert!(
            score.len() >= 1,
            "score 区（y=886）应有 ex-score 数字（实际 {} 条）",
            score.len()
        );
        // 数字单元格 UV：每位 36×46 网格格（gauge-num 显示 75 → 7/5 两格）
        assert!(
            y124.iter().all(|c| {
                c.uv.min.y == 0
                    && c.uv.width() == 36
                    && c.uv.height() == 46
                    && c.uv.min.x % 36 == 0
            }),
            "gauge 数字 UV 应为 36×46 网格格"
        );
        // 至少存在 gauge-num（75，2 位）与 gauge-dnum（1 位）的数字指令
        assert!(
            y124.len() >= 3,
            "y=124 应有 gauge-num + gauge-dnum 数字（实际 {} 条）",
            y124.len()
        );
    }

    #[test]
    fn evaluate_frame_notes() {
        let (_skin, desc, cfg, state) = load();
        *state.write().unwrap() = sample_state();
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        let nd = desc.note.as_ref().expect("应有 skin.note");
        let region = &nd.dst[0];
        let judge_y = region.y as f32;
        let px = region.h as f32 / 100.0;
        // scratch（lane 0）→ 皮肤轨道 5：note-s UV 0,0 140×30，底边对齐判定位置
        let scratch = cmds
            .iter()
            .filter(|c| c.uv == URect::new(0, 0, 140, 30))
            .collect::<Vec<_>>();
        assert!(!scratch.is_empty(), "scratch note（note-s）应存在");
        // px = 880×hispeed(1) = 880/measure；dy=0.4 → y=200+352=552
        assert!(
            scratch
                .iter()
                .any(|c| (c.y - 552.0).abs() < 0.5),
            "scratch 底边应对齐 200+0.4×880=552"
        );
        // 长音（lane 1 → 皮肤轨道 0）：ln_active=true → lna-w（140,90 90×30）body 拉伸
        let ln_body = cmds
            .iter()
            .filter(|c| c.uv == URect::new(140, 90, 230, 120))
            .collect::<Vec<_>>();
        assert!(!ln_body.is_empty(), "长音 body（lna-w）应存在");
        // body 底 = 判定线（ln_active=true：命中后 head 固定在判定线，body 从判定线
        // 延伸到 tail）→ 200；tail_y = 200 + 0.45 measure×880 = 596 → 高 396
        assert!(
            (ln_body[0].y - 200.0).abs() < 0.5,
            "命中后 body 底固定在判定线（200）"
        );
        assert!((ln_body[0].h - 396.0).abs() < 0.5, "body 高=0.45 measure×880=396");
        // 地雷：lane 5（键5）→ 皮肤轨道 4 → mine-w（140,390 90×30）
        assert!(
            cmds.iter().any(|c| c.uv == URect::new(140, 390, 230, 420)),
            "地雷（mine-w）应存在"
        );
        // consumed 音符不渲染：lane 2（键2）→ 皮肤轨道 1 = note-b（230,0 80×30），应无该指令
        let note_b = cmds
            .iter()
            .filter(|c| c.uv == URect::new(230, 0, 310, 30))
            .count();
        assert_eq!(note_b, 0, "consumed 音符不应渲染");
    }

    #[test]
    fn frame_interpolation() {
        // artist 淡入 destination：op=1008，frames 0/250/3250/3500/7000（a 0→255→255→0→0）
        let (_skin, desc, cfg, state) = load();
        let artist = desc
            .destinations
            .iter()
            .find(|d| d.id == "artist" && d.op == vec![1008])
            .expect("artist 动画 destination");
        // timer 关闭 → 首帧（a=0）
        let s = state.read().unwrap();
        let off = crate::skin::state::TIMER_OFF as f64;
        let f0 = frame_at(artist, off).unwrap();
        assert_eq!(f0.a, 0);
        // t=250 → 正好第二帧 a=255
        let f1 = frame_at(artist, 250.0).unwrap();
        assert_eq!(f1.a, 255);
        // t=125 → 半程 a≈127（0→255 中点）
        let fmid = frame_at(artist, 125.0).unwrap();
        assert!((fmid.a - 127).abs() <= 1, "a={}", fmid.a);
        // t=7000 → 末帧 a=0
        let fend = frame_at(artist, 7000.0).unwrap();
        assert_eq!(fend.a, 0);
        // 位置插值：t=125 x 应为 首帧x 与 250帧x 的中点
        assert!((fmid.x as f64 - (f0.x as f64 + f1.x as f64) / 2.0).abs() <= 1.0);
        drop(s);
        // loop=-1（judge 弹字）：播完（t > 末帧）→ 消失
        let judge = desc
            .judge_objects
            .iter()
            .find(|d| d.id == "judge-pg")
            .expect("judge-pg");
        assert!(frame_at(judge, 499.0).is_some(), "t=499 应显示");
        assert!(frame_at(judge, 501.0).is_none(), "t=501 播完应消失");
    }

    #[test]
    fn frame_at_entrance_animation_fixed() {
        // lane-bg-1p：loop=900 且末帧 time=900 → 播完固定末帧（不再循环滑动）
        let (_skin, desc, cfg, state) = load();
        let _ = cfg;
        let lane_bg = desc
            .destinations
            .iter()
            .find(|d| d.id == "lane-bg-1p")
            .expect("lane-bg-1p");
        assert_eq!(lane_bg.loop_, 900);
        assert_eq!(lane_bg.frames.last().unwrap().time, 900);
        // 动画中途：t=500 → 位置在 0..900 帧之间插值（y 从 1100 向 200）
        let f_mid = frame_at(lane_bg, 500.0).unwrap();
        assert!(f_mid.y > 200, "中途 y 应大于末帧 y（仍在滑入）");
        // 播完（t > 900）：固定在末帧 y=lane_y=200
        let f_end = frame_at(lane_bg, 5000.0).expect("播完应固定末帧");
        assert_eq!(f_end.y, 200, "入场动画播完应固定末帧 y=200");
        let f_end2 = frame_at(lane_bg, 10_000.0).unwrap();
        assert_eq!(f_end2.y, 200, "长时间后仍固定");
        // artist 淡入（loop=0, 末帧 7000）：仍然循环（tn != loop）
        let artist = desc
            .destinations
            .iter()
            .find(|d| d.id == "artist" && d.op == vec![1008])
            .unwrap();
        assert_ne!(artist.frames.last().unwrap().time as i32, artist.loop_);
        let f_loop = frame_at(artist, 7500.0).unwrap();
        assert_eq!(f_loop.a, 255, "t=7500 回绕到 500 → 区间 250-3250 a=255");
    }

    #[test]
    fn gauge_bg_background_renders() {
        // 血条底色（gauge-bg）与空粒（半透明 alpha=64）应都渲染：
        // 空粒不透明像素 alpha=64，DrawCmd.a 保留帧 alpha
        let (_skin, desc, cfg, state) = load();
        *state.write().unwrap() = sample_state();
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        let z = desc
            .destinations
            .iter()
            .position(|d| d.id == "gauge-bg")
            .expect("gauge-bg destination");
        let base = 1_000_000u64 + (z as u64) * 64;
        let bg = cmds.iter().find(|c| c.id == base).expect("gauge-bg 应有指令");
        assert!(bg.w > 100.0 && bg.h == 38.0, "gauge-bg 底色条: w={} h={}", bg.w, bg.h);
        // 血条粒（src-gauge）应同时存在满粒与半透明空粒（空粒 alpha 由图集提供）
        let grains: Vec<_> = cmds.iter().filter(|c| c.src == "src-gauge").collect();
        assert!(!grains.is_empty(), "应有血条粒");
    }

    #[test]
    fn gauge_nodes_selection() {
        let (_skin, desc, cfg, state) = load();
        let g = desc.gauge.as_ref().expect("应有 skin.gauge");
        assert_eq!(g.id, "gauge");
        assert_eq!(g.nodes.len(), 36);
        assert_eq!(g.parts, 50);
        // NORMAL（type=2）组：nodes[12..18] = r1,b1,r2,b2,r3,b3（64/96/80/112/64/96,0）
        // 0% → notes=0 → 全空粒：i<40 border 下 → b2(112,0)；i≥40 → r2(80,0)
        *state.write().unwrap() = sample_state();
        state.write().unwrap().gauge = 0.0;
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        assert_eq!(cmds.iter().filter(|c| c.src == "src-gauge").count(), 50, "应画 50 粒");
        let empty_border = cmds.iter().filter(|c| c.uv == URect::new(112, 0, 128, 34)).count();
        let empty_safe = cmds.iter().filter(|c| c.uv == URect::new(80, 0, 96, 34)).count();
        assert_eq!(empty_border, 39, "i=1..39 空粒 = b2");
        assert_eq!(empty_safe, 11, "i=40..50 空粒 = r2");
        // 100% → notes=50 → 全部满粒（状态 0，border 下 i<40 → 状态 1 = b1 96,0；
        // i≥40 状态 0 = r1 64,0）
        state.write().unwrap().gauge = 100.0;
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        let filled = cmds.iter().filter(|c| c.uv == URect::new(96, 0, 112, 34)).count();
        let safe = cmds.iter().filter(|c| c.uv == URect::new(64, 0, 80, 34)).count();
        assert_eq!(filled, 39, "i=1..39 border 下满粒 = b1(96,0)");
        assert_eq!(safe, 11, "i=40..50 border 上满粒 = r1(64,0)");
        // 50% → notes=25：i1-24 满(b1 96,0)、i25 当前(b3 96,0，border 下)、
        // i26-39 空(b2 112,0)、i40-50 空(r2 80,0)
        state.write().unwrap().gauge = 50.0;
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        let filled_border = cmds.iter().filter(|c| c.uv == URect::new(96, 0, 112, 34)).count();
        assert_eq!(filled_border, 25, "24 满 + 1 当前 = b 系 96,0");
        // 第 25 粒（当前粒）x = 50 + 800*24/50 = 434
        let cur = cmds
            .iter()
            .find(|c| (c.x - 434.0).abs() < 1.0)
            .expect("第 25 粒");
        assert_eq!(cur.uv, URect::new(96, 0, 112, 34), "当前粒 = b3");
        assert!(cmds.iter().filter(|c| c.src == "src-gauge").all(|c| (c.y - 74.0).abs() < 0.5), "gauge 底边 y=74");
        // 粒位置：gauge 固定末帧 x=50，第 i 粒 x = 50 + 800*(i-1)/50，粒宽 16
        let first = cmds
            .iter()
            .filter(|c| c.src == "src-gauge")
            .min_by_key(|c| c.x as i32)
            .unwrap();
        assert!((first.x - 50.0).abs() < 1.0, "首粒 x=50（末帧固定）");
        assert!((first.w - 16.0).abs() < 0.1, "粒宽 800/50=16");
    }

    #[test]
    fn hit_effects_emit() {
        let (_skin, desc, mut cfg, state) = load();
        // 开启全局选项 81（OPTION_LOADED，keybeam 显示条件）
        for (k, v) in cfg.op_map.iter_mut() {
            if *k == 81 {
                *v = true;
            }
        }
        *state.write().unwrap() = sample_state();
        {
            // timers 存**开启时刻**（scene_time=5000 基准）：动画时间 = 5000 - 开启时刻
            let mut s = state.write().unwrap();
            s.timers[crate::skin::state::TIMER_JUDGE_1P] = 4900; // elapsed 100ms
            s.timers[crate::skin::state::TIMER_BOMB_1P_SCRATCH + 1] = 4950; // elapsed 50ms
            s.timers[crate::skin::state::TIMER_KEYON_1P_SCRATCH + 1] = 4950; // elapsed 50ms
            // 最近判定：PG（lane 1）——judge-pg 弹字应显示，其余 judge-* 不显示
            s.judge_pops.push(crate::skin::state::JudgePop {
                lane: 1,
                judgement: 0,
                at_ms: 4900.0,
            });
        }
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        assert!(
            cmds.iter().any(|c| c.src == "src-judge"),
            "判定后应显示 judge 弹字"
        );
        // 判定过滤：只有 PG 弹字（uv y=0 起始段）显示，GR/PR 等不得重叠显示
        let judge_pg = cmds
            .iter()
            .filter(|c| c.src == "src-judge")
            .filter(|c| c.uv.min.y == 0)
            .count();
        let judge_pr = cmds
            .iter()
            .filter(|c| c.src == "src-judge")
            .filter(|c| c.uv.min.y == 560)
            .count();
        assert!(judge_pg >= 1, "PG 判定应显示 PG 弹字（{judge_pg}）");
        assert_eq!(judge_pr, 0, "POOR 弹字不得与 PG 重叠显示");
        assert!(cmds.iter().any(|c| c.src == "src-bomb"), "判定后应显示 bomb");
        assert!(
            cmds.iter().any(|c| c.src == "src-keybeam"),
            "按键后应显示 keybeam"
        );
        // JUDGE timer 关闭 → 弹字消失（loop=-1 播完也消失）
        {
            let mut s = state.write().unwrap();
            s.timers[crate::skin::state::TIMER_JUDGE_1P] = crate::skin::state::TIMER_OFF;
        }
        let cmds2 = evaluate_frame(&desc, &cfg, &state, &Default::default());
        assert!(
            !cmds2.iter().any(|c| c.src == "src-judge"),
            "judge 关闭后不显示弹字"
        );
    }

    #[test]
    fn emit_notes_scroll_weighted() {
        // scroll 2 的段内，note 位置像素翻倍（beatoraja 逐段）
        let (_skin, desc, cfg, state) = load();
        *state.write().unwrap() = sample_state();
        {
            let mut s = state.write().unwrap();
            s.scroll_timeline = vec![(0.0, 1.0), (10.3, 2.0)]; // 10.3 起 scroll=2
            s.notes.clear();
            s.notes.push(NoteState {
                idx: 0,
                lane: 0,
                position: 10.4,
                length: None,
                kind: 0,
                consumed: false,
                ln_active: false,
            });
        }
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        // now_y=10，note 10.4：10..10.3 scroll1（0.3×880）+ 10.3..10.4 scroll2（0.1×880×2）
        // y = 200 + (0.3×880 + 0.1×880×2) = 200 + 264 + 176 = 640
        let scratch = cmds
            .iter()
            .find(|c| c.uv == URect::new(0, 0, 140, 30))
            .expect("scratch note");
        assert!((scratch.y - 640.0).abs() < 1.0, "scroll 加权 y={}", scratch.y);
    }
    #[test]
    fn song_progress_point_moves() {
        // song-progress：angle=2，固定大小 12×24 随进度向下移动（y = 1026 - value×826）
        let (_skin, desc, cfg, state) = load();
        let sp = desc.sliders.iter().find(|s| s.id == "song-progress").unwrap();
        assert_eq!(sp.angle, 2);
        assert_eq!(sp.range, 826);
        *state.write().unwrap() = sample_state();
        {
            let mut s = state.write().unwrap();
            s.duration_sec = 0.0;
            s.total_sec = 100.0;
        }
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        let p = cmds
            .iter()
            .find(|c| c.uv == URect::new(0, 2000, 12, 2024))
            .expect("song-progress 点");
        // 固定大小（不被拉长）
        assert_eq!((p.w, p.h), (12.0, 24.0), "进度点大小应固定");
        // 0% → 顶部（y=1026）
        assert!((p.y - 1026.0).abs() < 0.5, "0% 应在顶部 y=1026");
        // 50% → 下移 413（1026 - 0.5×826）
        state.write().unwrap().duration_sec = 50.0;
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        let p50 = cmds
            .iter()
            .find(|c| c.uv == URect::new(0, 2000, 12, 2024))
            .unwrap();
        assert!((p50.y - (1026.0 - 413.0)).abs() < 1.0, "50% y={}", p50.y);
        // 100% → 底部（200）
        state.write().unwrap().duration_sec = 100.0;
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        let p100 = cmds
            .iter()
            .find(|c| c.uv == URect::new(0, 2000, 12, 2024))
            .unwrap();
        assert!((p100.y - 200.0).abs() < 1.0, "100% y={}", p100.y);
    }
    #[test]
    fn atlas_uvs_collect() {
        // collect_source_uvs：收集 src-notes 的图像区域（含 note 变体）
        let (_skin, desc, _cfg, _state) = load();
        let uvs = collect_source_uvs(&desc, &Default::default(), "src-notes");
        assert!(uvs.iter().any(|u| *u == URect::new(140, 0, 230, 30)), "note-w");
        assert!(uvs.iter().any(|u| *u == URect::new(230, 0, 310, 30)), "note-b");
        assert!(uvs.iter().any(|u| *u == URect::new(0, 0, 140, 30)), "note-s");
        // lnb divy=2 → 两帧都收集
        // lnb-w：140,120 90×60 divy=2 → 帧高 30
        assert!(uvs.iter().any(|u| *u == URect::new(140, 120, 230, 150)), "lnb 帧0");
        assert!(uvs.iter().any(|u| *u == URect::new(140, 150, 230, 180)), "lnb 帧1");
        // src-number 数字网格
        let nuvs = collect_source_uvs(&desc, &Default::default(), "src-number");
        assert!(nuvs.len() >= 11, "score-num divx=11 至少 11 格");
        // value_uv 与 emit_value 一致：score-num 第 7 格 = (7×36, 0, 8×36, 46)
        let v = desc.value("score-num").unwrap();
        assert_eq!(value_uv(v, 7), URect::new(252, 0, 288, 46));
        // 整图（w=-1，src_sizes 传入）→ 源图全尺寸
        let sizes = std::collections::HashMap::from([("src-bg".to_string(), (1920, 1080))]);
        let buvs = collect_source_uvs(&desc, &sizes, "src-bg");
        assert!(buvs.iter().any(|u| *u == URect::new(0, 0, 1920, 1080)), "bg 整图");
    }

    #[test]
    fn hispeed_multiplies_note_speed() {
        // 玩家 hispeed=2 → note 距判定线像素翻倍（px = region.h × hispeed）
        let (_skin, desc, cfg, state) = load();
        *state.write().unwrap() = sample_state();
        {
            let mut s = state.write().unwrap();
            s.hispeed = 2.0;
            s.notes.clear();
            s.notes.push(NoteState {
                idx: 0,
                lane: 0,
                position: 10.4,
                length: None,
                kind: 0,
                consumed: false,
                ln_active: false,
            });
        }
        let cmds = evaluate_frame(&desc, &cfg, &state, &Default::default());
        // now_y=10，dy=0.4 measure，px=880×2=1760 → y = 200 + 0.4×1760 = 904
        let scratch = cmds
            .iter()
            .find(|c| c.uv == URect::new(0, 0, 140, 30))
            .expect("scratch note");
        assert!((scratch.y - 904.0).abs() < 1.0, "hispeed=2 y={}", scratch.y);
    }
}



#[cfg(test)]
mod scroll_tests {
    use super::*;

    #[test]
    fn scroll_measure_works() {
        let tl = vec![(0.0, 1.0), (0.5, 2.0), (2.0, 0.5)];
        // now=0.3 → note=1.0：0.3..0.5 用 scroll1（0.2×1）+ 0.5..1.0 用 scroll2（0.5×2）= 1.2
        assert!((scroll_measure(0.3, 1.0, &tl) - 1.2).abs() < 1e-9);
        // past：反向
        assert!((scroll_measure(1.0, 0.3, &tl) + 1.2).abs() < 1e-9);
        // 跨多段：now=0.4 → note=2.5：0.4..0.5(1) + 0.5..2.0(2) + 2.0..2.5(0.5)
        let v = scroll_measure(0.4, 2.5, &tl);
        assert!((v - (0.1 * 1.0 + 1.5 * 2.0 + 0.5 * 0.5)).abs() < 1e-9);
        // 无 scroll 变化：线性
        let tl2 = vec![(0.0, 1.0)];
        assert!((scroll_measure(0.0, 1.0, &tl2) - 1.0).abs() < 1e-9);
        // 相同位置 = 0
        assert_eq!(scroll_measure(0.5, 0.5, &tl), 0.0);
    }

}
