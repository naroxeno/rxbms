//! 皮肤特效材质（GPU）：移植 beatoraja 的 ffmpeg / layer shader 语义。
//!
//! - **black-key（layer shader）**：`if(r==0&&g==0&&b==0) alpha=0`——黑底特效图
//!   （如 Bomb/Default.png）按纯黑抠像，替代 CPU luma-key 预处理（保留原始图）；
//! - **swap-rgb（ffmpeg shader）**：`vec4(c.b, c.g, c.r, c.a)`——BGA 视频帧以
//!   RGB（3 通道）上传、GPU 重排为 RGBA，省 25% 上传带宽与一次 CPU 转换。
//!
//! 应用方式：`Mesh2d + MeshMaterial2d<SkinFxMaterial>`（bevy `Material2d`），
//! 用于 `blend=2` 特效槽与 BGA 帧槽；普通槽仍用 `Sprite`（图集裁剪）。

use bevy::{
    prelude::*,
    reflect::TypePath,
    render::render_resource::{AsBindGroup, ShaderType},
    shader::ShaderRef,
    sprite_render::{AlphaMode2d, Material2d},
};

/// shader 路径（`assets/shaders/skin_fx.wgsl`）。
pub const SKIN_FX_SHADER_PATH: &str = "shaders/skin_fx.wgsl";

/// 特效标志：black-key 抠黑。
pub const FLAG_BLACK_KEY: u32 = 1 << 0;
/// 特效标志：RGB 通道重排（BGR/BRG → RGB，BGA 帧用）。
pub const FLAG_SWAP_RGB: u32 = 1 << 1;
/// 特效标志：luma-key 亮度淡出（`alpha = min(a, 亮度)`）——比纯黑抠像更平滑，
/// 抗锯齿灰边按亮度淡出，消除圆形特效的黑圈。
pub const FLAG_LUMA_KEY: u32 = 1 << 2;

/// 材质 uniform（与 WGSL `SkinFxMaterial` 布局一致：4×u32 + vec4 = 32 字节）。
///
/// 不用数组填充（encase 要求 uniform 数组 stride 为 16 的倍数）。
#[derive(ShaderType, Clone, Copy, Debug)]
pub struct SkinFxUniform {
    /// 特效标志位。
    pub flags: u32,
    /// 填充（对齐 uv_rect 到 16 字节）。
    pub _pad0: u32,
    pub _pad1: u32,
    pub _pad2: u32,
    /// 采样区域（0..1 的 min.x, min.y, max.x, max.y；整图为 (0,0,1,1)）。
    pub uv_rect: Vec4,
}

impl SkinFxUniform {
    /// 整图采样。
    #[must_use]
    pub fn full(flags: u32) -> Self {
        Self {
            flags,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
            uv_rect: Vec4::new(0.0, 0.0, 1.0, 1.0),
        }
    }
}

/// 皮肤特效材质。
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct SkinFxMaterial {
    #[uniform(0)]
    pub uniform: SkinFxUniform,
    #[texture(1)]
    #[sampler(2)]
    pub texture: Handle<Image>,
}

impl Material2d for SkinFxMaterial {
    fn fragment_shader() -> ShaderRef {
        SKIN_FX_SHADER_PATH.into()
    }

    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Blend
    }
}
