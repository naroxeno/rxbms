//! 声明式皮肤对象模型（对齐 beatoraja `JsonSkin` + `JSONSkinLoader` 语义）。
//!
//! 皮肤 `main()` 返回的描述表（`skin.source` / `skin.font` / `skin.image` /
//! `skin.value` / `skin.text` / `skin.slider` / `skin.graph` / `skin.destination` 等）
//! 解析为 Rust 结构。语义要点（beatoraja）：
//! - `source`：id → 纹理路径（可含 `*` 通配符，如 `Background/*.png`）
//! - `image`：纹理**裁剪区域**（x/y/w/h 为源图坐标，`w/h < 0` 表示整图）+
//!   `divx/divy` 网格切帧 + `timer/cycle` 动画参数
//! - `value`（数字）：`src` 引用 source，`digit` 位数按 `divx` 网格切数字字形，
//!   数据源为 `ref`（main_state 数字 id）或 `value`（Lua 回调）
//! - `destination`：对象渲染动画（关键帧数组 + timer/loop/op 条件）
//!
//! M2 范围：完整解析 + 静态求值；timer 动画插值、note/group/gauge 渲染在 M3。
//!
//! `#![allow(dead_code)]`：M2 对象模型尚未被 gameplay 引用（仅测试使用），
//! M3 接入 Bevy 渲染后移除。

#![allow(dead_code)]

use std::collections::HashMap;

use mlua::{Function, Lua, Table, Value};

use crate::skin::lua::{Result, SkinError, get_int, get_num, get_str, parse_seq};

/// 源纹理（`skin.source`）。
#[derive(Debug, Clone)]
pub struct Source {
    pub id: String,
    /// 路径，可含 `*` 通配符（如 `Background/*.png`）。
    pub path: String,
}

/// 字体定义（`skin.font`）。
#[derive(Debug, Clone)]
pub struct FontDef {
    pub id: String,
    pub path: String,
}

/// 图像对象（`skin.image`）。
#[derive(Debug, Clone)]
pub struct ImageDef {
    pub id: String,
    pub src: String,
    /// 源纹理裁剪区域（像素）；`w/h < 0` 表示源图全尺寸。
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// 网格切帧数（源区域均分 divx×divy 格）。
    pub divx: u32,
    pub divy: u32,
    pub timer: Option<i64>,
    /// 动画循环帧数（`divx*divy` 的子集）。
    pub cycle: i32,
}

/// 数字对象（`skin.value`）。
#[derive(Debug, Clone)]
pub struct ValueDef {
    pub id: String,
    pub src: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub divx: u32,
    pub divy: u32,
    pub timer: Option<i64>,
    pub cycle: i32,
    /// 对齐：beatoraja SkinNumber（0=左，1=右，2=中）。
    pub align: i32,
    /// 前导填充（beatoraja zeropadding：0=空不画，1=补 0，2=补灰色 0 图[10]）。
    pub zeropadding: i32,
    /// 显示位数。
    pub digit: u32,
    /// 位间距（像素）。
    pub padding: i32,
    /// main_state 数字 id（`ref`）。
    pub ref_id: Option<i64>,
    /// Lua 值回调（`value = function()`）。
    pub value: Option<Function>,
}

/// 文本对象（`skin.text`）。
#[derive(Debug, Clone)]
pub struct TextDef {
    pub id: String,
    pub font: String,
    pub size: i32,
    pub align: i32,
    pub ref_id: Option<i64>,
    pub value: Option<Function>,
    /// 常量文本。
    pub constant: Option<String>,
}

/// 滑条对象（`skin.slider`）。
#[derive(Debug, Clone)]
pub struct SliderDef {
    pub id: String,
    pub src: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub divx: u32,
    pub divy: u32,
    pub timer: Option<i64>,
    pub cycle: i32,
    /// 0=右 1=左 2=下 3=上（beatoraja SkinSlider）。
    pub angle: i32,
    pub range: i32,
    pub type_: i32,
    pub value: Option<Function>,
    pub min: f64,
    pub max: f64,
}

