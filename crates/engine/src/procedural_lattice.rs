//! Original ahr integer-coordinate lattice and r7 negative-seed hash.
//! General seed/3D/hash output goldens remain uncharacterized; this is the
//! qualified body translation, not Perlin/simplex or a random-provider facade.
mod fractional_power;
use crate::{ImageError, seeded_random::SeededRandom};

pub struct SeededLattice {
    samples: Vec<f32>,
}
impl SeededLattice {
    pub fn new(seed: i32) -> Result<Self, ImageError> {
        // ahr allocates its float[256] BEFORE constructing awm(seed).
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(256)
            .map_err(|_| ImageError::Allocation)?;
        samples.resize(256, 0.0);
        let mut random = SeededRandom::from_seed(seed).map_err(|_| ImageError::Overflow)?;
        for value in &mut samples {
            *value = random.sample() as f32;
        }
        Ok(Self { samples })
    }
    pub fn sample2(&self, x: i32, y: i32) -> f32 {
        let row = i32::from(PERMUTATION[(y & 255) as usize]);
        self.samples[usize::from(PERMUTATION[(x.wrapping_add(row) & 255) as usize])]
    }
    pub fn sample3(&self, x: i32, y: i32, z: i32) -> f32 {
        let depth = i32::from(PERMUTATION[(z & 255) as usize]);
        let row = i32::from(PERMUTATION[(y.wrapping_add(depth) & 255) as usize]);
        self.samples[usize::from(PERMUTATION[(x.wrapping_add(row) & 255) as usize])]
    }
    pub(crate) fn sample4(&self, x: i32, y: i32, z: i32, w: i32) -> f32 {
        let fourth = i32::from(PERMUTATION[(w & 255) as usize]);
        self.sample3(x, y, z.wrapping_add(fourth))
    }
}
/// r7.a(float,float): Single scaling and negative correction precede Double
/// remainder and power. The fractional part exposes the low bits of the
/// reference runtime's power calculation even when it differs by only one ulp.
pub fn coordinate_hash(x: f32, y: f32) -> f32 {
    let mut x = x * 1000.0f32;
    let mut y = y * 1000.0f32;
    if x < 0.0 {
        x = -x + f32::from_bits(0x3dfcd680);
    }
    if y < 0.0 {
        y = -y + f32::from_bits(0x3dfcd680);
    }
    let x = f64::from(x);
    let y = f64::from(y);
    let a = x % 5000.0;
    let b = y % 5000.0;
    let c = x / 6_000_000.0;
    let d = y / 6_000_000.0;
    let e = 2.1 + (b % 1000.0) / 3000.0 + c;
    let f = 2.2 + (a % 1000.0) / 4000.0 + d;
    let p = fractional_power::pow(a, e) % 1.0;
    let q = fractional_power::pow(b, f) % 1.0;
    let mut value = p + q;
    if value >= 1.0 {
        value -= 1.0;
    }
    value as f32
}

// One exact ahr FieldRVA permutation shared by procedural kernels and preview.
pub(crate) const PERMUTATION: [u8; 256] = [
    225, 155, 210, 108, 175, 199, 221, 144, 203, 116, 70, 213, 69, 158, 33, 252, 5, 82, 173, 133,
    222, 139, 174, 27, 9, 71, 90, 246, 75, 130, 91, 191, 169, 138, 2, 151, 194, 235, 81, 7, 25,
    113, 228, 159, 205, 253, 134, 142, 248, 65, 224, 217, 22, 121, 229, 63, 89, 103, 96, 104, 156,
    17, 201, 129, 36, 8, 165, 110, 237, 117, 231, 56, 132, 211, 152, 20, 181, 111, 239, 218, 170,
    163, 51, 172, 157, 47, 80, 212, 176, 250, 87, 49, 99, 242, 136, 189, 162, 115, 44, 43, 124, 94,
    150, 16, 141, 247, 32, 10, 198, 223, 255, 72, 53, 131, 84, 57, 220, 197, 58, 50, 208, 11, 241,
    28, 3, 192, 62, 202, 18, 215, 153, 24, 76, 41, 15, 179, 39, 46, 55, 6, 128, 167, 23, 188, 106,
    34, 187, 140, 164, 73, 112, 182, 244, 195, 227, 13, 35, 77, 196, 185, 26, 200, 226, 119, 31,
    123, 168, 125, 249, 68, 183, 230, 177, 135, 160, 180, 12, 1, 243, 148, 102, 166, 38, 238, 251,
    37, 240, 126, 64, 74, 161, 40, 184, 149, 171, 178, 101, 66, 29, 59, 146, 61, 254, 107, 42, 86,
    154, 4, 236, 232, 120, 21, 233, 209, 45, 98, 193, 114, 78, 19, 206, 14, 118, 127, 48, 79, 147,
    85, 30, 207, 219, 54, 88, 234, 190, 122, 95, 67, 143, 109, 137, 214, 145, 93, 92, 100, 245, 0,
    216, 186, 60, 83, 105, 97, 204, 52,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_wall_coordinate_hash_retains_the_original_power_low_bit() {
        let value = coordinate_hash(453.0 / 100.0, -16.0 / 100.0);
        assert_eq!(value.to_bits(), 0x3e20f83a);
    }

    #[test]
    fn lattice_retains_seed_magnitude_and_masked_integer_coordinates() {
        let a = SeededLattice::new(17).unwrap();
        let b = SeededLattice::new(-17).unwrap();
        for x in [-257, -1, 0, 1, 255, 256, i32::MAX] {
            for y in [-1, 0, 7, 256] {
                assert_eq!(a.sample2(x, y).to_bits(), b.sample2(x, y).to_bits());
                assert_eq!(
                    a.sample2(x, y).to_bits(),
                    a.sample2(x.wrapping_add(256), y).to_bits()
                );
                assert_eq!(
                    a.sample3(x, y, -9).to_bits(),
                    a.sample3(x, y, 247).to_bits()
                );
            }
        }
        assert!(matches!(
            SeededLattice::new(i32::MIN),
            Err(ImageError::Overflow)
        ));
        assert_eq!(coordinate_hash(0.0, 0.0).to_bits(), 0);
    }
}
