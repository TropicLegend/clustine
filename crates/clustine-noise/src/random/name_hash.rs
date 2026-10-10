// Adapted from SteelMC's `steel-utils/src/random/name_hash.rs`.

//! The two hashes of a name that the factories of generators seed from.

/// What a factory needs of a name such as `minecraft:temperature` or `octave_-7`:
/// the MD5 digest for the xoroshiro family (`RandomSupport.seedFromHashOf`) and Java's
/// `String.hashCode` for the legacy family.
///
/// It can be made at compile time, which is what generated tables of noises do:
/// `const OFFSET: NameHash = NameHash::new("minecraft:offset");`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameHash {
    /// The digest as two big-endian halves, the first eight bytes first.
    pub md5: [u64; 2],
    /// Java's `String.hashCode`.
    pub java_hash: i32,
}

impl NameHash {
    /// The hashes of a name.
    ///
    /// # Panics
    ///
    /// If the name is not ASCII. Java hashes UTF-16 units and the digest is taken of
    /// UTF-8, and the two agree byte for byte only for ASCII. The game's identifiers
    /// are ASCII by its own rule.
    #[must_use]
    pub const fn new(name: &str) -> Self {
        assert!(name.is_ascii(), "names given to a factory are ASCII");
        let digest = md5(name.as_bytes());
        let lo = u64::from_be_bytes([
            digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
        ]);
        let hi = u64::from_be_bytes([
            digest[8], digest[9], digest[10], digest[11], digest[12], digest[13], digest[14],
            digest[15],
        ]);
        Self {
            md5: [lo, hi],
            java_hash: java_hash_code(name.as_bytes()),
        }
    }
}

/// `String.hashCode` of an ASCII string, each of whose bytes is one UTF-16 unit.
const fn java_hash_code(bytes: &[u8]) -> i32 {
    let mut hash = 0_i32;
    let mut i = 0;
    while i < bytes.len() {
        hash = hash.wrapping_mul(31).wrapping_add(bytes[i] as i32);
        i += 1;
    }
    hash
}

/// How far each of the 64 steps of MD5 rotates (RFC 1321).
const SHIFTS: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

/// The table of RFC 1321: entry `i` is the integer part of `2^32 * abs(sin(i + 1))`.
const SINES: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

const INITIAL_STATE: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];

/// The MD5 digest of a message, written so that the compiler can work it out.
const fn md5(data: &[u8]) -> [u8; 16] {
    let mut state = INITIAL_STATE;
    // The message is followed by one byte 0x80, zeros, and its length in bits in the
    // last eight bytes of the last block.
    let blocks = (data.len() + 9).div_ceil(64);
    let length_in_bits = ((data.len() as u64).wrapping_mul(8)).to_le_bytes();

    let mut block_index = 0;
    while block_index < blocks {
        let mut block = [0_u8; 64];
        let mut i = 0;
        while i < 64 {
            let at = block_index * 64 + i;
            if at < data.len() {
                block[i] = data[at];
            } else if at == data.len() {
                block[i] = 0x80;
            }
            i += 1;
        }
        if block_index == blocks - 1 {
            i = 0;
            while i < 8 {
                block[56 + i] = length_in_bits[i];
                i += 1;
            }
        }
        state = md5_block(state, &block);
        block_index += 1;
    }

    let mut digest = [0_u8; 16];
    let mut word = 0;
    while word < 4 {
        let bytes = state[word].to_le_bytes();
        let mut i = 0;
        while i < 4 {
            digest[word * 4 + i] = bytes[i];
            i += 1;
        }
        word += 1;
    }
    digest
}

