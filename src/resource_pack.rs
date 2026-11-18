//! Minecraft Java 1.8.9 blockstate and block-model JSON loader.
//!
//! Reads an extracted resource-pack folder. The loader does not bundle game assets.
//! It supports the 1.8.9 variants format, model parent inheritance, model elements,
//! face data, and chained texture variables such as #all.

use std::collections::{BTreeMap, HashMap};
use std::error::Error as StdError;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const MAX_PARENT_DEPTH: usize = 64;
const MAX_TEXTURE_DEPTH: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceError {
    Io { path: String, message: String },
    Json { location: String, message: String },
    InvalidLocation(String),
    InvalidModel { location: String, reason: String },
    ParentCycle(Vec<String>),
    ParentDepthExceeded,
    TextureVariableMissing(String),
    TextureReferenceCycle(String),
    VariantMissing(String),
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "resource I/O error at {path}: {message}"),
            Self::Json { location, message } => write!(f, "invalid JSON in {location}: {message}"),
            Self::InvalidLocation(value) => write!(f, "invalid resource location: {value}"),
            Self::InvalidModel { location, reason } => write!(f, "invalid model {location}: {reason}"),
            Self::ParentCycle(chain) => write!(f, "model parent cycle: {}", chain.join(" -> ")),
            Self::ParentDepthExceeded => write!(f, "model parent chain exceeds {MAX_PARENT_DEPTH}"),
            Self::TextureVariableMissing(name) => write!(f, "missing model texture variable #{name}"),
            Self::TextureReferenceCycle(name) => write!(f, "texture variable cycle at #{name}"),
            Self::VariantMissing(key) => write!(f, "no blockstate variant matches {key}"),
        }
    }
}
impl StdError for ResourceError {}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ResourceLocation {
    pub namespace: String,
    pub path: String,
}

impl ResourceLocation {
    pub fn parse(value: &str, default_namespace: &str) -> Result<Self, ResourceError> {
        let (namespace, path) = match value.split_once(':') {
            Some((namespace, path)) => (namespace, path),
            None => (default_namespace, value),
        };
        let valid_namespace = !namespace.is_empty()
            && namespace.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-' || b == b'.');
        let valid_path = !path.is_empty()
            && !path.starts_with('/')
            && !path.contains('\\')
            && path.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-' || b == b'.' || b == b'/')
            && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..");
        if !valid_namespace || !valid_path {
            return Err(ResourceError::InvalidLocation(value.to_owned()));
        }
        Ok(Self { namespace: namespace.to_owned(), path: path.to_owned() })
    }

    pub fn canonical(&self) -> String {
        format!("{}:{}", self.namespace, self.path)
    }
}

#[derive(Clone, Debug)]
pub struct ResourcePack {
    root: PathBuf,
}

