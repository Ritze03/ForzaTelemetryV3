//! The race lines' start / finish marks in the 3D scene (D88): posts standing on the line at its
//! own height, drawn with the trail ribbon program, **depth-tested** so terrain and decks hide
//! them like the roads (the 2D marks were egui shapes over the picture and showed through hills).
//!
//! * a sprint's start: a green post with a dark outline (the 2D green dot);
//! * a circuit's start and a sprint's finish: a chequered post (white / black cells, the 2D flag).
//!
//! A post is a vertical ribbon of constant screen width ([`POST_PT`]) and a height that is
//! [`POST_H_PT`] points *at the car's distance* (`height_m`, from the camera's scale), so it stays
//! readable at every zoom and shrinks with the perspective like the icons. *Why posts and not
//! flat bars across the road:* a bar lying on the road is a hairline from the tilted camera and
//! has to be as wide as the road; the post is one ribbon per mark, a few vertices, and reads at
//! any angle. Built per frame (the size follows the zoom): a few hundred marks in the worst case
//! (`All`: 170 lines), ~30 vertices each.
//!
//! Marks whose line height is 4 m or more under the terrain (a tunnel start) are drawn without
//! the depth test, like the road tunnels, instead of being buried.

use egui::Color32;

use super::marker::{Trail3d, TrailSeg};
use crate::maprender::mesh3d::{known_y, LIFT_M, RACE_TUNNEL_DEPTH_M};
use crate::maprender::racesel::{MarkKind, RaceMark};
use crate::maprender::style;
use crate::maprender::terrain::Terrain;

/// Post width (points, x the size factor) and the outline's.
pub const POST_PT: f32 = 5.0;
pub const OUTLINE_PT: f32 = 8.0;
/// Post height in points at the car's distance (the 2D flag is 9 pt; a post is taller to read
/// from an angle).
pub const POST_H_PT: f32 = 18.0;

/// One batch of the post ribbons: all of one colour and width, drawn in one call.
pub struct MarkBatch {
    pub trail: Trail3d,
    /// Ribbon width in points (before `s` x `ppp`).
    pub width_pt: f32,
}

/// The posts of `marks` as ribbon batches, bottom layer first: `[dark outline of every post,
/// sprint-start green, chequer white, chequer dark]`, for the posts in the open (`tunnel` false)
/// or the ones in a tunnel (true). `height_m` = a post's real height, `exag` = the camera's
/// exaggeration (the shader scales world heights by it; the post keeps its real height).
pub fn batches(marks: &[RaceMark], terrain: &Terrain, height_m: f32, exag: f32, tunnel: bool) -> Vec<MarkBatch> {
    let col = |c: Color32| Trail3d { segs: Vec::new(), colour: c };
    let (mut outline, mut green, mut white, mut dark) = (col(style::START_DOT_OUTLINE), col(style::START_DOT), col(Color32::WHITE), col(Color32::from_gray(0x11)));
    let h = height_m / exag.max(0.1);
    for m in marks {
        let ground = terrain.height(m.at[0], m.at[1]);
        let y = known_y(m.y).unwrap_or(ground) + LIFT_M;
        if (ground - y >= RACE_TUNNEL_DEPTH_M) != tunnel {
            continue;
        }
        let seg = |a: f32, b: f32| TrailSeg { a: [m.at[0], y + h * a, m.at[1]], b: [m.at[0], y + h * b, m.at[1]], alpha: 1.0 };
        outline.segs.push(seg(0.0, 1.0));
        match m.kind {
            MarkKind::SprintStart => green.segs.push(seg(0.0, 1.0)),
            MarkKind::Chequered => {
                // three cells from the top: white, dark, white (the 2D flag's corner cell is white)
                white.segs.push(seg(2.0 / 3.0, 1.0));
                dark.segs.push(seg(1.0 / 3.0, 2.0 / 3.0));
                white.segs.push(seg(0.0, 1.0 / 3.0));
            }
        }
    }
    let mut out = vec![MarkBatch { trail: outline, width_pt: OUTLINE_PT }];
    out.extend([green, white, dark].map(|trail| MarkBatch { trail, width_pt: POST_PT }));
    out.retain(|b| !b.trail.segs.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posts_stand_on_the_line_and_tunnel_posts_are_apart() {
        let t = Terrain::synthetic();
        let g = t.height(0.0, 0.0);
        let marks = [
            RaceMark { at: [0.0, 0.0], y: g + 1.0, kind: MarkKind::SprintStart },
            RaceMark { at: [0.0, 0.0], y: 0.0, kind: MarkKind::Chequered },
            RaceMark { at: [0.0, 0.0], y: g - 10.0, kind: MarkKind::Chequered },
        ];
        let open = batches(&marks, &t, 20.0, 2.0, false);
        let under = batches(&marks, &t, 20.0, 2.0, true);
        // outline (2 posts), green (1), chequer white (2 cells), dark (1 cell)
        let n: Vec<usize> = open.iter().map(|b| b.trail.segs.len()).collect();
        assert_eq!(n, vec![2, 1, 2, 1]);
        assert_eq!(under.iter().map(|b| b.trail.segs.len()).collect::<Vec<_>>(), vec![1, 2, 1]);
        // the sprint post: from the line (+ the road lift) up by the real height / exaggeration
        let s = open[1].trail.segs[0];
        assert!((s.a[1] - (g + 1.0 + LIFT_M)).abs() < 1e-4 && (s.b[1] - s.a[1] - 10.0).abs() < 1e-4, "{s:?}");
        // an unknown height (0) takes the terrain
        assert!((open[0].trail.segs[1].a[1] - (g + LIFT_M)).abs() < 1e-4);
    }
}
