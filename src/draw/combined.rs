use crate::{c::c_void, BlendMode, Skeleton, SkeletonClipping};

use super::{ColorSpace, CullDirection};

#[allow(unused_imports)]
use crate::{draw::SimpleDrawer, extension};

/// Renderable mesh data produced by [`CombinedDrawer`].
pub struct CombinedRenderable {
    /// A list of vertex attributes for a mesh.
    pub vertices: Vec<[f32; 2]>,
    /// A list of UV attributes for a mesh.
    pub uvs: Vec<[f32; 2]>,
    /// A list of color attributes for a mesh.
    pub colors: Vec<[f32; 4]>,
    /// A list of dark color attributes for a mesh.
    /// See the [Spine User Guide](http://en.esotericsoftware.com/spine-slots#Tint-black).
    pub dark_colors: Vec<[f32; 4]>,
    /// A list of indices for a mesh.
    pub indices: Vec<u16>,
    /// The blend mode to use when drawing this mesh.
    pub blend_mode: BlendMode,
    /// The attachment's renderer object as a raw pointer. Usually represents the texture created
    /// from [`extension::set_create_texture_cb`].
    pub attachment_renderer_object: Option<*const c_void>,
}

impl Default for CombinedRenderable {
    fn default() -> Self {
        Self {
            vertices: Vec::new(),
            uvs: Vec::new(),
            colors: Vec::new(),
            dark_colors: Vec::new(),
            indices: Vec::new(),
            blend_mode: BlendMode::Normal,
            attachment_renderer_object: None,
        }
    }
}

/// Reusable working memory for [`CombinedDrawer::draw_into`].
#[derive(Default)]
pub struct CombinedDrawerScratch {
    world_vertices: Vec<f32>,
    renderables: Vec<CombinedRenderable>,
}

fn reuse_batch(renderables: &mut Vec<CombinedRenderable>, index: usize) -> &mut CombinedRenderable {
    if renderables.len() <= index {
        renderables.push(CombinedRenderable::default());
    }
    let renderable = &mut renderables[index];
    renderable.vertices.clear();
    renderable.uvs.clear();
    renderable.colors.clear();
    renderable.dark_colors.clear();
    renderable.indices.clear();
    renderable.blend_mode = BlendMode::Normal;
    renderable.attachment_renderer_object = None;
    renderable
}

/// A combined drawer with a mesh combining optimization.
///
/// Assumes use of the default atlas attachment loader.
///
/// See [`CombinedDrawer::draw`]
pub struct CombinedDrawer {
    pub cull_direction: CullDirection,
    pub premultiplied_alpha: bool,
    pub color_space: ColorSpace,
}

impl CombinedDrawer {
    /// This function returns a list of [`CombinedRenderable`] structs containing all the necessary
    /// data to create and render meshes. Attachments are batched together into a single renderable
    /// so long as their blend mode or renderer object is not different from the previous
    /// attachment. If a [`SkeletonClipping`] is provided, meshes will be properly clipped. The
    /// renderables are expected to be rendered in the order provided with the first renderable
    /// being drawn behind all the others.
    ///
    /// This drawer can provide a significant performance advantage over the [`SimpleDrawer`] in
    /// most cases.
    ///
    /// # Panics
    ///
    /// Panics if not using the default attachment loader with valid atlas regions,
    /// or if one attachment exceeds the 16-bit vertex limit after clipping.
    pub fn draw(
        &self,
        skeleton: &mut Skeleton,
        clipper: Option<&mut SkeletonClipping>,
    ) -> Vec<CombinedRenderable> {
        let mut scratch = CombinedDrawerScratch::default();
        let count = self.draw_into(skeleton, clipper, &mut scratch).len();
        scratch.renderables.truncate(count);
        scratch.renderables
    }