/// 图对象（`skin.graph`）。
#[derive(Debug, Clone)]
pub struct GraphDef {
    pub id: String,
    pub src: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub divx: u32,
    pub divy: u32,
    pub timer: Option<i64>,
    pub cycle: i32,
    pub angle: i32,
    pub type_: i32,
    pub value: Option<Function>,
    pub min: f64,
    pub max: f64,
}

/// destination 关键帧（`dst[]`，字段缺省继承前一帧，首帧缺省取 beatoraja 默认）。
#[derive(Debug, Clone)]
pub struct KeyFrame {
    pub time: i32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub a: i32,
    pub r: i32,
    pub g: i32,
    pub b: i32,
}

impl Default for KeyFrame {
    fn default() -> Self {
        Self {
            time: 0,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            a: 255,
            r: 255,
            g: 255,
            b: 255,
        }
    }
}

/// destination（`skin.destination`，对象渲染动画）。
#[derive(Debug, Clone)]
pub struct Destination {
    pub id: String,
    pub timer: Option<i64>,
    /// 循环：<=0 不循环；>0 循环周期（ms）。
    pub loop_: i32,
    /// 选项显示条件：正数 = 该 op 选中才显示，负数 = 未选中才显示。
    pub op: Vec<i64>,
    /// 应用的自定义偏移 id。
    pub offsets: Vec<i32>,
    /// 混合模式（beatoraja blend：0=正常，2=加色 additive）。
    pub blend: i32,
    pub filter: i32,
    pub frames: Vec<KeyFrame>,
}

/// note 对象（`skin.note`）：轨道布局 + 各类音符图像引用。
#[derive(Debug, Clone, Default)]
pub struct NoteDesc {
    /// 每轨道列：普通音符 image id（note/lnstart/lnend/lnbody/... 与轨道对齐）。
    pub note: Vec<String>,
    pub lnstart: Vec<String>,
    pub lnend: Vec<String>,
    pub lnbody: Vec<String>,
    pub lnactive: Vec<String>,
    pub hcnstart: Vec<String>,
    pub hcnend: Vec<String>,
    pub hcnbody: Vec<String>,
    pub hcnactive: Vec<String>,
    pub hcndamage: Vec<String>,
    pub hcnreactive: Vec<String>,
    pub mine: Vec<String>,
    /// 轨道区域（每列：x/y/w/h，虚拟坐标；判定线在 y+h）。
    pub dst: Vec<KeyFrame>,
    /// 每列音符高度（像素，皮肤坐标）。
    pub size: Vec<f32>,
    /// 分组绘制对象（小节线等，M3 后半）。
    pub group: Vec<Destination>,
}

/// 血量条对象（`skin.gauge`）：beatoraja SkinGauge 机制。
#[derive(Debug, Clone)]
pub struct GaugeDesc {
    pub id: String,
    /// 节点图像 id（6 组 × 6 状态；组 = gauge type，组内 6 状态 =
    /// [满, 满border, 空, 空border, 当前, 当前border]）。
    pub nodes: Vec<String>,
    /// 条的粒数（默认 50，border 可整除）。
    pub parts: i32,
}

impl Default for GaugeDesc {
    fn default() -> Self {
        Self {
            id: String::new(),
            nodes: Vec::new(),
            parts: 50,
        }
    }
}

/// 皮肤描述表（完整解析后的模型）。
#[derive(Debug, Default)]
pub struct SkinDesc {
    pub vw: f32,
    pub vh: f32,
    pub sources: HashMap<String, Source>,
    pub fonts: Vec<FontDef>,
    pub images: HashMap<String, ImageDef>,
    pub values: Vec<ValueDef>,
    pub texts: Vec<TextDef>,
    pub sliders: Vec<SliderDef>,
    pub graphs: Vec<GraphDef>,
    pub destinations: Vec<Destination>,
    /// note 对象（皮肤为玩法 skin 时存在）。
    pub note: Option<NoteDesc>,
    /// 血量条对象（`skin.gauge`）。
    pub gauge: Option<GaugeDesc>,
    /// 判定弹字对象（`skin.judge` 的 images+numbers 展平，复用 destination 渲染）。
    pub judge_objects: Vec<Destination>,
    /// Lua 回调句柄（value 闭包），防 GC 并保持引用。
    pub callbacks: Vec<Function>,
}

