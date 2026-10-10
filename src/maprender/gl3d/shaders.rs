//! The GLSL of the 3D scene, one source for two dialects: `#version 330 core` (desktop) and
//! `#version 300 es` (OpenGL ES 3.0, and the Windows compatibility context that `wglCreateContext`
//! gives, where 330 core compiles too). Only `in`/`out`, `texture`, `texelFetch`, `gl_VertexID`,
//! `fwidth`, uniform arrays and `layout(location)` on vertex inputs are used, all in both.
//!
//! The camera is the pinhole of [`crate::maprender::view::Camera`]: the vertex shaders multiply
//! `uVP` ([`Camera::view_proj_rel`]) with the point *relative to the car* (`uCar`), so the f32
//! math stays exact over the whole island. `clip.w` is the camera depth in viewport px, which
//! the road width rule, the far fade and the LOD use.

use egui_glow::glow::{self, HasContext};

/// Version line plus the precision statements ES requires (desktop needs none).
fn header(gl: &glow::Context) -> &'static str {
    if gl.version().is_embedded {
        "#version 300 es\nprecision highp float;\nprecision highp int;\nprecision highp sampler2D;\nprecision highp usampler2D;\n"
    } else {
        "#version 330 core\n"
    }
}

/// Shared by the terrain and road shaders.
const COMMON: &str = r#"
uniform mat4 uVP;        // Camera::view_proj_rel, input (x-car.x, y*exag-car_y*exag, z-car.z, 1)
uniform vec3 uCar;       // car.x, car_y*exag, car.z
uniform float uExag;
uniform vec3 uCam;       // px per metre at the car, focal P (both in viewport px), A1 of the depth map
vec4 projectW(vec3 w) {
  return uVP * vec4(w.x - uCar.x, w.y * uExag - uCar.y, w.z - uCar.z, 1.0);
}
uniform usampler2D uH;   // heights, R16UI: h = q * 0.1 - 10
uniform ivec2 uHSize;
uniform vec3 uHGeo;      // x0, z1, res
float hpx(ivec2 p) {
  p = clamp(p, ivec2(0), uHSize - 1);
  return float(texelFetch(uH, p, 0).r) * 0.1 - 10.0;
}
// Bilinear between pixel centres, edge-clamped: HeightGrid::height.
float heightAt(vec2 xz) {
  vec2 f = vec2((xz.x - uHGeo.x) / uHGeo.z - 0.5, (uHGeo.y - xz.y) / uHGeo.z - 0.5);
  ivec2 i0 = ivec2(floor(f));
  vec2 t = f - vec2(i0);
  float a = mix(hpx(i0), hpx(i0 + ivec2(1, 0)), t.x);
  float b = mix(hpx(i0 + ivec2(0, 1)), hpx(i0 + ivec2(1, 1)), t.x);
  return mix(a, b, t.y);
}
// FAR_MIN_SCALE .. +FAR_FADE_DEPTH of the tilted 2D base: things smaller than 5 % fade out.
float farFade(float camZ) {
  float k = uCam.y / camZ;
  return clamp((k - 0.05) / 0.10, 0.0, 1.0);
}
"#;

