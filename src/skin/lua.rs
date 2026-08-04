//! beatoraja Lua 皮肤装载器（M1 骨架）。
//!
//! 行为与 beatoraja `LuaSkinLoader` / `SkinLuaAccessor` 对齐：
//! - `.luaskin` 入口脚本被执行**两次**：第一次无 `skin_config` 全局 → 返回 header 表；
//!   第二次注入 `skin_config`（+ 真 `main_state` 模块）→ 返回 `main()` 描述表。
//! - header 阶段 `main_state` / `timer_util` / `event_util` 预置为空表，
//!   使 `require("main_state")` 不报错（beatoraja `setIsLoaded` 等价物）。
//! - `package.path` 指向皮肤目录（`<dir>/?.lua;<dir>/?/init.lua`），
//!   皮肤内 `require("Play5")` 相对皮肤目录解析，并复用 Lua 模块缓存
//!   （第二次执行 `.luaskin` 时 `Play5` 已缓存，直接调用其 `main()`）。
//!
//! 进度：M1 只做装载与 header/描述表解析；`main_state` 为占位骨架
//! （`number` 等返回 0，M3 接 gameplay 状态桥）；渲染对象模型 M2 实现。
//!
//! `#![allow(dead_code)]`：M1 装载器 API 尚未被 gameplay 引用（仅测试使用），
//! M2 渲染对象模型接入后移除。

#![allow(dead_code)]

use std::fmt;
use std::path::{Path, PathBuf};

use mlua::{Lua, Table, Value};

pub type Result<T> = std::result::Result<T, SkinError>;

/// 皮肤装载错误。
#[derive(Debug)]
pub enum SkinError {
    /// 读取脚本文件失败。
    Io(std::io::Error),
    /// Lua 执行/类型错误。
    Lua(mlua::Error),
    /// 皮肤脚本结构不符合预期（字段缺失/类型错误）。
    Format(String),
}

impl fmt::Display for SkinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkinError::Io(e) => write!(f, "皮肤文件读取失败: {e}"),
            SkinError::Lua(e) => write!(f, "皮肤脚本执行失败: {e}"),
            SkinError::Format(m) => write!(f, "皮肤结构异常: {m}"),
        }
    }
}

impl std::error::Error for SkinError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SkinError::Io(e) => Some(e),
            SkinError::Lua(e) => Some(e),
            SkinError::Format(_) => None,
        }
    }
}

/// 皮肤自定义选项项（header.property[].item[]）。
#[derive(Debug, Clone)]
pub struct OptionItem {
    pub name: String,
    /// 选项编号（皮肤内比较用，如 900/901）。
    pub op: i64,
}

/// 皮肤自定义选项（header.property[]）。
#[derive(Debug, Clone)]
pub struct CustomOption {
    pub name: String,
    /// 默认选中项名称（对应 item[].name）。
    pub def: Option<String>,
    pub items: Vec<OptionItem>,
}

/// 皮肤自定义文件选择（header.filepath[]）。
#[derive(Debug, Clone)]
pub struct CustomFile {
    pub name: String,
    /// 通配符路径，如 `Background/*.png`（M2 解析通配符）。
    pub path: String,
    /// 默认选择文件名。
    pub def: Option<String>,
}

/// 皮肤自定义偏移（header.offset[]）。
#[derive(Debug, Clone)]
pub struct CustomOffset {
    pub name: String,
    pub id: i64,
    /// 皮肤声明默认 a 值（offset 的 alpha 通道）。
    pub a: f64,
}

/// 皮肤 header（对应 beatoraja `SkinHeader`）。
#[derive(Debug, Clone)]
pub struct SkinHeader {
    pub skin_type: i64,
    pub name: String,
    pub author: String,
    /// 虚拟分辨率宽（皮肤坐标基准，如 1920）。
    pub w: f32,
    /// 虚拟分辨率高（如 1080）。
    pub h: f32,
    pub loadend: f32,
    pub playstart: f32,
    pub scene: f32,
    pub input: i64,
    pub close: f32,
    pub fadeout: f32,
    pub property: Vec<CustomOption>,
    pub filepath: Vec<CustomFile>,
    pub offset: Vec<CustomOffset>,
}

