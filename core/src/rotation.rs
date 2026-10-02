//! Discrete counterclockwise rotations in world coordinates (positive Y is up).
use bevy_math::Vec2;

pub const ROTATION_SHIFT: u32 = 9;
pub const ROTATION_MASK: u32 = 0b11 << ROTATION_SHIFT;

#[inline]
pub const fn decode_rotation(flags: u32) -> u32 {
    (flags & ROTATION_MASK) >> ROTATION_SHIFT
}

#[inline]
pub const fn with_rotation(flags: u32, rotation: u32) -> u32 {
    (flags & !ROTATION_MASK) | ((rotation & 3) << ROTATION_SHIFT)
}

#[inline]
pub const fn add_quarter_turns(rotation: u32, quarter_turns: i8) -> u32 {
    ((rotation & 3) as i32 + quarter_turns as i32).rem_euclid(4) as u32
}

#[inline]
pub fn rotate_quarter(v: Vec2, rotation: u32) -> Vec2 {
    match rotation & 3 {
        0 => v,
        1 => Vec2::new(-v.y, v.x),
        2 => -v,
        3 => Vec2::new(v.y, -v.x),
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_preserve_canonical_edges_and_turns_wrap() {
        for flags in [0, 0x1ff, u32::MAX] {
            for rotation in 0..4 {
                let encoded = with_rotation(flags, rotation);
                assert_eq!(decode_rotation(encoded), rotation);
                assert_eq!(encoded & !ROTATION_MASK, flags & !ROTATION_MASK);
                for turns in i8::MIN..=i8::MAX {
                    assert_eq!(
                        add_quarter_turns(rotation, turns),
                        (rotation as i32 + i32::from(turns)).rem_euclid(4) as u32
                    );
                }
            }
        }
        let v = Vec2::new(3.25, -7.5);
        assert_eq!(rotate_quarter(v, 1), Vec2::new(7.5, 3.25));
        assert_eq!(rotate_quarter(rotate_quarter(v, 1), 3), v);
    }
}