    /// Draws into reusable storage and returns only batches with indexed geometry.
    ///
    /// Spare attribute buffers stay in `scratch` for subsequent draws. Renderer object pointers
    /// in the returned slice remain valid only while their owning atlas is alive.
    ///
    /// # Panics
    ///
    /// Panics if not using the default attachment loader with valid atlas regions,
    /// or if one attachment exceeds the 16-bit vertex limit after clipping.
    pub fn draw_into<'a>(
        &self,
        skeleton: &mut Skeleton,
        mut clipper: Option<&mut SkeletonClipping>,
        scratch: &'a mut CombinedDrawerScratch,
    ) -> &'a [CombinedRenderable] {
        const MAX_VERTICES: usize = u16::MAX as usize + 1;
        let CombinedDrawerScratch {
            world_vertices,
            renderables,
        } = scratch;
        let mut renderable_count = 0;
        let mut renderable = reuse_batch(renderables, renderable_count);
        let index_order = self.cull_direction.index_order();
        for slot_index in 0..skeleton.slots_count() {
            let Some(slot) = skeleton.draw_order_at_index(slot_index) else {
                continue;
            };
            if !slot.bone().active() {
                if let Some(clipper) = clipper.as_deref_mut() {
                    clipper.clip_end(&slot);
                }
                continue;
            }

            if let Some(clipping_attachment) = slot.attachment().and_then(|a| a.as_clipping()) {
                if let Some(clipper) = clipper.as_deref_mut() {
                    clipper.clip_start(&slot, &clipping_attachment);
                }
                continue;
            } else if slot
                .attachment()
                .and_then(|a| a.as_mesh())
                .is_none_or(|mesh| mesh.triangles_count() == 0)
                && slot.attachment().and_then(|a| a.as_region()).is_none()
            {
                if let Some(clipper) = clipper.as_deref_mut() {
                    clipper.clip_end(&slot);
                }
                continue;
            }

            if let Some(mesh_attachment) = slot.attachment().and_then(|a| a.as_mesh()) {
                world_vertices.resize(mesh_attachment.world_vertices_length() as usize, 0.);
                unsafe {
                    mesh_attachment.compute_world_vertices(
                        &slot,
                        0,
                        mesh_attachment.world_vertices_length(),
                        &mut *world_vertices,
                        0,
                        2,
                    );
                }
            } else if let Some(region_attachment) = slot.attachment().and_then(|a| a.as_region()) {
                world_vertices.resize(8, 0.);
                unsafe {
                    region_attachment.compute_world_vertices(&slot, &mut *world_vertices, 0, 2);
                }
            }

            let next_blend_mode = slot.data().blend_mode();
            let next_attachment_renderer_object =
                slot.attachment().and_then(|a| a.as_mesh()).map_or_else(
                    || {
                        slot.attachment().and_then(|a| a.as_region()).map_or_else(
                            || {
                                unreachable!();
                            },
                            |region_attachment| {
                                let next_attachment_renderer_object = unsafe {
                                    region_attachment
                                        .renderer_object()
                                        .get_atlas_region()
                                        .unwrap()
                                        .page()
                                        .c_ptr_ref()
                                        .rendererObject
                                        .cast_const()
                                };
                                if next_attachment_renderer_object.is_null() {
                                    None
                                } else {
                                    Some(next_attachment_renderer_object)
                                }
                            },
                        )
                    },
                    |mesh_attachment| {
                        let next_attachment_renderer_object = unsafe {
                            mesh_attachment
                                .renderer_object()
                                .get_atlas_region()
                                .unwrap()
                                .page()
                                .c_ptr_ref()
                                .rendererObject
                                .cast_const()
                        };
                        if next_attachment_renderer_object.is_null() {
                            None
                        } else {
                            Some(next_attachment_renderer_object)
                        }
                    },
                );

            assert!(
                world_vertices.len() / 2 <= MAX_VERTICES,
                "Spine attachment exceeds u16 vertex indices"
            );
            if renderable.blend_mode != next_blend_mode
                || renderable.attachment_renderer_object != next_attachment_renderer_object
                || renderable.vertices.len() + world_vertices.len() / 2 > MAX_VERTICES
            {
                renderable_count += usize::from(!renderable.indices.is_empty());
                renderable = reuse_batch(renderables, renderable_count);
            }
            renderable.blend_mode = next_blend_mode;
            renderable.attachment_renderer_object = next_attachment_renderer_object;
            let CombinedRenderable {
                vertices,
                uvs,
                colors,
                dark_colors,
                indices,
                ..
            } = &mut *renderable;

            let mut vertex_base = vertices.len();
            let mut index_base = indices.len();
            vertices.extend_from_slice(world_vertices.as_chunks::<2>().0);

            let (color, dark_color) = if let Some(mesh_attachment) =
                slot.attachment().and_then(|a| a.as_mesh())
            {
                let mut color = mesh_attachment.color() * slot.color() * skeleton.color();
                let mut dark_color = slot.dark_color().unwrap_or_default();
                if self.premultiplied_alpha {
                    color.premultiply_alpha();
                    dark_color *= color.a;
                    dark_color.a = 1.0;
                } else {
                    dark_color.a = 0.;
                }
                color = match self.color_space {
                    ColorSpace::SRGB => color,
                    ColorSpace::Linear => color.nonlinear_to_linear(),
                };

                dark_color = match self.color_space {
                    ColorSpace::SRGB => dark_color,
                    ColorSpace::Linear => dark_color.nonlinear_to_linear(),
                };

                uvs.extend((0..world_vertices.len() / 2).map(|i| unsafe {
                    [
                        *mesh_attachment.uvs().add(i * 2),
                        *mesh_attachment.uvs().add(i * 2 + 1),
                    ]
                }));
                for i in (0..mesh_attachment.triangles_count() as usize).step_by(3) {
                    indices.extend(
                        index_order
                            .map(|offset| unsafe { *mesh_attachment.triangles().add(i + offset) }),
                    );
                }

                (color, dark_color)
            } else if let Some(region_attachment) = slot.attachment().and_then(|a| a.as_region()) {
                let mut color = region_attachment.color() * slot.color() * skeleton.color();
                let mut dark_color = slot.dark_color().unwrap_or_default();
                if self.premultiplied_alpha {
                    color.premultiply_alpha();
                    dark_color *= color.a;
                    dark_color.a = 1.0;
                } else {
                    dark_color.a = 0.;
                }
                color = match self.color_space {
                    ColorSpace::SRGB => color,
                    ColorSpace::Linear => color.nonlinear_to_linear(),
                };

                dark_color = match self.color_space {
                    ColorSpace::SRGB => dark_color,
                    ColorSpace::Linear => dark_color.nonlinear_to_linear(),
                };

                for i in 0..4 {
                    uvs.push([
                        region_attachment.uvs()[i as usize * 2],
                        region_attachment.uvs()[i as usize * 2 + 1],
                    ]);
                }

                for triangle in [[0, 1, 2], [2, 3, 0]] {
                    indices.extend(index_order.map(|i| triangle[i]));
                }

                (color, dark_color)
            } else {
                unreachable!();
            };

            colors.resize(vertices.len(), [color.r, color.g, color.b, color.a]);
            dark_colors.resize(
                vertices.len(),
                [dark_color.r, dark_color.g, dark_color.b, dark_color.a],
            );

            if let Some(clipper) = clipper.as_deref_mut() {
                if clipper.is_clipping() {
                    unsafe {
                        clipper.clip_triangles(
                            &mut vertices.as_mut_slice()[vertex_base..],
                            &mut indices.as_mut_slice()[index_base..],
                            &mut uvs.as_mut_slice()[vertex_base..],
                            2,
                        );
                        let clipped_triangles_size =
                            (*clipper.c_ptr_ref().clippedTriangles).size as usize;
                        let clipped_vertices_size =
                            (*clipper.c_ptr_ref().clippedVertices).size as usize;
                        let clipped_uvs_size = (*clipper.c_ptr_ref().clippedUVs).size as usize;
                        colors.resize(
                            vertex_base + (clipped_vertices_size / 2),
                            [color.r, color.g, color.b, color.a],
                        );
                        dark_colors.resize(
                            vertex_base + (clipped_vertices_size / 2),
                            [dark_color.r, dark_color.g, dark_color.b, dark_color.a],
                        );
                        indices.resize(index_base + clipped_triangles_size, 0);
                        std::ptr::copy_nonoverlapping(
                            (*clipper.c_ptr_ref().clippedTriangles).items,
                            indices.as_mut_ptr().add(index_base),
                            clipped_triangles_size,
                        );
                        vertices.resize(vertex_base + clipped_vertices_size / 2, [0., 0.]);
                        std::ptr::copy_nonoverlapping(
                            (*clipper.c_ptr_ref().clippedVertices).items,
                            vertices.as_mut_ptr().add(vertex_base).cast::<f32>(),
                            clipped_vertices_size,
                        );
                        uvs.resize(vertex_base + clipped_uvs_size / 2, [0., 0.]);
                        std::ptr::copy_nonoverlapping(
                            (*clipper.c_ptr_ref().clippedUVs).items,
                            uvs.as_mut_ptr().add(vertex_base).cast::<f32>(),
                            clipped_uvs_size,
                        );
                    }
                }
            }

            // Clipping can add vertices. Move just this attachment to a fresh batch
            // before rebasing its still-local indices; preserve the preceding geometry.
            if renderable.vertices.len() > MAX_VERTICES {
                assert!(
                    renderable.vertices.len() - vertex_base <= MAX_VERTICES,
                    "clipped Spine attachment exceeds u16 vertex indices"
                );
                renderable_count += 1;
                reuse_batch(renderables, renderable_count);
                let (previous, next) = renderables.split_at_mut(renderable_count);
                let previous = &mut previous[renderable_count - 1];
                renderable = &mut next[0];
                renderable.blend_mode = previous.blend_mode;
                renderable.attachment_renderer_object = previous.attachment_renderer_object;
                renderable
                    .vertices
                    .extend(previous.vertices.drain(vertex_base..));
                renderable.uvs.extend(previous.uvs.drain(vertex_base..));
                renderable
                    .colors
                    .extend(previous.colors.drain(vertex_base..));
                renderable
                    .dark_colors
                    .extend(previous.dark_colors.drain(vertex_base..));
                renderable
                    .indices
                    .extend(previous.indices.drain(index_base..));
                vertex_base = 0;
                index_base = 0;
            }
            for index in &mut renderable.indices[index_base..] {
                *index += vertex_base as u16;
            }

            if let Some(clipper) = clipper.as_deref_mut() {
                clipper.clip_end(&slot);
            }
        }

        renderable_count += usize::from(!renderable.indices.is_empty());

        if let Some(clipper) = clipper {
            clipper.clip_end2();
        }
        &renderables[..renderable_count]
    }
}

