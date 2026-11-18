//! Small dependency-free CPU triangle renderer for the 1.8.9 asset pipeline.
//!
//! It renders atlas-backed chunk meshes into an RGBA-like 0x00RRGGBB framebuffer.
//! A platform window/GPU upload layer can consume this buffer separately.

use std::error::Error as StdError;
use std::fmt;

use crate::mesh::{ModelMesh, ModelMeshVertex};
use crate::texture::TextureAtlas;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub position: [f32; 3],
    /// Minecraft yaw: 0 looks toward +Z; 90 looks toward -X.
    pub yaw: f32,
    /// Positive pitch looks down.
    pub pitch: f32,
    pub vertical_fov_degrees: f32,
    pub near_plane: f32,
    pub far_plane: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            position: [0.0, 64.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            vertical_fov_degrees: 70.0,
            near_plane: 0.05,
            far_plane: 512.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    InvalidDimensions,
    InvalidCamera,
    InvalidAtlas,
    InvalidMeshIndex { index: u32, vertices: usize },
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDimensions => write!(f, "framebuffer dimensions are invalid or too large"),
            Self::InvalidCamera => write!(f, "camera parameters are invalid"),
            Self::InvalidAtlas => write!(f, "texture atlas pixel buffer does not match its dimensions"),
            Self::InvalidMeshIndex { index, vertices } => {
                write!(f, "mesh index {index} exceeds vertex count {vertices}")
            }
        }
    }
}

impl StdError for RenderError {}

#[derive(Clone, Debug, PartialEq)]
pub struct FrameBuffer {
    width: u32,
    height: u32,
    /// Pixels use 0x00RRGGBB, suitable for simple window-buffer adapters.
    pixels: Vec<u32>,
    depth: Vec<f32>,
}

impl FrameBuffer {
    pub fn new(width: u32, height: u32) -> Result<Self, RenderError> {
        if width == 0 || height == 0 || width > 8192 || height > 8192 {
            return Err(RenderError::InvalidDimensions);
        }
        let count = (width as usize)
            .checked_mul(height as usize)
            .ok_or(RenderError::InvalidDimensions)?;
        Ok(Self {
            width,
            height,
            pixels: vec![0; count],
            depth: vec![f32::INFINITY; count],
        })
    }

    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn pixels(&self) -> &[u32] { &self.pixels }

    pub fn pixel(&self, x: u32, y: u32) -> Option<u32> {
        if x >= self.width || y >= self.height { return None; }
        self.pixels.get((y * self.width + x) as usize).copied()
    }

