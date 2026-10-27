use crate::constants::SpdConstants;

/// Input rectangle specification for SPD downsampling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectInput {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

impl RectInput {
    /// Creates a new `RectInput` at origin (0, 0).
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            left: 0,
            top: 0,
            width,
            height,
        }
    }

    /// Creates a new `RectInput` with specified offset and dimensions.
    pub fn with_offset(left: u32, top: u32, width: u32, height: u32) -> Self {
        Self {
            left,
            top,
            width,
            height,
        }
    }
}

impl SpdConstants {
    /// Calculates SPD constants for the given input rectangle.
    ///
    /// Computes dispatch thread group counts based on 64x64 tiles.
    /// Mip levels are capped at 12 as that's the max SPD supports.
    pub fn new(rect: RectInput) -> SpdConstants {
        // Offset of the first tile to downsample (each tile is 64x64)
        let work_group_offset_x = rect.left / 64;
        let work_group_offset_y = rect.top / 64;

        let end_index_x = (rect.left + rect.width - 1) / 64;
        let end_index_y = (rect.top + rect.height - 1) / 64;

        // Dispatch only the thread groups needed to cover the rect
        let dispatch_thread_group_count_x = end_index_x + 1 - work_group_offset_x;
        let dispatch_thread_group_count_y = end_index_y + 1 - work_group_offset_y;

        let num_work_groups = dispatch_thread_group_count_x * dispatch_thread_group_count_y;

        // Mip count based on largest dimension, capped at 12
        let resolution = rect.width.max(rect.height);
        let mips = (resolution as f32).log2().floor().min(12.0) as u32;

        SpdConstants {
            mips,
            num_work_groups,
            work_group_offset: [work_group_offset_x, work_group_offset_y],
            render_size: [rect.width, rect.height],
            _padding: [0; 2],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spd_setup_basic() {
        let rect = RectInput::new(1920, 1080);
        let constants = SpdConstants::new(rect);

        // For 1920x1080, we need (1920-1)/64 + 1 = 30 tiles in X
        // and (1080-1)/64 + 1 = 17 tiles in Y
        assert_eq!(constants.num_work_groups, 30 * 17);
        assert_eq!(constants.work_group_offset, [0, 0]);
        assert_eq!(constants.render_size, [1920, 1080]);

        // Mips should be log2(1920) floored, capped at 12
        let expected_mips = (1920.0f32).log2().floor().min(12.0) as u32;
        assert_eq!(constants.mips, expected_mips);
    }

    #[test]
    fn test_spd_setup_small_resolution() {
        let rect = RectInput::new(64, 64);
        let constants = SpdConstants::new(rect);

        assert_eq!(constants.num_work_groups, 1);
        assert_eq!(constants.work_group_offset, [0, 0]);
        assert_eq!(constants.render_size, [64, 64]);
        assert_eq!(constants.mips, 6); // log2(64) = 6
    }

    #[test]
    fn test_spd_setup_large_resolution() {
        let rect = RectInput::new(4096, 4096);
        let constants = SpdConstants::new(rect);

        let expected_tiles_x = (4096 - 1) / 64 + 1;
        let expected_tiles_y = (4096 - 1) / 64 + 1;
        assert_eq!(
            constants.num_work_groups,
            expected_tiles_x * expected_tiles_y
        );
        assert_eq!(constants.mips, 12); // Capped at 12
    }

    #[test]
    fn test_spd_setup_with_offset() {
        // Test with a 128x128 region starting at (64, 64)
        let rect = RectInput::with_offset(64, 64, 128, 128);
        let constants = SpdConstants::new(rect);

        // Work group offset should be (64/64, 64/64) = (1, 1)
        assert_eq!(constants.work_group_offset, [1, 1]);

        // End indices: (64 + 128 - 1) / 64 = 191 / 64 = 2
        // Dispatch count: 2 + 1 - 1 = 2 in each dimension
        assert_eq!(constants.num_work_groups, 2 * 2);

        // Render size should be the width/height, not including offset
        assert_eq!(constants.render_size, [128, 128]);

        // Mips based on 128x128
        assert_eq!(constants.mips, 7); // log2(128) = 7
    }

    #[test]
    fn test_spd_setup_with_large_offset() {
        // Test with a 256x256 region starting at (256, 128)
        let rect = RectInput::with_offset(256, 128, 256, 256);
        let constants = SpdConstants::new(rect);

        // Work group offset should be (256/64, 128/64) = (4, 2)
        assert_eq!(constants.work_group_offset, [4, 2]);

        // End indices:
        // X: (256 + 256 - 1) / 64 = 511 / 64 = 7
        // Y: (128 + 256 - 1) / 64 = 383 / 64 = 5
        // Dispatch count: (7 + 1 - 4, 5 + 1 - 2) = (4, 4)
        assert_eq!(constants.num_work_groups, 4 * 4);

        assert_eq!(constants.render_size, [256, 256]);
        assert_eq!(constants.mips, 8); // log2(256) = 8
    }

    #[test]
    fn test_rect_input_constructors() {
        let rect1 = RectInput::new(1920, 1080);
        assert_eq!(rect1.left, 0);
        assert_eq!(rect1.top, 0);
        assert_eq!(rect1.width, 1920);
        assert_eq!(rect1.height, 1080);

        let rect2 = RectInput::with_offset(100, 200, 800, 600);
        assert_eq!(rect2.left, 100);
        assert_eq!(rect2.top, 200);
        assert_eq!(rect2.width, 800);
        assert_eq!(rect2.height, 600);
    }
}
