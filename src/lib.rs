//! Bitter takes a slice of byte data and reads little-endian bits platform agonistically.
//!
//! There are two main APIs available: checked and unchecked functions. A checked function will
//! return a `Option` that will be `None` if there is not enough bits left in the stream.
//! Unchecked functions, which are denoted by having "unchecked" in their name, will exhibit
//! undefined behavior if there is not enough data left, but happen to be 2x faster (your numbers
//! will vary depending on use case).
//!
//! Tips:
//!
//! - Prefer checked functions for all but the most performance critical code
//! - Group all unchecked functions in a single block guarded by a `has_bits_remaining` call
//!
//! ## Example
//!
//! ```rust
//! use bitter::BitGet;
//! let mut bitter = BitGet::new(&[0xff, 0x04]);
//! assert_eq!(bitter.read_bit(), Some(true));
//! assert_eq!(bitter.read_u8(), Some(0x7f));
//! assert_eq!(bitter.read_u32_bits(7), Some(0x02));
//! ```
//!
//! Below, is a demonstration of guarding against potential panics:
//!
//! ```rust
//! # use bitter::BitGet;
//! let mut bitter = BitGet::new(&[0xff, 0x04]);
//! if bitter.has_bits_remaining(16) {
//!     assert_eq!(bitter.read_bit_unchecked(), true);
//!     assert_eq!(bitter.read_u8_unchecked(), 0x7f);
//!     assert_eq!(bitter.read_u32_bits_unchecked(7), 0x02);
//! }
//! # else {
//! #   panic!("Expected bytes")
//! # }
//! ```
//!
//! Another guard usage:
//!
//! ```rust
//! # use bitter::BitGet;
//! let mut bitter = BitGet::new(&[0xff, 0x04]);
//! if bitter.has_bits_remaining(16) {
//!     for _ in 0..8 {
//!         assert_eq!(bitter.read_bit_unchecked(), true);
//!     }
//!     assert_eq!(bitter.read_u8_unchecked(), 0x04);
//! }
//! # else {
//! #   panic!("Expected bytes")
//! # }
//! ```

use std::borrow::Cow;

/// Reads little-endian bits platform agonistically from a slice of byte data.
pub struct BitGet<'a> {
    /// The bit position in `current` that we are at
    pos: usize,

    /// Byte stream of data we are parsing
    data: &'a [u8],

    /// The eight bytes that current is pointing at
    current_val: u64,
}

macro_rules! gen_read {
    ($name:ident, $t:ty, $which:ident) => (
    #[inline]
    pub fn $name(&mut self) -> Option<$t> {
        if self.has_bits_remaining(::std::mem::size_of::<$t>() * 8) {
            Some(self.$which())
        } else {
            None
        }
   });
}

macro_rules! gen_read_unchecked {
    ($name:ident, $t:ty) => (
    #[inline]
    pub fn $name(&mut self) -> $t {
        let bts = ::std::mem::size_of::<$t>() * 8;
        if self.pos < BIT_WIDTH - bts {
            let res = (self.current_val >> self.pos) as $t;
            self.pos += bts;
            res
        } else {
            let little = (self.current_val >> self.pos) as $t;
            self.data = &self.data[BYTE_WIDTH..];
            self.current_val = self.read();
            let left = bts - (BIT_WIDTH - self.pos);
            let big = (self.current_val << (bts - left)) as $t;
            self.pos = left;
            little + big
        }
   });
}

const BYTE_WIDTH: usize = ::std::mem::size_of::<u64>();
const BIT_WIDTH: usize = BYTE_WIDTH * 8;

