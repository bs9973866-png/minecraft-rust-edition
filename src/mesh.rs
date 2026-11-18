//! CPU chunk meshing for full-cube legacy blocks.
//!
//! Produces indexed quads with block state and sampled block/sky light.
//! Non-cube block shapes and texture-atlas UV selection are separate future stages.

use crate::resource_pack::{FaceDirection, ResolvedBlockModel, ResourceError};
use crate::world::{BlockPos, BlockState, ChunkPos, World};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshVertex {
    /// Position local to the chunk in block units (Y remains world height).
    pub position: [f32; 3],
    pub block_state: u16,
    /// Face order: down, up, north, south, west, east.
    pub face: u8,
    pub block_light: u8,
    pub sky_light: u8,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChunkMesh {
    pub chunk: ChunkPos,
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
    pub visible_blocks: usize,
    pub quad_count: usize,
}

impl ChunkMesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

const FACE_OFFSETS: [(i32, i32, i32); 6] = [
    (0, -1, 0),
    (0, 1, 0),
    (0, 0, -1),
    (0, 0, 1),
    (-1, 0, 0),
    (1, 0, 0),
];

/// Build visible cube faces for one loaded chunk.
///
/// A shared face is omitted when the adjacent block is also in the current
/// full-cube collision/model subset. This reduces the mesh to the outside shell.
pub fn build_chunk_mesh(world: &World, chunk_pos: ChunkPos) -> ChunkMesh {
    let mut mesh = ChunkMesh { chunk: chunk_pos, ..ChunkMesh::default() };
    let Some(chunk) = world.get_chunk(chunk_pos) else {
        return mesh;
    };

    for y in 0..256_i32 {
        for z in 0..16_u8 {
            for x in 0..16_u8 {
                let state = chunk.get_block(x, y, z);
                if !state.has_full_cube_collision() {
                    continue;
                }
                mesh.visible_blocks += 1;
                let world_x = chunk_pos.x * 16 + i32::from(x);
                let world_z = chunk_pos.z * 16 + i32::from(z);
                let block_position = BlockPos::new(world_x, y, world_z);
                let block_light = chunk.get_block_light(x, y, z);
                let sky_light = chunk.get_sky_light(x, y, z);

                for (face, (dx, dy, dz)) in FACE_OFFSETS.iter().copied().enumerate() {
                    let neighbor = BlockPos::new(world_x + dx, y + dy, world_z + dz);
                    if world.get_block(neighbor).has_full_cube_collision() {
                        continue;
                    }
                    push_face(
                        &mut mesh,
                        x,
                        y,
                        z,
                        state,
                        face as u8,
                        block_light,
                        sky_light,
                    );
                }
                let _ = block_position;
            }
        }
    }
    mesh
}

fn push_face(
    mesh: &mut ChunkMesh,
    x: u8,
    y: i32,
    z: u8,
    state: BlockState,
    face: u8,
    block_light: u8,
    sky_light: u8,
) {
    let x = f32::from(x);
    let y = y as f32;
    let z = f32::from(z);
    let corners = face_corners(x, y, z, face);
    let base = mesh.vertices.len() as u32;
    for position in corners {
        mesh.vertices.push(MeshVertex {
            position,
            block_state: state.0,
            face,
            block_light,
            sky_light,
        });
    }
    mesh.indices.extend_from_slice(&[
        base, base + 1, base + 2,
        base, base + 2, base + 3,
    ]);
    mesh.quad_count += 1;
}

fn face_corners(x: f32, y: f32, z: f32, face: u8) -> [[f32; 3]; 4] {
    match face {
        // Down (-Y)
        0 => [
            [x, y, z],
            [x + 1.0, y, z],
            [x + 1.0, y, z + 1.0],
            [x, y, z + 1.0],
        ],
        // Up (+Y)
        1 => [
            [x, y + 1.0, z],
            [x, y + 1.0, z + 1.0],
            [x + 1.0, y + 1.0, z + 1.0],
            [x + 1.0, y + 1.0, z],
        ],
        // North (-Z)
        2 => [
            [x, y, z],
            [x, y + 1.0, z],
            [x + 1.0, y + 1.0, z],
            [x + 1.0, y, z],
        ],
        // South (+Z)
        3 => [
            [x, y, z + 1.0],
            [x + 1.0, y, z + 1.0],
            [x + 1.0, y + 1.0, z + 1.0],
            [x, y + 1.0, z + 1.0],
        ],
        // West (-X)
        4 => [
            [x, y, z],
            [x, y, z + 1.0],
            [x, y + 1.0, z + 1.0],
            [x, y + 1.0, z],
        ],
        // East (+X)
        _ => [
            [x + 1.0, y, z],
            [x + 1.0, y + 1.0, z],
            [x + 1.0, y + 1.0, z + 1.0],
            [x + 1.0, y, z + 1.0],
        ],
    }
}

/// One CPU-side vertex generated from an element in a JSON block model.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelMeshVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub texture: String,
    pub face: FaceDirection,
    pub cullface: Option<FaceDirection>,
    pub tint_index: Option<i32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelMesh {
    pub vertices: Vec<ModelMeshVertex>,
    pub indices: Vec<u32>,
    pub quad_count: usize,
}

/// Build CPU geometry for all elements in a resolved 1.8.9 block model.
/// Model coordinates are expressed in sixteenths of a block; block_origin is world space.
pub fn build_model_mesh(
    model: &ResolvedBlockModel,
    block_origin: [f32; 3],
) -> Result<ModelMesh, ResourceError> {
    let mut mesh = ModelMesh::default();
    for element in &model.elements {
        let from = [element.from[0] / 16.0, element.from[1] / 16.0, element.from[2] / 16.0];
        let to = [element.to[0] / 16.0, element.to[1] / 16.0, element.to[2] / 16.0];
        for (face_direction, face) in &element.faces {
            let texture = model.resolve_face_texture(face)?;
            let mut positions = model_face_corners(from, to, *face_direction);
            let uv = face.uv.unwrap_or([0.0, 0.0, 16.0, 16.0]);
            let mut uvs = [
                [uv[0] / 16.0, uv[1] / 16.0],
                [uv[2] / 16.0, uv[1] / 16.0],
                [uv[2] / 16.0, uv[3] / 16.0],
                [uv[0] / 16.0, uv[3] / 16.0],
            ];
            uvs.rotate_left((face.rotation / 90) as usize);
            if let Some(rotation) = &element.rotation {
                for position in &mut positions {
                    *position = rotate_model_point(*position, rotation);
                }
            }
            let base = mesh.vertices.len() as u32;
            for index in 0..4 {
                mesh.vertices.push(ModelMeshVertex {
                    position: [
                        positions[index][0] + block_origin[0],
                        positions[index][1] + block_origin[1],
                        positions[index][2] + block_origin[2],
                    ],
                    uv: uvs[index],
                    texture: texture.clone(),
                    face: *face_direction,
                    cullface: face.cullface,
                    tint_index: face.tint_index,
                });
            }
            mesh.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            mesh.quad_count += 1;
        }
    }
    Ok(mesh)
}

fn model_face_corners(from: [f32; 3], to: [f32; 3], face: FaceDirection) -> [[f32; 3]; 4] {
    let [x0, y0, z0] = from;
    let [x1, y1, z1] = to;
    match face {
        FaceDirection::Down => [[x0,y0,z0],[x1,y0,z0],[x1,y0,z1],[x0,y0,z1]],
        FaceDirection::Up => [[x0,y1,z0],[x0,y1,z1],[x1,y1,z1],[x1,y1,z0]],
        FaceDirection::North => [[x0,y0,z0],[x0,y1,z0],[x1,y1,z0],[x1,y0,z0]],
        FaceDirection::South => [[x0,y0,z1],[x1,y0,z1],[x1,y1,z1],[x0,y1,z1]],
        FaceDirection::West => [[x0,y0,z0],[x0,y0,z1],[x0,y1,z1],[x0,y1,z0]],
        FaceDirection::East => [[x1,y0,z0],[x1,y1,z0],[x1,y1,z1],[x1,y0,z1]],
    }
}

fn rotate_model_point(point: [f32; 3], rotation: &crate::resource_pack::ElementRotation) -> [f32; 3] {
    let origin = [rotation.origin[0] / 16.0, rotation.origin[1] / 16.0, rotation.origin[2] / 16.0];
    let mut relative = [point[0] - origin[0], point[1] - origin[1], point[2] - origin[2]];
    let radians = rotation.angle.to_radians();
    let (sin, cos) = radians.sin_cos();
    let [x, y, z] = relative;
    relative = match rotation.axis.as_str() {
        "x" => [x, y * cos - z * sin, y * sin + z * cos],
        "y" => [x * cos + z * sin, y, -x * sin + z * cos],
        "z" => [x * cos - y * sin, x * sin + y * cos, z],
        _ => relative,
    };
    if rotation.rescale {
        let factor = if cos.abs() < 1.0e-4 { 1.0 } else { 1.0 / cos.abs() };
        match rotation.axis.as_str() {
            "x" => { relative[1] *= factor; relative[2] *= factor; }
            "y" => { relative[0] *= factor; relative[2] *= factor; }
            "z" => { relative[0] *= factor; relative[1] *= factor; }
            _ => {}
        }
    }
    [relative[0] + origin[0], relative[1] + origin[1], relative[2] + origin[2]]
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_model_elements_generate_indexed_quads_with_texture_uv_and_cullface() {
        use crate::resource_pack::{ModelElement, ModelFace};
        let mut faces = std::collections::BTreeMap::new();
        for face in [FaceDirection::Down, FaceDirection::Up, FaceDirection::North, FaceDirection::South, FaceDirection::West, FaceDirection::East] {
            faces.insert(face, ModelFace {
                texture: "#all".into(),
                uv: Some([0.0, 0.0, 16.0, 16.0]),
                cullface: Some(face),
                rotation: if face == FaceDirection::North { 90 } else { 0 },
                tint_index: None,
            });
        }
        let model = ResolvedBlockModel {
            location: "minecraft:block/cube".into(),
            namespace: "minecraft".into(),
            ambient_occlusion: true,
            textures: std::collections::BTreeMap::from([("all".into(), "blocks/stone".into())]),
            elements: vec![ModelElement {
                from: [0.0, 0.0, 0.0],
                to: [16.0, 16.0, 16.0],
                rotation: None,
                shade: true,
                faces,
            }],
        };
        let mesh = build_model_mesh(&model, [4.0, 10.0, -2.0]).unwrap();
        assert_eq!(mesh.quad_count, 6);
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
        assert!(mesh.vertices.iter().all(|vertex| vertex.texture == "minecraft:blocks/stone"));
        assert!(mesh.vertices.iter().all(|vertex| vertex.position[0] >= 4.0 && vertex.position[0] <= 5.0));
        assert!(mesh.vertices.iter().all(|vertex| vertex.position[1] >= 10.0 && vertex.position[1] <= 11.0));
        assert!(mesh.vertices.iter().any(|vertex| vertex.cullface == Some(FaceDirection::North)));
    }

    #[test]
    fn element_rotation_moves_geometry_around_its_origin() {
        use crate::resource_pack::{ElementRotation, ModelElement, ModelFace};
        let mut faces = std::collections::BTreeMap::new();
        faces.insert(FaceDirection::North, ModelFace {
            texture: "blocks/stone".into(), uv: None, cullface: None, rotation: 0, tint_index: Some(1)
        });
        let model = ResolvedBlockModel {
            location: "minecraft:block/rotated".into(),
            namespace: "minecraft".into(),
            ambient_occlusion: true,
            textures: std::collections::BTreeMap::new(),
            elements: vec![ModelElement {
                from: [0.0, 0.0, 0.0],
                to: [16.0, 16.0, 16.0],
                rotation: Some(ElementRotation { origin: [8.0, 8.0, 8.0], axis: "y".into(), angle: 90.0, rescale: false }),
                shade: true,
                faces,
            }],
        };
        let mesh = build_model_mesh(&model, [0.0, 0.0, 0.0]).unwrap();
        assert_eq!(mesh.quad_count, 1);
        let first = mesh.vertices[0].position;
        assert!((first[0] - 0.0).abs() < 1.0e-5);
        assert!((first[2] - 1.0).abs() < 1.0e-5);
        assert_eq!(mesh.vertices[0].tint_index, Some(1));
    }
    #[test]
    fn one_block_builds_six_quads() {
        let mut world = World::new();
        world.set_block(BlockPos::new(2, 64, 3), BlockState(16));
        let mesh = build_chunk_mesh(&world, ChunkPos::new(0, 0));

        assert_eq!(mesh.visible_blocks, 1);
        assert_eq!(mesh.quad_count, 6);
        assert_eq!(mesh.vertex_count(), 24);
        assert_eq!(mesh.indices.len(), 36);
        assert_eq!(mesh.triangle_count(), 12);
        assert_eq!(mesh.vertices[0].block_state, 16);
        assert!(mesh.vertices.iter().all(|vertex| vertex.position[1] >= 64.0 && vertex.position[1] <= 65.0));
    }

    #[test]
    fn adjacent_blocks_do_not_emit_the_shared_face() {
        let mut world = World::new();
        world.set_block(BlockPos::new(0, 64, 0), BlockState(16));
        world.set_block(BlockPos::new(1, 64, 0), BlockState(16));
        let mesh = build_chunk_mesh(&world, ChunkPos::new(0, 0));

        assert_eq!(mesh.visible_blocks, 2);
        assert_eq!(mesh.quad_count, 10);
        assert_eq!(mesh.vertex_count(), 40);
        assert_eq!(mesh.indices.len(), 60);
    }

    #[test]
    fn meshing_crosses_chunk_edges_using_world_neighbors() {
        let mut world = World::new();
        world.set_block(BlockPos::new(15, 70, 0), BlockState(16));
        world.set_block(BlockPos::new(16, 70, 0), BlockState(16));

        let left = build_chunk_mesh(&world, ChunkPos::new(0, 0));
        let right = build_chunk_mesh(&world, ChunkPos::new(1, 0));
        assert_eq!(left.quad_count, 5);
        assert_eq!(right.quad_count, 5);
        assert!(left.vertices.iter().all(|vertex| vertex.position[0] >= 15.0 && vertex.position[0] <= 16.0));
        assert!(right.vertices.iter().all(|vertex| vertex.position[0] >= 0.0 && vertex.position[0] <= 1.0));
    }

    #[test]
    fn non_full_blocks_are_left_for_shape_specific_meshers() {
        let mut world = World::new();
        world.set_block(BlockPos::new(1, 64, 1), BlockState(6 << 4)); // sapling
        let mesh = build_chunk_mesh(&world, ChunkPos::new(0, 0));
        assert!(mesh.is_empty());
        assert_eq!(mesh.visible_blocks, 0);
    }

    #[test]
    fn missing_chunk_returns_empty_mesh() {
        let mesh = build_chunk_mesh(&World::new(), ChunkPos::new(-7, 12));
        assert!(mesh.is_empty());
        assert_eq!(mesh.chunk, ChunkPos::new(-7, 12));
    }
}
