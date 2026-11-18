//! PNG texture decoding and padded shelf-packed texture atlases.

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fmt;
use std::io::Cursor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextureError {
    Png(String),
    Resource(String),
    UnsupportedColorType,
    InvalidDimensions,
    TextureTooLarge { name: String, width: u32, height: u32 },
    AtlasFull,
    DuplicateName(String),
    MissingTexture(String),
    SizeOverflow,
}

impl fmt::Display for TextureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Png(message) => write!(f, "PNG decode error: {message}"),
            Self::Resource(message) => write!(f, "resource pack texture error: {message}"),
            Self::UnsupportedColorType => write!(f, "unsupported decoded PNG color type"),
            Self::InvalidDimensions => write!(f, "texture has zero dimensions"),
            Self::TextureTooLarge { name, width, height } => {
                write!(f, "texture {name} ({width}x{height}) exceeds atlas dimensions")
            }
            Self::AtlasFull => write!(f, "texture atlas is full"),
            Self::DuplicateName(name) => write!(f, "duplicate texture name: {name}"),
            Self::MissingTexture(name) => write!(f, "texture not found in atlas: {name}"),
            Self::SizeOverflow => write!(f, "texture dimensions overflow addressable memory"),
        }
    }
}
impl StdError for TextureError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureImage {
    pub width: u32,
    pub height: u32,
    /// Pixels are always tightly packed RGBA8, row-major, top-to-bottom.
    pub rgba: Vec<u8>,
}