/// 从 Lua 表读可空字段：`Value::Nil` → None。
fn get_opt<T>(t: &Table, key: &str, f: impl Fn(&Table) -> Result<T>) -> Result<Option<T>> {
    match t.get::<Value>(key).map_err(SkinError::Lua)? {
        Value::Nil => Ok(None),
        _ => f(t).map(Some),
    }
}

/// 从 Lua 表读数值字段（可空）。
fn get_opt_num(t: &Table, key: &str) -> Result<Option<f64>> {
    match t.get::<Value>(key).map_err(SkinError::Lua)? {
        Value::Nil => Ok(None),
        Value::Integer(i) => Ok(Some(i as f64)),
        Value::Number(n) => Ok(Some(n)),
        other => Err(SkinError::Format(format!(
            "字段 `{key}` 期望数值，实际 {other:?}"
        ))),
    }
}

/// 从 Lua 表读函数字段（可空）。
fn get_opt_func(t: &Table, key: &str) -> Result<Option<Function>> {
    match t.get::<Value>(key).map_err(SkinError::Lua)? {
        Value::Nil => Ok(None),
        Value::Function(f) => Ok(Some(f)),
        other => Err(SkinError::Format(format!(
            "字段 `{key}` 期望函数，实际 {other:?}"
        ))),
    }
}

/// 解析数字 id 字段（数字或字符串 id，如 font `{id=0}` / source `{id="x"}`）。
fn parse_id(t: &Table) -> Result<String> {
    match t.get::<Value>("id").map_err(SkinError::Lua)? {
        Value::Integer(i) => Ok(i.to_string()),
        Value::Number(n) => Ok(format!("{n}")),
        Value::String(s) => Ok(s
            .to_str()
            .map_err(|e| SkinError::Format(e.to_string()))?
            .to_string()),
        other => Err(SkinError::Format(format!("字段 `id` 期望字符串或数字，实际 {other:?}"))),
    }
}

/// 解析 id 字段（数字或字符串 id）。
fn get_id_field(t: &Table, key: &str) -> Result<String> {
    match t.get::<Value>(key).map_err(SkinError::Lua)? {
        Value::Integer(i) => Ok(i.to_string()),
        Value::Number(n) => Ok(format!("{n}")),
        Value::String(s) => Ok(s
            .to_str()
            .map_err(|e| SkinError::Format(e.to_string()))?
            .to_string()),
        other => Err(SkinError::Format(format!(
            "字段 `{key}` 期望字符串或数字，实际 {other:?}"
        ))),
    }
}

/// 裁剪区域公共字段（image/value/slider/graph 共用）。
struct CropFields {
    src: String,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    divx: u32,
    divy: u32,
    timer: Option<i64>,
    cycle: i32,
}

fn parse_crop(t: &Table) -> Result<CropFields> {
    Ok(CropFields {
        src: get_str(t, "src", "")?,
        x: get_int(t, "x", 0)? as i32,
        y: get_int(t, "y", 0)? as i32,
        w: get_int(t, "w", -1)? as i32,
        h: get_int(t, "h", -1)? as i32,
        divx: get_int(t, "divx", 1)? as u32,
        divy: get_int(t, "divy", 1)? as u32,
        timer: get_opt_num(t, "timer")?.map(|n| n as i64),
        cycle: get_int(t, "cycle", 0)? as i32,
    })
}

