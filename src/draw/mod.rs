//! Helpers types for drawing Spine skeletons.
//!
//! Drawers generate mesh information ready to be used in graphics libraries and game engines.
//!
//! Two implementations are currently provided:
//! - [`SimpleDrawer`]
//! - [`CombinedDrawer`]

mod combined;
mod simple;

pub use combined::*;
pub use simple::*;

/// Cull direction to use with helper draw functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CullDirection {
    Clockwise,
    CounterClockwise,
}

impl CullDirection {
    const fn index_order(self) -> [usize; 3] {
        match self {
            Self::Clockwise => [0, 1, 2],
            Self::CounterClockwise => [2, 1, 0],
        }
    }
}

/// Color space to use with helper draw functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    SRGB,
    Linear,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test::TestAsset, Physics, Skeleton, SkeletonClipping, SkeletonJson};
    use std::sync::Arc;

    #[test]
    fn large_mesh_uses_coordinate_pairs_without_empty_leading_batches() {
        const VERTICES: usize = 512;
        let atlas = Arc::new(TestAsset::spineboy().atlas());
        let region = atlas.regions().next().unwrap().name().to_owned();
        let coordinates = (0..VERTICES)
            .flat_map(|i| [i, i % 2])
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let uvs = vec!["0"; VERTICES * 2].join(",");
        let json = format!(
            r#"{{
            "bones":[{{"name":"root","x":7,"y":11}}],
            "slots":[{{"name":"hidden","bone":"root"}},
                     {{"name":"mesh","bone":"root","attachment":"mesh","blend":"additive"}}],
            "skins":[{{"name":"default","attachments":{{"mesh":{{"mesh":{{
                "type":"mesh","path":"{region}","uvs":[{uvs}],
                "vertices":[{coordinates}],"triangles":[0,1,511]
            }}}}}}}}]
        }}"#
        );
        let data = Arc::new(
            SkeletonJson::new(atlas)
                .read_skeleton_data(json.as_bytes())
                .unwrap(),
        );
        let mut skeleton = Skeleton::new(data);
        skeleton.update_world_transform(Physics::Pose);
        for (cull_direction, expected_indices) in [
            (CullDirection::Clockwise, [0, 1, 511]),
            (CullDirection::CounterClockwise, [511, 1, 0]),
        ] {
            let simple = SimpleDrawer {
                cull_direction,
                premultiplied_alpha: false,
                color_space: ColorSpace::SRGB,
            }
            .draw(&mut skeleton, None);
            let combined = CombinedDrawer {
                cull_direction,
                premultiplied_alpha: false,
                color_space: ColorSpace::SRGB,
            }
            .draw(&mut skeleton, None);
            assert_eq!(simple.len(), 1);
            assert_eq!(combined.len(), 1);
            assert_eq!(simple[0].vertices.len(), VERTICES);
            assert_eq!(simple[0].uvs.len(), VERTICES);
            assert_eq!(simple[0].indices, expected_indices);
            for (i, vertex) in simple[0].vertices.iter().enumerate() {
                assert_eq!(*vertex, [i as f32 + 7., (i % 2) as f32 + 11.]);
            }
            assert_eq!(combined[0].vertices, simple[0].vertices);
            assert_eq!(combined[0].uvs, simple[0].uvs);
            assert_eq!(combined[0].indices, simple[0].indices);
            assert_eq!(combined[0].colors.len(), VERTICES);
            assert_eq!(combined[0].dark_colors.len(), VERTICES);
        }
    }

    #[test]
    fn combined_batches_split_at_index_limit_including_clip_expansion() {
        const PREFIX_VERTICES: usize = 65_532;
        let atlas = Arc::new(TestAsset::spineboy().atlas());
        let region = atlas.regions().next().unwrap().name().to_owned();
        let coordinates = vec!["0"; PREFIX_VERTICES * 2].join(",");
        for (clipped, prefix_visible) in [(false, true), (true, true), (true, false)] {
            let (slots, attachments) = if clipped {
                (
                    r#",{"name":"clip","bone":"root","attachment":"clip"},
                        {"name":"last","bone":"root","attachment":"mesh"}"#
                        .to_owned(),
                    format!(
                        r#", "clip":{{"clip":{{"type":"clipping","end":"last",
                        "vertexCount":4,"vertices":[-1,-1,-1,1,1,1,1,-1]}}}},
                        "last":{{"mesh":{{"type":"mesh","path":"{region}",
                        "vertices":[-2,-1,2,-1,0,2],"uvs":[0,0,1,0,0.5,1],
                        "triangles":[0,1,2]}}}}"#
                    ),
                )
            } else {
                (
                    r#",{"name":"quad1","bone":"root","attachment":"quad"},
                        {"name":"quad2","bone":"root","attachment":"quad"}"#.to_owned(),
                    ["quad1", "quad2"].map(|slot| format!(
                        r#", "{slot}":{{"quad":{{"path":"{region}","width":2,"height":2}}}}"#
                    )).concat(),
                )
            };
            let prefix_indices = if prefix_visible { "0,1,65531" } else { "" };
            let json = format!(
                r#"{{
                "bones":[{{"name":"root"}}],
                "slots":[{{"name":"prefix","bone":"root","attachment":"mesh"}}{slots}],
                "skins":[{{"name":"default","attachments":{{
                    "prefix":{{"mesh":{{"type":"mesh","path":"{region}",
                    "vertices":[{coordinates}],"uvs":[{coordinates}],
                    "triangles":[{prefix_indices}]}}}}{attachments}
                }}}}]
            }}"#
            );
            let data = Arc::new(
                SkeletonJson::new(atlas.clone())
                    .read_skeleton_data(json.as_bytes())
                    .unwrap(),
            );
            let mut skeleton = Skeleton::new(data);
            skeleton.update_world_transform(Physics::Pose);
            for direction in [CullDirection::Clockwise, CullDirection::CounterClockwise] {
                let simple = SimpleDrawer {
                    cull_direction: direction,
                    premultiplied_alpha: false,
                    color_space: ColorSpace::SRGB,
                }
                .draw(&mut skeleton, Some(&mut SkeletonClipping::new()));
                let combined = CombinedDrawer {
                    cull_direction: direction,
                    premultiplied_alpha: false,
                    color_space: ColorSpace::SRGB,
                }
                .draw(&mut skeleton, Some(&mut SkeletonClipping::new()));
                assert_eq!(
                    combined.len(),
                    if prefix_visible { 2 } else { 1 },
                    "clipped={clipped}"
                );
                for batch in &combined {
                    assert!(!batch.indices.is_empty());
                    assert!(batch.vertices.len() <= 65_536);
                    assert_eq!(batch.vertices.len(), batch.uvs.len());
                    assert_eq!(batch.vertices.len(), batch.colors.len());
                    assert_eq!(batch.vertices.len(), batch.dark_colors.len());
                }
                if prefix_visible {
                    assert_eq!(
                        combined[0].vertices.len(),
                        if clipped { PREFIX_VERTICES } else { 65_536 }
                    );
                }
                assert_eq!(
                    combined.iter().map(|b| b.indices.len()).sum::<usize>(),
                    simple.iter().map(|b| b.indices.len()).sum::<usize>()
                );
                let expected = simple.iter().flat_map(|mesh| {
                    mesh.indices
                        .iter()
                        .map(|&i| (mesh.vertices[usize::from(i)], mesh.uvs[usize::from(i)]))
                });
                let actual = combined.iter().flat_map(|mesh| {
                    mesh.indices
                        .iter()
                        .map(|&i| (mesh.vertices[usize::from(i)], mesh.uvs[usize::from(i)]))
                });
                assert!(
                    expected.eq(actual),
                    "clipped={clipped}, direction={direction:?}"
                );
            }
        }
    }
}