pub const TERRAIN_VS: &str = r#"
layout(location = 0) in vec2 aIJ;
uniform ivec2 uBase;     // raster pixel of grid vertex (0, 0)
uniform int uStride;     // raster px per grid cell
uniform int uM;          // cells per side
uniform float uHasCoarser;
uniform vec2 uCarPx;     // the car's continuous raster position (fractional px)
out vec2 vXZ;
out vec3 vN;
out float vCamZ;
out float vEdge;
void main() {
  ivec2 ij = ivec2(aIJ);
  ivec2 px = uBase + ij * uStride;
  float h = hpx(px);
  // The outer edge of a level meets the coarser one: its odd vertices lie on the coarser
  // level's edge segments, so they take the mean of their two even neighbours (no cracks).
  bool bx = (ij.x == 0 || ij.x == uM), bz = (ij.y == 0 || ij.y == uM);
  if (uHasCoarser > 0.5) {
    if (bz && (ij.x & 1) == 1) h = 0.5 * (hpx(px - ivec2(uStride, 0)) + hpx(px + ivec2(uStride, 0)));
    else if (bx && (ij.y & 1) == 1) h = 0.5 * (hpx(px - ivec2(0, uStride)) + hpx(px + ivec2(0, uStride)));
  }
  // Geomorph: blend towards the coarser level's surface over the outer cells, so a level that
  // re-centres (every 16 * 2^l m) changes nothing visible: where rings meet alpha is exactly 1.
  // d = Chebyshev distance from the car in this level's cells. A level's outer boundary is
  // always >= 31 cells away (the centre snap is <= 1 cell), so alpha = 1 there and both sides
  // are identical; the blend starts at 24 cells, well outside the finer level's hole.
  float alpha = 0.0;
  if (uHasCoarser > 0.5) {
    vec2 dpx = abs(vec2(px) - uCarPx) / float(uStride);
    alpha = clamp((max(dpx.x, dpx.y) - 24.0) / 7.0, 0.0, 1.0);
    ivec2 o = ij & 1;
    float hc = h;
    if (o.x == 1 && o.y == 0) hc = 0.5 * (hpx(px - ivec2(uStride, 0)) + hpx(px + ivec2(uStride, 0)));
    else if (o.x == 0 && o.y == 1) hc = 0.5 * (hpx(px - ivec2(0, uStride)) + hpx(px + ivec2(0, uStride)));
    // odd-odd: the coarse cell's diagonal is the index buffer's (b, c), the anti-diagonal
    else if (o.x == 1 && o.y == 1) hc = 0.5 * (hpx(px + ivec2(uStride, -uStride)) + hpx(px + ivec2(-uStride, uStride)));
    h = mix(h, hc, alpha);
  }
  float hl = hpx(px - ivec2(uStride, 0)), hr = hpx(px + ivec2(uStride, 0));
  float hn = hpx(px - ivec2(0, uStride)), hs = hpx(px + ivec2(0, uStride));
  float sp = float(uStride) * uHGeo.z * 2.0;
  vN = normalize(vec3(-(hr - hl) * uExag / sp, 1.0, -(hn - hs) * uExag / sp));
  if (alpha > 0.0) {
    // the shading normal moves from the +-stride stencil to the +-2*stride one (the coarser level's)
    float hl2 = hpx(px - ivec2(2 * uStride, 0)), hr2 = hpx(px + ivec2(2 * uStride, 0));
    float hn2 = hpx(px - ivec2(0, 2 * uStride)), hs2 = hpx(px + ivec2(0, 2 * uStride));
    vec3 n2 = normalize(vec3(-(hr2 - hl2) * uExag / (2.0 * sp), 1.0, -(hn2 - hs2) * uExag / (2.0 * sp)));
    vN = normalize(mix(vN, n2, alpha));
  }
  vXZ = vec2(uHGeo.x + (float(px.x) + 0.5) * uHGeo.z, uHGeo.y - (float(px.y) + 0.5) * uHGeo.z);
  vec4 c = projectW(vec3(vXZ.x, h, vXZ.y));
  vCamZ = c.w;
  // The coarsest level ends in a straight edge 16 km out; a wide view (the Viewer at 8 km zoom)
  // reaches it, so its outer eighth fades into the backdrop instead of ending in a hard line.
  vEdge = uHasCoarser > 0.5 ? 0.0 : smoothstep(0.85, 1.0, max(abs(aIJ.x - 0.5 * float(uM)), abs(aIJ.y - 0.5 * float(uM))) / (0.5 * float(uM)));
  gl_Position = c;
}
"#;

pub const TERRAIN_FS: &str = r#"
in vec2 vXZ;
in vec3 vN;
in float vCamZ;
in float vEdge;
uniform sampler2D uMap;
uniform vec4 uMapGeo;    // origin_x, origin_z, px_per_m / image_w, px_per_m / image_h (MapCalibration::world_to_uv)
uniform vec3 uLook;      // brightness, saturation, opacity * fade alpha
uniform vec3 uNoMap;     // colour without a map
uniform vec3 uSun;
uniform float uShade;
uniform float uMirror;
uniform float uHasMap;
out vec4 oC;
void main() {
  vec2 uv = vec2((vXZ.x - uMapGeo.x) * uMapGeo.z, (uMapGeo.y - vXZ.y) * uMapGeo.w);
  if (uMirror < 0.5 && uHasMap > 0.5 && (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0)) discard;
  vec3 c = uHasMap > 0.5 ? texture(uMap, uv).rgb : uNoMap;
  float l = dot(c, vec3(0.299, 0.587, 0.114));
  c = mix(vec3(l), c, uLook.y) * uLook.x;
  float ndl = max(dot(normalize(vN), normalize(uSun)), 0.0);
  c *= mix(1.0, 0.45 + 0.75 * ndl, uShade);
  float a = uLook.z * farFade(vCamZ) * (1.0 - vEdge);
  oC = vec4(c * a, a);
}
"#;

