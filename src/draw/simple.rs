use crate::{c::c_void, BlendMode, Color, Skeleton, SkeletonClipping};

use super::{ColorSpace, CullDirection};

#[allow(unused_imports)]
use crate::extension;

/// Renderable mesh data produced by [`SimpleDrawer`].
#[derive(Clone)]
pub struct SimpleRenderable {
    /// The index of the slot in [`Skeleton`] that this renderable represents.
    pub slot_index: usize,
    /// A list of vertex attributes for a mesh.
    pub vertices: Vec<[f32; 2]>,
    /// A list of UV attributes for a mesh.
    pub uvs: Vec<[f32; 2]>,
    /// A list of indices for a mesh.
    pub indices: Vec<u16>,
    /// The color tint of the mesh.
    pub color: Color,
    /// The dark color tint of the mesh.
    /// See the [Spine User Guide](http://en.esotericsoftware.com/spine-slots#Tint-black).
    pub dark_color: Color,
    /// The blend mode to use when drawing this mesh.
    pub blend_mode: BlendMode,
    /// The attachment's renderer object as a raw pointer. Usually represents the texture created
    /// from [`extension::set_create_texture_cb`].
    pub attachment_renderer_object: Option<*const c_void>,
}

impl Default for SimpleRenderable {
    fn default() -> Self {
        Self {
            slot_index: 0,
            vertices: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            color: Color::default(),
            dark_color: Color::default(),
            blend_mode: BlendMode::Normal,
            attachment_renderer_object: None,
        }
    }
}

/// Reusable working memory for [`SimpleDrawer::draw_into`].
#[derive(Default)]
pub struct SimpleDrawerScratch {
    world_vertices: Vec<f32>,
    renderables: Vec<SimpleRenderable>,
}

/// A simple drawer with no optimizations.
///
/// Assumes use of the default atlas attachment loader.
///
/// See [`SimpleDrawer::draw`]
pub struct SimpleDrawer {
    /// The cull direction to use for the vertices.
    pub cull_direction: CullDirection,
    /// Set to `true` if the textures are expected to have premultiplied alpha.
    pub premultiplied_alpha: bool,
    /// The color space to use for the colors returned in [`SimpleRenderable`].
    pub color_space: ColorSpace,
}

impl SimpleDrawer {
    /// This function returns a list of [`SimpleRenderable`] structs containing all the necessary
    /// data to create and render meshes. One renderable is created for each visible attachment on
    /// the skeleton. If a [`SkeletonClipping`] is provided, meshes will be properly clipped. The
    /// renderables are expected to be rendered in the order provided with the first renderable
    /// being drawn behind all the others.
    ///
    /// # Panics
    ///
    /// Panics if not using the default attachment loader with valid atlas regions.
    pub fn draw(
        &self,
        skeleton: &mut Skeleton,
        clipper: Option<&mut SkeletonClipping>,
    ) -> Vec<SimpleRenderable> {
        let mut scratch = SimpleDrawerScratch::default();
        let count = self.draw_into(skeleton, clipper, &mut scratch).len();
        scratch.renderables.truncate(count);
        scratch.renderables
    }

