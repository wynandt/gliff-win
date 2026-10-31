// The present path's two passes, compiled once at start with D3DCompile
// (vs_5_0 / ps_5_0). The constants are gliff-sw's 13-bit BT.709
// limited-range inverse (gliff 089662fd crates/gliff-sw/src/convert.rs:48-52)
// divided by 8192, so this and the CPU oracle (`present_math`) agree;
// `present_math`'s tests read this file and check each line below.

static const float KY = 9539.0 / 8192.0;
static const float KRV = 14686.0 / 8192.0;
static const float KGU = -1747.0 / 8192.0;
static const float KGV = -4366.0 / 8192.0;
static const float KBU = 17305.0 / 8192.0;

Texture2D<float> plane_y : register(t0);
Texture2D<float> plane_u : register(t1);
Texture2D<float> plane_v : register(t2);
Texture2D<float4> picture : register(t3);
SamplerState blit_sampler : register(s0);

struct VsOut {
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
};

// One triangle over the viewport; no vertex buffer.
VsOut vs_full(uint id : SV_VertexID) {
    float2 uv = float2((id << 1) & 2, id & 2);
    VsOut o;
    o.pos = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    o.uv = uv;
    return o;
}

// Pass 1, into the stream-sized RGB texture: Y at the pixel, U and V at the
// pixel when they are full size (4:4:4), else at pixel / 2 (4:2:0, chroma
// repeated over each 2x2 block, as gliff-sw).
float4 ps_yuv(VsOut i) : SV_Target {
    int2 p = int2(i.pos.xy);
    uint luma_w, luma_h, chroma_w, chroma_h;
    plane_y.GetDimensions(luma_w, luma_h);
    plane_u.GetDimensions(chroma_w, chroma_h);
    int2 c = chroma_w == luma_w ? p : p / 2;
    float y = plane_y.Load(int3(p, 0)) * 255.0 - 16.0;
    float cb = plane_u.Load(int3(c, 0)) * 255.0 - 128.0;
    float cr = plane_v.Load(int3(c, 0)) * 255.0 - 128.0;
    float r = KY * y + KRV * cr;
    float g = KY * y + KGU * cb + KGV * cr;
    float b = KY * y + KBU * cb;
    return float4(saturate(float3(r, g, b) / 255.0), 1.0);
}

// Pass 2, into the back buffer's layout rect: point sampling at an integer
// factor, linear otherwise (the sampler is chosen per Layout::filter).
float4 ps_blit(VsOut i) : SV_Target {
    return float4(picture.Sample(blit_sampler, i.uv).rgb, 1.0);
}