#[cfg(test)]
mod test {
    use crate::test::TestAsset;

    use super::*;

    /// Ensure all the example assets draw without error.
    #[test]
    fn combined_drawer() {
        for json in [true, false] {
            for example_asset in TestAsset::all() {
                let (mut skeleton, _) = example_asset.instance(json);
                let drawer = CombinedDrawer {
                    cull_direction: CullDirection::Clockwise,
                    premultiplied_alpha: false,
                    color_space: ColorSpace::Linear,
                };
                let mut clipper = SkeletonClipping::new();
                let renderables = drawer.draw(&mut skeleton, Some(&mut clipper));
                assert!(!renderables.is_empty());
            }
        }
    }

    #[test]
    fn reused_combined_draw_matches_fresh_output_and_retains_capacity() {
        for json in [true, false] {
            for example_asset in TestAsset::all() {
                let (skeleton_data, _) = example_asset.instance_data(json);
                let mut expected_skeleton = Skeleton::new(skeleton_data.clone());
                let mut actual_skeleton = Skeleton::new(skeleton_data);
                let drawer = CombinedDrawer {
                    cull_direction: CullDirection::Clockwise,
                    premultiplied_alpha: false,
                    color_space: ColorSpace::Linear,
                };
                let mut expected_clipper = SkeletonClipping::new();
                let mut actual_clipper = SkeletonClipping::new();
                let mut scratch = CombinedDrawerScratch::default();
                let slots = actual_skeleton
                    .slots()
                    .map(|slot| slot.data().name().to_owned())
                    .collect::<Vec<_>>();
                let mut capacities = None;
                for hidden in [false, true, false, false] {
                    for skeleton in [&mut expected_skeleton, &mut actual_skeleton] {
                        if hidden {
                            for slot in &slots {
                                assert!(skeleton.set_attachment(slot, None));
                            }
                        } else {
                            skeleton.set_slots_to_setup_pose();
                        }
                    }
                    let expected = drawer.draw(&mut expected_skeleton, Some(&mut expected_clipper));
                    let actual = drawer.draw_into(
                        &mut actual_skeleton,
                        Some(&mut actual_clipper),
                        &mut scratch,
                    );
                    assert_eq!(actual.is_empty(), hidden);
                    assert_eq!(actual.len(), expected.len());
                    for (expected, actual) in expected.iter().zip(actual) {
                        assert_eq!(expected.vertices, actual.vertices);
                        assert_eq!(expected.uvs, actual.uvs);
                        assert_eq!(expected.colors, actual.colors);
                        assert_eq!(expected.dark_colors, actual.dark_colors);
                        assert_eq!(expected.indices, actual.indices);
                        assert_eq!(expected.blend_mode, actual.blend_mode);
                        assert_eq!(
                            expected.attachment_renderer_object,
                            actual.attachment_renderer_object
                        );
                    }
                    let current_capacities = (
                        scratch.renderables.capacity(),
                        scratch.world_vertices.capacity(),
                        scratch
                            .renderables
                            .iter()
                            .map(|renderable| {
                                (
                                    renderable.vertices.capacity(),
                                    renderable.uvs.capacity(),
                                    renderable.colors.capacity(),
                                    renderable.dark_colors.capacity(),
                                    renderable.indices.capacity(),
                                )
                            })
                            .collect::<Vec<_>>(),
                    );
                    if let Some(previous) = &capacities {
                        assert_eq!(&current_capacities, previous);
                    }
                    capacities = Some(current_capacities);
                }
            }
        }
    }
}
