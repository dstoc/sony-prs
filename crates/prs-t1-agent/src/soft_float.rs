//! Compatibility implementations for math symbols absent from older musl.
//!
//! Rust 1.98 can lower IEEE minimum/maximum operations to the C23
//! `fminimum_num*` and `fmaximum_num*` symbols. The PRS-T1 ARM musl target
//! does not provide those symbols yet, so define the four symbols needed by
//! the production dependency graph without adding a target-side library.

fn is_nan_f32(value: f32) -> bool {
    let bits = value.to_bits();
    bits & 0x7f80_0000 == 0x7f80_0000 && bits & 0x007f_ffff != 0
}

fn is_nan_f64(value: f64) -> bool {
    let bits = value.to_bits();
    bits & 0x7ff0_0000_0000_0000 == 0x7ff0_0000_0000_0000 && bits & 0x000f_ffff_ffff_ffff != 0
}

fn minimum_num_f32(left: f32, right: f32) -> f32 {
    if is_nan_f32(left) {
        return right;
    }
    if is_nan_f32(right) {
        return left;
    }
    if left == 0.0 && right == 0.0 {
        return if left.to_bits() & 0x8000_0000 != 0 {
            left
        } else {
            right
        };
    }
    if left < right {
        left
    } else {
        right
    }
}

fn maximum_num_f32(left: f32, right: f32) -> f32 {
    if is_nan_f32(left) {
        return right;
    }
    if is_nan_f32(right) {
        return left;
    }
    if left == 0.0 && right == 0.0 {
        return if left.to_bits() & 0x8000_0000 == 0 {
            left
        } else {
            right
        };
    }
    if left > right {
        left
    } else {
        right
    }
}

fn minimum_num_f64(left: f64, right: f64) -> f64 {
    if is_nan_f64(left) {
        return right;
    }
    if is_nan_f64(right) {
        return left;
    }
    if left == 0.0 && right == 0.0 {
        return if left.to_bits() & 0x8000_0000_0000_0000 != 0 {
            left
        } else {
            right
        };
    }
    if left < right {
        left
    } else {
        right
    }
}

fn maximum_num_f64(left: f64, right: f64) -> f64 {
    if is_nan_f64(left) {
        return right;
    }
    if is_nan_f64(right) {
        return left;
    }
    if left == 0.0 && right == 0.0 {
        return if left.to_bits() & 0x8000_0000_0000_0000 == 0 {
            left
        } else {
            right
        };
    }
    if left > right {
        left
    } else {
        right
    }
}

#[cfg(target_arch = "arm")]
#[unsafe(no_mangle)]
pub extern "C" fn fminimum_numf(left: f32, right: f32) -> f32 {
    minimum_num_f32(left, right)
}

#[cfg(target_arch = "arm")]
#[unsafe(no_mangle)]
pub extern "C" fn fmaximum_numf(left: f32, right: f32) -> f32 {
    maximum_num_f32(left, right)
}

#[cfg(target_arch = "arm")]
#[unsafe(no_mangle)]
pub extern "C" fn fminimum_num(left: f64, right: f64) -> f64 {
    minimum_num_f64(left, right)
}

#[cfg(target_arch = "arm")]
#[unsafe(no_mangle)]
pub extern "C" fn fmaximum_num(left: f64, right: f64) -> f64 {
    maximum_num_f64(left, right)
}

#[cfg(test)]
mod tests {
    use super::{maximum_num_f32, maximum_num_f64, minimum_num_f32, minimum_num_f64};

    #[test]
    fn minimum_and_maximum_preserve_nan_number_semantics() {
        let nan = f32::NAN;
        assert_eq!(minimum_num_f32(nan, 3.0), 3.0);
        assert_eq!(maximum_num_f32(3.0, nan), 3.0);
        assert!(minimum_num_f32(nan, nan).is_nan());

        let nan = f64::NAN;
        assert_eq!(minimum_num_f64(nan, 3.0), 3.0);
        assert_eq!(maximum_num_f64(3.0, nan), 3.0);
        assert!(maximum_num_f64(nan, nan).is_nan());
    }

    #[test]
    fn minimum_and_maximum_order_signed_zero() {
        assert!(minimum_num_f32(-0.0, 0.0).is_sign_negative());
        assert!(!maximum_num_f32(-0.0, 0.0).is_sign_negative());
        assert!(minimum_num_f64(-0.0, 0.0).is_sign_negative());
        assert!(!maximum_num_f64(-0.0, 0.0).is_sign_negative());
    }
}
