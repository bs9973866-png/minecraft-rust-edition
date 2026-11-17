//! Small, dependency-free geometry primitives used by the client simulation.

use std::ops::{Add, AddAssign, Div, Mul, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 { pub x: f64, pub y: f64, pub z: f64 }

impl Vec3 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0, z: 0.0 };
    pub const fn new(x: f64, y: f64, z: f64) -> Self { Self { x, y, z } }
    pub fn length_squared(self) -> f64 { self.x * self.x + self.y * self.y + self.z * self.z }
    pub fn length(self) -> f64 { self.length_squared().sqrt() }
    pub fn normalized(self) -> Self {
        let length = self.length();
        if length <= f64::EPSILON { Self::ZERO } else { self / length }
    }
    pub fn dot(self, other: Self) -> f64 { self.x * other.x + self.y * other.y + self.z * other.z }
    pub fn component(self, axis: usize) -> f64 {
        match axis { 0 => self.x, 1 => self.y, 2 => self.z, _ => 0.0 }
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output { Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z) }
}
impl AddAssign for Vec3 { fn add_assign(&mut self, rhs: Self) { *self = *self + rhs; } }
impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output { Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z) }
}
impl SubAssign for Vec3 { fn sub_assign(&mut self, rhs: Self) { *self = *self - rhs; } }
impl Mul<f64> for Vec3 {
    type Output = Self;
    fn mul(self, rhs: f64) -> Self::Output { Self::new(self.x * rhs, self.y * rhs, self.z * rhs) }
}
impl Div<f64> for Vec3 {
    type Output = Self;
    fn div(self, rhs: f64) -> Self::Output { Self::new(self.x / rhs, self.y / rhs, self.z / rhs) }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb { pub min: Vec3, pub max: Vec3 }

impl Aabb {
    pub const fn new(min: Vec3, max: Vec3) -> Self { Self { min, max } }
    pub fn translated(self, offset: Vec3) -> Self { Self::new(self.min + offset, self.max + offset) }
    pub fn intersects(self, other: Self) -> bool {
        self.max.x > other.min.x && self.min.x < other.max.x
            && self.max.y > other.min.y && self.min.y < other.max.y
            && self.max.z > other.min.z && self.min.z < other.max.z
    }
    pub fn contains(self, point: Vec3) -> bool {
        point.x >= self.min.x && point.x <= self.max.x
            && point.y >= self.min.y && point.y <= self.max.y
            && point.z >= self.min.z && point.z <= self.max.z
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vector_normalization_and_dot_product() {
        let vector = Vec3::new(3.0, 0.0, 4.0);
        assert_eq!(vector.length(), 5.0);
        assert_eq!(vector.normalized(), Vec3::new(0.6, 0.0, 0.8));
        assert_eq!(vector.dot(Vec3::new(0.0, 1.0, 0.0)), 0.0);
    }
    #[test]
    fn aabb_intersection_and_translation() {
        let first = Aabb::new(Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0));
        let touching = Aabb::new(Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 1.0));
        assert!(!first.intersects(touching));
        assert!(first.translated(Vec3::new(0.5, 0.0, 0.0)).intersects(touching));
        assert!(first.contains(Vec3::new(0.5, 0.5, 0.5)));
    }
}
