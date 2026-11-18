//! Basic player movement and full-cube collision for the 1.8.9-style client simulation.
//!
//! This is a first physics layer, not a complete implementation of every vanilla
//! block shape, fluid, ladder, potion or movement mode.

use crate::math::{Aabb, Vec3};
use crate::protocol::{PlayerMove, PlayerPositionLook};
use crate::world::{BlockPos, BlockState, World};

const DEFAULT_GRAVITY: f64 = 0.08;
const VERTICAL_DRAG: f64 = 0.98;
const AIR_DRAG: f64 = 0.91;
const GROUND_ACCELERATION_FACTOR: f64 = 0.162_771_36;
const AIR_ACCELERATION: f64 = 0.02;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerInput {
    /// Forward/back input in the range -1..=1.
    pub forward: f64,
    /// Strafe-right/left input in the range -1..=1.
    pub strafe: f64,
    /// A jump press for this tick (not a held-key state).
    pub jump: bool,
    /// Sprint movement multiplier. Sprint toggling itself is handled by the caller.
    pub sprint: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysicsTick {
    pub collided_horizontally: bool,
    pub collided_vertically: bool,
    pub landed: bool,
    pub hit_ceiling: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerPhysics {
    /// Player position is at the center of the feet.
    pub position: Vec3,
    pub motion: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub width: f64,
    pub height: f64,
    pub walk_speed: f64,
    pub jump_velocity: f64,
}

impl PlayerPhysics {
    pub fn new(position: Vec3) -> Self {
        Self {
            position,
            motion: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            width: 0.6,
            height: 1.8,
            walk_speed: 0.1,
            jump_velocity: 0.42,
        }
    }

    /// Apply S08 server correction, respecting the relative-coordinate/rotation flags.
    /// Returns the absolute transform that must be acknowledged with C06.
    pub fn apply_server_position_look(&mut self, update: PlayerPositionLook) -> PlayerMove {
        let relative_x = update.flags & 0x01 != 0;
        let relative_y = update.flags & 0x02 != 0;
        let relative_z = update.flags & 0x04 != 0;
        let relative_yaw = update.flags & 0x08 != 0;
        let relative_pitch = update.flags & 0x10 != 0;

        self.position.x = if relative_x { self.position.x + update.x } else { update.x };
        self.position.y = if relative_y { self.position.y + update.y } else { update.y };
        self.position.z = if relative_z { self.position.z + update.z } else { update.z };
        self.yaw = if relative_yaw { self.yaw + update.yaw } else { update.yaw };
        self.pitch = if relative_pitch { self.pitch + update.pitch } else { update.pitch };

        PlayerMove {
            x: self.position.x,
            y: self.position.y,
            z: self.position.z,
            yaw: self.yaw,
            pitch: self.pitch,
        }
    }

    pub fn bounding_box(&self) -> Aabb {
        let half = self.width * 0.5;
        Aabb::new(
            Vec3::new(self.position.x - half, self.position.y, self.position.z - half),
            Vec3::new(self.position.x + half, self.position.y + self.height, self.position.z + half),
        )
    }

    /// Run one movement tick using the current server/client world collision data.
    pub fn tick(&mut self, world: &World, input: PlayerInput) -> PhysicsTick {
        let was_on_ground = self.on_ground;
        let below = self.block_below(world);
        let ground_drag = if self.on_ground {
            below.slipperiness() * AIR_DRAG
        } else {
            AIR_DRAG
        };

        let acceleration = if self.on_ground {
            let denominator = ground_drag * ground_drag * ground_drag;
            self.walk_speed * GROUND_ACCELERATION_FACTOR / denominator.max(f64::EPSILON)
        } else {
            AIR_ACCELERATION
        } * if input.sprint { 1.3 } else { 1.0 };

        self.apply_movement_input(input.strafe, input.forward, acceleration);

        if input.jump && self.on_ground {
            self.motion.y = self.jump_velocity;
            self.on_ground = false;
        }

        let requested = self.motion;
        let (moved, collisions) = move_with_full_cube_collisions(world, self.bounding_box(), requested);
        self.position += moved;

        if collisions.x { self.motion.x = 0.0; }
        if collisions.y { self.motion.y = 0.0; }
        if collisions.z { self.motion.z = 0.0; }

        self.on_ground = collisions.y && requested.y < 0.0;
        self.motion.y -= DEFAULT_GRAVITY;
        self.motion.y *= VERTICAL_DRAG;

        let horizontal_drag = if self.on_ground {
            self.block_below(world).slipperiness() * AIR_DRAG
        } else {
            AIR_DRAG
        };
        self.motion.x *= horizontal_drag;
        self.motion.z *= horizontal_drag;

        PhysicsTick {
            collided_horizontally: collisions.x || collisions.z,
            collided_vertically: collisions.y,
            landed: !was_on_ground && self.on_ground,
            hit_ceiling: collisions.y && requested.y > 0.0,
        }
    }

    fn block_below(&self, world: &World) -> BlockState {
        let x = self.position.x.floor() as i32;
        let y = (self.position.y - 0.01).floor() as i32;
        let z = self.position.z.floor() as i32;
        world.get_block(BlockPos::new(x, y, z))
    }

    fn apply_movement_input(&mut self, strafe: f64, forward: f64, acceleration: f64) {
        let mut strafe = strafe.clamp(-1.0, 1.0);
        let mut forward = forward.clamp(-1.0, 1.0);
        let magnitude_squared = strafe * strafe + forward * forward;
        if magnitude_squared < 1.0e-4 { return; }

        let magnitude = magnitude_squared.sqrt().max(1.0);
        let scale = acceleration / magnitude;
        strafe *= scale;
        forward *= scale;

        let radians = f64::from(self.yaw).to_radians();
        let sin_yaw = radians.sin();
        let cos_yaw = radians.cos();
        self.motion.x += strafe * cos_yaw - forward * sin_yaw;
        self.motion.z += forward * cos_yaw + strafe * sin_yaw;
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AxisCollisions {
    x: bool,
    y: bool,
    z: bool,
}

/// Move in vanilla's Y, X, Z axis order, clipping against full-cube blocks.
/// Complex block collision shapes are intentionally excluded from this first pass.
fn move_with_full_cube_collisions(world: &World, mut bounds: Aabb, requested: Vec3) -> (Vec3, AxisCollisions) {
    let mut collisions = AxisCollisions::default();

    let y = clip_axis(world, bounds, requested.y, 1);
    collisions.y = (y - requested.y).abs() > f64::EPSILON;
    bounds = bounds.translated(Vec3::new(0.0, y, 0.0));

    let x = clip_axis(world, bounds, requested.x, 0);
    collisions.x = (x - requested.x).abs() > f64::EPSILON;
    bounds = bounds.translated(Vec3::new(x, 0.0, 0.0));

    let z = clip_axis(world, bounds, requested.z, 2);
    collisions.z = (z - requested.z).abs() > f64::EPSILON;

    (Vec3::new(x, y, z), collisions)
}

fn axis_value(vector: Vec3, axis: usize) -> f64 {
    match axis { 0 => vector.x, 1 => vector.y, _ => vector.z }
}

fn clip_axis(world: &World, bounds: Aabb, mut movement: f64, axis: usize) -> f64 {
    if movement.abs() <= f64::EPSILON { return 0.0; }

    let mut sweep_min = bounds.min;
    let mut sweep_max = bounds.max;
    match axis {
        0 => {
            sweep_min.x += movement.min(0.0);
            sweep_max.x += movement.max(0.0);
        }
        1 => {
            sweep_min.y += movement.min(0.0);
            sweep_max.y += movement.max(0.0);
        }
        _ => {
            sweep_min.z += movement.min(0.0);
            sweep_max.z += movement.max(0.0);
        }
    }

    let min_x = sweep_min.x.floor() as i32;
    let max_x = sweep_max.x.floor() as i32;
    let min_y = sweep_min.y.floor() as i32;
    let max_y = sweep_max.y.floor() as i32;
    let min_z = sweep_min.z.floor() as i32;
    let max_z = sweep_max.z.floor() as i32;

    for x in min_x..=max_x {
        for y in min_y..=max_y {
            for z in min_z..=max_z {
                let position = BlockPos::new(x, y, z);
                let state = world.get_block(position);
                let Some(block) = state.collision_box_at(position) else { continue; };
                if !overlaps_on_other_axes(bounds, block, axis) { continue; }

                let player_min = axis_value(bounds.min, axis);
                let player_max = axis_value(bounds.max, axis);
                let block_min = axis_value(block.min, axis);
                let block_max = axis_value(block.max, axis);

                if movement > 0.0 && player_max <= block_min {
                    movement = movement.min(block_min - player_max);
                } else if movement < 0.0 && player_min >= block_max {
                    movement = movement.max(block_max - player_min);
                }
            }
        }
    }
    movement
}

fn overlaps_on_other_axes(a: Aabb, b: Aabb, axis: usize) -> bool {
    match axis {
        0 => a.min.y < b.max.y && a.max.y > b.min.y && a.min.z < b.max.z && a.max.z > b.min.z,
        1 => a.min.x < b.max.x && a.max.x > b.min.x && a.min.z < b.max.z && a.max.z > b.min.z,
        _ => a.min.x < b.max.x && a.max.x > b.min.x && a.min.y < b.max.y && a.max.y > b.min.y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_floor(world: &mut World) {
        for x in -4..=4 {
            for z in -4..=4 {
                world.set_block(BlockPos::new(x, 0, z), BlockState(16)); // stone
            }
        }
    }

    #[test]
    fn gravity_lands_player_on_full_cube_floor() {
        let mut world = World::new();
        make_floor(&mut world);
        let mut player = PlayerPhysics::new(Vec3::new(0.5, 4.0, 0.5));

        let mut landed = false;
        for _ in 0..80 {
            let tick = player.tick(&world, PlayerInput::default());
            landed |= tick.landed;
            if player.on_ground { break; }
        }

        assert!(landed);
        assert!(player.on_ground);
        assert!((player.position.y - 1.0).abs() < 1.0e-9);
        assert!(player.motion.y < 0.0 && player.motion.y > -0.08);
    }

    #[test]
    fn jump_applies_vanilla_height_impulse() {
        let mut world = World::new();
        make_floor(&mut world);
        let mut player = PlayerPhysics::new(Vec3::new(0.5, 1.0, 0.5));
        player.on_ground = true;

        player.tick(&world, PlayerInput { jump: true, ..PlayerInput::default() });

        assert!(player.position.y > 1.0);
        assert!((player.position.y - 1.42).abs() < 1.0e-9);
        assert!(player.motion.y > 0.3);
        assert!(!player.on_ground);
    }

    #[test]
    fn player_lands_on_slab_and_snow_layer_at_the_correct_height() {
        let mut world = World::new();
        world.set_block(BlockPos::new(0, 0, 0), BlockState((44 << 4) | 0));
        let mut player = PlayerPhysics::new(Vec3::new(0.5, 3.0, 0.5));
        for _ in 0..80 {
            player.tick(&world, PlayerInput::default());
            if player.on_ground { break; }
        }
        assert!(player.on_ground);
        assert!((player.position.y - 0.5).abs() < 1.0e-9);

        let mut snow_world = World::new();
        snow_world.set_block(BlockPos::new(0, 0, 0), BlockState(78 << 4));
        let mut snow_player = PlayerPhysics::new(Vec3::new(0.5, 2.0, 0.5));
        for _ in 0..80 {
            snow_player.tick(&snow_world, PlayerInput::default());
            if snow_player.on_ground { break; }
        }
        assert!(snow_player.on_ground);
        assert!((snow_player.position.y - 0.125).abs() < 1.0e-9);
    }

    #[test]
    fn player_collides_with_cactus_inset_and_not_its_outer_empty_margin() {
        let mut world = World::new();
        world.set_block(BlockPos::new(1, 1, 0), BlockState(81 << 4));
        let mut player = PlayerPhysics::new(Vec3::new(0.4, 1.0, 0.5));
        player.on_ground = true;
        player.motion.x = 1.0;
        let tick = player.tick(&world, PlayerInput::default());
        assert!(tick.collided_horizontally);
        assert!((player.position.x - 0.7625).abs() < 1.0e-9);
    }
    #[test]
    fn full_cube_wall_stops_horizontal_motion() {
        let mut world = World::new();
        make_floor(&mut world);
        world.set_block(BlockPos::new(1, 1, 0), BlockState(16));
        world.set_block(BlockPos::new(1, 2, 0), BlockState(16));

        let mut player = PlayerPhysics::new(Vec3::new(0.5, 1.0, 0.5));
        player.on_ground = true;
        let mut collided = false;
        for _ in 0..40 {
            let tick = player.tick(&world, PlayerInput { strafe: 1.0, ..PlayerInput::default() });
            collided |= tick.collided_horizontally;
        }

        assert!(collided);
        assert!(player.position.x <= 0.700_000_001);
    }

    #[test]
    fn diagonal_input_is_normalized() {
        let mut world = World::new();
        make_floor(&mut world);
        let mut player = PlayerPhysics::new(Vec3::new(0.5, 1.0, 0.5));
        player.on_ground = true;

        player.tick(&world, PlayerInput { forward: 1.0, strafe: 1.0, ..PlayerInput::default() });

        let horizontal_speed = (player.motion.x * player.motion.x + player.motion.z * player.motion.z).sqrt();
        assert!(horizontal_speed <= 0.1 + 1.0e-9);
        assert!(horizontal_speed > 0.09);
    }
}
