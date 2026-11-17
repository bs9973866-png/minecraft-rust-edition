//! Entity state and server-driven entity updates for protocol 47.

use std::collections::BTreeMap;

use crate::protocol::ItemStack;

#[derive(Clone, Debug, PartialEq)]
pub enum EntityMetadataValue {
    Byte(i8),
    Short(i16),
    Int(i32),
    Float(f32),
    String(String),
    ItemStack(Option<ItemStack>),
    Coordinates([i32; 3]),
}

#[derive(Clone, Debug, PartialEq)]
pub enum EntityKind {
    Player { uuid: String, held_item: i16 },
    Mob { mob_type: u8 },
    Object { object_type: u8, data: i32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct EntityState {
    pub entity_id: i32,
    pub kind: EntityKind,
    /// Current interpolated position.
    pub position: [f64; 3],
    /// Latest server-authoritative destination position.
    pub target_position: [f64; 3],
    pub position_interpolation_ticks: u8,
    pub yaw: f32,
    pub pitch: f32,
    pub head_yaw: f32,
    pub target_yaw: f32,
    pub target_pitch: f32,
    pub target_head_yaw: f32,
    pub rotation_interpolation_ticks: u8,
    pub head_interpolation_ticks: u8,
    pub velocity: [f64; 3],
    pub on_ground: bool,
    pub equipment: BTreeMap<i16, Option<ItemStack>>,
    pub metadata: BTreeMap<u8, EntityMetadataValue>,
}

impl EntityState {
    /// Advance one simulation/render tick toward the latest server transform.
    pub fn tick_interpolation(&mut self) {
        if self.position_interpolation_ticks > 0 {
            let divisor = f64::from(self.position_interpolation_ticks);
            for axis in 0..3 {
                self.position[axis] += (self.target_position[axis] - self.position[axis]) / divisor;
            }
            self.position_interpolation_ticks -= 1;
            if self.position_interpolation_ticks == 0 {
                self.position = self.target_position;
            }
        }

        if self.rotation_interpolation_ticks > 0 {
            let divisor = f32::from(self.rotation_interpolation_ticks);
            self.yaw += angle_delta(self.yaw, self.target_yaw) / divisor;
            self.pitch += angle_delta(self.pitch, self.target_pitch) / divisor;
            self.rotation_interpolation_ticks -= 1;
            if self.rotation_interpolation_ticks == 0 {
                self.yaw = self.target_yaw;
                self.pitch = self.target_pitch;
            }
        }

        if self.head_interpolation_ticks > 0 {
            let divisor = f32::from(self.head_interpolation_ticks);
            self.head_yaw += angle_delta(self.head_yaw, self.target_head_yaw) / divisor;
            self.head_interpolation_ticks -= 1;
            if self.head_interpolation_ticks == 0 {
                self.head_yaw = self.target_head_yaw;
            }
        }
    }
}

fn angle_delta(current: f32, target: f32) -> f32 {
    (target - current + 180.0).rem_euclid(360.0) - 180.0
}

#[derive(Clone, Debug, PartialEq)]
pub enum EntityEvent {
    Spawn(EntityState),
    RelativeMove {
        entity_id: i32,
        delta: [f64; 3],
        rotation: Option<(f32, f32)>,
        on_ground: bool,
    },
    Teleport {
        entity_id: i32,
        position: [f64; 3],
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    },
    HeadLook {
        entity_id: i32,
        yaw: f32,
    },
    Velocity {
        entity_id: i32,
        velocity: [f64; 3],
    },
    Equipment {
        entity_id: i32,
        slot: i16,
        item: Option<ItemStack>,
    },
    Metadata {
        entity_id: i32,
        entries: BTreeMap<u8, EntityMetadataValue>,
    },
    Destroy(Vec<i32>),
}