/// The road ribbons, drawn in two passes (D81, `uPass`): **0 = casing**: every road that has a
/// casing at its full width (fill + casing px) in its casing colour, with the deck; **1 = fill**:
/// every road at its fill width in its own colour, nearer the eye by a depth step larger than all
/// the casing pass's ranks, so every fill lies over every casing. That is what makes junctions
/// clean: the outline runs round the union of the roads and never across another road's fill.
/// The fill pass leaves a cased road's deck to the casing pass (its bottom vertices stay up, so
/// walls and underside collapse); a road without a casing (the untyped slot, muted roads) is drawn
/// wholly in the fill pass. Hidden roads get alpha 0 in both.
///
/// **Translucent roads (D95, `uPass` 2):** a road whose alpha is below 1 (a type's opacity, the
/// muted look, the race line) is not drawn by those two passes (they hide it) but as one ribbon
/// in a pass of its own: a cased one at its full width in one go, the fill's colour inside and
/// the casing's colour round it, both at the road's alpha (`vFill`, `vInner`: the fill's share of
/// the half width), so no casing lies under the fill to show through it. The host draws this
/// pass twice, depth only first and then in colour with the depth test on `LEQUAL`, so of all
/// the overlapping translucent surfaces (the casing and fill of one road, two roads at a
/// junction, a deck's walls under its top) only the nearest is blended, once.
pub const ROAD_VS: &str = r#"
const float FEATHER = 1.2;  // px the ribbon is drawn wider than its nominal width, for the edge's anti-aliasing
layout(location = 0) in vec3 aPos;     // x, z, node height
layout(location = 1) in vec2 aTan;     // tangent x the mitre factor (mesh3d::Sample)
layout(location = 2) in float aS;
layout(location = 3) in uvec4 aFlags;  // side (1 / 255 = -1), bot, slot, cap centre (1)
layout(location = 4) in uint aRel;     // 1 = on the picked race line, 0 = other (in-race focus)
layout(location = 5) in float aAlong;  // the navigation route's mesh only: position along the route (segment + fraction)
uniform float uMode;       // 0 = terrain drape, 1 = node heights (RoadHeight)
uniform float uThick;      // deck thickness, m
uniform float uLift;       // metres above the ground (mesh3d::LIFT_M)
uniform vec4 uSlotA[10];   // rgb, alpha
uniform vec4 uSlotB[10];   // casing rgb, width factor
uniform vec4 uSlotC[10];   // dash m, gap m, casing alpha (0 = no casing), draw rank
uniform vec4 uRW;          // metres, min_px, max_px, casing_px  (px of the viewport, already x ppp x size factor)
uniform vec4 uFocus;       // mode (0 none, 1 muted, 2 hidden), muted alpha, muted width factor, 0
uniform vec3 uMuteRgb;
uniform vec2 uVp;          // viewport px
uniform vec2 uBias;        // depth bias toward the eye: base, per draw rank (fractions of the depth)
uniform float uPass;       // 0 = casing, 1 = fill (opaque roads), 2 = translucent roads (D95)
uniform float uCasingAlpha;
out vec4 vCol;
out vec4 vFill;            // translucent cased road: the fill's colour and alpha (vCol is the casing's)
out float vInner;          // ... and the fill's share of the ribbon's half width (0 = a plain ribbon)
out vec4 vDash;
out float vCamZ;
out float vShade;
out float vS;
out float vSide;
out float vEdge;          // the ribbon's nominal edge in `side` units (the ribbon is drawn FEATHER px wider)
out float vPx;            // the ribbon's nominal width, px
out float vWall;          // 1 at a deck's bottom vertices (the walls and underside are not feathered)
out float vWallCov;       // the share of a pixel a wall of this height covers
out float vBot;
out float vAlong;         // aAlong, for the fragment shader's cut at the car (uCut)
void main() {
  int slot = int(aFlags.z);
  float yT = heightAt(aPos.xy);
  // Cross-country (5) is always draped; a jump line (7) carries its taut string in aPos.z.
  float y = (slot == 5) ? yT : ((slot == 7) ? aPos.z : mix(yT, aPos.z, uMode));
  y += uLift;
  vec3 base = vec3(aPos.x, y, aPos.y);
  float cz0 = projectW(base).w;
  vec4 sa = uSlotA[slot], sb = uSlotB[slot], sc = uSlotC[slot];
  float wf = sb.w;
  vec3 rgb = sa.rgb;
  float alpha = sa.a;
  float casingA = sc.z;
  vec2 dash = sc.xy;
  if (uFocus.x > 0.5 && aRel == 0u) {
    // Hidden = alpha 0 (the fragment shader drops it); never a moved vertex, which would
    // drag the triangle it shares with a visible sample across the screen.
    rgb = uMuteRgb; alpha = uFocus.x > 1.5 ? 0.0 : uFocus.y; wf = wf * uFocus.z; casingA = 0.0; dash = vec2(0.0);
  }
  if (alpha <= 0.0) casingA = 0.0;      // a road switched off or hidden has no casing either
  // Translucent roads (alpha < 1 after the focus) are the third pass's; the other two hide them.
  bool trans = alpha < 0.999;
  bool transPass = uPass > 1.5;
  float ppm = uCam.x * uCam.y / max(cz0, 1.0);          // px per metre at this depth
  float wpx = max(clamp(ppm * uRW.x, uRW.y, uRW.z) * wf, 0.7);
  bool cased = casingA > 0.0;
  bool casingPass = uPass < 0.5;
  float px = wpx;
  float inner = 0.0;
  vec4 fillCol = vec4(rgb, alpha);
  if (transPass && cased) {
    // One ribbon: the casing's width, the fill's share of it marked for the fragment shader.
    px = wpx + uRW.w;
    inner = wpx / px;                                    // (of the nominal width; scaled below)
    rgb = sb.rgb;
    alpha = alpha * uCasingAlpha;
  } else if (casingPass) {
    px = wpx + uRW.w;
    rgb = sb.rgb;
    alpha = cased ? uCasingAlpha : 0.0;
    dash = vec2(0.0);
  }
  if (trans != transPass) { alpha = 0.0; fillCol.a = 0.0; }
  // The ribbon is drawn FEATHER screen px wider than its nominal width, and the fragment shader
  // fades its alpha over that margin by the distance to the nominal edge (D98): an anti-aliased
  // edge that does not depend on the multisampling, which also keeps a sub-pixel road a steady
  // grey. "Screen px": a road running across the view is foreshortened (tilt), so a feather of
  // FEATHER px / ppm metres would be a fraction of a pixel there; `across` is the px per metre
  // measured across the ribbon on the screen.
  vec3 perp = vec3(-aTan.y, 0.0, aTan.x);
  vec4 cp = projectW(base + perp);
  vec4 c0 = projectW(base);
  float across = length((cp.xy / cp.w - c0.xy / c0.w) * 0.5 * uVp) / max(length(perp), 1e-3);
  across = clamp(across, 0.2 * ppm, 2.0 * ppm);
  float hwNom = 0.5 * px / ppm;                          // nominal half width, metres
  float hw = hwNom + 0.5 * FEATHER / across;             // drawn half width
  // A cap's centre vertex sits on the point itself (mesh3d::push_vertices).
  float side = aFlags.w == 1u ? 0.0 : ((aFlags.x == 1u) ? 1.0 : -1.0);
  vec3 p = base + perp * side * hw;
  bool deck = transPass || !(cased && !casingPass);      // the casing pass drew this road's deck
  float drop = uThick / uExag;
  if (aFlags.y == 1u && deck) p.y -= drop;
  // A deck's wall seen from above is often under a pixel high: it is then drawn that much
  // fainter (its height in px, 1 at most) instead of flickering on and off with the pixel grid.
  vec3 pTop = p + vec3(0.0, (aFlags.y == 1u && deck) ? drop : 0.0, 0.0);
  vec4 wt = projectW(pTop);
  vec4 wb = projectW(pTop - vec3(0.0, drop, 0.0));
  float wallPx = length((wt.xy / wt.w - wb.xy / wb.w) * 0.5 * uVp);
  // ... so the fill pass drops a cased road's walls and underside (collapsed onto the top, they
  // would still be drawn over it where back faces are not culled).
  vBot = (!deck && aFlags.y == 1u) ? 1.0 : 0.0;
  vCol = vec4(rgb, alpha);
  vFill = fillCol;
  vEdge = hwNom / hw;
  vInner = inner * vEdge;
  vPx = px * across / ppm;
  vWall = aFlags.y == 1u ? 1.0 : 0.0;
  vWallCov = min(wallPx, 1.0);
  vDash = vec4(dash, 0.0, 0.0);
  vShade = aFlags.y == 1u ? 0.72 : 1.0;
  vS = aS;
  vAlong = aAlong;
  vSide = side;
  vec4 c = projectW(p);
  // Toward the eye along the view ray (same screen position): beats the coarse terrain levels
  // that sit above the exact bilinear surface the road follows, ranks overlapping types, and
  // (uBias per pass) puts every fill over every casing.
  float k = uBias.x + uBias.y * sc.w;
  c.z -= uCam.z * c.w * k;
  vCamZ = c.w;
  gl_Position = c;
}
"#;