/// `skin_config` 注入值（M1：全部取默认；M5 接入设置持久化）。
#[derive(Debug, Clone, Default)]
pub struct SkinConfigValues {
    /// 选项名 → 选中 op 值。
    pub option: Vec<(String, i64)>,
    /// 偏移名 → (x, y, w, h, r, a) 六元组。
    pub offset: Vec<(String, [f64; 6])>,
    /// 文件选择名 → 选中文件名。
    pub file_path: Vec<(String, String)>,
    /// 全部选项 item op → 是否被选中（beatoraja `IntIntMap` 等价物，
    /// 用于 destination `op` 条件判定：正 = 选中才显示，负 = 未选中才显示）。
    pub op_map: Vec<(i64, bool)>,
}

impl SkinConfigValues {
    /// 按 header 默认值构建：op 取 `def` 指定项（无则第一项），
    /// offset 全 0（等价用户未修改设置），file_path 取 `def`（通配符 M2 解析）。
    pub fn from_header(header: &SkinHeader) -> Self {
        let option: Vec<(String, i64)> = header
            .property
            .iter()
            .map(|p| {
                let op = p
                    .def
                    .as_ref()
                    .and_then(|def| p.items.iter().find(|i| &i.name == def))
                    .or_else(|| p.items.first())
                    .map(|i| i.op)
                    .unwrap_or(0);
                (p.name.clone(), op)
            })
            .collect();
        let offset = header
            .offset
            .iter()
            .map(|o| (o.name.clone(), [0.0f64; 6]))
            .collect();
        let file_path = header
            .filepath
            .iter()
            .filter_map(|f| f.def.clone().map(|d| (f.name.clone(), d)))
            .collect();
        // 所有 item op 值 → 选中标记（beatoraja loadJsonSkin 的 option map）
        let mut op_map = Vec::new();
        for p in &header.property {
            let selected = option
                .iter()
                .find(|(n, _)| n == &p.name)
                .map(|(_, op)| *op);
            for item in &p.items {
                op_map.push((item.op, Some(item.op) == selected));
            }
        }
        // 全局皮肤选项（beatoraja SkinProperty.OPTION_*，非皮肤自定义项）：
        // 81=OPTION_LOADED（谱面加载完成）、32=OPTION_AUTOPLAYOFF（非 Auto）、
        // 1008=OPTION_TABLE_SONG（曲名列表）、80=OPTION_NOW_LOADING（加载中）、
        // 194=OPTION_NO_BACKBMP（无背景位图；rxbms 无 BGA → 视为 true）。
        // 81/32/80 运行时由 sync_skin_state 更新。
        op_map.push((81, false));
        op_map.push((32, true));
        op_map.push((1008, false));
        op_map.push((80, false));
        op_map.push((194, true));
        Self {
            option,
            offset,
            file_path,
            op_map,
        }
    }

    /// destination `op` 条件判定：正 = 该 op 值被选中，负 = 未选中。
    /// 不存在的选项视为未选中（beatoraja `option.get(id, -1)` 语义）。
    pub fn is_option_enabled(&self, id: i64) -> bool {
        self.op_map
            .iter()
            .find(|(k, _)| *k == id)
            .map(|(_, v)| *v)
            .unwrap_or(false)
    }
}

/// Lua 皮肤装载器：持有 Lua 状态与皮肤目录。
pub struct LuaSkin {
    lua: Lua,
    dir: PathBuf,
}

