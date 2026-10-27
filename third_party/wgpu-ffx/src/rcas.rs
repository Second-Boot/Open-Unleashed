use crate::constants::RcasConstants;

/// Setup required constant values for RCAS.
///
/// The scale is {0.0 := maximum sharpness, to N>0, where N is the number of
/// stops (halving) of the reduction of sharpness}.
///
/// # Arguments
/// * `sharpness` - Sharpness value where 0.0 is maximum and higher values reduce sharpness
pub fn populate_rcas_constants(sharpness: f32) -> RcasConstants {
    // Transform from stops to linear value
    let sharpness_linear = (-sharpness).exp2();

    let h_sharp = [sharpness_linear, sharpness_linear];

    RcasConstants {
        rcas_config: [sharpness_linear.to_bits(), pack_half_2x16(h_sharp), 0, 0],
    }
}

/// Pack two f32 values into a single u32 as f16 values
fn pack_half_2x16(values: [f32; 2]) -> u32 {
    let half0 = half::f16::from_f32(values[0]);
    let half1 = half::f16::from_f32(values[1]);
    bytemuck::cast([half0, half1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rcas_constants_zero_sharpness() {
        let constants = populate_rcas_constants(0.0);
        // At sharpness 0.0, exp2(0) = 1.0
        assert_eq!(constants.rcas_config[0], 1.0f32.to_bits());
    }

    #[test]
    fn test_rcas_constants_max_sharpness() {
        let constants = populate_rcas_constants(2.0);
        // At sharpness 2.0, exp2(-2.0) = 0.25
        assert_eq!(constants.rcas_config[0], 0.25f32.to_bits());
    }
}