pub const ROAD_FS: &str = r#"
in vec4 vCol;
in vec4 vFill;
in float vInner;
in vec4 vDash;
in float vCamZ;
in float vShade;
in float vS;
in float vSide;
in float vEdge;
in float vPx;
in float vWall;
in float vWallCov;
in float vBot;
in float vAlong;
uniform float uCut;        // the navigation route: the car's position along it (aAlong scale); below -1e8 = no cut
out vec4 oC;
// The navigation route's live progress: what lies behind the car (vAlong < uCut) is not drawn.
// The edge is anti-aliased over one pixel by the gradient of vAlong (the position along the
// route changes by that much per pixel), so the cut moves smoothly, also between two samples.
float cutCover() {
  if (uCut < -1.0e8) return 1.0;
  return clamp((vAlong - uCut) / max(fwidth(vAlong), 1e-5) + 0.5, 0.0, 1.0);
}
// Coverage of the pixel by the ribbon's nominal width: the distance to its edge in px (the
// gradient of `side` is the local px per unit, so it holds in perspective and foreshortened too),
// scaled down for a ribbon narrower than the pixel. A deck's walls (`vWall`) are not feathered at
// their edge but fainter when they are under a pixel high (`vWallCov`).
float ribbonCover() {
  float e = clamp((vEdge - abs(vSide)) / max(fwidth(vSide), 1e-4) + 0.5, 0.0, 1.0) * min(vPx, 1.0);
  return mix(e, vWallCov, vWall);
}
// Coverage of a dash pattern (dash, gap in metres) along the chain at the pixel: the distance to
// the nearest dash edge over the metres one pixel spans, and the pattern's mean share once a
// period is no longer wider than ~2 px (a hard edge there is moire, not a pattern).
float dashCover(vec2 d) {
  if (d.x <= 0.0) return 1.0;
  float per = d.x + d.y;
  float t = mod(vS, per);
  float w = max(fwidth(vS), 1e-4);
  float dist = t < d.x ? min(t, d.x - t) : -min(t - d.x, per - t);
  float c = clamp(dist / w + 0.5, 0.0, 1.0);
  return mix(d.x / per, c, clamp(per / w - 1.0, 0.0, 1.0));
}
void main() {
  if (vBot > 0.0) discard;
  float dc = dashCover(vDash.xy);
  float fade = farFade(vCamZ);
  if (vInner > 0.0) {
    // A translucent cased road (D95): the fill's colour inside, the casing's round it, the seam
    // anti-aliased by a mix, never two layers. The deck's walls and underside are all casing.
    // A dashed fill leaves its gaps empty (the map shows through).
    float f = vShade > 0.99 ? clamp((vInner - abs(vSide)) / max(fwidth(vSide), 1e-4) + 0.5, 0.0, 1.0) : 0.0;
    float fa = vFill.a * f * dc;
    float cov = ribbonCover() * fade * cutCover();
    float a = (vCol.a * (1.0 - f) + fa) * cov;
    if (a < 0.004) discard;
    vec3 c = (vCol.rgb * (vCol.a * (1.0 - f)) + vFill.rgb * fa) * vShade * cov;
    oC = vec4(c, a);
    return;
  }
  vec3 c = vCol.rgb * vShade;
  float al = vCol.a * fade * dc * ribbonCover() * cutCover();
  if (al < 0.004) discard;
  oC = vec4(c * al, al);
}
"#;