    pub fn clear(&mut self, color: u32) {
        self.pixels.fill(color & 0x00FF_FFFF);
        self.depth.fill(f32::INFINITY);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub input_triangles: usize,
    pub rasterized_triangles: usize,
    pub shaded_pixels: usize,
}

#[derive(Clone, Copy)]
struct CameraVertex {
    x: f32,
    y: f32,
    z: f32,
    u: f32,
    v: f32,
    block_light: f32,
    sky_light: f32,
}

#[derive(Clone, Copy)]
struct ScreenVertex {
    x: f32,
    y: f32,
    z: f32,
    u: f32,
    v: f32,
    block_light: f32,
    sky_light: f32,
}

/// Render an indexed model mesh into a caller-owned framebuffer.
/// Triangles crossing the near plane are skipped rather than clipped.
pub fn render_mesh(
    target: &mut FrameBuffer,
    mesh: &ModelMesh,
    atlas: &TextureAtlas,
    camera: &Camera,
) -> Result<RenderStats, RenderError> {
    if !camera.position.iter().all(|value| value.is_finite())
        || !camera.yaw.is_finite()
        || !camera.pitch.is_finite()
        || !camera.vertical_fov_degrees.is_finite()
        || !(1.0..179.0).contains(&camera.vertical_fov_degrees)
        || !camera.near_plane.is_finite()
        || !camera.far_plane.is_finite()
        || camera.near_plane <= 0.0
        || camera.far_plane <= camera.near_plane
    {
        return Err(RenderError::InvalidCamera);
    }
    let expected = (atlas.width as usize)
        .checked_mul(atlas.height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(RenderError::InvalidAtlas)?;
    if atlas.width == 0 || atlas.height == 0 || atlas.rgba.len() != expected {
        return Err(RenderError::InvalidAtlas);
    }
    for &index in &mesh.indices {
        if index as usize >= mesh.vertices.len() {
            return Err(RenderError::InvalidMeshIndex { index, vertices: mesh.vertices.len() });
        }
    }

    let yaw = camera.yaw.to_radians();
    let pitch = camera.pitch.to_radians();
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let (sin_pitch, cos_pitch) = pitch.sin_cos();
    let forward = [-sin_yaw * cos_pitch, -sin_pitch, cos_yaw * cos_pitch];
    let right = [cos_yaw, 0.0, sin_yaw];
    let up = [
        forward[1] * right[2] - forward[2] * right[1],
        forward[2] * right[0] - forward[0] * right[2],
        forward[0] * right[1] - forward[1] * right[0],
    ];
    let focal = target.height as f32
        / (2.0 * (camera.vertical_fov_degrees.to_radians() * 0.5).tan());
    let cx = target.width as f32 * 0.5;
    let cy = target.height as f32 * 0.5;

    let mut stats = RenderStats {
        input_triangles: mesh.indices.len() / 3,
        ..RenderStats::default()
    };

    for triangle in mesh.indices.chunks_exact(3) {
        let source = [
            &mesh.vertices[triangle[0] as usize],
            &mesh.vertices[triangle[1] as usize],
            &mesh.vertices[triangle[2] as usize],
        ];
        let Some(a) = project_vertex(source[0], camera, right, up, forward, focal, cx, cy) else { continue; };
        let Some(b) = project_vertex(source[1], camera, right, up, forward, focal, cx, cy) else { continue; };
        let Some(c) = project_vertex(source[2], camera, right, up, forward, focal, cx, cy) else { continue; };
        if rasterize_triangle(target, atlas, [a, b, c], camera.near_plane, camera.far_plane) {
            stats.rasterized_triangles += 1;
        }
    }

    stats.shaded_pixels = target.depth.iter().filter(|depth| depth.is_finite()).count();
    Ok(stats)
}

fn project_vertex(
    vertex: &ModelMeshVertex,
    camera: &Camera,
    right: [f32; 3],
    up: [f32; 3],
    forward: [f32; 3],
    focal: f32,
    center_x: f32,
    center_y: f32,
) -> Option<ScreenVertex> {
    let relative = [
        vertex.position[0] - camera.position[0],
        vertex.position[1] - camera.position[1],
        vertex.position[2] - camera.position[2],
    ];
    let dot = |axis: [f32; 3]| relative[0] * axis[0] + relative[1] * axis[1] + relative[2] * axis[2];
    let x = dot(right);
    let y = dot(up);
    let z = dot(forward);
    if !z.is_finite() || z <= camera.near_plane || z >= camera.far_plane { return None; }
    Some(ScreenVertex {
        x: center_x + x * focal / z,
        y: center_y - y * focal / z,
        z,
        u: vertex.uv[0],
        v: vertex.uv[1],
        block_light: f32::from(vertex.block_light.min(15)),
        sky_light: f32::from(vertex.sky_light.min(15)),
    })
}

fn edge(a: ScreenVertex, b: ScreenVertex, x: f32, y: f32) -> f32 {
    (x - a.x) * (b.y - a.y) - (y - a.y) * (b.x - a.x)
}

fn rasterize_triangle(
    target: &mut FrameBuffer,
    atlas: &TextureAtlas,
    vertices: [ScreenVertex; 3],
    near: f32,
    far: f32,
) -> bool {
    let [a, b, c] = vertices;
    let area = edge(a, b, c.x, c.y);
    if !area.is_finite() || area.abs() < 1.0e-6 { return false; }

    let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as u32;
    let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as u32;
    let max_x = a.x.max(b.x).max(c.x).ceil().min(target.width as f32 - 1.0) as u32;
    let max_y = a.y.max(b.y).max(c.y).ceil().min(target.height as f32 - 1.0) as u32;
    if min_x > max_x || min_y > max_y { return false; }

    let mut wrote_pixel = false;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let sample_x = x as f32 + 0.5;
            let sample_y = y as f32 + 0.5;
            let w0 = edge(b, c, sample_x, sample_y) / area;
            let w1 = edge(c, a, sample_x, sample_y) / area;
            let w2 = edge(a, b, sample_x, sample_y) / area;
            if w0 < -1.0e-5 || w1 < -1.0e-5 || w2 < -1.0e-5 { continue; }

            let inverse_z = w0 / a.z + w1 / b.z + w2 / c.z;
            if inverse_z <= 0.0 { continue; }
            let depth = 1.0 / inverse_z;
            if depth < near || depth > far { continue; }
            let offset = (y * target.width + x) as usize;
            if depth >= target.depth[offset] { continue; }

            let perspective = |av: f32, bv: f32, cv: f32| {
                (w0 * av / a.z + w1 * bv / b.z + w2 * cv / c.z) / inverse_z
            };
            let u = perspective(a.u, b.u, c.u).clamp(0.0, 1.0);
            let v = perspective(a.v, b.v, c.v).clamp(0.0, 1.0);
            let texture_x = ((u * atlas.width as f32).floor() as u32).min(atlas.width - 1);
            let texture_y = ((v * atlas.height as f32).floor() as u32).min(atlas.height - 1);
            let texture_offset = ((texture_y * atlas.width + texture_x) * 4) as usize;
            let rgba = &atlas.rgba[texture_offset..texture_offset + 4];
            if rgba[3] == 0 { continue; }

            let light = (perspective(a.block_light, b.block_light, c.block_light)
                .max(perspective(a.sky_light, b.sky_light, c.sky_light)) / 15.0)
                .clamp(0.0, 1.0);
            let brightness = 0.12 + light * 0.88;
            let source = [
                (f32::from(rgba[0]) * brightness).round() as u32,
                (f32::from(rgba[1]) * brightness).round() as u32,
                (f32::from(rgba[2]) * brightness).round() as u32,
            ];
            let destination = target.pixels[offset];
            let alpha = u32::from(rgba[3]);
            let inverse_alpha = 255 - alpha;
            let blend = |source_channel: u32, shift: u32| {
                let destination_channel = (destination >> shift) & 0xFF;
                (source_channel * alpha + destination_channel * inverse_alpha + 127) / 255
            };
            target.pixels[offset] =
                (blend(source[0], 16) << 16) | (blend(source[1], 8) << 8) | blend(source[2], 0);
            if alpha >= 250 {
                target.depth[offset] = depth;
            }
            wrote_pixel = true;
        }
    }
    wrote_pixel
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::ModelMeshVertex;
    use crate::resource_pack::FaceDirection;
    use std::collections::BTreeMap;

