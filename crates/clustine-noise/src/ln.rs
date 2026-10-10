// Clustine's own; not adapted from SteelMC, which calls the platform's logarithm here.

//! A natural logarithm that is the same on every platform and rounded correctly.
//!
//! The game draws normally distributed numbers with `Math.log`. A Java runtime answers
//! that with a routine of its own, which is nearly always the correctly rounded
//! logarithm. `libm`'s logarithm is the older one of `StrictMath`, which is off by one
//! in the last place for about six arguments in a hundred: the ninth normal number
//! of a legacy generator seeded with zero already differs from the game's with it.
//!
//! So the logarithm is computed here to about a hundred bits, in pairs of doubles
//! whose sum is the value, and rounded once at the end. Only additions,
//! multiplications and divisions of doubles are used, which every platform rounds the
//! same way. Whether the runtime's routine is correctly rounded for every argument is
//! not known; the comparison with a Java runtime that the terrain plan asks for
//! (step G6) has to say.

/// A value as the unevaluated sum of two doubles, the second much smaller.
type Pair = (f64, f64);

/// ln 2 to about 106 bits.
const LN_2: Pair = (std::f64::consts::LN_2, 2.319_046_813_846_299_6e-17);

/// How many terms of the series are taken beyond the first. The series is in a
/// variable below 0.03, so twenty-two terms reach beyond a hundred bits.
const TERMS: u32 = 22;

/// The natural logarithm of `x`, correctly rounded (`Math.log` as a Java runtime
/// answers it, see the module's note). Zero, negative, subnormal and non-finite
/// arguments are left to `libm`; the game never asks for them.
#[must_use]
pub fn ln(x: f64) -> f64 {
    if !(x.is_finite() && x >= f64::MIN_POSITIVE) {
        return libm::log(x);
    }

    // x = mantissa * 2^exponent with the mantissa between the roots of a half and of
    // two, so that an argument near one has an exponent of zero and loses nothing.
    let bits = x.to_bits();
    let mut exponent = ((bits >> 52) & 0x7FF) as i32 - 1023;
    let mut mantissa = f64::from_bits((bits & 0x000F_FFFF_FFFF_FFFF) | 0x3FF0_0000_0000_0000);
    if mantissa >= std::f64::consts::SQRT_2 {
        mantissa *= 0.5;
        exponent += 1;
    }

    // ln(1 + f) = 2 (s + s^3/3 + s^5/5 + ...) with s = f / (2 + f).
    let f = mantissa - 1.0;
    let s = divide((f, 0.0), two_sum(2.0, f));
    let s_squared = multiply(s, s);
    let mut series = reciprocal(2 * TERMS + 1);
    for term in (0..TERMS).rev() {
        series = add(multiply(series, s_squared), reciprocal(2 * term + 1));
    }
    let of_mantissa = multiply(s, series);
    let of_mantissa = (2.0 * of_mantissa.0, 2.0 * of_mantissa.1);

    let of_exponent = multiply((f64::from(exponent), 0.0), LN_2);
    let (high, low) = add(of_exponent, of_mantissa);
    high + low
}

/// The sum of two doubles and what rounding took from it.
#[inline]
fn two_sum(a: f64, b: f64) -> Pair {
    let sum = a + b;
    let b_as_added = sum - a;
    (sum, (a - (sum - b_as_added)) + (b - b_as_added))
}

/// [`two_sum`] where `a` is known to be the larger in magnitude.
#[inline]
fn quick_two_sum(a: f64, b: f64) -> Pair {
    let sum = a + b;
    (sum, b - (sum - a))
}

/// A double as two halves of 26 bits each, whose products are exact.
#[inline]
fn split(a: f64) -> Pair {
    let scaled = 134_217_729.0 * a;
    let high = scaled - (scaled - a);
    (high, a - high)
}

/// The product of two doubles and what rounding took from it (Dekker's product,
/// which needs no fused multiplication).
#[inline]
fn two_product(a: f64, b: f64) -> Pair {
    let product = a * b;
    let (a_high, a_low) = split(a);
    let (b_high, b_low) = split(b);
    let error = ((a_high * b_high - product) + a_high * b_low + a_low * b_high) + a_low * b_low;
    (product, error)
}

fn add(a: Pair, b: Pair) -> Pair {
    let (high, high_error) = two_sum(a.0, b.0);
    let (low, low_error) = two_sum(a.1, b.1);
    let (high, carried) = quick_two_sum(high, high_error + low);
    quick_two_sum(high, carried + low_error)
}

