//! Streaming FNV-1a 64 hash shared by the benchmark tools.

pub struct Fnv1a(u64);

impl Fnv1a {
    pub fn new() -> Self {
        Self(14695981039346656037)
    }

    pub fn write(&mut self, data: &[u8]) {
        for &byte in data {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(1099511628211);
        }
    }

    /// Writes the little-endian bytes of a match length and then its offset,
    /// each as a u64.
    pub fn write_match(&mut self, length: usize, offset: usize) {
        for v in [length as u64, offset as u64] {
            self.write(&v.to_le_bytes());
        }
    }

    pub fn sum(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values() {
        let mut hash = Fnv1a::new();
        assert_eq!(hash.sum(), 14695981039346656037);
        hash.write(b"a");
        assert_eq!(hash.sum(), 0xaf63_dc4c_8601_ec8c);
    }

    /// Checks `write_match` against FNV-1a 64 computed independently over the
    /// little-endian bytes of 3 and then 7.
    #[test]
    fn write_match() {
        let mut hash = Fnv1a::new();
        hash.write_match(3, 7);
        assert_eq!(hash.sum(), 0x3c3852543d680a01);
    }
}
