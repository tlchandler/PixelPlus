//! Stateless, platform-independent hashing and noise.
//!
//! Effects must render bit-identical frames on the leader and on every
//! follower, possibly running different builds, so randomness comes from
//! explicit integer hashing (never `thread_rng` or `std`'s `DefaultHasher`).

/// SplitMix64 finaliser: a fast, well-distributed 64-bit mix.
#[inline]
pub(crate) fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Hash three integers.
#[inline]
pub(crate) fn hash3(a: u64, b: u64, c: u64) -> u64 {
    mix64(a ^ mix64(b ^ mix64(c)))
}

/// Uniform float in `[0, 1)` from a hash.
#[inline]
pub(crate) fn unit(h: u64) -> f32 {
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// FNV-1a hash of a string (stable across platforms and versions).
pub(crate) fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

#[inline]
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn lattice(seed: u64, x: i64, y: i64, z: i64) -> f32 {
    unit(hash3(seed ^ x as u64, y as u64, z as u64))
}

/// Smooth 3-D value noise in `[0, 1]`.
pub(crate) fn value_noise3(seed: u64, x: f64, y: f64, z: f64) -> f32 {
    let (xf, yf, zf) = (x.floor(), y.floor(), z.floor());
    let (xi, yi, zi) = (xf as i64, yf as i64, zf as i64);
    let (tx, ty, tz) = (
        smooth((x - xf) as f32),
        smooth((y - yf) as f32),
        smooth((z - zf) as f32),
    );
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let mut layer = [0f32; 2];
    for (dz, out) in layer.iter_mut().enumerate() {
        let z = zi + dz as i64;
        let a = lerp(lattice(seed, xi, yi, z), lattice(seed, xi + 1, yi, z), tx);
        let b = lerp(
            lattice(seed, xi, yi + 1, z),
            lattice(seed, xi + 1, yi + 1, z),
            tx,
        );
        *out = lerp(a, b, ty);
    }
    lerp(layer[0], layer[1], tz)
}

/// Two-octave fractal noise in `[0, 1]`.
pub(crate) fn fbm3(seed: u64, x: f64, y: f64, z: f64) -> f32 {
    (value_noise3(seed, x, y, z) * 2.0 + value_noise3(seed ^ 0x5A5A, x * 2.03, y * 2.03, z * 1.7))
        / 3.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashing_is_stable() {
        // Pinned values: changing these would desynchronise mixed-version clusters.
        assert_eq!(fnv1a(""), 0xCBF2_9CE4_8422_2325);
        assert_eq!(fnv1a("a"), 0xAF63_DC4C_8601_EC8C);
        assert_eq!(mix64(0), 0xE220_A839_7B1D_CDAF);
    }

    #[test]
    fn unit_and_noise_ranges() {
        for i in 0..1000u64 {
            let u = unit(mix64(i));
            assert!((0.0..1.0).contains(&u));
            let n = value_noise3(7, i as f64 * 0.37, i as f64 * -0.11, 3.3);
            assert!((0.0..=1.0).contains(&n));
            let f = fbm3(7, i as f64 * 0.37, 1.0, -2.0);
            assert!((0.0..=1.0).contains(&f));
        }
    }

    #[test]
    fn noise_is_continuous() {
        let a = value_noise3(1, 3.999_999, 2.5, 1.0);
        let b = value_noise3(1, 4.000_001, 2.5, 1.0);
        assert!((a - b).abs() < 1e-3);
    }
}