impl LuaSkin {
    /// 创建装载器并初始化 `package.path` 与 `main_state` 骨架模块。
    ///
    /// `main_state` 在**首次执行脚本前**导出（对齐 beatoraja：构造 loader 时
    /// 即 `exportMainStateAccessor`），保证 `Play5.lua` 顶层的
    /// `main_state = require("main_state")` 第一次就绑定真表，
    /// 第二次执行 `.luaskin` 时模块缓存的 upvalue 仍指向同一表。
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        let lua = Lua::new();
        let skin = Self { lua, dir };
        skin.setup_package_path()?;
        skin.export_main_state_skeleton()?;
        Ok(skin)
    }

    /// 皮肤目录。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 底层 Lua 状态（对象模型解析/回调调用用）。
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// `package.path = <dir>/?.lua;<dir>/?/init.lua`（模块相对皮肤目录解析）。
    fn setup_package_path(&self) -> Result<()> {
        let pkg: Table = self.lua.globals().get("package").map_err(SkinError::Lua)?;
        let path = format!(
            "{}/?.lua;{}/?/init.lua",
            self.dir.display(),
            self.dir.display()
        );
        pkg.set("path", path).map_err(SkinError::Lua)
    }

    /// header 阶段预置空模块，`require("main_state")` 等不报错。
    ///
    /// 注意：`main_state` 已被骨架表覆盖（见 [`Self::new`]），此方法保留用于
    /// 未来"纯 header 解析"场景（beatoraja 无参构造器等价物）。
    fn preset_empty_modules(&self) -> Result<()> {
        let loaded = self.package_loaded()?;
        for name in ["timer_util", "event_util"] {
            loaded
                .set(name, self.lua.create_table().map_err(SkinError::Lua)?)
                .map_err(SkinError::Lua)?;
        }
        Ok(())
    }

    fn package_loaded(&self) -> Result<Table> {
        let pkg: Table = self.lua.globals().get("package").map_err(SkinError::Lua)?;
        pkg.get("loaded").map_err(SkinError::Lua)
    }

    /// 执行入口脚本（`.luaskin`），返回其返回值（header 表或 main 描述表）。
    fn exec_entry(&self, entry: &Path) -> Result<Table> {
        let src = std::fs::read_to_string(entry).map_err(SkinError::Io)?;
        let chunk = self
            .lua
            .load(&src)
            .set_name(&format!("@{}", entry.display()));
        chunk.eval::<Table>().map_err(SkinError::Lua)
    }

    /// 第一次执行：加载 header 表。
    pub fn load_header(&self, entry: &Path) -> Result<SkinHeader> {
        let t = self.exec_entry(entry)?;
        SkinHeader::from_table(&self.lua, &t)
    }

    /// 第二次执行：注入 `skin_config` 后，返回 `main()` 描述表。
    pub fn load_skin(&self, entry: &Path, header: &SkinHeader, config: &SkinConfigValues) -> Result<Table> {
        self.export_skin_config(header, config)?;
        self.exec_entry(entry)
    }

    /// 注入全局 `skin_config` 表（option / offset / file_path / get_path / enabled_options）。
    fn export_skin_config(&self, header: &SkinHeader, config: &SkinConfigValues) -> Result<()> {
        let cfg = self.lua.create_table().map_err(SkinError::Lua)?;

        let file_path = self.lua.create_table().map_err(SkinError::Lua)?;
        for (name, path) in &config.file_path {
            file_path
                .set(name.as_str(), path.as_str())
                .map_err(SkinError::Lua)?;
        }
        cfg.set("file_path", file_path).map_err(SkinError::Lua)?;

        // get_path：皮肤目录相对路径解析（M1 简化；custom file 映射 M5）
        let dir = self.dir.clone();
        let get_path = self
            .lua
            .create_function(move |_, p: String| Ok(format!("{}/{}", dir.display(), p)))
            .map_err(SkinError::Lua)?;
        cfg.set("get_path", get_path).map_err(SkinError::Lua)?;

        let option = self.lua.create_table().map_err(SkinError::Lua)?;
        let enabled = self.lua.create_table().map_err(SkinError::Lua)?;
        for (i, (name, op)) in config.option.iter().enumerate() {
            option.set(name.as_str(), *op).map_err(SkinError::Lua)?;
            enabled
                .set(i + 1, *op)
                .map_err(SkinError::Lua)?;
        }
        cfg.set("option", option).map_err(SkinError::Lua)?;
        cfg.set("enabled_options", enabled).map_err(SkinError::Lua)?;

        let offsets = self.lua.create_table().map_err(SkinError::Lua)?;
        for (name, o) in &config.offset {
            let ot = self.lua.create_table().map_err(SkinError::Lua)?;
            for (k, v) in ["x", "y", "w", "h", "r", "a"].into_iter().zip(o) {
                ot.set(k, *v).map_err(SkinError::Lua)?;
            }
            offsets.set(name.as_str(), ot).map_err(SkinError::Lua)?;
        }
        cfg.set("offset", offsets).map_err(SkinError::Lua)?;

        self.lua
            .globals()
            .set("skin_config", cfg)
            .map_err(SkinError::Lua)?;
        // header 参数保持引用（未来版本皮肤可用）；当前仅保证全局存在
        let _ = header;
        Ok(())
    }

    /// 导出 `main_state` 模块骨架：所有取值 API 返回占位值，M3 接入 gameplay 状态。
    fn export_main_state_skeleton(&self) -> Result<()> {
        let ms = self.lua.create_table().map_err(SkinError::Lua)?;
        let f = |_: &Lua, _: i64| Ok(0i64);
        ms.set("number", self.lua.create_function(f).map_err(SkinError::Lua)?)
            .map_err(SkinError::Lua)?;
        ms.set(
            "float_number",
            self.lua
                .create_function(|_: &Lua, _: i64| Ok(0.0f64))
                .map_err(SkinError::Lua)?,
        )
        .map_err(SkinError::Lua)?;
        ms.set(
            "text",
            self.lua
                .create_function(|_: &Lua, _: i64| Ok(String::new()))
                .map_err(SkinError::Lua)?,
        )
        .map_err(SkinError::Lua)?;
        // timer：beatoraja 的 TIMER_OFF_VALUE = Long.MIN_VALUE
        ms.set(
            "timer",
            self.lua
                .create_function(|_: &Lua, _: i64| Ok(i64::MIN as f64))
                .map_err(SkinError::Lua)?,
        )
        .map_err(SkinError::Lua)?;
        ms.set(
            "time",
            self.lua
                .create_function(|_: &Lua, ()| Ok(0.0f64))
                .map_err(SkinError::Lua)?,
        )
        .map_err(SkinError::Lua)?;
        ms.set(
            "rate",
            self.lua
                .create_function(|_: &Lua, ()| Ok(0.0f64))
                .map_err(SkinError::Lua)?,
        )
        .map_err(SkinError::Lua)?;
        ms.set(
            "exscore",
            self.lua
                .create_function(|_: &Lua, ()| Ok(0i64))
                .map_err(SkinError::Lua)?,
        )
        .map_err(SkinError::Lua)?;
        ms.set(
            "gauge",
            self.lua
                .create_function(|_: &Lua, ()| Ok(0.0f64))
                .map_err(SkinError::Lua)?,
        )
        .map_err(SkinError::Lua)?;

        let loaded = self.package_loaded()?;
        loaded.set("main_state", ms).map_err(SkinError::Lua)
    }
}