    fn triangle_mesh(z: f32, light: u8) -> ModelMesh {
        let vertex = |position| ModelMeshVertex {
            position,
            uv: [0.5, 0.5],
            texture: "test:blocks/pixel".into(),
            face: FaceDirection::North,
            cullface: None,
            tint_index: None,
            block_light: light,
            sky_light: light,
        };
        ModelMesh {
            vertices: vec![
                vertex([-1.0, -1.0, z]),
                vertex([1.0, -1.0, z]),
                vertex([0.0, 1.0, z]),
            ],
            indices: vec![0, 1, 2],
            quad_count: 1,
        }
    }

    fn red_atlas() -> TextureAtlas {
        TextureAtlas {
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
            regions: BTreeMap::new(),
        }
    }

    #[test]
    fn projects_and_shades_an_atlas_textured_triangle() {
        let mut target = FrameBuffer::new(64, 64).unwrap();
        target.clear(0);
        let stats = render_mesh(&mut target, &triangle_mesh(3.0, 15), &red_atlas(), &Camera {
            position: [0.0, 0.0, 0.0],
            ..Camera::default()
        }).unwrap();

        assert_eq!(stats.input_triangles, 1);
        assert_eq!(stats.rasterized_triangles, 1);
        assert!(stats.shaded_pixels > 100);
        assert_eq!(target.pixel(32, 32), Some(0x00FF_0000));
    }

    #[test]
    fn skips_triangles_behind_the_near_plane_and_rejects_bad_indices() {
        let mut target = FrameBuffer::new(32, 32).unwrap();
        let stats = render_mesh(&mut target, &triangle_mesh(0.01, 15), &red_atlas(), &Camera {
            position: [0.0, 0.0, 0.0],
            ..Camera::default()
        }).unwrap();
        assert_eq!(stats.rasterized_triangles, 0);
        assert!(matches!(
            render_mesh(&mut target, &ModelMesh {
                vertices: vec![],
                indices: vec![0],
                quad_count: 0,
            }, &red_atlas(), &Camera::default()),
            Err(RenderError::InvalidMeshIndex { .. })
        ));
    }
}