    /// Draws into reusable storage and returns only the current renderables.
    ///
    /// Spare attribute buffers stay in `scratch` for subsequent draws. Renderer object pointers
    /// in the returned slice remain valid only while their owning atlas is alive.
    ///
    /// # Panics
    ///
    /// Panics if not using the default attachment loader with valid atlas regions.
    pub fn draw_into<'a>(
        &self,
        skeleton: &mut Skeleton,
        mut clipper: Option<&mut SkeletonClipping>,
        scratch: &'a mut SimpleDrawerScratch,
    ) -> &'a [SimpleRenderable] {
        let SimpleDrawerScratch {
            world_vertices,
            renderables,
        } = scratch;
        let index_order = self.cull_direction.index_order();
        let mut renderable_count = 0;
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

            if renderables.len() <= renderable_count {
                renderables.push(SimpleRenderable::default());
            }
            let renderable = &mut renderables[renderable_count];
            let vertices = &mut renderable.vertices;
            let indices = &mut renderable.indices;
            let uvs = &mut renderable.uvs;
            vertices.clear();
            indices.clear();
            uvs.clear();

            let mut color;

            if let Some(mut mesh_attachment) = slot.attachment().and_then(|a| a.as_mesh()) {
                mesh_attachment.update_region();
                color = mesh_attachment.color();

                // Spine counts XY coordinates, not vertices. Size the native output before writing.
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

                let vertex_count = world_vertices.len() / 2;
                vertices.extend_from_slice(world_vertices.as_chunks::<2>().0);
                uvs.extend((0..vertex_count).map(|i| unsafe {
                    [
                        *mesh_attachment.uvs().add(i * 2),
                        *mesh_attachment.uvs().add(i * 2 + 1),
                    ]
                }));

                indices.reserve(mesh_attachment.triangles_count() as usize);
                for i in (0..mesh_attachment.triangles_count() as usize).step_by(3) {
                    indices.extend(
                        index_order
                            .map(|offset| unsafe { *mesh_attachment.triangles().add(i + offset) }),
                    );
                }
            } else if let Some(region_attachment) = slot.attachment().and_then(|a| a.as_region()) {
                color = region_attachment.color();

                world_vertices.resize(8, 0.);
                unsafe {
                    region_attachment.compute_world_vertices(&slot, &mut *world_vertices, 0, 2);
                }

                vertices.reserve(4);
                uvs.reserve(4);
                for i in 0..4 {
                    vertices.push([
                        world_vertices[i as usize * 2],
                        world_vertices[i as usize * 2 + 1],
                    ]);

                    uvs.push([
                        region_attachment.uvs()[i as usize * 2],
                        region_attachment.uvs()[i as usize * 2 + 1],
                    ]);
                }

                indices.reserve(6);
                for triangle in [[0, 1, 2], [2, 3, 0]] {
                    indices.extend(index_order.map(|i| triangle[i]));
                }
            } else if let Some(clipping_attachment) =
                slot.attachment().and_then(|a| a.as_clipping())
            {
                if let Some(clipper) = clipper.as_deref_mut() {
                    clipper.clip_start(&slot, &clipping_attachment);
                }
                continue;
            } else {
                if let Some(clipper) = clipper.as_deref_mut() {
                    clipper.clip_end(&slot);
                }
                continue;
            }

            if let Some(clipper) = clipper.as_deref_mut() {
                if clipper.is_clipping() {
                    unsafe {
                        clipper.clip_triangles(
                            vertices.as_mut_slice(),
                            indices.as_mut_slice(),
                            uvs.as_mut_slice(),
                            2,
                        );
                        let clipped_vertices_size =
                            (*clipper.c_ptr_ref().clippedVertices).size as usize;
                        vertices.resize(clipped_vertices_size / 2, [0., 0.]);
                        std::ptr::copy_nonoverlapping(
                            (*clipper.c_ptr_ref().clippedVertices).items,
                            vertices.as_mut_ptr().cast::<f32>(),
                            clipped_vertices_size,
                        );
                        let clipped_triangles_size =
                            (*clipper.c_ptr_ref().clippedTriangles).size as usize;
                        indices.resize(clipped_triangles_size, 0);
                        std::ptr::copy_nonoverlapping(
                            (*clipper.c_ptr_ref().clippedTriangles).items,
                            indices.as_mut_ptr(),
                            clipped_triangles_size,
                        );
                        let clipped_uvs_size = (*clipper.c_ptr_ref().clippedUVs).size as usize;
                        uvs.resize(clipped_uvs_size / 2, [0., 0.]);
                        std::ptr::copy_nonoverlapping(
                            (*clipper.c_ptr_ref().clippedUVs).items,
                            uvs.as_mut_ptr().cast::<f32>(),
                            clipped_uvs_size,
                        );
                    }
                }
            }

            let attachment_renderer_object =
                slot.attachment().and_then(|a| a.as_mesh()).map_or_else(
                    || {
                        slot.attachment().and_then(|a| a.as_region()).and_then(
                            |region_attachment| unsafe {
                                let attachment_renderer_object = region_attachment
                                    .renderer_object()
                                    .get_atlas_region()
                                    .unwrap()
                                    .page()
                                    .c_ptr_ref()
                                    .rendererObject
                                    .cast_const();
                                if attachment_renderer_object.is_null() {
                                    None
                                } else {
                                    Some(attachment_renderer_object)
                                }
                            },
                        )
                    },
                    |mesh_attachment| unsafe {
                        let attachment_renderer_object = mesh_attachment
                            .renderer_object()
                            .get_atlas_region()
                            .unwrap()
                            .page()
                            .c_ptr_ref()
                            .rendererObject
                            .cast_const();
                        if attachment_renderer_object.is_null() {
                            None
                        } else {
                            Some(attachment_renderer_object)
                        }
                    },
                );

            color *= slot.color() * skeleton.color();
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

            renderable.slot_index = slot_index;
            renderable.color = color;
            renderable.dark_color = dark_color;
            renderable.blend_mode = slot.data().blend_mode();
            renderable.attachment_renderer_object = attachment_renderer_object;
            renderable_count += 1;
            if let Some(clipper) = clipper.as_deref_mut() {
                clipper.clip_end(&slot);
            }
        }

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
    fn simple_drawer() {
        for json in [true, false] {
            for example_asset in TestAsset::all() {
                let (mut skeleton, _) = example_asset.instance(json);
                let drawer = SimpleDrawer {
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
    fn reused_simple_draw_matches_fresh_output_and_retains_capacity() {
        for json in [true, false] {
            for example_asset in TestAsset::all() {
                let (skeleton_data, _) = example_asset.instance_data(json);
                let mut expected_skeleton = Skeleton::new(skeleton_data.clone());
                let mut actual_skeleton = Skeleton::new(skeleton_data);
                let drawer = SimpleDrawer {
                    cull_direction: CullDirection::Clockwise,
                    premultiplied_alpha: false,
                    color_space: ColorSpace::Linear,
                };
                let mut expected_clipper = SkeletonClipping::new();
                let mut actual_clipper = SkeletonClipping::new();
                let mut scratch = SimpleDrawerScratch::default();
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
                        assert_eq!(expected.slot_index, actual.slot_index);
                        assert_eq!(expected.vertices, actual.vertices);
                        assert_eq!(expected.uvs, actual.uvs);
                        assert_eq!(expected.indices, actual.indices);
                        assert_eq!(expected.color, actual.color);
                        assert_eq!(expected.dark_color, actual.dark_color);
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