/// 从 Lua 表读数值字段（Integer/Number 均可），缺省返回默认值。
pub(crate) fn get_num(t: &Table, key: &str, default: f64) -> Result<f64> {
    match t.get::<Value>(key).map_err(SkinError::Lua)? {
        Value::Integer(i) => Ok(i as f64),
        Value::Number(n) => Ok(n),
        Value::Nil => Ok(default),
        other => Err(SkinError::Format(format!(
            "字段 `{key}` 期望数值，实际 {other:?}"
        ))),
    }
}

/// 从 Lua 表读整数字段，缺省返回默认值。
pub(crate) fn get_int(t: &Table, key: &str, default: i64) -> Result<i64> {
    get_num(t, key, default as f64).map(|n| n as i64)
}

/// 从 Lua 表读字符串字段（缺省返回默认值）。
pub(crate) fn get_str(t: &Table, key: &str, default: &str) -> Result<String> {
    match t.get::<Value>(key).map_err(SkinError::Lua)? {
        Value::String(s) => Ok(s.to_str().map_err(|e| SkinError::Format(e.to_string()))?.to_string()),
        Value::Nil => Ok(default.to_string()),
        other => Err(SkinError::Format(format!(
            "字段 `{key}` 期望字符串，实际 {other:?}"
        ))),
    }
}