/// The own-car marker (D77 / D78, `marker.rs`): a model in metres (x right, y up from its ground,
/// z forward) placed at the telemetry position, turned by the yaw, scaled `uK.x`; heights are
/// divided by the exaggeration so the model keeps its shape. `uK.y` > 0 = the outline hull,
/// pushed out along the smoothed normal and drawn flat dark.
pub const MARKER_VS: &str = r#"
layout(location = 0) in vec3 aPos;
layout(location = 1) in vec3 aN;
layout(location = 2) in vec3 aSm;
layout(location = 3) in vec4 aMat;
uniform vec3 uPos;       // the model's ground point: x, y (real metres), z
uniform vec2 uYaw;       // sin, cos of the telemetry yaw
uniform vec2 uK;         // model scale, outline push (metres; 0 = the model itself)
out vec3 vN;
out vec4 vMat;
void main() {
  vec3 l = aPos * uK.x + aSm * uK.y;
  vec3 r = vec3(uYaw.y, 0.0, -uYaw.x);
  vec3 f = vec3(uYaw.x, 0.0, uYaw.y);
  vec3 w = vec3(uPos.x + l.x * r.x + l.z * f.x, uPos.y + l.y / uExag, uPos.z + l.x * r.z + l.z * f.z);
  vN = aN.x * r + vec3(0.0, aN.y, 0.0) + aN.z * f;
  vMat = aMat;
  gl_Position = projectW(w);
}
"#;

