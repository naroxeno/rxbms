// 皮肤特效材质片段着色器（bevy Material2d）
// 移植 beatoraja 的 ffmpeg.frag（通道重排）与 layer.frag（黑色抠像）。

#import bevy_sprite::{
    mesh2d_vertex_output::VertexOutput,
    mesh2d_view_bindings::view,
}

#ifdef SRGB_OUTPUT
#import bevy_render::color_operations::linear_to_srgb
#endif

struct SkinFxMaterial {
    flags: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    uv_rect: vec4<f32>, // min.xy, max.xy（0..1）
};

const FLAG_BLACK_KEY: u32 = 1u;
const FLAG_SWAP_RGB: u32 = 2u;

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: SkinFxMaterial;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var texture_sampler: sampler;

@fragment
fn fragment(
    mesh: VertexOutput,
) -> @location(0) vec4<f32> {
    let uv = mix(material.uv_rect.xy, material.uv_rect.zw, mesh.uv);
    var c = textureSample(texture, texture_sampler, uv);

    // ffmpeg shader：BGR/BRG → RGB 通道重排（BGA 帧以 3 通道 RGB 上传，省带宽）
    if ((material.flags & FLAG_SWAP_RGB) != 0u) {
        c = vec4(c.b, c.g, c.r, c.a);
    }
    // layer shader：纯黑像素透明（黑底特效图抠像，beatoraja `if(r==0&&g==0&&b==0)`）
    if ((material.flags & FLAG_BLACK_KEY) != 0u && c.r == 0.0 && c.g == 0.0 && c.b == 0.0) {
        c.a = 0.0;
    }
    // luma-key：按亮度淡出（比纯黑抠像平滑，消除特效圆形的抗锯齿黑圈）
    if ((material.flags & FLAG_LUMA_KEY) != 0u) {
        let luma = max(c.r, max(c.g, c.b));
        c.a = min(c.a, luma);
    }
#ifdef SRGB_OUTPUT
    c = vec4(linear_to_srgb(c.rgb), c.a);
#endif
    return c;
}
