//! End-to-end CPU asset preparation from a layered 1.8.9 resource-pack stack.

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fmt;

use crate::mesh::{build_chunk_model_mesh, BlockModelRegistry, ModelMesh};
use crate::resource_pack::{ModelLoader, ResourceError, ResourcePackStack};
use crate::texture::{TextureAtlas, TextureAtlasBuilder, TextureError};
use crate::world::{BlockState, ChunkPos, World};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetPipelineError {
    Resource(String),
    Texture(String),
}

impl fmt::Display for AssetPipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resource(message) => write!(f, "resource-pack pipeline error: {message}"),
            Self::Texture(message) => write!(f, "texture pipeline error: {message}"),
        }
    }
}

impl StdError for AssetPipelineError {}

impl From<ResourceError> for AssetPipelineError {
    fn from(value: ResourceError) -> Self { Self::Resource(value.to_string()) }
}

impl From<TextureError> for AssetPipelineError {
    fn from(value: TextureError) -> Self { Self::Texture(value.to_string()) }
}

/// Connects one protocol-47 legacy state to its 1.8.9 blockstate JSON entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockAssetSpec {
    pub state: BlockState,
    pub block_location: String,
    pub properties: BTreeMap<String, String>,
    /// Stable per-block seed used for weighted blockstate variants.
    pub seed: u64,
}

impl BlockAssetSpec {
    pub fn new(
        state: BlockState,
        block_location: impl Into<String>,
        properties: BTreeMap<String, String>,
        seed: u64,
    ) -> Self {
        Self { state, block_location: block_location.into(), properties, seed }
    }
}

/// Ready-to-use CPU-side block models and their shared RGBA texture atlas.
#[derive(Clone, Debug)]
pub struct LoadedBlockAssets {
    pub registry: BlockModelRegistry,
    pub atlas: TextureAtlas,
    pub loaded_states: usize,
}

impl LoadedBlockAssets {
    /// Load all requested states, build one shared atlas, and preserve blockstate rotations.
    pub fn load<I>(pack_stack: ResourcePackStack, specs: I) -> Result<Self, AssetPipelineError>
    where
        I: IntoIterator<Item = BlockAssetSpec>,
    {
        let mut loader = ModelLoader::with_stack(pack_stack);
        let mut registry = BlockModelRegistry::default();
        let mut atlas_builder = TextureAtlasBuilder::default();
        let mut loaded_states = 0usize;

        for spec in specs {
            let (model, variant) = loader.load_weighted_variant(
                &spec.block_location,
                &spec.properties,
                spec.seed,
            )?;
            atlas_builder.add_model_textures_from_stack(loader.pack_stack(), &model)?;
            registry.insert_variant(spec.state, model, variant);
            loaded_states += 1;
        }

        Ok(Self {
            registry,
            atlas: atlas_builder.build()?,
            loaded_states,
        })
    }

    /// Mesh one loaded chunk and remap model UVs into the prepared atlas.
    pub fn build_chunk_mesh(
        &self,
        world: &World,
        chunk: ChunkPos,
    ) -> Result<ModelMesh, AssetPipelineError> {
        let mut mesh = build_chunk_model_mesh(world, chunk, &self.registry)?;
        self.atlas.remap_model_mesh_uvs(&mut mesh)?;
        Ok(mesh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::world::{BlockPos, ChunkPos};

    fn temp_root() -> PathBuf {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("rust-mc-assets-{stamp}"))
    }

    fn write_resource(root: &PathBuf, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn one_pixel_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[220, 40, 80, 255]).unwrap();
        }
        bytes
    }

    #[test]
    fn pack_loads_blockstate_model_texture_atlas_and_chunk_mesh_together() {
        let root = temp_root();
        write_resource(&root, "assets/example/blockstates/stone.json",
            br#"{"variants":{"":{"model":"block/stone"}}}"#);
        write_resource(&root, "assets/example/models/block/stone.json",
            br##"{"textures":{"all":"blocks/stone"},"elements":[{"from":[0,0,0],"to":[16,16,16],"faces":{"north":{"texture":"#all","uv":[0,0,16,16],"cullface":"north"},"south":{"texture":"#all","uv":[0,0,16,16],"cullface":"south"},"east":{"texture":"#all","uv":[0,0,16,16],"cullface":"east"},"west":{"texture":"#all","uv":[0,0,16,16],"cullface":"west"},"up":{"texture":"#all","uv":[0,0,16,16],"cullface":"up"},"down":{"texture":"#all","uv":[0,0,16,16],"cullface":"down"}}}]}"##);
        write_resource(&root, "assets/example/textures/blocks/stone.png", &one_pixel_png());

        let stack = ResourcePackStack::open([root.clone()]).unwrap();
        let properties = BTreeMap::new();
        let assets = LoadedBlockAssets::load(
            stack,
            [BlockAssetSpec::new(BlockState(16), "example:stone", properties, 1234)],
        ).unwrap();

        let mut world = World::new();
        world.set_block(BlockPos::new(2, 20, 3), BlockState(16));
        let mesh = assets.build_chunk_mesh(&world, ChunkPos::new(0, 0)).unwrap();

        assert_eq!(assets.loaded_states, 1);
        assert_eq!(assets.registry.len(), 1);
        assert_eq!(assets.atlas.regions.len(), 1);
        assert_eq!(mesh.quad_count, 6);
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
        assert!(mesh.vertices.iter().all(|vertex| vertex.uv.iter().all(|value| (0.0..=1.0).contains(value))));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn blockstate_y_rotation_is_applied_to_geometry_and_cull_direction() {
        let root = temp_root();
        write_resource(&root, "assets/example/blockstates/test.json",
            br#"{"variants":{"":{"model":"block/test","y":90}}}"#);
        write_resource(&root, "assets/example/models/block/test.json",
            br##"{"textures":{"all":"blocks/test"},"elements":[{"from":[0,0,0],"to":[16,16,16],"faces":{"north":{"texture":"#all","uv":[0,0,16,16],"cullface":"north"}}}]}"##);
        write_resource(&root, "assets/example/textures/blocks/test.png", &one_pixel_png());

        let assets = LoadedBlockAssets::load(
            ResourcePackStack::open([root.clone()]).unwrap(),
            [BlockAssetSpec::new(BlockState(32), "example:test", BTreeMap::new(), 0)],
        ).unwrap();
        let mut world = World::new();
        let mesh = assets.build_chunk_mesh(&world, ChunkPos::new(0, 0)).unwrap();
        assert_eq!(mesh.quad_count, 1);
        assert!(mesh.vertices.iter().all(|vertex| vertex.face == crate::resource_pack::FaceDirection::West));
        world.set_block(BlockPos::new(1, 20, 0), BlockState(16));
        // Rotated north cullface points west, so an east-adjacent cube must not cull it.
        let mesh = assets.build_chunk_mesh(&world, ChunkPos::new(0, 0)).unwrap();
        assert_eq!(mesh.quad_count, 1);
        fs::remove_dir_all(root).unwrap();
    }
}