pub const MARKER_FS: &str = r#"
in vec3 vN;
in vec4 vMat;
uniform vec3 uColor;     // the car colour, linear 0..1 of the sRGB values (as egui's)
uniform float uHull;     // 1 = the outline pass
uniform float uAlpha;
out vec4 oC;
void main() {
  if (uHull > 0.5) {
    float a = 0.9 * uAlpha;
    oC = vec4(vec3(0.02, 0.025, 0.035) * a, a);
    return;
  }
  vec3 n = normalize(vN);
  // Tops at full colour (a white car stays white from above), sides darker, lit from the
  // terrain's sun side so the shape reads.
  float sh = clamp(0.60 + 0.40 * n.y + 0.16 * dot(n.xz, normalize(vec2(-0.5, 0.35))), 0.0, 1.0);
  vec3 c = (uColor * vMat.x + vMat.yzw) * sh;
  oC = vec4(c * uAlpha, uAlpha);
}
"#;

/// The trail ribbons: each vertex carries its end and the other end of its segment and is pushed
/// sideways on screen, so the line is `uW.x` px wide at the car and follows the perspective
/// (clamped like the flat trail's taper).
pub const TRAIL_VS: &str = r#"
layout(location = 0) in vec3 aP;
layout(location = 1) in vec3 aQ;
layout(location = 2) in vec2 aSA;   // side (+1 / -1), alpha
uniform vec2 uW;         // width px at the car, depth bias
uniform vec2 uVp;        // viewport px
out float vA;
out float vCamZ;
void main() {
  vec4 p = projectW(aP);
  vec4 q = projectW(aQ);
  vec2 hv = 0.5 * uVp;
  vec2 sp = p.xy / max(p.w, 1e-3) * hv;
  vec2 sq = q.xy / max(q.w, 1e-3) * hv;
  vec2 d = sq - sp;
  d = dot(d, d) > 1e-8 ? normalize(d) : vec2(1.0, 0.0);
  float k = clamp(uCam.y / max(p.w, 1.0), 0.4, 1.5);
  vec2 off = vec2(-d.y, d.x) * aSA.x * 0.5 * uW.x * k;
  p.xy += off / hv * p.w;
  p.z -= uCam.z * p.w * uW.y;
  // A segment reaching behind the eye has no screen direction: leave it out.
  vA = (p.w > 1.0 && q.w > 1.0) ? aSA.y : 0.0;
  vCamZ = p.w;
  gl_Position = p;
}
"#;

pub const TRAIL_FS: &str = r#"
in float vA;
in float vCamZ;
uniform vec3 uColor;
uniform float uAlpha;    // fade x pass strength (the hidden pass is fainter)
out vec4 oC;
void main() {
  float a = vA * uAlpha * farFade(vCamZ);
  if (a < 0.004) discard;
  oC = vec4(uColor * a, a);
}
"#;