impl TextureImage {
    pub fn from_png(bytes: &[u8]) -> Result<Self, TextureError> {
        let mut decoder = png::Decoder::new(Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(|error| TextureError::Png(error.to_string()))?;
        let mut buffer = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buffer).map_err(|error| TextureError::Png(error.to_string()))?;
        if info.width == 0 || info.height == 0 {
            return Err(TextureError::InvalidDimensions);
        }
        let source = &buffer[..info.buffer_size()];
        let pixel_count = (info.width as usize)
            .checked_mul(info.height as usize)
            .ok_or(TextureError::SizeOverflow)?;
        let mut rgba = Vec::with_capacity(pixel_count.checked_mul(4).ok_or(TextureError::SizeOverflow)?);

        match info.color_type {
            png::ColorType::Rgba => rgba.extend_from_slice(source),
            png::ColorType::Rgb => {
                for pixel in source.chunks_exact(3) {
                    rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
                }
            }
            png::ColorType::Grayscale => {
                for value in source {
                    rgba.extend_from_slice(&[*value, *value, *value, 255]);
                }
            }
            png::ColorType::GrayscaleAlpha => {
                for pixel in source.chunks_exact(2) {
                    rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
                }
            }
            png::ColorType::Indexed => return Err(TextureError::UnsupportedColorType),
        }
        if rgba.len() != pixel_count * 4 {
            return Err(TextureError::Png("decoded pixel buffer has an unexpected length".into()));
        }
        Ok(Self { width: info.width, height: info.height, rgba })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl AtlasRect {
    pub fn uv_bounds(self, atlas_width: u32, atlas_height: u32) -> [f32; 4] {
        [
            self.x as f32 / atlas_width as f32,
            self.y as f32 / atlas_height as f32,
            (self.x + self.width) as f32 / atlas_width as f32,
            (self.y + self.height) as f32 / atlas_height as f32,
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureAtlas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub regions: BTreeMap<String, AtlasRect>,
}

impl TextureAtlas {
    pub fn rect(&self, name: &str) -> Result<AtlasRect, TextureError> {
        self.regions.get(name).copied().ok_or_else(|| TextureError::MissingTexture(name.to_owned()))
    }

    pub fn uv_bounds(&self, name: &str) -> Result<[f32; 4], TextureError> {
        Ok(self.rect(name)?.uv_bounds(self.width, self.height))
    }

    /// Remap model-local 0..1 UV coordinates into this atlas in place.
    pub fn remap_model_mesh_uvs(&self, mesh: &mut crate::mesh::ModelMesh) -> Result<(), TextureError> {
        for vertex in &mut mesh.vertices {
            let [u0, v0, u1, v1] = self.uv_bounds(&vertex.texture)?;
            vertex.uv = [
                u0 + vertex.uv[0] * (u1 - u0),
                v0 + vertex.uv[1] * (v1 - v0),
            ];
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct TextureAtlasBuilder {
    max_width: u32,
    max_height: u32,
    padding: u32,
    textures: BTreeMap<String, TextureImage>,
}

impl Default for TextureAtlasBuilder {
    fn default() -> Self {
        Self::new(2048, 2048, 1)
    }
}

impl TextureAtlasBuilder {
    pub fn new(max_width: u32, max_height: u32, padding: u32) -> Self {
        Self {
            max_width,
            max_height,
            padding,
            textures: BTreeMap::new(),
        }
    }

    pub fn add_png(&mut self, name: impl Into<String>, bytes: &[u8]) -> Result<(), TextureError> {
        self.add_image(name, TextureImage::from_png(bytes)?)
    }

    /// Load every texture used by a resolved model directly from an extracted resource pack.
    /// Already-added texture locations are skipped, so multiple models can share atlas entries.
    pub fn add_model_textures(
        &mut self,
        pack: &crate::resource_pack::ResourcePack,
        model: &crate::resource_pack::ResolvedBlockModel,
    ) -> Result<usize, TextureError> {
        let mut added = 0;
        for element in &model.elements {
            for face in element.faces.values() {
                let location = model.resolve_face_texture(face)
                    .map_err(|error| TextureError::Resource(error.to_string()))?;
                if self.textures.contains_key(&location) {
                    continue;
                }
                let bytes = pack.read_texture(&location, "minecraft")
                    .map_err(|error| TextureError::Resource(error.to_string()))?;
                self.add_png(location, &bytes)?;
                added += 1;
            }
        }
        Ok(added)
    }

    pub fn add_model_textures_from_stack(
        &mut self,
        pack: &crate::resource_pack::ResourcePackStack,
        model: &crate::resource_pack::ResolvedBlockModel,
    ) -> Result<usize, TextureError> {
        let mut added = 0;
        for element in &model.elements {
            for face in element.faces.values() {
                let location = model.resolve_face_texture(face)
                    .map_err(|error| TextureError::Resource(error.to_string()))?;
                if self.textures.contains_key(&location) {
                    continue;
                }
                let bytes = pack.read_texture(&location, "minecraft")
                    .map_err(|error| TextureError::Resource(error.to_string()))?;
                self.add_png(location, &bytes)?;
                added += 1;
            }
        }
        Ok(added)
    }



    pub fn add_image(&mut self, name: impl Into<String>, image: TextureImage) -> Result<(), TextureError> {
        let name = name.into();
        if name.is_empty() {
            return Err(TextureError::DuplicateName(name));
        }
        if image.width == 0 || image.height == 0 {
            return Err(TextureError::InvalidDimensions);
        }
        let expected = (image.width as usize)
            .checked_mul(image.height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(TextureError::SizeOverflow)?;
        if expected != image.rgba.len() {
            return Err(TextureError::InvalidDimensions);
        }
        if self.textures.contains_key(&name) {
            return Err(TextureError::DuplicateName(name));
        }
        self.textures.insert(name, image);
        Ok(())
    }

    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    pub fn build(self) -> Result<TextureAtlas, TextureError> {
        if self.max_width == 0 || self.max_height == 0 {
            return Err(TextureError::InvalidDimensions);
        }
        let mut entries: Vec<_> = self.textures.into_iter().collect();
        entries.sort_by(|a, b| {
            b.1.height.cmp(&a.1.height)
                .then_with(|| b.1.width.cmp(&a.1.width))
                .then_with(|| a.0.cmp(&b.0))
        });

        let mut placements = Vec::with_capacity(entries.len());
        let mut cursor_x = 0_u32;
        let mut cursor_y = 0_u32;
        let mut row_height = 0_u32;
        let mut used_width = 0_u32;

        for (name, image) in &entries {
            let padded_width = image.width.checked_add(self.padding.saturating_mul(2)).ok_or(TextureError::SizeOverflow)?;
            let padded_height = image.height.checked_add(self.padding.saturating_mul(2)).ok_or(TextureError::SizeOverflow)?;
            if padded_width > self.max_width || padded_height > self.max_height {
                return Err(TextureError::TextureTooLarge {
                    name: name.clone(),
                    width: image.width,
                    height: image.height,
                });
            }
            if cursor_x.checked_add(padded_width).ok_or(TextureError::SizeOverflow)? > self.max_width {
                cursor_x = 0;
                cursor_y = cursor_y.checked_add(row_height).ok_or(TextureError::SizeOverflow)?;
                row_height = 0;
            }
            if cursor_y.checked_add(padded_height).ok_or(TextureError::SizeOverflow)? > self.max_height {
                return Err(TextureError::AtlasFull);
            }
            let rect = AtlasRect {
                x: cursor_x + self.padding,
                y: cursor_y + self.padding,
                width: image.width,
                height: image.height,
            };
            placements.push((name.clone(), rect));
            cursor_x = cursor_x.checked_add(padded_width).ok_or(TextureError::SizeOverflow)?;
            row_height = row_height.max(padded_height);
            used_width = used_width.max(cursor_x);
        }
        let used_height = cursor_y.checked_add(row_height).ok_or(TextureError::SizeOverflow)?;
        let width = used_width.max(1).checked_next_power_of_two().ok_or(TextureError::SizeOverflow)?;
        let height = used_height.max(1).checked_next_power_of_two().ok_or(TextureError::SizeOverflow)?;
        if width > self.max_width || height > self.max_height {
            return Err(TextureError::AtlasFull);
        }
        let total_bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(TextureError::SizeOverflow)?;
        let mut rgba = vec![0_u8; total_bytes];
        let images: BTreeMap<_, _> = entries.into_iter().collect();
        let mut regions = BTreeMap::new();

        for (name, rect) in placements {
            let image = images.get(&name).expect("placement must reference a stored texture");
            copy_with_padding(image, rect, self.padding, width, &mut rgba)?;
            regions.insert(name, rect);
        }

        Ok(TextureAtlas { width, height, rgba, regions })
    }
}

fn copy_with_padding(
    image: &TextureImage,
    rect: AtlasRect,
    padding: u32,
    atlas_width: u32,
    output: &mut [u8],
) -> Result<(), TextureError> {
    let start_x = rect.x.checked_sub(padding).ok_or(TextureError::SizeOverflow)?;
    let start_y = rect.y.checked_sub(padding).ok_or(TextureError::SizeOverflow)?;
    let padded_width = image.width.checked_add(padding.saturating_mul(2)).ok_or(TextureError::SizeOverflow)?;
    let padded_height = image.height.checked_add(padding.saturating_mul(2)).ok_or(TextureError::SizeOverflow)?;

    for padded_y in 0..padded_height {
        let source_y = padded_y.saturating_sub(padding).min(image.height - 1);
        for padded_x in 0..padded_width {
            let source_x = padded_x.saturating_sub(padding).min(image.width - 1);
            let source_index = ((source_y * image.width + source_x) * 4) as usize;
            let target_x = start_x + padded_x;
            let target_y = start_y + padded_y;
            let target_index = ((target_y * atlas_width + target_x) * 4) as usize;
            output[target_index..target_index + 4].copy_from_slice(&image.rgba[source_index..source_index + 4]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn make_png(width: u32, height: u32, color: png::ColorType, data: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        {
            let mut encoder = png::Encoder::new(Cursor::new(&mut output), width, height);
            encoder.set_color(color);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(data).unwrap();
        }
        output
    }

    #[test]
    fn decodes_rgb_grayscale_and_rgba_pngs_to_rgba8() {
        let rgb = make_png(1, 1, png::ColorType::Rgb, &[10, 20, 30]);
        assert_eq!(TextureImage::from_png(&rgb).unwrap().rgba, [10, 20, 30, 255]);

        let gray = make_png(1, 1, png::ColorType::Grayscale, &[90]);
        assert_eq!(TextureImage::from_png(&gray).unwrap().rgba, [90, 90, 90, 255]);

        let rgba = make_png(1, 1, png::ColorType::Rgba, &[1, 2, 3, 4]);
        assert_eq!(TextureImage::from_png(&rgba).unwrap().rgba, [1, 2, 3, 4]);
    }

    #[test]
    fn packs_textures_with_padding_and_exposes_normalized_uvs() {
        let mut builder = TextureAtlasBuilder::new(16, 16, 1);
        builder.add_image("stone", TextureImage {
            width: 2, height: 2,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255],
        }).unwrap();
        builder.add_image("grass", TextureImage {
            width: 1, height: 3,
            rgba: vec![0, 10, 0, 255, 0, 20, 0, 255, 0, 30, 0, 255],
        }).unwrap();

        let atlas = builder.build().unwrap();
        assert_eq!(atlas.width, 8);
        assert_eq!(atlas.height, 8);
        assert_eq!(atlas.regions.len(), 2);
        let stone = atlas.rect("stone").unwrap();
        assert_eq!((stone.width, stone.height), (2, 2));
        let bounds = atlas.uv_bounds("stone").unwrap();
        assert!(bounds[0] < bounds[2] && bounds[1] < bounds[3]);

        let padding_index = ((stone.y * atlas.width + (stone.x - 1)) * 4) as usize;
        assert_eq!(&atlas.rgba[padding_index..padding_index + 4], &[255, 0, 0, 255]);
        assert!(matches!(atlas.rect("missing"), Err(TextureError::MissingTexture(_))));
    }

    #[test]
    fn loads_model_textures_from_resource_pack_and_deduplicates_them() {
        use crate::resource_pack::{
            FaceDirection, ModelElement, ModelFace, ResolvedBlockModel, ResourcePack,
        };
        use std::collections::BTreeMap;
        use std::fs;
        use std::path::PathBuf;
        use std::time::{SystemTime, UNIX_EPOCH};

        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root: PathBuf = std::env::temp_dir().join(format!("texture-pack-tests-{stamp}"));
        let texture_path = root.join("assets/minecraft/textures/blocks/test.png");
        fs::create_dir_all(texture_path.parent().unwrap()).unwrap();
        fs::write(&texture_path, make_png(1, 1, png::ColorType::Rgba, &[4, 5, 6, 255])).unwrap();

        let mut faces = BTreeMap::new();
        faces.insert(FaceDirection::North, ModelFace {
            texture: "#all".into(), uv: None, cullface: None, rotation: 0, tint_index: None,
        });
        let model = ResolvedBlockModel {
            location: "minecraft:block/test".into(),
            namespace: "minecraft".into(),
            ambient_occlusion: true,
            textures: BTreeMap::from([("all".into(), "blocks/test".into())]),
            elements: vec![ModelElement {
                from: [0.0; 3], to: [16.0; 3], rotation: None, shade: true, faces,
            }],
        };

        let pack = ResourcePack::open(&root).unwrap();
        let mut builder = TextureAtlasBuilder::default();
        assert_eq!(builder.add_model_textures(&pack, &model).unwrap(), 1);
        assert_eq!(builder.add_model_textures(&pack, &model).unwrap(), 0);
        assert_eq!(builder.texture_count(), 1);
        let atlas = builder.build().unwrap();
        assert!(atlas.rect("minecraft:blocks/test").is_ok());
        assert_eq!(atlas.rect("minecraft:blocks/test").unwrap().width, 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remaps_model_mesh_uvs_into_the_named_atlas_region() {
        let mut builder = TextureAtlasBuilder::new(16, 16, 1);
        builder.add_image("minecraft:blocks/stone", TextureImage {
            width: 2, height: 2, rgba: vec![255; 16]
        }).unwrap();
        builder.add_image("minecraft:blocks/dirt", TextureImage {
            width: 2, height: 2, rgba: vec![0; 16]
        }).unwrap();
        let atlas = builder.build().unwrap();
        let stone_bounds = atlas.uv_bounds("minecraft:blocks/stone").unwrap();
        let mut mesh = crate::mesh::ModelMesh {
            vertices: vec![
                crate::mesh::ModelMeshVertex {
                    position: [0.0, 0.0, 0.0],
                    uv: [0.0, 0.0],
                    texture: "minecraft:blocks/stone".into(),
                    face: crate::resource_pack::FaceDirection::North,
                    cullface: None,
                    tint_index: None,
                },
                crate::mesh::ModelMeshVertex {
                    position: [1.0, 1.0, 1.0],
                    uv: [1.0, 1.0],
                    texture: "minecraft:blocks/stone".into(),
                    face: crate::resource_pack::FaceDirection::North,
                    cullface: None,
                    tint_index: None,
                },
            ],
            indices: vec![0, 1],
            quad_count: 0,
        };
        atlas.remap_model_mesh_uvs(&mut mesh).unwrap();
        assert_eq!(mesh.vertices[0].uv, [stone_bounds[0], stone_bounds[1]]);
        assert_eq!(mesh.vertices[1].uv, [stone_bounds[2], stone_bounds[3]]);
    }

    #[test]
    fn rejects_duplicate_names_bad_dimensions_and_overflow() {
        let mut builder = TextureAtlasBuilder::new(8, 8, 1);
        let image = TextureImage { width: 1, height: 1, rgba: vec![1, 2, 3, 4] };
        builder.add_image("same", image.clone()).unwrap();
        assert!(matches!(builder.add_image("same", image), Err(TextureError::DuplicateName(_))));

        let mut too_large = TextureAtlasBuilder::new(2, 2, 1);
        too_large.add_image("wide", TextureImage {
            width: 4, height: 1, rgba: vec![0; 16]
        }).unwrap();
        assert!(matches!(too_large.build(), Err(TextureError::TextureTooLarge { .. })));

        assert!(matches!(TextureImage::from_png(b"not png"), Err(TextureError::Png(_))));
    }
}