impl SkinDesc {
    /// 解析 `main()` 返回的描述表。
    pub fn from_table(_lua: &Lua, t: &Table, vw: f32, vh: f32) -> Result<Self> {
        let mut desc = Self {
            vw,
            vh,
            ..Default::default()
        };

        // skin.source
        if let Value::Table(src_t) = t.get::<Value>("source").map_err(SkinError::Lua)? {
            for s in parse_seq(&src_t, "source", |s| {
                Ok(Source {
                    id: parse_id(&s)?,
                    path: get_str(&s, "path", "")?,
                })
            })? {
                desc.sources.insert(s.id.clone(), s);
            }
        }

        // skin.font
        if let Value::Table(font_t) = t.get::<Value>("font").map_err(SkinError::Lua)? {
            desc.fonts = parse_seq(&font_t, "font", |f| {
                Ok(FontDef {
                    id: parse_id(&f)?,
                    path: get_str(&f, "path", "")?,
                })
            })?;
        }

        // skin.image
        if let Value::Table(img_t) = t.get::<Value>("image").map_err(SkinError::Lua)? {
            for img in parse_seq(&img_t, "image", |img| {
                let c = parse_crop(&img)?;
                Ok(ImageDef {
                    id: parse_id(&img)?,
                    src: c.src,
                    x: c.x,
                    y: c.y,
                    w: c.w,
                    h: c.h,
                    divx: c.divx,
                    divy: c.divy,
                    timer: c.timer,
                    cycle: c.cycle,
                })
            })? {
                desc.images.insert(img.id.clone(), img);
            }
        }

        // skin.value
        if let Value::Table(v_t) = t.get::<Value>("value").map_err(SkinError::Lua)? {
            desc.values = parse_seq(&v_t, "value", |v| {
                let c = parse_crop(&v)?;
                Ok(ValueDef {
                    id: parse_id(&v)?,
                    src: c.src,
                    x: c.x,
                    y: c.y,
                    w: c.w,
                    h: c.h,
                    divx: c.divx,
                    divy: c.divy,
                    timer: c.timer,
                    cycle: c.cycle,
                    align: get_int(&v, "align", 0)? as i32,
                    zeropadding: get_int(&v, "zeropadding", 0)? as i32,
                    digit: get_int(&v, "digit", 1)? as u32,
                    padding: get_int(&v, "padding", 0)? as i32,
                    ref_id: get_opt_num(&v, "ref")?.map(|n| n as i64),
                    value: get_opt_func(&v, "value")?,
                })
            })?;
        }

        // skin.text
        if let Value::Table(txt_t) = t.get::<Value>("text").map_err(SkinError::Lua)? {
            desc.texts = parse_seq(&txt_t, "text", |txt| {
                Ok(TextDef {
                    id: parse_id(&txt)?,
                    font: get_id_field(&txt, "font")?,
                    size: get_int(&txt, "size", 24)? as i32,
                    align: get_int(&txt, "align", 0)? as i32,
                    ref_id: get_opt_num(&txt, "ref")?.map(|n| n as i64),
                    value: get_opt_func(&txt, "value")?,
                    constant: get_opt(&txt, "constantText", |x| get_str(x, "constantText", ""))?,
                })
            })?;
        }

        // skin.slider
        if let Value::Table(sl_t) = t.get::<Value>("slider").map_err(SkinError::Lua)? {
            desc.sliders = parse_seq(&sl_t, "slider", |s| {
                let c = parse_crop(&s)?;
                Ok(SliderDef {
                    id: parse_id(&s)?,
                    src: c.src,
                    x: c.x,
                    y: c.y,
                    w: c.w,
                    h: c.h,
                    divx: c.divx,
                    divy: c.divy,
                    timer: c.timer,
                    cycle: c.cycle,
                    angle: get_int(&s, "angle", 0)? as i32,
                    range: get_int(&s, "range", 0)? as i32,
                    type_: get_int(&s, "type", 0)? as i32,
                    value: get_opt_func(&s, "value")?,
                    min: get_num(&s, "min", 0.0)?,
                    max: get_num(&s, "max", 0.0)?,
                })
            })?;
        }

        // skin.graph
        if let Value::Table(g_t) = t.get::<Value>("graph").map_err(SkinError::Lua)? {
            desc.graphs = parse_seq(&g_t, "graph", |g| {
                let c = parse_crop(&g)?;
                Ok(GraphDef {
                    id: parse_id(&g)?,
                    src: c.src,
                    x: c.x,
                    y: c.y,
                    w: c.w,
                    h: c.h,
                    divx: c.divx,
                    divy: c.divy,
                    timer: c.timer,
                    cycle: c.cycle,
                    angle: get_int(&g, "angle", 1)? as i32,
                    type_: get_int(&g, "type", 0)? as i32,
                    value: get_opt_func(&g, "value")?,
                    min: get_num(&g, "min", 0.0)?,
                    max: get_num(&g, "max", 0.0)?,
                })
            })?;
        }

        // 收集 Lua 值回调句柄（防 GC；解析闭包外统一收集避免借用冲突）
        for v in &desc.values {
            if let Some(f) = &v.value {
                desc.callbacks.push(f.clone());
            }
        }
        for t in &desc.texts {
            if let Some(f) = &t.value {
                desc.callbacks.push(f.clone());
            }
        }
        for s in &desc.sliders {
            if let Some(f) = &s.value {
                desc.callbacks.push(f.clone());
            }
        }
        for g in &desc.graphs {
            if let Some(f) = &g.value {
                desc.callbacks.push(f.clone());
            }
        }

        // skin.note（玩法 skin 专用：轨道布局 + 音符图像）
        if let Value::Table(n_t) = t.get::<Value>("note").map_err(SkinError::Lua)? {
            let str_list = |key: &str| -> Result<Vec<String>> {
                match n_t.get::<Value>(key).map_err(SkinError::Lua)? {
                    Value::Table(x) => x
                        .sequence_values::<Value>()
                        .map(|v| {
                            let v = v.map_err(|e| {
                                SkinError::Format(format!("`skin.note.{key}` 元素错误: {e}"))
                            })?;
                            Ok(match v {
                                Value::String(s) => s
                                    .to_str()
                                    .map_err(|e| SkinError::Format(e.to_string()))?
                                    .to_string(),
                                Value::Integer(i) => i.to_string(),
                                _ => String::new(),
                            })
                        })
                        .collect(),
                    _ => Ok(Vec::new()),
                }
            };
            let dst = match n_t.get::<Value>("dst").map_err(SkinError::Lua)? {
                Value::Table(x) => parse_seq(&x, "skin.note.dst", |kf| {
                    Ok(KeyFrame {
                        time: 0,
                        x: get_int(&kf, "x", 0)? as i32,
                        y: get_int(&kf, "y", 0)? as i32,
                        w: get_int(&kf, "w", 0)? as i32,
                        h: get_int(&kf, "h", 0)? as i32,
                        ..Default::default()
                    })
                })?,
                _ => Vec::new(),
            };
            let size = match n_t.get::<Value>("size").map_err(SkinError::Lua)? {
                Value::Table(x) => x
                    .sequence_values::<f64>()
                    .map(|v| v.map(|n| n as f32).map_err(|e| {
                        SkinError::Format(format!("`skin.note.size` 元素错误: {e}"))
                    }))
                    .collect::<Result<Vec<_>>>()?,
                _ => Vec::new(),
            };
            desc.note = Some(NoteDesc {
                note: str_list("note")?,
                lnstart: str_list("lnstart")?,
                lnend: str_list("lnend")?,
                lnbody: str_list("lnbody")?,
                lnactive: str_list("lnactive")?,
                hcnstart: str_list("hcnstart")?,
                hcnend: str_list("hcnend")?,
                hcnbody: str_list("hcnbody")?,
                hcnactive: str_list("hcnactive")?,
                hcndamage: str_list("hcndamage")?,
                hcnreactive: str_list("hcnreactive")?,
                mine: str_list("mine")?,
                dst,
                size,
                group: Vec::new(),
            });
        }

        // skin.gauge（血量条：nodes 按血量百分比选图）
        if let Value::Table(g_t) = t.get::<Value>("gauge").map_err(SkinError::Lua)? {
            let nodes = match g_t.get::<Value>("nodes").map_err(SkinError::Lua)? {
                Value::Table(x) => x
                    .sequence_values::<String>()
                    .map(|v| {
                        v.map_err(|e| {
                            SkinError::Format(format!("`skin.gauge.nodes` 元素错误: {e}"))
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
                _ => Vec::new(),
            };
            desc.gauge = Some(GaugeDesc {
                id: get_id_field(&g_t, "id")?,
                nodes,
                parts: get_int(&g_t, "parts", 50)? as i32,
            });
        }

        // skin.judge（判定弹字：images/numbers 复用 destination 语义，展平合并）
        if let Value::Table(j_t) = t.get::<Value>("judge").map_err(SkinError::Lua)? {
            for j in parse_seq(&j_t, "judge", |j| Ok(j))? {
                for key in ["images", "numbers"] {
                    if let Value::Table(list) = j.get::<Value>(key).map_err(SkinError::Lua)? {
                        for item in parse_seq(&list, &format!("judge[].{key}"), |item| {
                            parse_destination(&item)
                        })? {
                            desc.judge_objects.push(item);
                        }
                    }
                }
            }
        }

        // skin.destination
        if let Value::Table(d_t) = t.get::<Value>("destination").map_err(SkinError::Lua)? {
            desc.destinations = parse_seq(&d_t, "destination", |d| parse_destination(&d))?;
        }

        Ok(desc)
    }


    /// 按 id 查图像对象。
    pub fn image(&self, id: &str) -> Option<&ImageDef> {
        self.images.get(id)
    }

    /// 按 id 查数字对象。
    pub fn value(&self, id: &str) -> Option<&ValueDef> {
        self.values.iter().find(|v| v.id == id)
    }

    /// 按 id 查文本对象。
    pub fn text(&self, id: &str) -> Option<&TextDef> {
        self.texts.iter().find(|t| t.id == id)
    }
}

/// 解析 destination 表（`skin.destination[]` / `skin.judge[].images|numbers[]`）。
fn parse_destination(d: &Table) -> Result<Destination> {
    // 数值 op 条件（正/负选项 id）
    let mut op = Vec::new();
    if let Value::Table(op_t) = d.get::<Value>("op").map_err(SkinError::Lua)? {
        for o in op_t.sequence_values::<i64>() {
            let o = o.map_err(|e| SkinError::Format(format!("`op` 元素错误: {e}")))?;
            op.push(o);
        }
    }
    let mut frames = Vec::new();
    if let Value::Table(f_t) = d.get::<Value>("dst").map_err(SkinError::Lua)? {
        // 关键帧：缺省字段继承前一帧（beatoraja setDestination 逻辑）
        let mut prev = KeyFrame::default();
        let mut first = true;
        for kf in parse_seq(&f_t, "dst", |kf| Ok(kf))? {
            let mut f = prev.clone();
            if !first {
                f.time = get_int(&kf, "time", prev.time as i64)? as i32;
                f.x = get_int(&kf, "x", prev.x as i64)? as i32;
                f.y = get_int(&kf, "y", prev.y as i64)? as i32;
                f.w = get_int(&kf, "w", prev.w as i64)? as i32;
                f.h = get_int(&kf, "h", prev.h as i64)? as i32;
                f.a = get_int(&kf, "a", prev.a as i64)? as i32;
                f.r = get_int(&kf, "r", prev.r as i64)? as i32;
                f.g = get_int(&kf, "g", prev.g as i64)? as i32;
                f.b = get_int(&kf, "b", prev.b as i64)? as i32;
            } else {
                // 首帧：x/y/w/h 缺省 0（beatoraja 首帧默认），a/r/g/b 缺省 255
                f.time = get_int(&kf, "time", 0)? as i32;
                f.x = get_int(&kf, "x", 0)? as i32;
                f.y = get_int(&kf, "y", 0)? as i32;
                f.w = get_int(&kf, "w", 0)? as i32;
                f.h = get_int(&kf, "h", 0)? as i32;
                f.a = get_int(&kf, "a", 255)? as i32;
                f.r = get_int(&kf, "r", 255)? as i32;
                f.g = get_int(&kf, "g", 255)? as i32;
                f.b = get_int(&kf, "b", 255)? as i32;
                first = false;
            }
            frames.push(f.clone());
            prev = f;
        }
    }
    let offsets = match d.get::<Value>("offsets").map_err(SkinError::Lua)? {
        Value::Table(x) => x
            .sequence_values::<i64>()
            .map(|o| o.map(|n| n as i32).map_err(|e| {
                SkinError::Format(format!("`offsets` 元素错误: {e}"))
            }))
            .collect::<Result<Vec<_>>>()?,
        _ => Vec::new(),
    };
    Ok(Destination {
        id: parse_id(d)?,
        timer: get_opt_num(d, "timer")?.map(|n| n as i64),
        loop_: get_int(d, "loop", 0)? as i32,
        op,
        offsets,
        blend: get_int(d, "blend", 0)? as i32,
        filter: get_int(d, "filter", 0)? as i32,
        frames,
    })
}

/// 通配符路径解析：`Background/*.png` + 选中名 `Default` → `Background/Default.png`。
///
/// 无选中名时枚举皮肤目录下匹配 `*` 的文件，取第一个（按文件名排序）。
pub fn resolve_wildcard(dir: &std::path::Path, pattern: &str, selected: Option<&str>) -> Result<String> {
    if let Some(sel) = selected {
        return Ok(pattern.replace('*', sel));
    }
    // 枚举：pattern 形如 `<prefix>*<suffix>`，`*` 在最后一段
    let (prefix, suffix) = match pattern.split_once('*') {
        Some((p, s)) => (p, s),
        None => return Ok(pattern.to_string()),
    };
    let full_dir = dir.join(prefix);
    let mut found: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&full_dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("") && name.ends_with(suffix.trim_start_matches('/')) {
                found.push(format!("{prefix}{name}"));
            }
        }
    }
    found.sort();
    found
        .first()
        .cloned()
        .ok_or_else(|| SkinError::Format(format!("通配符无匹配文件: {pattern}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skin::lua::LuaSkin;

    fn load_desc() -> (LuaSkin, SkinDesc) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/test_skin/Play");
        let skin = LuaSkin::new(&dir).expect("创建 LuaSkin 失败");
        let entry = dir.join("Play5.luaskin");
        let header = skin.load_header(&entry).unwrap();
        let cfg = crate::skin::lua::SkinConfigValues::from_header(&header);
        let t = skin.load_skin(&entry, &header, &cfg).unwrap();
        let desc = SkinDesc::from_table(&skin.lua(), &t, header.w, header.h).expect("解析描述表失败");
        (skin, desc)
    }

    #[test]
    fn parse_play5_full_desc() {
        let (_skin, desc) = load_desc();
        assert_eq!(desc.vw, 1920.0);
        assert_eq!(desc.vh, 1080.0);
        // source：16 项（含通配符）
        assert_eq!(desc.sources.len(), 16);
        assert_eq!(desc.sources["src-bg"].path, "Background/*.png");
        assert_eq!(desc.sources["src-number"].path, "Parts/System/Number.png");
        // font：2 项
        assert_eq!(desc.fonts.len(), 2);
        assert_eq!(desc.fonts[0].path, "Fonts/SarasaUiJ-SemiBold.ttf");
        // image 对象（含循环插入的 bomb-*）约 100+
        assert!(desc.images.len() >= 100, "images 过少: {}", desc.images.len());
        // 关键对象字段：note-w 裁剪 140,0 90×30；judge-line 776×15
        let note_w = desc.image("note-w").unwrap();
        assert_eq!((note_w.x, note_w.y, note_w.w, note_w.h), (140, 0, 90, 30));
        assert_eq!(note_w.src, "src-notes");
        let judge_line = desc.image("judge-line").unwrap();
        assert_eq!((judge_line.w, judge_line.h), (776, 15));
        // lnb-w divy=2 动画
        let lnb = desc.image("lnb-w").unwrap();
        assert_eq!(lnb.divy, 2);
        // value 数字对象
        assert!(desc.values.len() >= 30, "values 过少: {}", desc.values.len());
        let score = desc.value("score-num").unwrap();
        assert_eq!((score.digit, score.divx), (6, 11));
        assert_eq!(score.ref_id, Some(100));
        let ex = desc.value("ex-score-5d").unwrap();
        assert!(ex.value.is_some(), "ex-score-5d 应为 value 回调");
        assert_eq!(ex.ref_id, None);
        // text
        assert_eq!(desc.texts.len(), 6);
        assert_eq!(desc.texts[0].font, "1");
        // slider / graph
        assert_eq!(desc.sliders.len(), 2);
        assert_eq!(desc.sliders[0].angle, 2);
        assert_eq!(desc.graphs.len(), 7);
        // destination（bg/frame/title/artist 等；judge 在 skin.judge，M3 解析）
        assert!(desc.destinations.len() >= 20, "destinations 过少: {}", desc.destinations.len());
        let bg = desc
            .destinations
            .iter()
            .find(|d| d.id == "bg")
            .expect("缺少 bg destination");
        assert_eq!(bg.frames[0].w, 1920);
        assert_eq!(bg.frames[0].h, 1080);
        // artist op=-1008：数字数组解析正确
        let artist_static = desc
            .destinations
            .iter()
            .find(|d| d.id == "artist" && d.op == vec![-1008])
            .expect("缺少 artist 静态 destination");
        assert!(artist_static.op.contains(&-1008));
        // artist 淡入：loop=0 多关键帧，op=1008
        let artist = desc
            .destinations
            .iter()
            .find(|d| d.id == "artist" && d.op == vec![1008])
            .expect("缺少 artist 动画 destination");
        assert_eq!(artist.loop_, 0);
        assert_eq!(artist.frames[0].a, 0);
        assert_eq!(artist.frames[1].a, 255);
        // 帧继承：第 2 帧缺省 x/y/w/h 继承第 1 帧
        assert_eq!(artist.frames[1].x, artist.frames[0].x);
        assert_eq!(artist.frames[1].w, artist.frames[0].w);
        // judge-pg 由 skin.judge 驱动，不在 destination 中
        assert!(
            !desc.destinations.iter().any(|d| d.id == "judge-pg"),
            "judge-pg 不应在 destination"
        );
        // skin.judge 展平：judge-pg/judge-num-pg 等判定弹字对象
        assert!(desc.judge_objects.len() >= 12, "judge_objects 过少: {}", desc.judge_objects.len());
        assert!(
            desc.judge_objects.iter().any(|d| d.id == "judge-pg"),
            "缺少 judge-pg 判定弹字"
        );
        assert!(
            desc.judge_objects.iter().any(|d| d.id == "judge-num-pg"),
            "缺少 judge-num-pg 判定数字"
        );
        let judge_pg = desc.judge_objects.iter().find(|d| d.id == "judge-pg").unwrap();
        assert_eq!(judge_pg.timer, Some(46), "judge-pg 应使用 JUDGE timer");
        assert_eq!(judge_pg.loop_, -1, "judge-pg 播完应消失");
    }

    #[test]
    fn resolve_wildcard_selected() {
        assert_eq!(
            resolve_wildcard(std::path::Path::new("."), "Background/*.png", Some("Default")).unwrap(),
            "Background/Default.png"
        );
        // 无通配符直接返回
        assert_eq!(
            resolve_wildcard(std::path::Path::new("."), "Parts/System/Number.png", None).unwrap(),
            "Parts/System/Number.png"
        );
    }

    #[test]
    fn resolve_wildcard_enumerate() {
        // 枚举真实皮肤目录 Background/（Default.png + Black.png）
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/test_skin/Play");
        let r = resolve_wildcard(&dir, "Background/*.png", None).unwrap();
        assert!(r == "Background/Black.png" || r == "Background/Default.png", "r={r}");
    }
}

#[cfg(test)]
mod tests7 {
    use super::*;
    use crate::skin::lua::LuaSkin;

    #[test]
    fn load_play7_desc() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/test_skin/Play");
        let skin = LuaSkin::new(&dir).expect("LuaSkin");
        let entry = dir.join("Play7.luaskin");
        let header = skin.load_header(&entry).expect("header");
        assert_eq!(header.name, "FAm Breeze 1.1");
        let cfg = crate::skin::lua::SkinConfigValues::from_header(&header);
        let t = skin.load_skin(&entry, &header, &cfg).expect("main");
        let desc = SkinDesc::from_table(skin.lua(), &t, header.w, header.h).expect("desc");
        // 7K：8 轨道（7 键 + scratch 最后）
        let nd = desc.note.as_ref().expect("note");
        assert_eq!(nd.note.len(), 8, "Play7 应为 8 轨道");
        assert_eq!(nd.note[7], "note-s", "scratch 在最后");
        assert_eq!(nd.dst.len(), 8, "7K dst 8 项");
        // 5K 入口仍为 6 轨道
        let entry5 = dir.join("Play5.luaskin");
        let h5 = skin.load_header(&entry5).unwrap();
        let c5 = crate::skin::lua::SkinConfigValues::from_header(&h5);
        let t5 = skin.load_skin(&entry5, &h5, &c5).unwrap();
        let d5 = SkinDesc::from_table(skin.lua(), &t5, h5.w, h5.h).unwrap();
        assert_eq!(d5.note.unwrap().note.len(), 6, "Play5 应为 6 轨道");
    }
}
