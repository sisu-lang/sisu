//! Printing an `f64`: the shortest text that reads back as the same value, or with a fixed number
//! of decimals.

use std::io::Write;

/// The longest text of an `f64`: `-2.2250738585072014e-308`.
const FLOAT_BYTES: usize = 24;

/// `x` in the shortest text that reads back as `x`, in `buffer`: Rust's `{:?}`, which spells
/// NaN `NaN`, with `nan` for it.
pub(crate) fn shortest(x: f64, buffer: &mut [u8; FLOAT_BYTES]) -> &[u8] {
    if x.is_nan() {
        return b"nan";
    }
    let mut rest = &mut buffer[..];
    write!(rest, "{x:?}").expect("24 bytes should hold every `f64`");
    let len = FLOAT_BYTES - rest.len();
    &buffer[..len]
}

/// Writes `x` as `print` does to `out`, and returns the end of the text: `out` plus the bytes
/// it wrote, at most 24. Interpolation calls this for an `f64`.
///
/// # Safety
///
/// `out` has room for the text of `x`, at most 24 bytes.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_write_f64(x: f64, out: *mut u8) -> *mut u8 {
    // SAFETY: the caller promises room for the text, at most the 24 bytes `shortest` gives.
    unsafe { crate::text::write(shortest(x, &mut [0; FLOAT_BYTES]), out) }
}

/// The longest text of an `f64` at the largest precision the checker allows, 30: `-f64::MAX`,
/// a sign, 309 digits, a point and 30 decimals. A precision `p` takes at most `311 + p`.
const FIXED_BYTES: usize = 341;

/// Writes `x` with `precision` decimals, rounded half to even from its exact binary value and
/// never in exponent form, to `out`. Returns the end of the text: `out` plus the bytes it
/// wrote, at most `311 + precision`. NaN is `nan`. Infinity is `inf` or `-inf`, without
/// decimals. Interpolation calls this for an `f64` with a precision.
///
/// # Safety
///
/// `precision` is at most 30, and `out[..311 + precision]` is writable.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_write_f64_fixed(x: f64, precision: u8, out: *mut u8) -> *mut u8 {
    if x.is_nan() {
        // SAFETY: the caller promises at least 311 bytes, and `nan` takes 3.
        return unsafe { crate::text::write(b"nan", out) };
    }
    let mut buffer = [0; FIXED_BYTES];
    let mut rest = &mut buffer[..];
    let precision = usize::from(precision);
    write!(rest, "{x:.precision$}")
        .expect("341 bytes should hold every `f64` at a precision of at most 30");
    let len = FIXED_BYTES - rest.len();
    // SAFETY: the caller promises `311 + precision` bytes, which the text never passes.
    unsafe { crate::text::write(&buffer[..len], out) }
}

#[cfg(test)]
mod tests {
    use super::{FLOAT_BYTES, shortest, sisu_write_f64, sisu_write_f64_fixed};

    /// `x` at `precision`, written into exactly the `311 + precision` bytes the caller gives.
    fn fixed(x: f64, precision: u8) -> String {
        let mut out = vec![0; 311 + usize::from(precision)];
        // SAFETY: `out` has room for `311 + precision` bytes.
        #[expect(unsafe_code, reason = "calls the runtime's C ABI function")]
        let end = unsafe { sisu_write_f64_fixed(x, precision, out.as_mut_ptr()) };
        out.truncate(end.addr() - out.as_ptr().addr());
        String::from_utf8(out).expect("the text should be ASCII")
    }

    /// Rounding is half to even from the exact binary value, the sign stays on a negative value
    /// that rounds to zero, and the specials ignore the precision.
    #[test]
    fn fixed_precision() {
        for (x, precision, want) in [
            // 2.675 is 2.67499999999999982236431605997495353221893310546875.
            (2.675, 2, "2.67"),
            (0.5, 0, "0"),
            (1.5, 0, "2"),
            (2.5, 0, "2"),
            (-0.004, 2, "-0.00"),
            (f64::NAN, 2, "nan"),
            (-f64::NAN, 2, "nan"),
            (f64::INFINITY, 3, "inf"),
            (f64::NEG_INFINITY, 3, "-inf"),
        ] {
            assert_eq!(fixed(x, precision), want, "{x} at {precision}");
        }
    }

    /// `-f64::MAX` at the largest precision fills its bound: a sign, 309 digits, a point and 30
    /// decimals.
    #[test]
    fn fixed_fills_its_bound() {
        let text = fixed(-f64::MAX, 30);
        assert_eq!(text.len(), 341);
        assert!(
            text.starts_with("-179769313486231570814527423731704356798"),
            "{text}"
        );
        assert!(
            text.ends_with("858368.000000000000000000000000000000"),
            "{text}"
        );
    }

    /// Interpolation writes the text `print` does; the longest fills the 24 bytes it is given.
    #[test]
    fn write_f64_should_fill_its_bound() {
        for (x, want) in [
            (-2.225_073_858_507_201_4e-308, "-2.2250738585072014e-308"),
            (1.0, "1.0"),
            (f64::NAN, "nan"),
        ] {
            let mut out = [0; FLOAT_BYTES];
            // SAFETY: `out` has room for 24 bytes.
            #[expect(unsafe_code, reason = "calls the runtime's C ABI function")]
            let end = unsafe { sisu_write_f64(x, out.as_mut_ptr()) };
            let len = end.addr() - out.as_ptr().addr();
            assert_eq!(&out[..len], want.as_bytes());
        }
    }

    #[test]
    fn shortest_matches_the_spec() {
        let rows = [
            (1.0, "1.0"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e16"),
            (0.0001, "0.0001"),
            (1e-5, "1e-5"),
            (-0.0, "-0.0"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::MAX, "1.7976931348623157e308"),
            (5e-324, "5e-324"),
            (f64::NAN, "nan"),
            (-f64::NAN, "nan"),
            // The longest output.
            (-2.225_073_858_507_201_4e-308, "-2.2250738585072014e-308"),
        ];
        for (x, text) in rows {
            assert_eq!(shortest(x, &mut [0; FLOAT_BYTES]), text.as_bytes(), "{x}");
        }
        assert_eq!("-2.2250738585072014e-308".len(), FLOAT_BYTES);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "100,000 rounds take hours under Miri; shortest_matches_the_spec covers the path"
    )]
    fn shortest_reads_back() {
        let mut bits = 0x9E37_79B9_7F4A_7C15_u64;
        for _ in 0..100_000 {
            // A xorshift step gives bit patterns across every exponent and sign.
            bits ^= bits << 13;
            bits ^= bits >> 7;
            bits ^= bits << 17;
            let x = f64::from_bits(bits);
            if !x.is_finite() {
                continue;
            }
            let mut buffer = [0; FLOAT_BYTES];
            let text =
                std::str::from_utf8(shortest(x, &mut buffer)).expect("the text should be ASCII");
            let back: f64 = text.parse().expect("the text should parse as an `f64`");
            assert_eq!(back.to_bits(), x.to_bits(), "{text}");
        }
    }
}