fn multiply(a: Pair, b: Pair) -> Pair {
    let (product, error) = two_product(a.0, b.0);
    quick_two_sum(product, error + (a.0 * b.1 + a.1 * b.0))
}

fn divide(a: Pair, b: Pair) -> Pair {
    // Long division, three digits of a double each.
    let first = a.0 / b.0;
    let remainder = add(a, negate(multiply((first, 0.0), b)));
    let second = remainder.0 / b.0;
    let remainder = add(remainder, negate(multiply((second, 0.0), b)));
    let third = remainder.0 / b.0;
    let (high, low) = quick_two_sum(first, second);
    add((high, low), (third, 0.0))
}

fn negate(a: Pair) -> Pair {
    (-a.0, -a.1)
}

fn reciprocal(of: u32) -> Pair {
    divide((1.0, 0.0), (f64::from(of), 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logarithm_is_the_correctly_rounded_one() {
        // Arguments as their bits, and the bits of the logarithm rounded to the
        // nearest double from seventy decimal digits (Python's decimal module). The
        // first is the argument for which libm's logarithm is one off. These are a
        // few of 250,118 arguments compared so when this was written: 200,000 between
        // zero and one, 50,000 across all exponents, and those near one and at the
        // powers of two. None differed; libm's differed for 14,398 of them.
        for &(argument, expected) in CASES {
            let argument = f64::from_bits(argument);
            assert_eq!(
                ln(argument).to_bits(),
                expected,
                "ln({argument:e}) = {:e}",
                f64::from_bits(expected)
            );
        }
    }

    #[test]
    fn the_logarithm_of_one_is_zero_and_the_odd_arguments_are_libms() {
        assert_eq!(ln(1.0).to_bits(), 0.0_f64.to_bits());
        assert_eq!(ln(0.0), f64::NEG_INFINITY);
        assert_eq!(ln(f64::INFINITY), f64::INFINITY);
        assert!(ln(-1.0).is_nan());
        assert!(ln(f64::NAN).is_nan());
        assert_eq!(ln(5.0e-324).to_bits(), libm::log(5.0e-324).to_bits());
    }

    #[test]
    fn libms_logarithm_is_not_the_correctly_rounded_one() {
        // The reason this module exists. Should libm ever change, this says so.
        let (argument, expected) = CASES[0];
        let argument = f64::from_bits(argument);
        assert_eq!(libm::log(argument).to_bits(), expected + 1);
        assert_eq!(ln(argument).to_bits(), expected);
    }

    const CASES: &[(u64, u64)] = &[
        (0x3fe8_1c47_b8a7_2375, 0xbfd2_1e24_707c_9607),
        (0x3fe7_bf6c_bb8c_6682, 0xbfd3_167e_bf6d_882f),
        (0x3fea_7e61_0b05_2e61, 0xbfc8_2b95_f4a4_ac73),
        (0x3fee_e3b3_df0c_61ee, 0xbfa2_1590_5214_65b8),
        (0x3fec_b8c3_fbf5_df09, 0xbfbb_ab3b_a7ff_e0e4),
        (0x3fdb_5b62_dab4_1ade, 0xbfeb_3279_8ffa_2ed1),
        (0x3fe3_5156_f351_d3aa, 0xbfe0_269a_a937_5891),
        (0x3fe8_e0d6_d982_5b8c, 0xbfd0_1c85_980e_1872),
        (0x3fe7_3041_ad02_58ac, 0xbfd4_9cf2_af05_a960),
        (0x3fc1_3db0_a76d_7100, 0xc000_09b5_d43f_b01d),
        (0x3fdc_59f0_5f05_229c, 0xbfea_0e00_c51c_abf4),
        (0x3fe7_aea7_cf80_bccb, 0xbfd3_43c0_1f8f_b39c),
        (0x3f4a_e500_18bc_5800, 0xc01c_6bcf_bfbb_f2af),
        (0x2102_4d3f_0c94_f197, 0xc075_6f93_3cee_b676),
        (0x19e2_8f8b_cbe2_1d1f, 0xc07a_5fa6_9990_3852),
        (0x4ba8_0c87_85a9_1220, 0x4060_40d5_328e_8b89),
        (0x64ec_3c62_03b4_93d8, 0x4079_a37c_e128_6a29),
        (0x12cc_8e47_e510_5b74, 0xc07f_490e_e997_fe7c),
        (0x017f_909c_dd6e_3ed0, 0xc085_a3bd_e5f2_013a),
        (0x16d6_68b3_23b1_1318, 0xc07c_7c10_4487_af36),
        (0x5fee_05bb_e4d3_403c, 0x4076_2d3d_b404_893e),
        (0x2eb4_67dc_09c7_3131, 0xc067_e217_b9d0_e1fd),
        (0x3837_e7f0_a9d7_6be8, 0xc055_631e_0c39_eb70),
        (0x034d_917d_e4f7_7d40, 0xc085_0374_5222_46ec),
        (0x6cbb_2e6d_83c1_13f7, 0x407f_0d2c_55bb_f4e0),
        (0x3fe0_0000_0000_0000, 0xbfe6_2e42_fefa_39ef),
        (0x3fef_0000_0000_0000, 0xbfa0_415d_89e7_4444),
        (0x3fef_f000_0000_0000, 0xbf60_0401_55d5_889e),
        (0x3fef_ff00_0000_0000, 0xbf20_0040_0155_5d56),
        (0x3fef_fff0_0000_0000, 0xbee0_0004_0001_5556),
        (0x3fef_ffff_0000_0000, 0xbea0_0000_4000_0155),
        (0x3fef_ffff_f000_0000, 0xbe60_0000_0400_0001),
        (0x3fef_ffff_ff00_0000, 0xbe20_0000_0040_0000),
        (0x3fef_ffff_fff0_0000, 0xbde0_0000_0004_0000),
        (0x3fef_ffff_ffff_0000, 0xbda0_0000_0000_4000),
        (0x3fef_ffff_ffff_f000, 0xbd60_0000_0000_0400),
        (0x3fef_ffff_ffff_ff00, 0xbd20_0000_0000_0040),
        (0x3fef_ffff_ffff_fff0, 0xbce0_0000_0000_0004),
        (0x3ff8_0000_0000_0000, 0x3fd9_f323_ecbf_984c),
        (0x3ff0_8000_0000_0000, 0x3f9f_829b_0e78_3300),
        (0x3ff0_0800_0000_0000, 0x3f5f_f802_a9ab_10e6),
        (0x3ff0_0080_0000_0000, 0x3f1f_ff80_02aa_9aab),
        (0x3ff0_0008_0000_0000, 0x3edf_fff8_0002_aaaa),
        (0x3ff0_0000_8000_0000, 0x3e9f_ffff_8000_02ab),
        (0x3ff0_0000_0800_0000, 0x3e5f_ffff_f800_0003),
        (0x3ff0_0000_0080_0000, 0x3e1f_ffff_ff80_0000),
        (0x3ff0_0000_0008_0000, 0x3ddf_ffff_fff8_0000),
        (0x3ff0_0000_0000_8000, 0x3d9f_ffff_ffff_8000),
        (0x3ff0_0000_0000_0800, 0x3d5f_ffff_ffff_f800),
        (0x3ff0_0000_0000_0080, 0x3d1f_ffff_ffff_ff80),
        (0x3ff0_0000_0000_0008, 0x3cdf_ffff_ffff_fff8),
        (0x0010_0000_0000_0000, 0xc086_232b_dd7a_bcd2),
        (0x20b0_0000_0000_0000, 0xc075_a92d_6d00_5c94),
        (0x4000_0000_0000_0000, 0x3fe6_2e42_fefa_39ef),
        (0x4010_0000_0000_0000, 0x3ff6_2e42_fefa_39ef),
        (0x4090_0000_0000_0000, 0x401b_b9d3_beb8_c86b),
        (0x7fe0_0000_0000_0000, 0x4086_28b7_6e3a_7b61),
        (0x3fe6_a09e_667f_3bcc, 0xbfd6_2e42_fefa_39f1),
        (0x3fe6_a09e_667f_3bcd, 0xbfd6_2e42_fefa_39ee),
        (0x3ff6_a09e_667f_3bcd, 0x3fd6_2e42_fefa_39f0),
        (0x3ff6_a09e_667f_3bcc, 0x3fd6_2e42_fefa_39ee),
        (0x4008_0000_0000_0000, 0x3ff1_93ea_7aad_030b),
        (0x4024_0000_0000_0000, 0x4002_6bb1_bbb5_5516),
        (0x7fef_ffff_ffff_ffff, 0x4086_2e42_fefa_39ef),
    ];
}
