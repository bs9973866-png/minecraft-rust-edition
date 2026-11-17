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
    pub position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub head_yaw: f32,
    pub velocity: [f64; 3],
    pub on_ground: bool,
    pub metadata: BTreeMap<u8, EntityMetadataValue>,
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
    Metadata {
        entity_id: i32,
        entries: BTreeMap<u8, EntityMetadataValue>,
    },
    Destroy(Vec<i32>),
}