impl<'a> BitGet<'a> {
    pub fn new(data: &'a [u8]) -> BitGet<'a> {
        let mut res = BitGet {
            pos: 0,
            current_val: 0,
            data,
        };

        res.current_val = res.read();
        res
    }

    gen_read!(read_u8, u8, read_u8_unchecked);
    gen_read!(read_i8, i8, read_i8_unchecked);
    gen_read!(read_u16, u16, read_u16_unchecked);
    gen_read!(read_i16, i16, read_i16_unchecked);
    gen_read!(read_u32, u32, read_u32_unchecked);
    gen_read!(read_i32, i32, read_i32_unchecked);
    gen_read!(read_u64, u64, read_u64_unchecked);
    gen_read!(read_i64, i64, read_i64_unchecked);

    gen_read_unchecked!(read_u8_unchecked, u8);
    gen_read_unchecked!(read_i8_unchecked, i8);
    gen_read_unchecked!(read_u16_unchecked, u16);
    gen_read_unchecked!(read_i16_unchecked, i16);
    gen_read_unchecked!(read_u32_unchecked, u32);
    gen_read_unchecked!(read_i32_unchecked, i32);

    /// Generates the bitmask for 32-bit integers (yes, even though the return type is u64)
    #[inline]
    fn bit_mask(bits: usize) -> u64 {
        debug_assert!(bits < ::std::mem::size_of::<u64>() * 8);
        (1 << bits) - 1
    }

    /// Determines if our bit position is not byte aligned
    #[inline]
    fn is_mid_byte(&self) -> bool {
        // 0x38 = 32 | 24 | 16 | 8
        self.pos & !0x38 != 0
    }

    /// Assuming that `self.data` is pointing to data not yet seen. slurp up 8 bytes or the
    /// rest of the data (whatever is smaller)
    ///
    /// Clippy lint can be unsuppressed once Clippy recognizes this pattern as correct.
    /// https://github.com/rust-lang/rust-clippy/issues/2881
    #[allow(clippy::cast_ptr_alignment)]
    #[inline]
    fn read(&mut self) -> u64 {
        unsafe {
            if self.data.len() > BYTE_WIDTH {
                ::std::ptr::read_unaligned(self.data.as_ptr() as *const u8 as *const u64).to_le()
            } else {
                let mut data: u64 = 0;
                let len = self.data.len();
                ::std::ptr::copy_nonoverlapping(
                    self.data.as_ptr(),
                    &mut data as *mut u64 as *mut u8,
                    len,
                );
                data.to_le()
            }
        }
    }

    #[inline]
    pub fn read_u64_unchecked(&mut self) -> u64 {
        // While reading 64bit we can take advantage of an optimization trick not available to
        // other reads as 64bits is the same as our cache size
        let bts = ::std::mem::size_of::<u64>() * 8;
        let little = self.current_val >> self.pos;
        self.data = &self.data[BYTE_WIDTH..];
        self.current_val = self.read();
        if self.pos == 0 {
            little
        } else {
            let big = self.current_val << (bts - self.pos);
            little + big
        }
    }

    #[inline]
    pub fn read_i64_unchecked(&mut self) -> i64 {
        self.read_u64_unchecked() as i64
    }

    #[inline]
    pub fn read_u32_bits_unchecked(&mut self, bits: i32) -> u32 {
        let bts = bits as usize;
        let new_pos = self.pos + bts;
        if new_pos < BIT_WIDTH {
            let res = (self.current_val >> self.pos) & BitGet::bit_mask(bts);
            self.pos = new_pos;
            res as u32
        } else {
            let little = self.current_val >> self.pos;
            self.data = &self.data[BYTE_WIDTH..];
            self.current_val = self.read();
            let left = new_pos - BIT_WIDTH;
            let big = (self.current_val & BitGet::bit_mask(left)) << (bts - left);
            self.pos = left;
            (little + big) as u32
        }
    }

    #[inline]
    pub fn read_u32_bits(&mut self, bits: i32) -> Option<u32> {
        if self.has_bits_remaining(bits as usize) {
            Some(self.read_u32_bits_unchecked(bits))
        } else {
            None
        }
    }

    #[inline]
    pub fn read_i32_bits_unchecked(&mut self, bits: i32) -> i32 {
        self.read_u32_bits_unchecked(bits) as i32
    }

    #[inline]
    pub fn read_i32_bits(&mut self, bits: i32) -> Option<i32> {
        self.read_u32_bits(bits).map(|x| x as i32)
    }

    /// Return approximately how many bytes are left in the bitstream. This can overestimate by one
    /// when the bit position is non-zero. Thus it is recommended to always compare with a greater
    /// than sign to avoid undefined behavior with unchecked reads
    ///
    /// ```rust
    /// # use bitter::BitGet;
    /// let mut bitter = BitGet::new(&[0xff]);
    /// assert_eq!(bitter.approx_bytes_remaining(), 1);
    /// assert!(bitter.read_bit().is_some());
    /// assert_eq!(bitter.approx_bytes_remaining(), 1);
    /// assert!(bitter.read_u32_bits(7).is_some());
    /// assert_eq!(bitter.approx_bytes_remaining(), 0);
    /// ```
    #[inline]
    pub fn approx_bytes_remaining(&self) -> usize {
        self.data.len() - (self.pos >> 3)
    }

    /// Returns the exact number of bits remaining in the bitstream if the number of bits can fit
    /// within a `usize`. For large byte slices, the calculating the number of bits can cause an
    /// overflow, hence an `Option` is returned. See `has_bits_remaining` for a more performant and
    /// ergonomic alternative.
    ///
    /// ```rust
    /// # use bitter::BitGet;
    /// let mut bitter = BitGet::new(&[0xff]);
    /// assert_eq!(bitter.bits_remaining(), Some(8));
    /// assert!(bitter.read_bit().is_some());
    /// assert_eq!(bitter.bits_remaining(), Some(7));
    /// assert!(bitter.read_u32_bits(7).is_some());
    /// assert_eq!(bitter.bits_remaining(), Some(0));
    /// ```
    #[inline]
    pub fn bits_remaining(&self) -> Option<usize> {
        self.approx_bytes_remaining()
            .checked_mul(8)
            .map(|x| x - (self.pos & 0x7) as usize)
    }

    /// Returns true if at least `bits` number of bits are left in the stream. A more performant
    /// and ergonomic way than `bits_remaining`.
    ///
    /// ```rust
    /// # use bitter::BitGet;
    /// let mut bitter = BitGet::new(&[0xff]);
    /// assert!(bitter.has_bits_remaining(7));
    /// assert!(bitter.has_bits_remaining(8));
    /// assert!(!bitter.has_bits_remaining(9));
    ///
    /// assert!(bitter.read_bit().is_some());
    /// assert!(bitter.has_bits_remaining(7));
    /// assert!(!bitter.has_bits_remaining(8));
    ///
    /// assert!(bitter.read_u32_bits(7).is_some());
    /// assert!(!bitter.has_bits_remaining(7));
    /// assert!(bitter.has_bits_remaining(0));
    /// ```
    #[inline]
    pub fn has_bits_remaining(&self, bits: usize) -> bool {
        if self.data.len() > bits + BIT_WIDTH {
            true
        } else {
            let bytes_requested = bits >> 3;
            if self.data.len() > bytes_requested + BYTE_WIDTH {
                true
            } else {
                let pos_bytes = self.pos >> 3;
                let diff = self.data.len();
                if !self.is_mid_byte() {
                    let bytes_left = diff - pos_bytes;
                    if bytes_left == bytes_requested {
                        bits & 0x7 == 0
                    } else {
                        bytes_left > bytes_requested
                    }
                } else {
                    let whole_bytes_left = diff - pos_bytes - 1;
                    if whole_bytes_left == bytes_requested {
                        (self.pos & 0x7) <= (8 - (bits & 0x7))
                    } else {
                        whole_bytes_left > bytes_requested
                    }
                }
            }
        }
    }

    /// Returns if the bitstream has no bits left
    ///
    /// ```rust
    /// # use bitter::BitGet;
    /// let mut bitter = BitGet::new(&[0b1010_1010, 0b0101_0101]);
    /// assert_eq!(bitter.is_empty(), false);
    /// assert_eq!(bitter.read_u16_unchecked(), 0b0101_0101_1010_1010);
    /// assert_eq!(bitter.is_empty(), true);
    /// ```
    pub fn is_empty(&self) -> bool {
        self.approx_bytes_remaining() == 0
    }

    /// Reads a bit from the bitstream if available
    ///
    /// ```rust
    /// # use bitter::BitGet;
    /// let mut bitter = BitGet::new(&[0b1010_1010, 0b0101_0101]);
    /// assert_eq!(bitter.read_bit(), Some(false));
    /// ```
    #[inline]
    pub fn read_bit(&mut self) -> Option<bool> {
        self.read_u32_bits(1).map(|x| x != 0)
    }

    /// *Assumes* there is at least one bit left in the stream.
    ///
    /// ```rust
    /// # use bitter::BitGet;
    /// let mut bitter = BitGet::new(&[0b1010_1010, 0b0101_0101]);
    /// assert_eq!(bitter.read_bit_unchecked(), false);
    /// ```
    #[inline]
    pub fn read_bit_unchecked(&mut self) -> bool {
        self.read_u32_bits_unchecked(1) & 1 != 0
    }

    /// Reads a `f32` from the bitstream if available. The standard IEEE-754 binary layout float is
    /// used.
    #[inline]
    pub fn read_f32(&mut self) -> Option<f32> {
        self.read_u32().map(f32::from_bits)
    }

    /// Reads a `f32` from the bitstream. The standard IEEE-754 binary layout float is used.
    #[inline]
    pub fn read_f32_unchecked(&mut self) -> f32 {
        f32::from_bits(self.read_u32_unchecked())
    }

    /// If the next bit is available and on, decode the next chunk of data (which can return None).
    /// The return value can be one of the following:
    ///
    /// - None: Not enough data was available
    /// - Some(None): Bit was off so data not decoded
    /// - Some(x): Bit was on and data was decoded
    ///
    /// ```rust
    /// # use bitter::BitGet;
    /// let mut bitter = BitGet::new(&[0xff, 0x04]);
    /// assert_eq!(bitter.if_get(BitGet::read_u8), Some(Some(0x7f)));
    /// assert_eq!(bitter.if_get(BitGet::read_u8), Some(None));
    /// assert_eq!(bitter.if_get(BitGet::read_u8), None);
    /// ```
    #[cfg_attr(feature = "cargo-clippy", allow(clippy::option_option))]
    pub fn if_get<T, F>(&mut self, mut f: F) -> Option<Option<T>>
    where
        F: FnMut(&mut Self) -> Option<T>,
    {
        self.read_bit()
            .and_then(|bit| if bit { f(self).map(Some) } else { Some(None) })
    }

    /// If the next bit is available and on, decode the next chunk of data.  The return value can
    /// be one of the following:
    ///
    /// - Some(None): Bit was off so data not decoded
    /// - Some(x): Bit was on and data was decoded
    ///
    /// ```rust
    /// # use bitter::BitGet;
    /// let mut bitter = BitGet::new(&[0xff, 0x04]);
    /// assert_eq!(bitter.if_get_unchecked(BitGet::read_u8_unchecked), Some(0x7f));
    /// assert_eq!(bitter.if_get_unchecked(BitGet::read_u8_unchecked), None);
    /// ```
    /// # Panics
    ///
    /// Will panic if no data is left for the bit or for the data to be decoded.
    pub fn if_get_unchecked<T, F>(&mut self, mut f: F) -> Option<T>
    where
        F: FnMut(&mut Self) -> T,
    {
        if self.read_bit_unchecked() {
            Some(f(self))
        } else {
            None
        }
    }

    /// If the number of requested bytes are available return them to the client. Since the current
    /// bit position may not be byte aligned, all bytes requested may need to be shifted
    /// appropriately.
    ///
    /// ```rust
    /// # use bitter::BitGet;
    /// let mut bitter = BitGet::new(&[0b1010_1010, 0b0101_0101]);
    /// assert_eq!(bitter.read_bit_unchecked(), false);
    /// assert_eq!(bitter.read_bytes(1).map(|x| x[0]), Some(0b1101_0101));
    /// ```
    pub fn read_bytes(&mut self, bytes: i32) -> Option<Cow<[u8]>> {
        let bts = bytes as usize;
        let pos_bytes = self.pos >> 3;
        let bytes_left = self.data.len() - pos_bytes;
        if !self.is_mid_byte() && bytes_left >= bts {
            let end = pos_bytes + bts;
            let res = Some(Cow::Borrowed(&self.data[pos_bytes..end]));

            self.data = &self.data[end..];
            self.current_val = self.read();
            self.pos = 0;
            res
        } else if bytes_left > bts {
            let mut res = Vec::with_capacity(bts);
            for _ in 0..bytes {
                res.push(self.read_u8_unchecked());
            }
            Some(Cow::Owned(res))
        } else {
            None
        }
    }

    /// Reads a value that takes up at most `bits` bits and doesn't exceed `max`. This function
    /// *assumes* that `max` has the same bitwidth as `bits`. It doesn't make sense to call this
    /// function `bits = 8` and `max = 30`, you'd change your argument to `bits = 5`. If `bits` are
    /// not available return `None`
    ///
    /// ```rust
    /// # use bitter::BitGet;
    /// // Reads 5 bits or stops if the 5th bit would send the accumulator over 20
    /// let mut bitter = BitGet::new(&[0b1111_1000]);
    /// assert_eq!(bitter.read_bits_max(5, 20), Some(8));
    /// ```
    #[inline]
    pub fn read_bits_max(&mut self, bits: i32, max: i32) -> Option<u32> {
        self.read_u32_bits(bits - 1).and_then(|data| {
            let max = max as u32;
            let up = data + (1 << (bits - 1));
            if up >= max {
                Some(data)
            } else {
                // Check the next bit
                self.read_bit().map(|x| if x { up } else { data })
            }
        })
    }

    /// Reads a value that takes up at most `bits` bits and doesn't exceed `max`. This function
    /// *assumes* that `max` has the same bitwidth as `bits`. It doesn't make sense to call this
    /// function `bits = 8` and `max = 30`, you'd change your argument to `bits = 5`
    ///
    /// ```rust
    /// # use bitter::BitGet;
    /// // Reads 5 bits or stops if the 5th bit would send the accumulator over 20
    /// let mut bitter = BitGet::new(&[0b1111_1000]);
    /// assert_eq!(bitter.read_bits_max(5, 20), Some(8));
    /// ```
    #[inline]
    pub fn read_bits_max_unchecked(&mut self, bits: i32, max: i32) -> u32 {
        let data = self.read_u32_bits_unchecked(bits - 1);
        let max = max as u32;

        // If the next bit is on, what would our value be
        let up = data + (1 << (bits - 1));

        // If we have the potential to equal or exceed max don't read the next bit, else read the
        // next bit
        if up >= max || !self.read_bit_unchecked() {
            data
        } else {
            up
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BitGet;

    #[test]
    fn test_bit_reads() {
        let mut bitter = BitGet::new(&[0b1010_1010, 0b0101_0101]);
        assert_eq!(bitter.bits_remaining(), Some(16));
        assert_eq!(bitter.read_bit().unwrap(), false);
        assert_eq!(bitter.bits_remaining(), Some(15));
        assert_eq!(bitter.read_bit().unwrap(), true);
        assert_eq!(bitter.read_bit().unwrap(), false);
        assert_eq!(bitter.read_bit().unwrap(), true);
        assert_eq!(bitter.read_bit().unwrap(), false);
        assert_eq!(bitter.read_bit().unwrap(), true);
        assert_eq!(bitter.read_bit().unwrap(), false);
        assert_eq!(bitter.read_bit().unwrap(), true);

        assert_eq!(bitter.read_bit().unwrap(), true);
        assert_eq!(bitter.read_bit().unwrap(), false);
        assert_eq!(bitter.read_bit().unwrap(), true);
        assert_eq!(bitter.read_bit().unwrap(), false);
        assert_eq!(bitter.read_bit().unwrap(), true);
        assert_eq!(bitter.read_bit().unwrap(), false);
        assert_eq!(bitter.read_bit().unwrap(), true);
        assert_eq!(bitter.read_bit().unwrap(), false);

        assert_eq!(bitter.read_bit(), None);
    }

    #[test]
    fn test_bit_unchecked_reads() {
        let mut bitter = BitGet::new(&[0b1010_1010, 0b0101_0101]);
        assert_eq!(bitter.read_bit_unchecked(), false);
        assert_eq!(bitter.read_bit_unchecked(), true);
        assert_eq!(bitter.read_bit_unchecked(), false);
        assert_eq!(bitter.read_bit_unchecked(), true);
        assert_eq!(bitter.read_bit_unchecked(), false);
        assert_eq!(bitter.read_bit_unchecked(), true);
        assert_eq!(bitter.read_bit_unchecked(), false);
        assert_eq!(bitter.read_bit_unchecked(), true);

        assert_eq!(bitter.read_bit_unchecked(), true);
        assert_eq!(bitter.read_bit_unchecked(), false);
        assert_eq!(bitter.read_bit_unchecked(), true);
        assert_eq!(bitter.read_bit_unchecked(), false);
        assert_eq!(bitter.read_bit_unchecked(), true);
        assert_eq!(bitter.read_bit_unchecked(), false);
        assert_eq!(bitter.read_bit_unchecked(), true);
        assert_eq!(bitter.read_bit_unchecked(), false);

        assert_eq!(bitter.read_bit(), None);
    }

    #[test]
    fn test_bit_unchecked_bits_reads() {
        let mut bitter = BitGet::new(&[0b1010_1010, 0b0101_0101]);
        assert_eq!(bitter.read_u32_bits_unchecked(1), 0);
        assert_eq!(bitter.read_u32_bits_unchecked(1), 1);
        assert_eq!(bitter.read_u32_bits_unchecked(1), 0);
        assert_eq!(bitter.read_u32_bits_unchecked(1), 1);
        assert_eq!(bitter.read_u32_bits_unchecked(1), 0);
        assert_eq!(bitter.read_u32_bits_unchecked(1), 1);
        assert_eq!(bitter.read_u32_bits_unchecked(1), 0);
        assert_eq!(bitter.read_u32_bits_unchecked(1), 1);

        assert_eq!(bitter.read_u32_bits_unchecked(1), 1);
        assert_eq!(bitter.read_u32_bits_unchecked(1), 0);
        assert_eq!(bitter.read_u32_bits_unchecked(1), 1);
        assert_eq!(bitter.read_u32_bits_unchecked(1), 0);
        assert_eq!(bitter.read_u32_bits_unchecked(1), 1);
        assert_eq!(bitter.read_u32_bits_unchecked(1), 0);
        assert_eq!(bitter.read_u32_bits_unchecked(1), 1);
        assert_eq!(bitter.read_u32_bits_unchecked(1), 0);

        assert_eq!(bitter.read_bit(), None);
    }

    #[test]
    fn test_has_remaining_bits() {
        let mut bitter = BitGet::new(&[0xff, 0xff]);
        assert!(bitter.has_bits_remaining(7));
        assert!(bitter.has_bits_remaining(8));
        assert!(bitter.has_bits_remaining(9));

        assert!(bitter.read_u32_bits(9).is_some());
        assert!(bitter.has_bits_remaining(7));
        assert!(!bitter.has_bits_remaining(8));
        assert!(!bitter.has_bits_remaining(9));

        assert!(bitter.read_u32_bits(7).is_some());
        assert!(!bitter.has_bits_remaining(7));
        assert!(bitter.has_bits_remaining(0));
    }

    #[test]
    fn test_has_remaining_bits2() {
        let mut bitter = BitGet::new(&[0xff, 0xff, 0xff, 0xff]);
        assert!(bitter.read_u32_bits(31).is_some());
        assert!(!bitter.has_bits_remaining(2));
        assert!(bitter.has_bits_remaining(1));
    }

    #[test]
    fn test_16_bits_reads() {
        let mut bitter = BitGet::new(&[0b1010_1010, 0b0101_0101]);
        assert_eq!(bitter.read_u32_bits(16), Some(0b0101_0101_1010_1010));
    }

    #[test]
    fn test_bit_bits_reads() {
        let mut bitter = BitGet::new(&[0b1010_1010, 0b0101_0101]);
        assert_eq!(bitter.read_u32_bits(1), Some(0));
        assert_eq!(bitter.read_u32_bits(1), Some(1));
        assert_eq!(bitter.read_u32_bits(1), Some(0));
        assert_eq!(bitter.read_u32_bits(1), Some(1));
        assert_eq!(bitter.read_u32_bits(1), Some(0));
        assert_eq!(bitter.read_u32_bits(1), Some(1));
        assert_eq!(bitter.read_u32_bits(1), Some(0));
        assert_eq!(bitter.read_u32_bits(1), Some(1));
        assert_eq!(bitter.read_u32_bits(1), Some(1));
        assert_eq!(bitter.read_u32_bits(1), Some(0));
        assert_eq!(bitter.read_u32_bits(1), Some(1));
        assert_eq!(bitter.read_u32_bits(1), Some(0));
        assert_eq!(bitter.read_u32_bits(1), Some(1));
        assert_eq!(bitter.read_u32_bits(1), Some(0));
        assert_eq!(bitter.read_u32_bits(1), Some(1));
        assert_eq!(bitter.read_u32_bits(1), Some(0));

        assert_eq!(bitter.read_u32_bits(1), None);
    }

    #[test]
    fn test_read_bytes() {
        let mut bitter = BitGet::new(&[0b1010_1010, 0b0101_0101]);
        assert_eq!(
            bitter.read_bytes(2).map(|x| x.into_owned()),
            Some(vec![0b1010_1010, 0b0101_0101])
        );

        let mut bitter = BitGet::new(&[0b1010_1010, 0b0101_0101]);
        assert_eq!(bitter.read_bit_unchecked(), false);
        assert_eq!(bitter.read_bytes(2), None);
        assert_eq!(
            bitter.read_bytes(1).map(|x| x.into_owned()),
            Some(vec![0b1101_0101])
        );
    }

    #[test]
    fn test_read_bytes2() {
        let mut bitter = BitGet::new(&[]);
        assert_eq!(bitter.read_bytes(1).map(|x| x.into_owned()), None);
    }

    #[test]
    fn test_read_bytes3() {
        let mut bitter = BitGet::new(&[0, 0]);
        assert_eq!(
            bitter.read_bytes(1).map(|x| x.into_owned()).unwrap(),
            vec![0]
        );
    }

    #[test]
    fn test_read_bytes4() {
        let mut bitter = BitGet::new(&[0, 120]);
        assert_eq!(
            bitter.read_bytes(1).map(|x| x.into_owned()).unwrap(),
            vec![0]
        );
        assert_eq!(bitter.read_u8_unchecked(), 120);
    }

    #[test]
    fn test_read_bytes5() {
        let mut bitter = BitGet::new(&[119, 0, 120]);
        assert_eq!(bitter.read_u32_bits_unchecked(8), 119);
        assert_eq!(
            bitter.read_bytes(1).map(|x| x.into_owned()).unwrap(),
            vec![0]
        );
        assert_eq!(bitter.read_u32_bits_unchecked(8), 120);
    }

    #[test]
    fn test_u8_reads() {
        let mut bitter = BitGet::new(&[0xff, 0xfe, 0xfa, 0xf7, 0xf5, 0xf0, 0xb1, 0xb2]);
        assert_eq!(bitter.read_u8(), Some(0xff));
        assert_eq!(bitter.read_u8(), Some(0xfe));
        assert_eq!(bitter.read_u8(), Some(0xfa));
        assert_eq!(bitter.read_u8(), Some(0xf7));
        assert_eq!(bitter.read_u8(), Some(0xf5));
        assert_eq!(bitter.read_u8(), Some(0xf0));
        assert_eq!(bitter.read_u8(), Some(0xb1));
        assert_eq!(bitter.read_u8(), Some(0xb2));
        assert_eq!(bitter.read_u8(), None);
    }

    #[test]
    fn test_u8_unchecked_reads() {
        let mut bitter = BitGet::new(&[0xff, 0xfe, 0xfa, 0xf7, 0xf5, 0xf0, 0xb1, 0xb2]);
        assert_eq!(bitter.read_u8_unchecked(), 0xff);
        assert_eq!(bitter.read_u8_unchecked(), 0xfe);
        assert_eq!(bitter.read_u8_unchecked(), 0xfa);
        assert_eq!(bitter.read_u8_unchecked(), 0xf7);
        assert_eq!(bitter.read_u8_unchecked(), 0xf5);
        assert_eq!(bitter.read_u8_unchecked(), 0xf0);
        assert_eq!(bitter.read_u8_unchecked(), 0xb1);
        assert_eq!(bitter.read_u8_unchecked(), 0xb2);
        assert_eq!(bitter.read_u8(), None);
    }

    #[test]
    fn test_u64_reads() {
        let mut bitter = BitGet::new(&[
            0xff, 0xfe, 0xfa, 0xf7, 0xf5, 0xf0, 0xb1, 0xb2, 0x01, 0xff, 0xfe, 0xfa, 0xf7, 0xf5,
            0xf0, 0xb1, 0xb3,
        ]);
        assert_eq!(bitter.read_u64(), Some(0xb2b1_f0f5_f7fa_feff));
        assert_eq!(bitter.read_u8(), Some(0x01));
        assert_eq!(bitter.read_u64(), Some(0xb3b1_f0f5_f7fa_feff));
    }

    #[test]
    fn test_u64_unchecked_reads() {
        let mut bitter = BitGet::new(&[
            0xff, 0xfe, 0xfa, 0xf7, 0xf5, 0xf0, 0xb1, 0xb2, 0x01, 0xff, 0xfe, 0xfa, 0xf7, 0xf5,
            0xf0, 0xb1, 0xb3,
        ]);
        assert_eq!(bitter.read_u64_unchecked(), 0xb2b1_f0f5_f7fa_feff);
        assert_eq!(bitter.read_u8_unchecked(), 0x01);
        assert_eq!(bitter.read_u64_unchecked(), 0xb3b1_f0f5_f7fa_feff);
    }

    #[test]
    fn test_i64_unchecked_reads() {
        let mut bitter = BitGet::new(&[
            0xff, 0xfe, 0xfa, 0xf7, 0xf5, 0xf0, 0xb1, 0xb2, 0x01, 0xff, 0xfe, 0xfa, 0xf7, 0xf5,
            0xf0, 0xb1, 0xb3,
        ]);

        unsafe {
            assert_eq!(
                bitter.read_i64_unchecked(),
                std::mem::transmute::<u64, i64>(0xb2b1_f0f5_f7fa_feff)
            );
            assert_eq!(bitter.read_u8_unchecked(), 0x01);
            assert_eq!(
                bitter.read_i64_unchecked(),
                std::mem::transmute::<u64, i64>(0xb3b1_f0f5_f7fa_feff)
            );
        }
    }

    #[test]
    fn test_u32_bit_read() {
        let mut bitter = BitGet::new(&[0xff, 0x00, 0xab, 0xcd]);
        assert_eq!(bitter.read_u32_bits(32), Some(0xcdab00ff));
    }

    #[test]
    fn test_u32_reads() {
        let mut bitter = BitGet::new(&[
            0xff,
            0x00,
            0xab,
            0xcd,
            0b1111_1110,
            0b0000_0001,
            0b0101_0110,
            0b1001_1011,
            0b0101_0101,
        ]);
        assert_eq!(bitter.read_u32(), Some(0xcdab00ff));
        assert_eq!(bitter.read_bit(), Some(false));
        assert_eq!(bitter.read_u32(), Some(0xcdab00ff));
        assert_eq!(bitter.read_bit(), Some(false));
        assert_eq!(bitter.read_u32(), None);
    }

    #[test]
    fn test_f32_reads() {
        let mut bitter = BitGet::new(&[
            0b0111_1011,
            0b0001_0100,
            0b1010_1110,
            0b0011_1101,
            0b1111_0110,
            0b0010_1000,
            0b0101_1100,
            0b0111_1011,
            0b0000_0010,
        ]);
        assert_eq!(bitter.read_f32(), Some(0.085));
        assert_eq!(bitter.read_bit(), Some(false));
        assert_eq!(bitter.read_f32(), Some(0.085));
    }

    #[test]
    fn test_f32_unchecked_reads() {
        let mut bitter = BitGet::new(&[
            0b0111_1011,
            0b0001_0100,
            0b1010_1110,
            0b0011_1101,
            0b1111_0110,
            0b0010_1000,
            0b0101_1100,
            0b0111_1011,
            0b0000_0010,
        ]);
        assert_eq!(bitter.read_f32_unchecked(), 0.085);
        assert_eq!(bitter.read_bit(), Some(false));
        assert_eq!(bitter.read_f32_unchecked(), 0.085);
    }

    #[test]
    fn test_u32_bits() {
        let mut bitter = BitGet::new(&[0xff, 0xdd, 0xee, 0xff, 0xdd, 0xee]);
        assert_eq!(bitter.read_u32_bits(10), Some(0x1ff));
        assert_eq!(bitter.read_u32_bits(10), Some(0x3b7));
        assert_eq!(bitter.read_u32_bits(10), Some(0x3fe));
        assert_eq!(bitter.read_u32_bits(10), Some(0x377));
        assert_eq!(bitter.read_u32_bits(10), None);
    }

    #[test]
    fn test_u32_unchecked() {
        let mut bitter = BitGet::new(&[
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        ]);
        assert_eq!(bitter.read_u32_unchecked(), 0xffff_ffff);
        assert_eq!(bitter.read_u32_bits_unchecked(30), 0x3fff_ffff);
        assert_eq!(bitter.read_u32_unchecked(), 0xffff_ffff);
    }

    #[test]
    fn test_u32_bits_unchecked() {
        let mut bitter = BitGet::new(&[0xff, 0xdd, 0xee, 0xff, 0xdd, 0xee, 0xaa, 0xbb, 0xcc, 0xdd]);
        assert_eq!(bitter.read_u32_bits_unchecked(10), 0x1ff);
        assert_eq!(bitter.read_u32_bits_unchecked(10), 0x3b7);
        assert_eq!(bitter.read_u32_bits_unchecked(10), 0x3fe);
        assert_eq!(bitter.read_u32_bits_unchecked(10), 0x377);
        assert_eq!(bitter.read_u32_bits_unchecked(8), 0xee);
        assert_eq!(bitter.read_u32_bits_unchecked(8), 0xaa);
        assert_eq!(bitter.read_u32_bits_unchecked(8), 0xbb);
        assert_eq!(bitter.read_u32_bits_unchecked(8), 0xcc);
        assert_eq!(bitter.read_u32_bits_unchecked(8), 0xdd);
        assert_eq!(bitter.read_bit(), None);
    }

    #[test]
    fn test_u32_bits_unchecked2() {
        let mut bitter = BitGet::new(&[
            0x9c, 0x73, 0xce, 0x39, 0xe7, 0x9c, 0x73, 0xce, 0x39, 0xe7, 0x9c, 0x73, 0xce, 0x39,
            0xe7,
        ]);
        for _ in 0..10 {
            assert_eq!(bitter.read_u32_bits_unchecked(5), 28);
        }
    }

    #[test]
    fn test_i32_bits_unchecked() {
        let mut bitter = BitGet::new(&[0xff, 0xdd, 0xee, 0xff, 0xdd, 0xee, 0xaa, 0xbb, 0xcc, 0xdd]);
        assert_eq!(bitter.read_i32_bits_unchecked(10), 0x1ff);
        assert_eq!(bitter.read_i32_bits_unchecked(10), 0x3b7);
        assert_eq!(bitter.read_i32_bits_unchecked(10), 0x3fe);
        assert_eq!(bitter.read_i32_bits_unchecked(10), 0x377);
        assert_eq!(bitter.read_i32_bits_unchecked(8), 0xee);
        assert_eq!(bitter.read_i32_bits_unchecked(8), 0xaa);
        assert_eq!(bitter.read_i32_bits_unchecked(8), 0xbb);
        assert_eq!(bitter.read_i32_bits_unchecked(8), 0xcc);
        assert_eq!(bitter.read_i32_bits_unchecked(8), 0xdd);
        assert_eq!(bitter.read_bit(), None);
    }

    #[test]
    fn test_u32_bits2() {
        let mut bitter = BitGet::new(&[
            0x9c, 0x73, 0xce, 0x39, 0xe7, 0x9c, 0x73, 0xce, 0x39, 0xe7, 0x9c, 0x73, 0xce, 0x39,
            0xe7,
        ]);
        for _ in 0..10 {
            assert_eq!(bitter.read_u32_bits(5), Some(28));
        }
    }

    #[test]
    fn test_i32_bits2() {
        let mut bitter = BitGet::new(&[
            0x9c, 0x73, 0xce, 0x39, 0xe7, 0x9c, 0x73, 0xce, 0x39, 0xe7, 0x9c, 0x73, 0xce, 0x39,
            0xe7,
        ]);
        for _ in 0..10 {
            assert_eq!(bitter.read_i32_bits(5), Some(28));
        }
    }

    #[test]
    fn test_max_read() {
        let mut bitter = BitGet::new(&[0b1111_1000]);
        assert_eq!(bitter.read_bits_max(5, 20), Some(8));

        let mut bitter = BitGet::new(&[0b1111_0000]);
        assert_eq!(bitter.read_bits_max(5, 20), Some(16));

        let mut bitter = BitGet::new(&[0b1110_0010]);
        assert_eq!(bitter.read_bits_max(5, 20), Some(2));
    }

    #[test]
    fn test_max_read_unchecked() {
        let mut bitter = BitGet::new(&[0b1111_1000]);
        assert_eq!(bitter.read_bits_max_unchecked(5, 20), 8);

        let mut bitter = BitGet::new(&[0b1111_0000]);
        assert_eq!(bitter.read_bits_max_unchecked(5, 20), 16);

        let mut bitter = BitGet::new(&[0b1110_0010]);
        assert_eq!(bitter.read_bits_max_unchecked(5, 20), 2);
    }

    #[test]
    fn test_if_get() {
        let mut bitter = BitGet::new(&[0xff, 0x04]);
        assert_eq!(bitter.if_get(BitGet::read_u8), Some(Some(0x7f)));
        assert_eq!(bitter.if_get(BitGet::read_u8), Some(None));
        assert_eq!(bitter.if_get(BitGet::read_u8), None);
    }

    #[test]
    fn test_if_get_unchecked() {
        let mut bitter = BitGet::new(&[0xff, 0x04]);
        assert_eq!(
            bitter.if_get_unchecked(BitGet::read_u8_unchecked),
            Some(0x7f)
        );
        assert_eq!(bitter.if_get_unchecked(BitGet::read_u8_unchecked), None);
    }

    #[test]
    fn test_approx_bytes_and_empty() {
        let mut bitter = BitGet::new(&[0xff, 0x04]);
        assert!(!bitter.is_empty());
        assert_eq!(bitter.approx_bytes_remaining(), 2);
        assert!(bitter.read_bit().is_some());
        assert!(!bitter.is_empty());
        assert_eq!(bitter.approx_bytes_remaining(), 2);
        assert!(bitter.read_u32_bits(6).is_some());
        assert!(!bitter.is_empty());
        assert_eq!(bitter.approx_bytes_remaining(), 2);
        assert!(bitter.read_bit().is_some());
        assert!(!bitter.is_empty());
        assert_eq!(bitter.approx_bytes_remaining(), 1);
        assert!(bitter.read_bit().is_some());
        assert!(!bitter.is_empty());
        assert_eq!(bitter.approx_bytes_remaining(), 1);
        assert!(bitter.read_u32_bits(7).is_some());
        assert!(bitter.is_empty());
        assert_eq!(bitter.approx_bytes_remaining(), 0);
    }

    #[test]
    fn regression1() {
        let data = vec![0b0000_0010, 0b0011_1111, 0b1011_1100];
        let mut bits = BitGet::new(data.as_slice());

        assert_eq!(bits.read_u32_bits(3), Some(2));
        assert_eq!(bits.read_u8(), Some(224));
        assert_eq!(bits.read_bit(), Some(true));
        assert_eq!(bits.read_u32_bits(13), None);
    }

    #[test]
    fn regression2() {
        let data = vec![2, 63, 63, 2, 63, 2, 0, 0, 0];
        let mut bits = BitGet::new(data.as_slice());

        assert_eq!(bits.read_u32_bits(3), Some(2));
        assert_eq!(bits.read_u8(), Some(224));
        assert_eq!(bits.read_u64(), None);
    }
}
