//! A fast, well-mixing hasher for the per-frame caches.

/// A folded-multiply hasher: each word is folded in with a 64×64→128-bit
/// multiply whose halves are xored together, so every output bit depends on
/// every input bit.
///
/// Every visible cell is hashed every frame to find its row's cached layout,
/// every glyph is hashed to find its bitmap, and the layout cache trusts the
/// 64-bit key alone. The standard hasher is SipHash,
/// whose resistance to crafted collisions buys nothing here and which showed
/// up in profiles at a few percent of a frame. This costs about the same as
/// FxHash but, unlike it, mixes well enough that accidental collisions stay
/// as unlikely as with SipHash.
pub struct FoldHasher(u64);

impl Default for FoldHasher {
    fn default() -> Self {
        FoldHasher(0x243f_6a88_85a3_08d3)
    }
}

impl std::hash::Hasher for FoldHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.write_u64(u64::from_le_bytes(c.try_into().unwrap()));
        }
        for &b in chunks.remainder() {
            self.write_u8(b);
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.write_u64(i as u64)
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.write_u64(i as u64)
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.write_u64(i as u64)
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        let x = (self.0 ^ i).wrapping_add(0x9e37_79b9_7f4a_7c15);
        let p = (x as u128).wrapping_mul(0xf135_7aea_2e62_a9c5);
        self.0 = (p as u64) ^ ((p >> 64) as u64);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64)
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

/// A `HashMap` keyed through [`FoldHasher`].
pub type FastMap<K, V> = std::collections::HashMap<K, V, std::hash::BuildHasherDefault<FoldHasher>>;