pub const COMP_VS: &str = r#"
out vec2 vUv;
void main() {
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  vUv = p;
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

/// The scene texture is premultiplied; the rounded-rectangle mask (the HUD pill) and the fade
/// alpha multiply all four channels.
pub const COMP_FS: &str = r#"
in vec2 vUv;
uniform sampler2D uTex;
uniform vec2 uUv;
uniform vec2 uSizePx;
uniform float uRadius;
uniform float uAlpha;
out vec4 oC;
void main() {
  vec4 c = texture(uTex, vUv * uUv);
  vec2 q = abs(vUv * uSizePx - 0.5 * uSizePx) - (0.5 * uSizePx - vec2(uRadius));
  float sdf = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - uRadius;
  oC = c * (uAlpha * clamp(0.5 - sdf, 0.0, 1.0));
}
"#;

/// Which stage uses [`COMMON`]: the composite does not.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Common {
    Yes,
    No,
}

/// A linked program and its uniform locations (looked up once).
pub struct Prog {
    pub p: glow::Program,
    locs: Vec<(&'static str, Option<glow::UniformLocation>)>,
}

impl Prog {
    pub fn u(&self, name: &str) -> Option<&glow::UniformLocation> {
        self.locs.iter().find(|(n, _)| *n == name).and_then(|(_, l)| l.as_ref())
    }
}

/// Compile and link; `uniforms` are looked up once and found by [`Prog::u`]. The error text
/// carries the driver's info log.
pub fn compile(gl: &glow::Context, name: &str, vs: &str, fs: &str, common: Common, uniforms: &[&'static str]) -> Result<Prog, String> {
    let hdr = header(gl);
    let pre = if common == Common::Yes { COMMON } else { "" };
    // SAFETY: plain GL object creation on the current context.
    unsafe {
        let mk = |ty: u32, src: &str| -> Result<glow::Shader, String> {
            let s = gl.create_shader(ty)?;
            gl.shader_source(s, &format!("{hdr}{pre}{src}"));
            gl.compile_shader(s);
            if !gl.get_shader_compile_status(s) {
                let log = gl.get_shader_info_log(s);
                gl.delete_shader(s);
                return Err(format!("{name} {} shader: {}", if ty == glow::VERTEX_SHADER { "vertex" } else { "fragment" }, log.trim()));
            }
            Ok(s)
        };
        let v = mk(glow::VERTEX_SHADER, vs)?;
        let f = match mk(glow::FRAGMENT_SHADER, fs) {
            Ok(f) => f,
            Err(e) => {
                gl.delete_shader(v);
                return Err(e);
            }
        };
        let p = gl.create_program()?;
        gl.attach_shader(p, v);
        gl.attach_shader(p, f);
        gl.link_program(p);
        gl.delete_shader(v);
        gl.delete_shader(f);
        if !gl.get_program_link_status(p) {
            let log = gl.get_program_info_log(p);
            gl.delete_program(p);
            return Err(format!("{name} link: {}", log.trim()));
        }
        let locs = uniforms.iter().map(|&n| (n, gl.get_uniform_location(p, n))).collect();
        Ok(Prog { p, locs })
    }
}

pub const TERRAIN_UNIFORMS: &[&str] = &[
    "uVP", "uCar", "uExag", "uCam", "uH", "uHSize", "uHGeo", "uBase", "uNoMap", "uStride", "uM", "uHasCoarser", "uCarPx", "uMap", "uMapGeo", "uLook", "uSun", "uShade", "uMirror",
    "uHasMap",
];
pub const ROAD_UNIFORMS: &[&str] = &[
    "uVP", "uCar", "uExag", "uCam", "uH", "uHSize", "uHGeo", "uMode", "uThick", "uLift", "uSlotA", "uSlotB", "uSlotC", "uRW", "uFocus", "uMuteRgb", "uBias",
    "uCasingAlpha", "uPass", "uVp", "uCut",
];
pub const COMP_UNIFORMS: &[&str] = &["uTex", "uUv", "uSizePx", "uRadius", "uAlpha"];
pub const MARKER_UNIFORMS: &[&str] = &["uVP", "uCar", "uExag", "uCam", "uPos", "uYaw", "uK", "uColor", "uHull", "uAlpha"];
pub const TRAIL_UNIFORMS: &[&str] = &["uVP", "uCar", "uExag", "uCam", "uH", "uHSize", "uHGeo", "uW", "uVp", "uColor", "uAlpha"];