/// 解析 Lua 数组表为 Vec<T>（元素经 `f` 转换）。
pub(crate) fn parse_seq<T>(
    t: &Table,
    what: &str,
    f: impl Fn(Table) -> Result<T>,
) -> Result<Vec<T>> {
    let mut out = Vec::new();
    for item in t.sequence_values::<Table>() {
        let item = item.map_err(|e| {
            SkinError::Format(format!("`{what}` 数组元素不是表: {e}"))
        })?;
        out.push(f(item)?);
    }
    Ok(out)
}

impl SkinHeader {
    /// 从 `.luaskin` 返回的 header 表解析（缺省字段取 beatoraja 默认）。
    fn from_table(lua: &Lua, t: &Table) -> Result<Self> {
        let skin_type = get_int(t, "type", 1)?;
        let name = get_str(t, "name", "")?;
        let author = get_str(t, "author", "")?;
        let w = get_num(t, "w", 1280.0)? as f32;
        let h = get_num(t, "h", 720.0)? as f32;
        let loadend = get_num(t, "loadend", 0.0)? as f32;
        let playstart = get_num(t, "playstart", 0.0)? as f32;
        let scene = get_num(t, "scene", 0.0)? as f32;
        let input = get_int(t, "input", 0)?;
        let close = get_num(t, "close", 0.0)? as f32;
        let fadeout = get_num(t, "fadeout", 0.0)? as f32;

        let empty = lua.create_table().map_err(SkinError::Lua)?;
        let property_t: Table = match t.get::<Value>("property").map_err(SkinError::Lua)? {
            Value::Table(x) => x,
            Value::Nil => empty.clone(),
            other => {
                return Err(SkinError::Format(format!(
                    "字段 `property` 期望数组，实际 {other:?}"
                )))
            }
        };
        let property = parse_seq(&property_t, "property", |p| {
            let items_t: Table = match p.get::<Value>("item").map_err(SkinError::Lua)? {
                Value::Table(x) => x,
                Value::Nil => lua.create_table().map_err(SkinError::Lua)?,
                other => {
                    return Err(SkinError::Format(format!(
                        "property[].item 期望数组，实际 {other:?}"
                    )))
                }
            };
            let items = parse_seq(&items_t, "property[].item", |i| {
                Ok(OptionItem {
                    name: get_str(&i, "name", "")?,
                    op: get_int(&i, "op", 0)?,
                })
            })?;
            Ok(CustomOption {
                name: get_str(&p, "name", "")?,
                def: match p.get::<Value>("def").map_err(SkinError::Lua)? {
                    Value::String(s) => {
                        Some(s.to_str().map_err(|e| SkinError::Format(e.to_string()))?.to_string())
                    }
                    _ => None,
                },
                items,
            })
        })?;

        let filepath_t: Table = match t.get::<Value>("filepath").map_err(SkinError::Lua)? {
            Value::Table(x) => x,
            Value::Nil => empty.clone(),
            other => {
                return Err(SkinError::Format(format!(
                    "字段 `filepath` 期望数组，实际 {other:?}"
                )))
            }
        };
        let filepath = parse_seq(&filepath_t, "filepath", |f| {
            Ok(CustomFile {
                name: get_str(&f, "name", "")?,
                path: get_str(&f, "path", "")?,
                def: match f.get::<Value>("def").map_err(SkinError::Lua)? {
                    Value::String(s) => {
                        Some(s.to_str().map_err(|e| SkinError::Format(e.to_string()))?.to_string())
                    }
                    _ => None,
                },
            })
        })?;

        let offset_t: Table = match t.get::<Value>("offset").map_err(SkinError::Lua)? {
            Value::Table(x) => x,
            Value::Nil => empty,
            other => {
                return Err(SkinError::Format(format!(
                    "字段 `offset` 期望数组，实际 {other:?}"
                )))
            }
        };
        let offset = parse_seq(&offset_t, "offset", |o| {
            Ok(CustomOffset {
                name: get_str(&o, "name", "")?,
                id: get_int(&o, "id", 0)?,
                a: get_num(&o, "a", 0.0)?,
            })
        })?;

        Ok(Self {
            skin_type,
            name,
            author,
            w,
            h,
            loadend,
            playstart,
            scene,
            input,
            close,
            fadeout,
            property,
            filepath,
            offset,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skin_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/test_skin/Play")
    }

    #[test]
    fn load_header_play5() {
        let skin = LuaSkin::new(skin_dir()).expect("创建 LuaSkin 失败");
        let header = skin
            .load_header(&skin_dir().join("Play5.luaskin"))
            .expect("加载 header 失败");
        assert_eq!(header.skin_type, 1);
        assert_eq!(header.name, "FAm Breeze 1.1");
        assert_eq!((header.w, header.h), (1920.0, 1080.0));
        assert_eq!((header.loadend, header.playstart), (3000.0, 1500.0));
        assert_eq!(header.scene, 3_600_000.0);
        // property：5 个自定义选项
        assert_eq!(header.property.len(), 5);
        assert_eq!(header.property[0].name, "Lane Side - 轨道位置");
        assert_eq!(header.property[0].items[0].op, 900);
        // def 解析：Ghost Display 默认 Off(910)
        assert_eq!(header.property[1].def.as_deref(), Some("Off"));
        assert_eq!(header.property[1].items[0].op, 910);
        // filepath：9 项通配符定义
        assert_eq!(header.filepath.len(), 9);
        assert_eq!(header.filepath[0].path, "Background/*.png");
        assert_eq!(header.filepath[0].def.as_deref(), Some("Default"));
        // offset：1 项
        assert_eq!(header.offset.len(), 1);
        assert_eq!(header.offset[0].a, 0.0);
    }

    #[test]
    fn skin_config_defaults_match_header() {
        let skin = LuaSkin::new(skin_dir()).expect("创建 LuaSkin 失败");
        let header = skin.load_header(&skin_dir().join("Play5.luaskin")).unwrap();
        let cfg = SkinConfigValues::from_header(&header);
        // Lane Side 无 def → 第一项 900
        assert_eq!(
            cfg.option.iter().find(|(n, _)| n == "Lane Side - 轨道位置").map(|(_, op)| *op),
            Some(900)
        );
        // Ghost Display def=Off → 910
        assert_eq!(
            cfg.option.iter().find(|(n, _)| n == "Ghost Display - 分数差显示").map(|(_, op)| *op),
            Some(910)
        );
        // offset 默认 a=0
        assert_eq!(
            cfg.offset.iter().find(|(n, _)| n == "Lane Line Transparency - 轨道分隔线透明度 (0-100)").map(|(_, o)| o[5]),
            Some(0.0)
        );
    }

    #[test]
    fn load_main_play5_returns_declarative_skin() {
        let skin = LuaSkin::new(skin_dir()).expect("创建 LuaSkin 失败");
        let entry = skin_dir().join("Play5.luaskin");
        let header = skin.load_header(&entry).unwrap();
        let cfg = SkinConfigValues::from_header(&header);
        let desc = skin.load_skin(&entry, &header, &cfg).expect("加载 main 描述表失败");

        // 描述表关键结构（声明式 skin 对象列表）
        let image: Table = desc.get("image").expect("缺少 skin.image");
        let n_image = image.raw_len() as usize;
        assert!(n_image >= 20, "skin.image 过少: {n_image}");

        let value: Table = desc.get("value").expect("缺少 skin.value");
        assert!(value.raw_len() >= 30, "skin.value 过少: {}", value.raw_len());

        let text: Table = desc.get("text").expect("缺少 skin.text");
        assert!(text.raw_len() >= 4, "skin.text 过少: {}", text.raw_len());

        let slider: Table = desc.get("slider").expect("缺少 skin.slider");
        assert_eq!(slider.raw_len(), 2, "skin.slider 应为 2 项");

        let graph: Table = desc.get("graph").expect("缺少 skin.graph");
        assert_eq!(graph.raw_len(), 7, "skin.graph 应为 7 项");

        let note: Table = desc.get("note").expect("缺少 skin.note");
        let note_id: String = note.get("id").expect("skin.note.id");
        assert_eq!(note_id, "notes");
    }
}
