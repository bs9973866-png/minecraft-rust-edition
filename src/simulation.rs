//! Client-side world/player state that joins decoded Play events to the simulation.

use crate::math::Vec3;
use crate::physics::{PhysicsTick, PlayerInput, PlayerPhysics};
use crate::protocol::{PlayEvent, PlayerMove};
use crate::world::{World, WorldError};

#[derive(Clone, Debug)]
pub struct ClientSimulation {
    pub world: World,
    pub player: PlayerPhysics,
    pub tick_count: u64,
}

impl ClientSimulation {
    pub fn new(spawn_position: Vec3) -> Self {
        Self {
            world: World::new(),
            player: PlayerPhysics::new(spawn_position),
            tick_count: 0,
        }
    }

    /// Advance one local simulation tick and smooth server-driven entity transforms.
    pub fn tick(&mut self, input: PlayerInput) -> PhysicsTick {
        self.tick_count = self.tick_count.wrapping_add(1);
        self.world.tick_entity_interpolation();
        self.player.tick(&self.world, input)
    }

    /// Return the first non-air block under the player's crosshair.
    pub fn target_block(&self, max_distance: f64) -> Option<crate::world::RayHit> {
        let eye = Vec3::new(
            self.player.position.x,
            self.player.position.y + self.player.height * 0.9,
            self.player.position.z,
        );
        self.world.raycast(eye, self.player.look_direction(), max_distance)
    }

    /// Apply a decoded Play event to local simulation state.
    /// A returned PlayerMove must be sent back to the server as a C06 acknowledgement.
    pub fn apply_play_event(&mut self, event: &PlayEvent) -> Result<Option<PlayerMove>, WorldError> {
        match event {
            PlayEvent::Chunk(chunk) => self.world.apply_chunk_data(chunk.clone())?,
            PlayEvent::ChunkBulk(bulk) => self.world.apply_chunk_batch(bulk.columns.clone())?,
            PlayEvent::BlockChange(change) => {
                self.world.apply_block_change(change.position, change.state);
            }
            PlayEvent::MultiBlockChange(change) => {
                self.world.apply_block_changes(
                    change.records.iter().map(|record| (record.position, record.state))
                );
            }
            PlayEvent::Entity(entity_event) => {
                self.world.apply_entity_event(entity_event.clone());
            }
            PlayEvent::PlayerPositionLook(update) => {
                return Ok(Some(self.player.apply_server_position_look(*update)));
            }
            PlayEvent::Respawn(_) => {
                self.world.clear();
                self.player.motion = Vec3::ZERO;
                self.player.on_ground = false;
            }
            _ => {}
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{EntityEvent, EntityKind, EntityState};
    use crate::physics::PlayerInput;
    use crate::protocol::{BlockChange, PlayerPositionLook};
    use crate::world::{BlockPos, BlockState};

    #[test]
    fn applies_s08_relative_and_absolute_fields_and_returns_c06_transform() {
        let mut simulation = ClientSimulation::new(Vec3::new(10.0, 20.0, 30.0));
        simulation.player.yaw = 45.0;
        simulation.player.pitch = 10.0;

        let response = simulation.apply_play_event(&PlayEvent::PlayerPositionLook(PlayerPositionLook {
            x: 1.0, y: -2.0, z: 0.5, yaw: 90.0, pitch: -5.0,
            flags: 0x01 | 0x04 | 0x08,
        })).unwrap().unwrap();

        assert_eq!((simulation.player.position.x, simulation.player.position.y, simulation.player.position.z), (11.0, -2.0, 30.5));
        assert_eq!((simulation.player.yaw, simulation.player.pitch), (135.0, -5.0));
        assert_eq!((response.x, response.y, response.z, response.yaw, response.pitch), (11.0, -2.0, 30.5, 135.0, -5.0));
    }

    #[test]
    fn applies_block_entity_and_respawn_events_to_simulation() {
        let mut simulation = ClientSimulation::new(Vec3::new(0.5, 2.0, 0.5));
        let block = BlockChange { position: BlockPos::new(1, 2, 3), state: BlockState(16) };
        simulation.apply_play_event(&PlayEvent::BlockChange(block)).unwrap();
        assert_eq!(simulation.world.get_block(BlockPos::new(1, 2, 3)), BlockState(16));

        let entity = EntityState {
            entity_id: 9,
            kind: EntityKind::Mob { mob_type: 54 },
            position: [1.0, 2.0, 3.0],
            target_position: [1.0, 2.0, 3.0],
            position_interpolation_ticks: 0,
            yaw: 0.0, pitch: 0.0, head_yaw: 0.0,
            target_yaw: 0.0, target_pitch: 0.0, target_head_yaw: 0.0,
            rotation_interpolation_ticks: 0, head_interpolation_ticks: 0,
            velocity: [0.0; 3], on_ground: false,
            equipment: std::collections::BTreeMap::new(),
            metadata: std::collections::BTreeMap::new(),
        };
        simulation.apply_play_event(&PlayEvent::Entity(EntityEvent::Spawn(entity))).unwrap();
        assert_eq!(simulation.world.entity_count(), 1);

        simulation.player.motion = Vec3::new(1.0, 2.0, 3.0);
        simulation.apply_play_event(&PlayEvent::Respawn(crate::protocol::RespawnInfo {
            dimension: -1, difficulty: 2, game_mode: 0, level_type: "default".into()
        })).unwrap();
        assert_eq!(simulation.world.chunk_count(), 0);
        assert_eq!(simulation.world.entity_count(), 0);
        assert_eq!(simulation.player.motion, Vec3::ZERO);
        assert!(!simulation.player.on_ground);
    }

    #[test]
    fn simulation_tick_advances_player_and_entity_interpolation() {
        let mut simulation = ClientSimulation::new(Vec3::new(0.5, 1.0, 0.5));
        simulation.world.set_block(BlockPos::new(0, 0, 0), BlockState(16));
        simulation.player.on_ground = true;

        simulation.tick(PlayerInput { forward: 1.0, ..PlayerInput::default() });
        assert_eq!(simulation.tick_count, 1);
        assert!(simulation.player.position.z > 0.5);
    }
    #[test]
    fn crosshair_raycast_uses_player_eye_height_and_yaw() {
        let mut simulation = ClientSimulation::new(Vec3::new(0.5, 1.0, 0.5));
        simulation.world.set_block(BlockPos::new(0, 2, 3), BlockState(16));
        let forward_hit = simulation.target_block(5.0).unwrap();
        assert_eq!(forward_hit.position, BlockPos::new(0, 2, 3));

        simulation.world.clear();
        simulation.player.yaw = 90.0;
        simulation.world.set_block(BlockPos::new(-2, 2, 0), BlockState(16));
        let side_hit = simulation.target_block(5.0).unwrap();
        assert_eq!(side_hit.position, BlockPos::new(-2, 2, 0));
    }


}
