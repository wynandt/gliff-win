// Upstream: the constants are `gliff_sw::convert`'s BT.709 limited-range inverse
// (`KY`, `KRV`, `KGU`, `KGV`, `KBU`) over 8192; `present_math`'s tests read this file.

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

VsOut vs_full(uint id : SV_VertexID) {
    float2 uv = float2((id << 1) & 2, id & 2);
    VsOut o;
    o.pos = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    o.uv = uv;
    return o;
}

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

float4 ps_blit(VsOut i) : SV_Target {
    return float4(picture.Sample(blit_sampler, i.uv).rgb, 1.0);
}

float4 ps_stats(VsOut i) : SV_Target {
    return picture.Sample(blit_sampler, i.uv);
}