impl ResourcePack {
    /// Open an extracted pack directory containing an assets folder.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ResourceError> {
        let root = root.as_ref().to_path_buf();
        let assets = root.join("assets");
        if !assets.is_dir() {
            return Err(ResourceError::Io {
                path: assets.display().to_string(),
                message: "expected an extracted pack with an assets directory".into(),
            });
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn object_path(&self, kind: &str, extension: &str, location: &ResourceLocation) -> PathBuf {
        self.root.join("assets")
            .join(&location.namespace)
            .join(kind)
            .join(format!("{}.{}", location.path, extension))
    }

    fn read_text(&self, path: &Path, location: &str) -> Result<String, ResourceError> {
        fs::read_to_string(path).map_err(|error| ResourceError::Io {
            path: format!("{location} ({})", path.display()),
            message: error.to_string(),
        })
    }

    pub fn load_blockstate(&self, block: &str) -> Result<BlockStateFile, ResourceError> {
        let location = ResourceLocation::parse(block, "minecraft")?;
        let path = self.object_path("blockstates", "json", &location);
        let json = self.read_text(&path, &location.canonical())?;
        parse_blockstate_json(&json, &location.canonical())
    }

    /// Return the PNG file bytes for a texture location. Image decoding is a renderer task.
    pub fn read_texture(&self, texture: &str, default_namespace: &str) -> Result<Vec<u8>, ResourceError> {
        let location = ResourceLocation::parse(texture, default_namespace)?;
        let path = self.object_path("textures", "png", &location);
        fs::read(&path).map_err(|error| ResourceError::Io {
            path: format!("{} ({})", location.canonical(), path.display()),
            message: error.to_string(),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelVariant {
    pub model: String,
    pub x: i32,
    pub y: i32,
    pub uvlock: bool,
    pub weight: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockStateFile {
    pub variants: BTreeMap<String, Vec<ModelVariant>>,
}

impl BlockStateFile {
    pub fn variants_for(&self, properties: &BTreeMap<String, String>) -> Option<&[ModelVariant]> {
        for (key, variants) in &self.variants {
            if parse_variant_key(key).as_ref() == Some(properties) {
                return Some(variants.as_slice());
            }
        }
        self.variants.get("").map(Vec::as_slice)
    }

    pub fn variants_for_key(&self, key: &str) -> Option<&[ModelVariant]> {
        self.variants.get(key).map(Vec::as_slice)
    }
}

#[derive(Deserialize)]
struct RawBlockState {
    variants: BTreeMap<String, RawVariantSet>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawVariantSet {
    One(RawVariant),
    Many(Vec<RawVariant>),
}

#[derive(Deserialize)]
struct RawVariant {
    model: String,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    uvlock: bool,
    #[serde(default = "default_weight")]
    weight: u32,
}

fn default_weight() -> u32 {
    1
}

fn parse_variant_key(key: &str) -> Option<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    if key.is_empty() {
        return Some(result);
    }
    for pair in key.split(',') {
        let (name, value) = pair.split_once('=')?;
        if name.is_empty() || value.is_empty() || result.insert(name.to_owned(), value.to_owned()).is_some() {
            return None;
        }
    }
    Some(result)
}

pub fn parse_blockstate_json(source: &str, location: &str) -> Result<BlockStateFile, ResourceError> {
    let raw: RawBlockState = serde_json::from_str(source).map_err(|error| ResourceError::Json {
        location: location.to_owned(),
        message: error.to_string(),
    })?;
    let mut variants = BTreeMap::new();
    for (key, set) in raw.variants {
        let entries = match set {
            RawVariantSet::One(one) => vec![one],
            RawVariantSet::Many(many) => many,
        };
        if entries.is_empty() {
            return Err(ResourceError::InvalidModel {
                location: location.to_owned(),
                reason: format!("variant {key:?} has no models"),
            });
        }
        let mut parsed = Vec::with_capacity(entries.len());
        for entry in entries {
            if entry.weight == 0 || ![0, 90, 180, 270].contains(&entry.x) || ![0, 90, 180, 270].contains(&entry.y) {
                return Err(ResourceError::InvalidModel {
                    location: location.to_owned(),
                    reason: format!("variant {key:?} has an invalid weight or rotation"),
                });
            }
            ResourceLocation::parse(&entry.model, "minecraft")?;
            parsed.push(ModelVariant {
                model: entry.model,
                x: entry.x,
                y: entry.y,
                uvlock: entry.uvlock,
                weight: entry.weight,
            });
        }
        variants.insert(key, parsed);
    }
    Ok(BlockStateFile { variants })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum FaceDirection {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl FaceDirection {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "down" => Some(Self::Down),
            "up" => Some(Self::Up),
            "north" => Some(Self::North),
            "south" => Some(Self::South),
            "west" => Some(Self::West),
            "east" => Some(Self::East),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelFace {
    pub texture: String,
    pub uv: Option<[f32; 4]>,
    pub cullface: Option<FaceDirection>,
    pub rotation: u16,
    pub tint_index: Option<i32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ElementRotation {
    pub origin: [f32; 3],
    pub axis: String,
    pub angle: f32,
    pub rescale: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelElement {
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub rotation: Option<ElementRotation>,
    pub shade: bool,
    pub faces: BTreeMap<FaceDirection, ModelFace>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedBlockModel {
    pub location: String,
    pub namespace: String,
    pub ambient_occlusion: bool,
    pub textures: BTreeMap<String, String>,
    pub elements: Vec<ModelElement>,
}

impl ResolvedBlockModel {
    /// Resolve a texture name or variable, following chained #references.
    pub fn resolve_texture(&self, variable: &str) -> Result<String, ResourceError> {
        let mut current = if variable.starts_with('#') { variable.to_owned() } else { format!("#{variable}") };
        let mut visited = Vec::new();
        for _ in 0..MAX_TEXTURE_DEPTH {
            if !current.starts_with('#') {
                return Ok(ResourceLocation::parse(&current, &self.namespace)?.canonical());
            }
            let key = current[1..].to_owned();
            if visited.iter().any(|previous| previous == &key) {
                return Err(ResourceError::TextureReferenceCycle(key));
            }
            visited.push(key.clone());
            current = self.textures.get(&key).cloned()
                .ok_or_else(|| ResourceError::TextureVariableMissing(key))?;
        }
        Err(ResourceError::TextureReferenceCycle(variable.to_owned()))
    }

    pub fn resolve_face_texture(&self, face: &ModelFace) -> Result<String, ResourceError> {
        self.resolve_texture(&face.texture)
    }
}

#[derive(Clone, Debug)]
pub struct ModelLoader {
    pack: ResourcePack,
    cache: HashMap<String, ResolvedBlockModel>,
}

impl ModelLoader {
    pub fn new(pack: ResourcePack) -> Self {
        Self { pack, cache: HashMap::new() }
    }

    pub fn pack(&self) -> &ResourcePack {
        &self.pack
    }

    pub fn load_blockstate(&self, block: &str) -> Result<BlockStateFile, ResourceError> {
        self.pack.load_blockstate(block)
    }

    /// Load every model referenced by the matching blockstate variant.
    pub fn load_variant_models(
        &mut self,
        block: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<Vec<(ResolvedBlockModel, ModelVariant)>, ResourceError> {
        let block_location = ResourceLocation::parse(block, "minecraft")?;
        let state = self.pack.load_blockstate(block)?;
        let key = properties.iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(",");
        let variants = state.variants_for(properties)
            .ok_or_else(|| ResourceError::VariantMissing(key))?
            .to_vec();

        let mut output = Vec::with_capacity(variants.len());
        for variant in variants {
            let location = ResourceLocation::parse(&variant.model, &block_location.namespace)?;
            output.push((self.load_model(&location.canonical())?, variant));
        }
        Ok(output)
    }

    pub fn load_model(&mut self, location: &str) -> Result<ResolvedBlockModel, ResourceError> {
        let location = ResourceLocation::parse(location, "minecraft")?;
        self.load_model_inner(&location, &mut Vec::new())
    }

    fn load_model_inner(
        &mut self,
        location: &ResourceLocation,
        chain: &mut Vec<String>,
    ) -> Result<ResolvedBlockModel, ResourceError> {
        let canonical = location.canonical();
        if let Some(model) = self.cache.get(&canonical) {
            return Ok(model.clone());
        }
        if chain.len() >= MAX_PARENT_DEPTH {
            return Err(ResourceError::ParentDepthExceeded);
        }
        if chain.iter().any(|entry| entry == &canonical) {
            let mut cycle = chain.clone();
            cycle.push(canonical);
            return Err(ResourceError::ParentCycle(cycle));
        }

        chain.push(canonical.clone());
        let path = self.pack.object_path("models", "json", location);
        let json = self.pack.read_text(&path, &canonical)?;
        let raw: RawModel = serde_json::from_str(&json).map_err(|error| ResourceError::Json {
            location: canonical.clone(),
            message: error.to_string(),
        })?;

        let mut model = if let Some(parent) = raw.parent.as_deref() {
            if parent == "builtin/generated" || parent == "builtin/entity" {
                ResolvedBlockModel {
                    location: canonical.clone(),
                    namespace: location.namespace.clone(),
                    ambient_occlusion: true,
                    textures: BTreeMap::new(),
                    elements: Vec::new(),
                }
            } else {
                let parent_location = ResourceLocation::parse(parent, &location.namespace)?;
                self.load_model_inner(&parent_location, chain)?
            }
        } else {
            ResolvedBlockModel {
                location: canonical.clone(),
                namespace: location.namespace.clone(),
                ambient_occlusion: true,
                textures: BTreeMap::new(),
                elements: Vec::new(),
            }
        };

        model.location = canonical.clone();
        model.namespace = location.namespace.clone();
        if let Some(value) = raw.ambient_occlusion {
            model.ambient_occlusion = value;
        }
        for (key, value) in raw.textures {
            if key.is_empty() || value.is_empty() {
                return Err(ResourceError::InvalidModel {
                    location: canonical,
                    reason: "texture keys and values cannot be empty".into(),
                });
            }
            model.textures.insert(key, value);
        }
        if let Some(elements) = raw.elements {
            model.elements = elements.into_iter()
                .map(|element| convert_element(element, &model.location))
                .collect::<Result<Vec<_>, _>>()?;
        }

        chain.pop();
        self.cache.insert(model.location.clone(), model.clone());
        Ok(model)
    }
}

#[derive(Deserialize)]
struct RawModel {
    #[serde(default)]
    parent: Option<String>,
    #[serde(default, rename = "ambientocclusion")]
    ambient_occlusion: Option<bool>,
    #[serde(default)]
    textures: BTreeMap<String, String>,
    #[serde(default)]
    elements: Option<Vec<RawElement>>,
}

#[derive(Deserialize)]
struct RawElement {
    from: [f32; 3],
    to: [f32; 3],
    #[serde(default)]
    rotation: Option<RawElementRotation>,
    #[serde(default = "default_shade")]
    shade: bool,
    faces: BTreeMap<String, RawFace>,
}

fn default_shade() -> bool {
    true
}

#[derive(Deserialize)]
struct RawElementRotation {
    origin: [f32; 3],
    axis: String,
    angle: f32,
    #[serde(default)]
    rescale: bool,
}

#[derive(Deserialize)]
struct RawFace {
    texture: String,
    #[serde(default)]
    uv: Option<[f32; 4]>,
    #[serde(default)]
    cullface: Option<String>,
    #[serde(default)]
    rotation: u16,
    #[serde(default)]
    tintindex: Option<i32>,
}

fn convert_element(raw: RawElement, location: &str) -> Result<ModelElement, ResourceError> {
    let coordinates_valid = raw.from.iter().chain(raw.to.iter())
        .all(|value| value.is_finite() && (-16.0..=32.0).contains(value));
    if !coordinates_valid {
        return Err(ResourceError::InvalidModel {
            location: location.to_owned(),
            reason: "element coordinates must be finite and between -16 and 32".into(),
        });
    }
    let rotation = if let Some(value) = raw.rotation {
        if !["x", "y", "z"].contains(&value.axis.as_str())
            || !value.angle.is_finite()
            || value.angle.abs() > 45.0
            || !value.origin.iter().all(|coordinate| coordinate.is_finite())
        {
            return Err(ResourceError::InvalidModel {
                location: location.to_owned(),
                reason: "element rotation has an invalid axis, angle, or origin".into(),
            });
        }
        Some(ElementRotation {
            origin: value.origin,
            axis: value.axis,
            angle: value.angle,
            rescale: value.rescale,
        })
    } else {
        None
    };

    let mut faces = BTreeMap::new();
    for (name, face) in raw.faces {
        let direction = FaceDirection::parse(&name).ok_or_else(|| ResourceError::InvalidModel {
            location: location.to_owned(),
            reason: format!("unknown face direction {name}"),
        })?;
        if face.texture.is_empty() || ![0, 90, 180, 270].contains(&face.rotation) {
            return Err(ResourceError::InvalidModel {
                location: location.to_owned(),
                reason: format!("face {name} has an invalid texture or rotation"),
            });
        }
        if face.uv.as_ref().is_some_and(|uv| !uv.iter().all(|coordinate| coordinate.is_finite())) {
            return Err(ResourceError::InvalidModel {
                location: location.to_owned(),
                reason: format!("face {name} has non-finite UV coordinates"),
            });
        }
        let cullface = match face.cullface {
            Some(value) => Some(FaceDirection::parse(&value).ok_or_else(|| ResourceError::InvalidModel {
                location: location.to_owned(),
                reason: format!("face {name} has invalid cullface {value}"),
            })?),
            None => None,
        };
        faces.insert(direction, ModelFace {
            texture: face.texture,
            uv: face.uv,
            cullface,
            rotation: face.rotation,
            tint_index: face.tintindex,
        });
    }
    if faces.is_empty() {
        return Err(ResourceError::InvalidModel {
            location: location.to_owned(),
            reason: "element must contain at least one face".into(),
        });
    }

    Ok(ModelElement {
        from: raw.from,
        to: raw.to,
        rotation,
        shade: raw.shade,
        faces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root() -> PathBuf {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("rust-mc-resource-tests-{stamp}"))
    }

    fn write_file(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn parses_variant_arrays_and_matches_properties_without_order_dependence() {
        let json = r#"{
            "variants": {
                "": { "model": "block/cube_all" },
                "facing=north,half=bottom": [
                    { "model": "block/stairs", "x": 0, "y": 90, "uvlock": true },
                    { "model": "block/stairs_alt", "weight": 2 }
                ]
            }
        }"#;
        let state = parse_blockstate_json(json, "minecraft:stairs").unwrap();
        let mut properties = BTreeMap::new();
        properties.insert("half".to_owned(), "bottom".to_owned());
        properties.insert("facing".to_owned(), "north".to_owned());
        let variants = state.variants_for(&properties).unwrap();
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].model, "block/stairs");
        assert_eq!(variants[0].y, 90);
        assert!(variants[0].uvlock);
        assert_eq!(variants[1].weight, 2);
    }

    #[test]
    fn loads_parent_models_and_resolves_chained_textures() {
        let root = temp_root();
        write_file(&root, "assets/minecraft/models/block/base.json", r##"{
            "ambientocclusion": false,
            "textures": { "all": "blocks/stone", "particle": "#all" },
            "elements": [{
                "from": [0, 0, 0], "to": [16, 16, 16],
                "faces": { "north": { "texture": "#particle", "cullface": "north" } }
            }]
        }"##);
        write_file(&root, "assets/minecraft/models/block/child.json", r##"{
            "parent": "block/base",
            "textures": { "side": "#all" }
        }"##);

        let pack = ResourcePack::open(&root).unwrap();
        let mut loader = ModelLoader::new(pack);
        let model = loader.load_model("minecraft:block/child").unwrap();
        assert_eq!(model.elements.len(), 1);
        assert!(!model.ambient_occlusion);
        assert_eq!(model.resolve_texture("side").unwrap(), "minecraft:blocks/stone");
        assert_eq!(
            model.resolve_face_texture(&model.elements[0].faces[&FaceDirection::North]).unwrap(),
            "minecraft:blocks/stone"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_blockstate_and_texture_files_from_resource_pack() {
        let root = temp_root();
        write_file(&root, "assets/example/blockstates/test.json", r#"{"variants":{"":{"model":"block/cube"}}}"#);
        let texture_path = root.join("assets/example/textures/blocks/test.png");
        fs::create_dir_all(texture_path.parent().unwrap()).unwrap();
        fs::write(&texture_path, [137_u8, 80, 78, 71]).unwrap();

        let pack = ResourcePack::open(&root).unwrap();
        assert!(pack.load_blockstate("example:test").is_ok());
        assert_eq!(pack.read_texture("blocks/test", "example").unwrap(), [137, 80, 78, 71]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_path_traversal_invalid_rotations_and_texture_cycles() {
        assert!(matches!(
            ResourceLocation::parse("minecraft:../secret", "minecraft"),
            Err(ResourceError::InvalidLocation(_))
        ));
        assert!(matches!(
            ResourceLocation::parse("minecraft:a//b", "minecraft"),
            Err(ResourceError::InvalidLocation(_))
        ));

        let invalid = r#"{"variants":{"":{"model":"block/cube","y":45}}}"#;
        assert!(parse_blockstate_json(invalid, "minecraft:invalid").is_err());

        let model = ResolvedBlockModel {
            location: "minecraft:block/cycle".into(),
            namespace: "minecraft".into(),
            ambient_occlusion: true,
            textures: BTreeMap::from([
                ("a".to_owned(), "#b".to_owned()),
                ("b".to_owned(), "#a".to_owned()),
            ]),
            elements: Vec::new(),
        };
        assert!(matches!(model.resolve_texture("a"), Err(ResourceError::TextureReferenceCycle(_))));
    }
}
