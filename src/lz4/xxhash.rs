const PRIME1: u32 = 0x9e37_79b1;
const PRIME2: u32 = 0x85eb_ca77;
const PRIME3: u32 = 0xc2b2_ae3d;
const PRIME4: u32 = 0x27d4_eb2f;
const PRIME5: u32 = 0x1656_67b1;

/// Implements streaming XXH32 with seed zero, following
/// <https://github.com/Cyan4973/xxHash/blob/dev/doc/xxhash_spec.md>.
/// Length arithmetic deliberately wraps modulo 2^32; `striped` records whether
/// any full stripe was processed even when the total length wraps around.
#[derive(Clone)]
pub(super) struct Xxh32 {
    acc: [u32; 4],
    tail: [u8; 16],
    used: usize,
    length: u32,
    striped: bool,
}

fn le32(p: &[u8]) -> u32 {
    u32::from_le_bytes([p[0], p[1], p[2], p[3]])
}

impl Xxh32 {
    pub(super) fn new() -> Xxh32 {
        Xxh32 { acc: [0; 4], tail: [0; 16], used: 0, length: 0, striped: false }
    }

    fn stripe(&mut self, p: &[u8]) {
        if !self.striped {
            self.acc = [PRIME1.wrapping_add(PRIME2), PRIME2, 0, 0u32.wrapping_sub(PRIME1)];
            self.striped = true;
        }
        for (i, acc) in self.acc.iter_mut().enumerate() {
            *acc = acc.wrapping_add(le32(&p[i * 4..]).wrapping_mul(PRIME2)).rotate_left(13).wrapping_mul(PRIME1);
        }
    }

    pub(super) fn write(&mut self, mut p: &[u8]) {
        self.length = self.length.wrapping_add(p.len() as u32);
        if self.used > 0 {
            let n = p.len().min(16 - self.used);
            self.tail[self.used..self.used + n].copy_from_slice(&p[..n]);
            self.used += n;
            p = &p[n..];
            if self.used < 16 {
                return;
            }
            let tail = self.tail;
            self.stripe(&tail);
            self.used = 0;
        }
        while p.len() >= 16 {
            self.stripe(&p[..16]);
            p = &p[16..];
        }
        self.tail[..p.len()].copy_from_slice(p);
        self.used = p.len();
    }

    /// Returns the digest of everything written so far without finalizing the
    /// state.
    pub(super) fn sum32(&self) -> u32 {
        let mut sum = PRIME5;
        if self.striped {
            sum = self.acc[0]
                .rotate_left(1)
                .wrapping_add(self.acc[1].rotate_left(7))
                .wrapping_add(self.acc[2].rotate_left(12))
                .wrapping_add(self.acc[3].rotate_left(18));
        }
        sum = sum.wrapping_add(self.length);
        let mut p = &self.tail[..self.used];
        while p.len() >= 4 {
            sum = sum.wrapping_add(le32(p).wrapping_mul(PRIME3)).rotate_left(17).wrapping_mul(PRIME4);
            p = &p[4..];
        }
        for &c in p {
            sum = sum.wrapping_add(u32::from(c).wrapping_mul(PRIME5)).rotate_left(11).wrapping_mul(PRIME1);
        }
        sum ^= sum >> 15;
        sum = sum.wrapping_mul(PRIME2);
        sum ^= sum >> 13;
        sum = sum.wrapping_mul(PRIME3);
        sum ^ (sum >> 16)
    }
}

/// Returns the XXH32 digest of `p`.
pub(super) fn checksum(p: &[u8]) -> u32 {
    let mut h = Xxh32::new();
    h.write(p);
    h.sum32()
}

#[cfg(test)]
impl Xxh32 {
    pub(super) fn set_length(&mut self, length: u32) {
        self.length = length;
    }

    pub(super) fn length(&self) -> u32 {
        self.length
    }

    pub(super) fn striped(&self) -> bool {
        self.striped
    }

    pub(super) fn clear_striped(&mut self) {
        self.striped = false;
    }
}