/// One block of MD5, in the notation of RFC 1321.
const fn md5_block(state: [u32; 4], block: &[u8; 64]) -> [u32; 4] {
    let mut words = [0_u32; 16];
    let mut i = 0;
    while i < 16 {
        words[i] = u32::from_le_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
        i += 1;
    }

    let [mut a, mut b, mut c, mut d] = state;
    i = 0;
    while i < 64 {
        let (mixed, word) = if i < 16 {
            ((b & c) | (!b & d), i)
        } else if i < 32 {
            ((d & b) | (!d & c), (5 * i + 1) % 16)
        } else if i < 48 {
            (b ^ c ^ d, (3 * i + 5) % 16)
        } else {
            (c ^ (b | !d), (7 * i) % 16)
        };
        let sum = a
            .wrapping_add(mixed)
            .wrapping_add(SINES[i])
            .wrapping_add(words[word]);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(sum.rotate_left(SHIFTS[i]));
        i += 1;
    }

    [
        state[0].wrapping_add(a),
        state[1].wrapping_add(b),
        state[2].wrapping_add(c),
        state[3].wrapping_add(d),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(digest: [u8; 16]) -> String {
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn the_digest_is_md5_by_the_test_suite_of_rfc_1321() {
        let cases = [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (message, expected) in cases {
            assert_eq!(hex(md5(message.as_bytes())), expected, "{message:?}");
        }
    }

    #[test]
    fn the_digest_is_right_where_the_padding_changes_its_length() {
        // 55 bytes are the most that fit one block with the padding, 56 the fewest
        // that need two, 64 a whole block of message. From Python's hashlib.
        let cases = [
            (55, "ef1772b6dff9a122358552954ad0df65"),
            (56, "3b0c8ac703f828b04c6c197006d17218"),
            (63, "b06521f39153d618550606be297466d5"),
            (64, "014842d480b571495a4a0363793f7367"),
            (65, "c743a45e0d2e6a95cb859adae0248435"),
        ];
        for (length, expected) in cases {
            let message = "a".repeat(length);
            assert_eq!(hex(md5(message.as_bytes())), expected, "{length} bytes");
        }
    }

    #[test]
    fn names_hash_as_python_and_java_hash_them() {
        // The digests are from Python's hashlib, the hash codes from Java's definition
        // worked in Python.
        let cases: [(&str, u64, u64, i32); 11] = [
            (
                "minecraft:clay_bands",
                0x1656_8f44_636e_3bc9,
                0xc1fd_d673_95e0_296c,
                1_665_294_701,
            ),
            (
                "minecraft:offset",
                0x0805_18cf_6af2_5384,
                0x3f3d_fb40_a54f_ebd5,
                -920_384_768,
            ),
            (
                "minecraft:aquifer",
                0x7bb1_5cc4_03c6_ace6,
                0x0bdd_56bc_9d23_2691,
                -1_973_797_502,
            ),
            (
                "minecraft:ore",
                0x9b88_124d_e600_116d,
                0x2ae6_8055_aa4a_7761,
                1_768_646_549,
            ),
            (
                "minecraft:terrain",
                0x1ee5_5522_2ef9_6f14,
                0xe2be_dfdb_ebe4_3d33,
                1_657_813_608,
            ),
            (
                "minecraft:overworld",
                0x41fa_8411_17c1_27f4,
                0xf08d_9586_b638_45d5,
                1_104_210_353,
            ),
            (
                "octave_0",
                0xd507_0808_6cef_4d7c,
                0x6e16_51ec_c7f4_3309,
                1_261_148_513,
            ),
            (
                "octave_-7",
                0xf112_6812_8982_754f,
                0x257a_1d67_0430_b0aa,
                440_898_201,
            ),
            (
                "TEST STRING",
                0x2d7d_6874_3275_8a8e,
                0xeeca_7b7e_5d51_8e7f,
                169_383_775,
            ),
            (
                "test_noise",
                0x0b5c_e6e4_4c0c_ef6c,
                0xb340_469c_ef4d_6e08,
                2_065_668_397,
            ),
            (
                "minecraft:a_name_that_is_longer_than_one_block_of_md5_which_is_64_bytes",
                0xf1d6_b486_d9fd_fef3,
                0x14ff_764c_9a70_c358,
                -679_947_219,
            ),
        ];
        for (name, lo, hi, java_hash) in cases {
            let hash = NameHash::new(name);
            assert_eq!(hash.md5, [lo, hi], "{name}");
            assert_eq!(hash.java_hash, java_hash, "{name}");
        }
    }

    #[test]
    fn a_name_can_be_hashed_at_compile_time() {
        const OFFSET: NameHash = NameHash::new("minecraft:offset");
        assert_eq!(OFFSET, NameHash::new("minecraft:offset"));
    }
}
