// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

// Copyright 2026 The InterOS Authors.
//
// VP8 (lossy WebP) keyframe decoder adhering to RFC 6386 and libwebp.
//
// Implements zero-heap, single-pass bounded lossy WebP keyframe decoding.
// Scratch storage is allocated in static .bss (non-reentrant, single-threaded).

#![allow(dead_code)]

pub const MAX_VP8_DIMENSION: u32 = 1024;
pub const MAX_MB_COLS: usize = 64; // 1024 / 16

pub const Y_STRIDE: usize = 1056;
pub const UV_STRIDE: usize = 544;

pub const Y_BAND_ROWS: usize = 24;
pub const UV_BAND_ROWS: usize = 16;

pub const Y_BAND_SIZE: usize = Y_BAND_ROWS * Y_STRIDE;
pub const UV_BAND_SIZE: usize = UV_BAND_ROWS * UV_STRIDE;

pub const Y_OFFSET: usize = 4 * Y_STRIDE + 16;
pub const UV_OFFSET: usize = 4 * UV_STRIDE + 16;

#[derive(Clone, Copy, Default)]
pub struct MbInfo {
    pub y_mode: u8,
    pub uv_mode: u8,
    pub segment_id: u8,
    pub skip_coeff: u8,
    pub eob_mask: u32,
    pub sub_modes: [u8; 16],
}

pub struct Vp8Scratch {
    pub y_bands: [[u8; Y_BAND_SIZE]; 2],
    pub u_bands: [[u8; UV_BAND_SIZE]; 2],
    pub v_bands: [[u8; UV_BAND_SIZE]; 2],
    pub mb_info_prev: [MbInfo; MAX_MB_COLS + 1],
    pub mb_info_curr: [MbInfo; MAX_MB_COLS + 1],
    pub above_token_ctx: [[u8; 9]; MAX_MB_COLS],
    pub left_token_ctx: [u8; 9],
    pub coeffs: [i16; 25 * 16 * MAX_MB_COLS],
    pub coeff_probs: [[[[u8; 11]; 3]; 8]; 4],
    pub tmp_y: [u8; MAX_MB_COLS * 16],
    pub tmp_u: [u8; MAX_MB_COLS * 8],
    pub tmp_v: [u8; MAX_MB_COLS * 8],
    pub top_pred_y: [u8; MAX_MB_COLS * 16 + 4],
    pub top_pred_u: [u8; MAX_MB_COLS * 8 + 4],
    pub top_pred_v: [u8; MAX_MB_COLS * 8 + 4],
}

const VP8_SCRATCH_EMPTY: Vp8Scratch = Vp8Scratch {
    y_bands: [[0; Y_BAND_SIZE]; 2],
    u_bands: [[0; UV_BAND_SIZE]; 2],
    v_bands: [[0; UV_BAND_SIZE]; 2],
    mb_info_prev: [MbInfo {
        y_mode: 0,
        uv_mode: 0,
        segment_id: 0,
        skip_coeff: 0,
        eob_mask: 0,
        sub_modes: [0; 16],
    }; MAX_MB_COLS + 1],
    mb_info_curr: [MbInfo {
        y_mode: 0,
        uv_mode: 0,
        segment_id: 0,
        skip_coeff: 0,
        eob_mask: 0,
        sub_modes: [0; 16],
    }; MAX_MB_COLS + 1],
    above_token_ctx: [[0; 9]; MAX_MB_COLS],
    left_token_ctx: [0; 9],
    coeffs: [0; 25 * 16 * MAX_MB_COLS],
    coeff_probs: [[[[0; 11]; 3]; 8]; 4],
    tmp_y: [0; MAX_MB_COLS * 16],
    tmp_u: [0; MAX_MB_COLS * 8],
    tmp_v: [0; MAX_MB_COLS * 8],
    top_pred_y: [0; MAX_MB_COLS * 16 + 4],
    top_pred_u: [0; MAX_MB_COLS * 8 + 4],
    top_pred_v: [0; MAX_MB_COLS * 8 + 4],
};

/// Scratch arena for VP8 keyframe decoding.
///
/// SAFETY: the scratch buffer is sound because callers in InterOS applications
/// are strictly single-threaded and non-reentrant. Apps run a single event loop
/// on one task and invoke decoding from that loop, never from an interrupt
/// handler or a concurrent worker thread.
static mut VP8_SCRATCH: Vp8Scratch = VP8_SCRATCH_EMPTY;

#[inline]
pub(crate) fn vp8_scratch() -> &'static mut Vp8Scratch {
    unsafe { &mut *core::ptr::addr_of_mut!(VP8_SCRATCH) }
}


pub const DC_Q_LOOKUP: [i16; 128] = [4, 5, 6, 7, 8, 9, 10, 10, 11, 12, 13, 14, 15, 16, 17, 17, 18, 19, 20, 20, 21, 21, 22, 22, 23, 23, 24, 25, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 91, 93, 95, 96, 98, 100, 101, 102, 104, 106, 108, 110, 112, 114, 116, 118, 122, 124, 126, 128, 130, 132, 134, 136, 138, 140, 143, 145, 148, 151, 154, 157];
pub const AC_Q_LOOKUP: [i16; 128] = [4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 60, 62, 64, 66, 68, 70, 72, 74, 76, 78, 80, 82, 84, 86, 88, 90, 92, 94, 96, 98, 100, 102, 104, 106, 108, 110, 112, 114, 116, 119, 122, 125, 128, 131, 134, 137, 140, 143, 146, 149, 152, 155, 158, 161, 164, 167, 170, 173, 177, 181, 185, 189, 193, 197, 201, 205, 209, 213, 217, 221, 225, 229, 234, 239, 245, 249, 254, 259, 264, 269, 274, 279, 284];
pub const KF_B_MODE_PROBS: [[[u8; 9]; 10]; 10] = [[[231, 120, 48, 89, 115, 113, 120, 152, 112], [152, 179, 64, 126, 170, 118, 46, 70, 95], [175, 69, 143, 80, 85, 82, 72, 155, 103], [56, 58, 10, 171, 218, 189, 17, 13, 152], [144, 71, 10, 38, 171, 213, 144, 34, 26], [114, 26, 17, 163, 44, 195, 21, 10, 173], [121, 24, 80, 195, 26, 62, 44, 64, 85], [170, 46, 55, 19, 136, 160, 33, 206, 71], [63, 20, 8, 114, 114, 208, 12, 9, 226], [81, 40, 11, 96, 182, 84, 29, 16, 36]], [[134, 183, 89, 137, 98, 101, 106, 165, 148], [72, 187, 100, 130, 157, 111, 32, 75, 80], [66, 102, 167, 99, 74, 62, 40, 234, 128], [41, 53, 9, 178, 241, 141, 26, 8, 107], [104, 79, 12, 27, 217, 255, 87, 17, 7], [74, 43, 26, 146, 73, 166, 49, 23, 157], [65, 38, 105, 160, 51, 52, 31, 115, 128], [87, 68, 71, 44, 114, 51, 15, 186, 23], [47, 41, 14, 110, 182, 183, 21, 17, 194], [66, 45, 25, 102, 197, 189, 23, 18, 22]], [[88, 88, 147, 150, 42, 46, 45, 196, 205], [43, 97, 183, 117, 85, 38, 35, 179, 61], [39, 53, 200, 87, 26, 21, 43, 232, 171], [56, 34, 51, 104, 114, 102, 29, 93, 77], [107, 54, 32, 26, 51, 1, 81, 43, 31], [39, 28, 85, 171, 58, 165, 90, 98, 64], [34, 22, 116, 206, 23, 34, 43, 166, 73], [68, 25, 106, 22, 64, 171, 36, 225, 114], [34, 19, 21, 102, 132, 188, 16, 76, 124], [62, 18, 78, 95, 85, 57, 50, 48, 51]], [[193, 101, 35, 159, 215, 111, 89, 46, 111], [60, 148, 31, 172, 219, 228, 21, 18, 111], [112, 113, 77, 85, 179, 255, 38, 120, 114], [40, 42, 1, 196, 245, 209, 10, 25, 109], [100, 80, 8, 43, 154, 1, 51, 26, 71], [88, 43, 29, 140, 166, 213, 37, 43, 154], [61, 63, 30, 155, 67, 45, 68, 1, 209], [142, 78, 78, 16, 255, 128, 34, 197, 171], [41, 40, 5, 102, 211, 183, 4, 1, 221], [51, 50, 17, 168, 209, 192, 23, 25, 82]], [[125, 98, 42, 88, 104, 85, 117, 175, 82], [95, 84, 53, 89, 128, 100, 113, 101, 45], [75, 79, 123, 47, 51, 128, 81, 171, 1], [57, 17, 5, 71, 102, 57, 53, 41, 49], [115, 21, 2, 10, 102, 255, 166, 23, 6], [38, 33, 13, 121, 57, 73, 26, 1, 85], [41, 10, 67, 138, 77, 110, 90, 47, 114], [101, 29, 16, 10, 85, 128, 101, 196, 26], [57, 18, 10, 102, 102, 213, 34, 20, 43], [117, 20, 15, 36, 163, 128, 68, 1, 26]], [[138, 31, 36, 171, 27, 166, 38, 44, 229], [67, 87, 58, 169, 82, 115, 26, 59, 179], [63, 59, 90, 180, 59, 166, 93, 73, 154], [40, 40, 21, 116, 143, 209, 34, 39, 175], [57, 46, 22, 24, 128, 1, 54, 17, 37], [47, 15, 16, 183, 34, 223, 49, 45, 183], [46, 17, 33, 183, 6, 98, 15, 32, 183], [65, 32, 73, 115, 28, 128, 23, 128, 205], [40, 3, 9, 115, 51, 192, 18, 6, 223], [87, 37, 9, 115, 59, 77, 64, 21, 47]], [[104, 55, 44, 218, 9, 54, 53, 130, 226], [64, 90, 70, 205, 40, 41, 23, 26, 57], [54, 57, 112, 184, 5, 41, 38, 166, 213], [30, 34, 26, 133, 152, 116, 10, 32, 134], [75, 32, 12, 51, 192, 255, 160, 43, 51], [39, 19, 53, 221, 26, 114, 32, 73, 255], [31, 9, 65, 234, 2, 15, 1, 118, 73], [88, 31, 35, 67, 102, 85, 55, 186, 85], [56, 21, 23, 111, 59, 205, 45, 37, 192], [55, 38, 70, 124, 73, 102, 1, 34, 98]], [[102, 61, 71, 37, 34, 53, 31, 243, 192], [69, 60, 71, 38, 73, 119, 28, 222, 37], [68, 45, 128, 34, 1, 47, 11, 245, 171], [62, 17, 19, 70, 146, 85, 55, 62, 70], [75, 15, 9, 9, 64, 255, 184, 119, 16], [37, 43, 37, 154, 100, 163, 85, 160, 1], [63, 9, 92, 136, 28, 64, 32, 201, 85], [86, 6, 28, 5, 64, 255, 25, 248, 1], [56, 8, 17, 132, 137, 255, 55, 116, 128], [58, 15, 20, 82, 135, 57, 26, 121, 40]], [[164, 50, 31, 137, 154, 133, 25, 35, 218], [51, 103, 44, 131, 131, 123, 31, 6, 158], [86, 40, 64, 135, 148, 224, 45, 183, 128], [22, 26, 17, 131, 240, 154, 14, 1, 209], [83, 12, 13, 54, 192, 255, 68, 47, 28], [45, 16, 21, 91, 64, 222, 7, 1, 197], [56, 21, 39, 155, 60, 138, 23, 102, 213], [85, 26, 85, 85, 128, 128, 32, 146, 171], [18, 11, 7, 63, 144, 171, 4, 4, 246], [35, 27, 10, 146, 174, 171, 12, 26, 128]], [[190, 80, 35, 99, 180, 80, 126, 54, 45], [85, 126, 47, 87, 176, 51, 41, 20, 32], [101, 75, 128, 139, 118, 146, 116, 128, 85], [56, 41, 15, 176, 236, 85, 37, 9, 62], [146, 36, 19, 30, 171, 255, 97, 27, 20], [71, 30, 17, 119, 118, 255, 17, 18, 138], [101, 38, 60, 138, 55, 70, 43, 26, 142], [138, 45, 61, 62, 219, 1, 81, 188, 64], [32, 41, 20, 117, 151, 142, 20, 21, 163], [112, 19, 12, 61, 195, 128, 48, 4, 24]]];
pub const K_DEFAULT_COEFF_PROBS: [[[[u8; 11]; 3]; 8]; 4] = [[[[128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128], [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128], [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128]], [[253, 136, 254, 255, 228, 219, 128, 128, 128, 128, 128], [189, 129, 242, 255, 227, 213, 255, 219, 128, 128, 128], [106, 126, 227, 252, 214, 209, 255, 255, 128, 128, 128]], [[1, 98, 248, 255, 236, 226, 255, 255, 128, 128, 128], [181, 133, 238, 254, 221, 234, 255, 154, 128, 128, 128], [78, 134, 202, 247, 198, 180, 255, 219, 128, 128, 128]], [[1, 185, 249, 255, 243, 255, 128, 128, 128, 128, 128], [184, 150, 247, 255, 236, 224, 128, 128, 128, 128, 128], [77, 110, 216, 255, 236, 230, 128, 128, 128, 128, 128]], [[1, 101, 251, 255, 241, 255, 128, 128, 128, 128, 128], [170, 139, 241, 252, 236, 209, 255, 255, 128, 128, 128], [37, 116, 196, 243, 228, 255, 255, 255, 128, 128, 128]], [[1, 204, 254, 255, 245, 255, 128, 128, 128, 128, 128], [207, 160, 250, 255, 238, 128, 128, 128, 128, 128, 128], [102, 103, 231, 255, 211, 171, 128, 128, 128, 128, 128]], [[1, 152, 252, 255, 240, 255, 128, 128, 128, 128, 128], [177, 135, 243, 255, 234, 225, 128, 128, 128, 128, 128], [80, 129, 211, 255, 194, 224, 128, 128, 128, 128, 128]], [[1, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128], [246, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128], [255, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128]]], [[[198, 35, 237, 223, 193, 187, 162, 160, 145, 155, 62], [131, 45, 198, 221, 172, 176, 220, 157, 252, 221, 1], [68, 47, 146, 208, 149, 167, 221, 162, 255, 223, 128]], [[1, 149, 241, 255, 221, 224, 255, 255, 128, 128, 128], [184, 141, 234, 253, 222, 220, 255, 199, 128, 128, 128], [81, 99, 181, 242, 176, 190, 249, 202, 255, 255, 128]], [[1, 129, 232, 253, 214, 197, 242, 196, 255, 255, 128], [99, 121, 210, 250, 201, 198, 255, 202, 128, 128, 128], [23, 91, 163, 242, 170, 187, 247, 210, 255, 255, 128]], [[1, 200, 246, 255, 234, 255, 128, 128, 128, 128, 128], [109, 178, 241, 255, 231, 245, 255, 255, 128, 128, 128], [44, 130, 201, 253, 205, 192, 255, 255, 128, 128, 128]], [[1, 132, 239, 251, 219, 209, 255, 165, 128, 128, 128], [94, 136, 225, 251, 218, 190, 255, 255, 128, 128, 128], [22, 100, 174, 245, 186, 161, 255, 199, 128, 128, 128]], [[1, 182, 249, 255, 232, 235, 128, 128, 128, 128, 128], [124, 143, 241, 255, 227, 234, 128, 128, 128, 128, 128], [35, 77, 181, 251, 193, 211, 255, 205, 128, 128, 128]], [[1, 157, 247, 255, 236, 231, 255, 255, 128, 128, 128], [121, 141, 235, 255, 225, 227, 255, 255, 128, 128, 128], [45, 99, 188, 251, 195, 217, 255, 224, 128, 128, 128]], [[1, 1, 251, 255, 213, 255, 128, 128, 128, 128, 128], [203, 1, 248, 255, 255, 128, 128, 128, 128, 128, 128], [137, 1, 177, 255, 224, 255, 128, 128, 128, 128, 128]]], [[[253, 9, 248, 251, 207, 208, 255, 192, 128, 128, 128], [175, 13, 224, 243, 193, 185, 249, 198, 255, 255, 128], [73, 17, 171, 221, 161, 179, 236, 167, 255, 234, 128]], [[1, 95, 247, 253, 212, 183, 255, 255, 128, 128, 128], [239, 90, 244, 250, 211, 209, 255, 255, 128, 128, 128], [155, 77, 195, 248, 188, 195, 255, 255, 128, 128, 128]], [[1, 24, 239, 251, 218, 219, 255, 205, 128, 128, 128], [201, 51, 219, 255, 196, 186, 128, 128, 128, 128, 128], [69, 46, 190, 239, 201, 218, 255, 228, 128, 128, 128]], [[1, 191, 251, 255, 255, 128, 128, 128, 128, 128, 128], [223, 165, 249, 255, 213, 255, 128, 128, 128, 128, 128], [141, 124, 248, 255, 255, 128, 128, 128, 128, 128, 128]], [[1, 16, 248, 255, 255, 128, 128, 128, 128, 128, 128], [190, 36, 230, 255, 236, 255, 128, 128, 128, 128, 128], [149, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128]], [[1, 226, 255, 128, 128, 128, 128, 128, 128, 128, 128], [247, 192, 255, 128, 128, 128, 128, 128, 128, 128, 128], [240, 128, 255, 128, 128, 128, 128, 128, 128, 128, 128]], [[1, 134, 252, 255, 255, 128, 128, 128, 128, 128, 128], [213, 62, 250, 255, 255, 128, 128, 128, 128, 128, 128], [55, 93, 255, 128, 128, 128, 128, 128, 128, 128, 128]], [[128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128], [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128], [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128]]], [[[202, 24, 213, 235, 186, 191, 220, 160, 240, 175, 255], [126, 38, 182, 232, 169, 184, 228, 174, 255, 187, 128], [61, 46, 138, 219, 151, 178, 240, 170, 255, 216, 128]], [[1, 112, 230, 250, 199, 191, 247, 159, 255, 255, 128], [166, 109, 228, 252, 211, 215, 255, 174, 128, 128, 128], [39, 77, 162, 232, 172, 180, 245, 178, 255, 255, 128]], [[1, 52, 220, 246, 198, 199, 249, 220, 255, 255, 128], [124, 74, 191, 243, 183, 193, 250, 221, 255, 255, 128], [24, 71, 130, 219, 154, 170, 243, 182, 255, 255, 128]], [[1, 182, 225, 249, 219, 240, 255, 224, 128, 128, 128], [149, 150, 226, 252, 216, 205, 255, 171, 128, 128, 128], [28, 108, 170, 242, 183, 194, 254, 223, 255, 255, 128]], [[1, 81, 230, 252, 204, 203, 255, 192, 128, 128, 128], [123, 102, 209, 247, 188, 196, 255, 233, 128, 128, 128], [20, 95, 153, 243, 164, 173, 255, 203, 128, 128, 128]], [[1, 222, 248, 255, 216, 213, 128, 128, 128, 128, 128], [168, 175, 246, 252, 235, 205, 255, 255, 128, 128, 128], [47, 116, 215, 255, 211, 212, 255, 255, 128, 128, 128]], [[1, 121, 236, 253, 212, 214, 255, 255, 128, 128, 128], [141, 84, 213, 252, 201, 202, 255, 219, 128, 128, 128], [42, 80, 160, 240, 162, 185, 255, 205, 128, 128, 128]], [[1, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128], [244, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128], [238, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128]]]];
pub const K_COEFF_ENTROPY_UPDATE_PROBS: [[[[u8; 11]; 3]; 8]; 4] = [[[[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]], [[176, 246, 255, 255, 255, 255, 255, 255, 255, 255, 255], [223, 241, 252, 255, 255, 255, 255, 255, 255, 255, 255], [249, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 244, 252, 255, 255, 255, 255, 255, 255, 255, 255], [234, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255], [253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 246, 254, 255, 255, 255, 255, 255, 255, 255, 255], [239, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255], [254, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 248, 254, 255, 255, 255, 255, 255, 255, 255, 255], [251, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255], [251, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255], [254, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 254, 253, 255, 254, 255, 255, 255, 255, 255, 255], [250, 255, 254, 255, 254, 255, 255, 255, 255, 255, 255], [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]]], [[[217, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [225, 252, 241, 253, 255, 255, 254, 255, 255, 255, 255], [234, 250, 241, 250, 253, 255, 253, 254, 255, 255, 255]], [[255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255], [223, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255], [238, 253, 254, 254, 255, 255, 255, 255, 255, 255, 255]], [[255, 248, 254, 255, 255, 255, 255, 255, 255, 255, 255], [249, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 253, 255, 255, 255, 255, 255, 255, 255, 255, 255], [247, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255], [252, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255], [253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 254, 253, 255, 255, 255, 255, 255, 255, 255, 255], [250, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]]], [[[186, 251, 250, 255, 255, 255, 255, 255, 255, 255, 255], [234, 251, 244, 254, 255, 255, 255, 255, 255, 255, 255], [251, 251, 243, 253, 254, 255, 254, 255, 255, 255, 255]], [[255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255], [236, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255], [251, 253, 253, 254, 254, 255, 255, 255, 255, 255, 255]], [[255, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255], [254, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255], [254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255], [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]]], [[[248, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [250, 254, 252, 254, 255, 255, 255, 255, 255, 255, 255], [248, 254, 249, 253, 255, 255, 255, 255, 255, 255, 255]], [[255, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255], [246, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255], [252, 254, 251, 254, 254, 255, 255, 255, 255, 255, 255]], [[255, 254, 252, 255, 255, 255, 255, 255, 255, 255, 255], [248, 254, 253, 255, 255, 255, 255, 255, 255, 255, 255], [253, 255, 254, 254, 255, 255, 255, 255, 255, 255, 255]], [[255, 251, 254, 255, 255, 255, 255, 255, 255, 255, 255], [245, 251, 254, 255, 255, 255, 255, 255, 255, 255, 255], [253, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 251, 253, 255, 255, 255, 255, 255, 255, 255, 255], [252, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255], [255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 252, 255, 255, 255, 255, 255, 255, 255, 255, 255], [249, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 255, 253, 255, 255, 255, 255, 255, 255, 255, 255], [250, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]], [[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]]]];



pub fn dimensions(chunk: &[u8]) -> Result<(u32, u32), ()> {
    if chunk.len() < 10 {
        return Err(());
    }
    let tag = (chunk[0] as u32) | ((chunk[1] as u32) << 8) | ((chunk[2] as u32) << 16);
    let key_frame = tag & 1;
    let version = (tag >> 1) & 7;
    let show_frame = (tag >> 4) & 1;
    if key_frame != 0 || version != 0 || show_frame != 1 {
        return Err(());
    }
    if chunk[3..6] != [0x9d, 0x01, 0x2a] {
        return Err(());
    }
    let w_raw = (chunk[6] as u16) | ((chunk[7] as u16) << 8);
    let h_raw = (chunk[8] as u16) | ((chunk[9] as u16) << 8);
    let width = (w_raw & 0x3fff) as u32;
    let height = (h_raw & 0x3fff) as u32;
    if width == 0 || height == 0 || width > MAX_VP8_DIMENSION || height > MAX_VP8_DIMENSION {
        return Err(());
    }
    Ok((width, height))
}

pub struct BoolDecoder<'a> {
    pub input: &'a [u8],
    pub pos: usize,
    pub range: u32,
    pub value: u32,
    pub bit_count: i32,
}

impl<'a> BoolDecoder<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        if input.len() >= 2 {
            let value = ((input[0] as u32) << 8) | (input[1] as u32);
            Self {
                input,
                pos: 2,
                range: 255,
                value,
                bit_count: 0,
            }
        } else {
            Self {
                input,
                pos: 0,
                range: 255,
                value: 0,
                bit_count: 0,
            }
        }
    }

    #[inline(always)]
    pub fn get(&mut self, prob: u8) -> u8 {
        let split = 1 + (((self.range - 1) * (prob as u32)) >> 8);
        let big_split = split << 8;
        let retval = if self.value >= big_split {
            self.range -= split;
            self.value -= big_split;
            1
        } else {
            self.range = split;
            0
        };
        while self.range < 128 {
            self.value <<= 1;
            self.range <<= 1;
            self.bit_count += 1;
            if self.bit_count == 8 {
                self.bit_count = 0;
                if self.pos < self.input.len() {
                    self.value |= self.input[self.pos] as u32;
                    self.pos += 1;
                }
            }
        }
        retval
    }

    #[inline(always)]
    pub fn get_bit(&mut self) -> u8 {
        self.get(128)
    }

    #[inline(always)]
    pub fn get_uint(&mut self, bits: usize) -> u32 {
        let mut z = 0u32;
        for i in (0..bits).rev() {
            z |= (self.get_bit() as u32) << i;
        }
        z
    }

    #[inline(always)]
    pub fn get_int(&mut self, bits: usize) -> i32 {
        let z = self.get_uint(bits) as i32;
        if self.get_bit() != 0 { -z } else { z }
    }

    #[inline(always)]
    pub fn maybe_get_int(&mut self, bits: usize) -> i32 {
        if self.get_bit() != 0 {
            self.get_int(bits)
        } else {
            0
        }
    }

    pub fn read_tree(&mut self, tree: &[i32], probs: &[u8]) -> i32 {
        let mut i = 0usize;
        loop {
            let prob_idx = i >> 1;
            let prob = if prob_idx < probs.len() { probs[prob_idx] } else { 128 };
            let bit = self.get(prob) as usize;
            if i + bit >= tree.len() {
                return 0;
            }
            let next_node = tree[i + bit];
            if next_node <= 0 {
                return -next_node;
            }
            i = next_node as usize;
        }
    }
}

pub const KF_Y_MODE_TREE: [i32; 8] = [-4, 2, 4, 6, -0, -1, -2, -3];
pub const KF_Y_MODE_PROBS: [u8; 4] = [145, 156, 163, 128];

pub const UV_MODE_TREE: [i32; 6] = [-0, 2, -1, 4, -2, -3];
pub const KF_UV_MODE_PROBS: [u8; 3] = [142, 114, 183];

pub const B_MODE_TREE: [i32; 18] = [
    -0, 2,
    -1, 4,
    -2, 6,
    8, 12,
    -3, 10,
    -5, -6,
    -4, 14,
    -7, 16,
    -8, -9,
];

pub const DC_PRED: u8 = 0;
pub const V_PRED: u8 = 1;
pub const H_PRED: u8 = 2;
pub const TM_PRED: u8 = 3;
pub const B_PRED: u8 = 4;

pub const B_DC_PRED: u8 = 0;
pub const B_TM_PRED: u8 = 1;
pub const B_VE_PRED: u8 = 2;
pub const B_HE_PRED: u8 = 3;
pub const B_LD_PRED: u8 = 4;
pub const B_RD_PRED: u8 = 5;
pub const B_VR_PRED: u8 = 6;
pub const B_VL_PRED: u8 = 7;
pub const B_HD_PRED: u8 = 8;
pub const B_HU_PRED: u8 = 9;

#[inline(always)]
fn above_block_mode(curr: &MbInfo, above: &MbInfo, b: usize) -> u8 {
    if b < 4 {
        match above.y_mode {
            DC_PRED => B_DC_PRED,
            V_PRED => B_VE_PRED,
            H_PRED => B_HE_PRED,
            TM_PRED => B_TM_PRED,
            B_PRED => above.sub_modes[b + 12],
            _ => B_DC_PRED,
        }
    } else {
        curr.sub_modes[b - 4]
    }
}

#[inline(always)]
fn left_block_mode(curr: &MbInfo, left: &MbInfo, b: usize) -> u8 {
    if (b & 3) == 0 {
        match left.y_mode {
            DC_PRED => B_DC_PRED,
            V_PRED => B_VE_PRED,
            H_PRED => B_HE_PRED,
            TM_PRED => B_TM_PRED,
            B_PRED => left.sub_modes[b + 3],
            _ => B_DC_PRED,
        }
    } else {
        curr.sub_modes[b - 1]
    }
}

#[derive(Clone, Copy, Default)]
pub struct DequantFactors {
    pub quant_idx: i32,
    pub y1: [i16; 2],
    pub uv: [i16; 2],
    pub y2: [i16; 2],
}

#[inline(always)]
fn clamp_q(q: i32) -> usize {
    if q < 0 { 0 } else if q > 127 { 127 } else { q as usize }
}

#[inline(always)]
fn dc_q(q: i32) -> i16 {
    DC_Q_LOOKUP[clamp_q(q)]
}

#[inline(always)]
fn ac_q(q: i32) -> i16 {
    AC_Q_LOOKUP[clamp_q(q)]
}

#[derive(Default)]
pub struct SegmentHdr {
    pub enabled: bool,
    pub update_map: bool,
    pub update_data: bool,
    pub abs: bool,
    pub quant_idx: [i32; 4],
    pub lf_level: [i32; 4],
    pub tree_probs: [u8; 3],
}

#[derive(Default)]
pub struct QuantHdr {
    pub q_index: u8,
    pub y1_dc_delta: i32,
    pub y2_dc_delta: i32,
    pub y2_ac_delta: i32,
    pub uv_dc_delta: i32,
    pub uv_ac_delta: i32,
    pub delta_update: bool,
}

fn dequant_init(
    factors: &mut [DequantFactors; 4],
    seg: &SegmentHdr,
    quant_hdr: &QuantHdr,
) {
    let count = if seg.enabled { 4 } else { 1 };
    for i in 0..count {
        let mut q = quant_hdr.q_index as i32;
        if seg.enabled {
            q = if !seg.abs { q + seg.quant_idx[i] } else { seg.quant_idx[i] };
        }
        let dqf = &mut factors[i];
        if dqf.quant_idx != q || quant_hdr.delta_update {
            dqf.y1[0] = dc_q(q + quant_hdr.y1_dc_delta);
            dqf.y1[1] = ac_q(q);
            dqf.uv[0] = dc_q(q + quant_hdr.uv_dc_delta);
            if dqf.uv[0] > 132 {
                dqf.uv[0] = 132;
            }
            dqf.uv[1] = ac_q(q + quant_hdr.uv_ac_delta);
            dqf.y2[0] = dc_q(q + quant_hdr.y2_dc_delta) * 2;
            let mut y2_ac = ac_q(q + quant_hdr.y2_ac_delta) as i32 * 155 / 100;
            if y2_ac < 8 {
                y2_ac = 8;
            }
            dqf.y2[1] = y2_ac as i16;
            dqf.quant_idx = q;
        }
    }
}

pub const ZIGZAG: [usize; 16] = [
    0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15,
];

pub const COEFF_BANDS: [usize; 16] = [
    0, 1, 2, 3, 6, 4, 5, 6, 6, 6, 6, 6, 6, 6, 6, 7,
];

pub const LEFT_CONTEXT_INDEX: [usize; 25] = [
    0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8,
];

pub const ABOVE_CONTEXT_INDEX: [usize; 25] = [
    0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 4, 5, 4, 5, 6, 7, 6, 7, 8,
];

pub const EXTRABITS_CAT6: [u8; 11] = [129, 130, 133, 140, 153, 177, 196, 230, 243, 254, 254];

pub fn vp8_walsh(input: &[i16], output: &mut [i16]) {
    let mut op = [0i32; 16];
    for i in 0..4 {
        let a1 = input[i] as i32 + input[i + 12] as i32;
        let b1 = input[i + 4] as i32 + input[i + 8] as i32;
        let c1 = input[i + 4] as i32 - input[i + 8] as i32;
        let d1 = input[i] as i32 - input[i + 12] as i32;
        op[i] = a1 + b1;
        op[i + 4] = c1 + d1;
        op[i + 8] = a1 - b1;
        op[i + 12] = d1 - c1;
    }
    for i in 0..4 {
        let a1 = op[i * 4] + op[i * 4 + 3];
        let b1 = op[i * 4 + 1] + op[i * 4 + 2];
        let c1 = op[i * 4 + 1] - op[i * 4 + 2];
        let d1 = op[i * 4] - op[i * 4 + 3];
        let a2 = a1 + b1;
        let b2 = c1 + d1;
        let c2 = a1 - b1;
        let d2 = d1 - c1;
        output[i * 4] = ((a2 + 3) >> 3) as i16;
        output[i * 4 + 1] = ((b2 + 3) >> 3) as i16;
        output[i * 4 + 2] = ((c2 + 3) >> 3) as i16;
        output[i * 4 + 3] = ((d2 + 3) >> 3) as i16;
    }
}

const COSPI8SQRT2MINUS1: i32 = 20091;
const SINPI8SQRT2: i32 = 35468;

fn idct_columns(input: &[i16], output: &mut [i16; 16]) {
    for i in 0..4 {
        let a1 = input[i] as i32 + input[i + 8] as i32;
        let b1 = input[i] as i32 - input[i + 8] as i32;

        let temp1 = (input[i + 4] as i32 * SINPI8SQRT2) >> 16;
        let temp2 = input[i + 12] as i32 + ((input[i + 12] as i32 * COSPI8SQRT2MINUS1) >> 16);
        let c1 = temp1 - temp2;

        let temp1 = input[i + 4] as i32 + ((input[i + 4] as i32 * COSPI8SQRT2MINUS1) >> 16);
        let temp2 = (input[i + 12] as i32 * SINPI8SQRT2) >> 16;
        let d1 = temp1 + temp2;

        output[0 * 4 + i] = (a1 + d1) as i16;
        output[3 * 4 + i] = (a1 - d1) as i16;
        output[1 * 4 + i] = (b1 + c1) as i16;
        output[2 * 4 + i] = (b1 - c1) as i16;
    }
}

#[inline(always)]
fn clamp_u8(v: i32) -> u8 {
    if v < 0 { 0 } else if v > 255 { 255 } else { v as u8 }
}

pub fn vp8_idct_add(recon: &mut [u8], offset: usize, stride: usize, coeffs: &[i16]) {
    let mut tmp = [0i16; 16];
    idct_columns(coeffs, &mut tmp);

    for i in 0..4 {
        let a1 = tmp[i * 4 + 0] as i32 + tmp[i * 4 + 2] as i32;
        let b1 = tmp[i * 4 + 0] as i32 - tmp[i * 4 + 2] as i32;

        let temp1 = (tmp[i * 4 + 1] as i32 * SINPI8SQRT2) >> 16;
        let temp2 = tmp[i * 4 + 3] as i32 + ((tmp[i * 4 + 3] as i32 * COSPI8SQRT2MINUS1) >> 16);
        let c1 = temp1 - temp2;

        let temp1 = tmp[i * 4 + 1] as i32 + ((tmp[i * 4 + 1] as i32 * COSPI8SQRT2MINUS1) >> 16);
        let temp2 = (tmp[i * 4 + 3] as i32 * SINPI8SQRT2) >> 16;
        let d1 = temp1 + temp2;

        let row_off = offset + i * stride;
        recon[row_off + 0] = clamp_u8(recon[row_off + 0] as i32 + ((a1 + d1 + 4) >> 3));
        recon[row_off + 3] = clamp_u8(recon[row_off + 3] as i32 + ((a1 - d1 + 4) >> 3));
        recon[row_off + 1] = clamp_u8(recon[row_off + 1] as i32 + ((b1 + c1 + 4) >> 3));
        recon[row_off + 2] = clamp_u8(recon[row_off + 2] as i32 + ((b1 - c1 + 4) >> 3));
    }
}



#[inline(always)]
fn predict_dc_nxn(plane: &mut [u8], offset: usize, stride: usize, n: usize) {
    let mut dc = 0i32;
    for i in 0..n {
        let left = plane[offset + i * stride - 1] as i32;
        let above = plane[offset + i - stride] as i32;
        dc += left + above;
    }
    dc = match n {
        16 => (dc + 16) >> 5,
        8 => (dc + 8) >> 4,
        _ => (dc + 4) >> 3,
    };
    let val = dc as u8;
    for i in 0..n {
        for j in 0..n {
            plane[offset + i * stride + j] = val;
        }
    }
}

#[inline(always)]
fn predict_v_nxn(plane: &mut [u8], offset: usize, stride: usize, n: usize) {
    for i in 0..n {
        for j in 0..n {
            plane[offset + i * stride + j] = plane[offset + j - stride];
        }
    }
}

#[inline(always)]
fn predict_h_nxn(plane: &mut [u8], offset: usize, stride: usize, n: usize) {
    for i in 0..n {
        let val = plane[offset + i * stride - 1];
        for j in 0..n {
            plane[offset + i * stride + j] = val;
        }
    }
}

#[inline(always)]
fn predict_tm_nxn(plane: &mut [u8], offset: usize, stride: usize, n: usize) {
    let top_left = plane[offset - stride - 1] as i32;
    for j in 0..n {
        let left = plane[offset + j * stride - 1] as i32;
        for i in 0..n {
            let above = plane[offset + i - stride] as i32;
            plane[offset + j * stride + i] = clamp_u8(left + above - top_left);
        }
    }
}

fn predict_ve_4x4(plane: &mut [u8], offset: usize, stride: usize) {
    let a_m1 = plane[offset - stride - 1] as i32;
    let a0 = plane[offset - stride] as i32;
    let a1 = plane[offset - stride + 1] as i32;
    let a2 = plane[offset - stride + 2] as i32;
    let a3 = plane[offset - stride + 3] as i32;
    let a4 = plane[offset - stride + 4] as i32;

    let p0 = (a_m1 + 2 * a0 + a1 + 2) >> 2;
    let p1 = (a0 + 2 * a1 + a2 + 2) >> 2;
    let p2 = (a1 + 2 * a2 + a3 + 2) >> 2;
    let p3 = (a2 + 2 * a3 + a4 + 2) >> 2;

    for i in 0..4 {
        plane[offset + i * stride + 0] = p0 as u8;
        plane[offset + i * stride + 1] = p1 as u8;
        plane[offset + i * stride + 2] = p2 as u8;
        plane[offset + i * stride + 3] = p3 as u8;
    }
}

fn predict_he_4x4(plane: &mut [u8], offset: usize, stride: usize) {
    for i in 0..4 {
        let left_m_stride = plane[offset + i * stride - stride - 1] as i32;
        let left_0 = plane[offset + i * stride - 1] as i32;
        let left_p_stride = if i < 3 {
            plane[offset + (i + 1) * stride - 1] as i32
        } else {
            // Reference quirk: last line uses left[0] twice!
            left_0
        };
        let val = ((left_m_stride + 2 * left_0 + left_p_stride + 2) >> 2) as u8;
        plane[offset + i * stride + 0] = val;
        plane[offset + i * stride + 1] = val;
        plane[offset + i * stride + 2] = val;
        plane[offset + i * stride + 3] = val;
    }
}

fn predict_ld_4x4(plane: &mut [u8], offset: usize, stride: usize) {
    let mut a = [0i32; 8];
    for k in 0..8 {
        a[k] = plane[offset + k - stride] as i32;
    }
    let p0 = (a[0] + 2 * a[1] + a[2] + 2) >> 2;
    let p1 = (a[1] + 2 * a[2] + a[3] + 2) >> 2;
    let p2 = (a[2] + 2 * a[3] + a[4] + 2) >> 2;
    let p3 = (a[3] + 2 * a[4] + a[5] + 2) >> 2;
    let p4 = (a[4] + 2 * a[5] + a[6] + 2) >> 2;
    let p5 = (a[5] + 2 * a[6] + a[7] + 2) >> 2;
    // Reference quirk: last uses above[7] twice!
    let p6 = (a[6] + 2 * a[7] + a[7] + 2) >> 2;

    plane[offset + 0 * stride + 0] = p0 as u8;
    plane[offset + 0 * stride + 1] = p1 as u8;
    plane[offset + 0 * stride + 2] = p2 as u8;
    plane[offset + 0 * stride + 3] = p3 as u8;

    plane[offset + 1 * stride + 0] = p1 as u8;
    plane[offset + 1 * stride + 1] = p2 as u8;
    plane[offset + 1 * stride + 2] = p3 as u8;
    plane[offset + 1 * stride + 3] = p4 as u8;

    plane[offset + 2 * stride + 0] = p2 as u8;
    plane[offset + 2 * stride + 1] = p3 as u8;
    plane[offset + 2 * stride + 2] = p4 as u8;
    plane[offset + 2 * stride + 3] = p5 as u8;

    plane[offset + 3 * stride + 0] = p3 as u8;
    plane[offset + 3 * stride + 1] = p4 as u8;
    plane[offset + 3 * stride + 2] = p5 as u8;
    plane[offset + 3 * stride + 3] = p6 as u8;
}

fn predict_rd_4x4(plane: &mut [u8], offset: usize, stride: usize) {
    let l0 = plane[offset - 1] as i32;
    let l1 = plane[offset + stride - 1] as i32;
    let l2 = plane[offset + 2 * stride - 1] as i32;
    let l3 = plane[offset + 3 * stride - 1] as i32;
    let a_m1 = plane[offset - stride - 1] as i32;
    let a0 = plane[offset - stride] as i32;
    let a1 = plane[offset - stride + 1] as i32;
    let a2 = plane[offset - stride + 2] as i32;
    let a3 = plane[offset - stride + 3] as i32;

    let p0 = (l0 + 2 * a_m1 + a0 + 2) >> 2;
    let p1 = (a_m1 + 2 * a0 + a1 + 2) >> 2;
    let p2 = (a0 + 2 * a1 + a2 + 2) >> 2;
    let p3 = (a1 + 2 * a2 + a3 + 2) >> 2;
    let p4 = (l1 + 2 * l0 + a_m1 + 2) >> 2;
    let p5 = (l2 + 2 * l1 + l0 + 2) >> 2;
    let p6 = (l3 + 2 * l2 + l1 + 2) >> 2;

    plane[offset + 0 * stride + 0] = p0 as u8;
    plane[offset + 0 * stride + 1] = p1 as u8;
    plane[offset + 0 * stride + 2] = p2 as u8;
    plane[offset + 0 * stride + 3] = p3 as u8;

    plane[offset + 1 * stride + 0] = p4 as u8;
    plane[offset + 1 * stride + 1] = p0 as u8;
    plane[offset + 1 * stride + 2] = p1 as u8;
    plane[offset + 1 * stride + 3] = p2 as u8;

    plane[offset + 2 * stride + 0] = p5 as u8;
    plane[offset + 2 * stride + 1] = p4 as u8;
    plane[offset + 2 * stride + 2] = p0 as u8;
    plane[offset + 2 * stride + 3] = p1 as u8;

    plane[offset + 3 * stride + 0] = p6 as u8;
    plane[offset + 3 * stride + 1] = p5 as u8;
    plane[offset + 3 * stride + 2] = p4 as u8;
    plane[offset + 3 * stride + 3] = p0 as u8;
}

fn predict_vr_4x4(plane: &mut [u8], offset: usize, stride: usize) {
    let l0 = plane[offset - 1] as i32;
    let l1 = plane[offset + stride - 1] as i32;
    let l2 = plane[offset + 2 * stride - 1] as i32;
    let a_m1 = plane[offset - stride - 1] as i32;
    let a0 = plane[offset - stride] as i32;
    let a1 = plane[offset - stride + 1] as i32;
    let a2 = plane[offset - stride + 2] as i32;
    let a3 = plane[offset - stride + 3] as i32;

    let p0 = (a_m1 + a0 + 1) >> 1;
    let p1 = (a0 + a1 + 1) >> 1;
    let p2 = (a1 + a2 + 1) >> 1;
    let p3 = (a2 + a3 + 1) >> 1;

    let p4 = (l0 + 2 * a_m1 + a0 + 2) >> 2;
    let p5 = (a_m1 + 2 * a0 + a1 + 2) >> 2;
    let p6 = (a0 + 2 * a1 + a2 + 2) >> 2;
    let p7 = (a1 + 2 * a2 + a3 + 2) >> 2;

    let p8 = (l1 + 2 * l0 + a_m1 + 2) >> 2;
    let p9 = (l2 + 2 * l1 + l0 + 2) >> 2;

    plane[offset + 0 * stride + 0] = p0 as u8;
    plane[offset + 0 * stride + 1] = p1 as u8;
    plane[offset + 0 * stride + 2] = p2 as u8;
    plane[offset + 0 * stride + 3] = p3 as u8;

    plane[offset + 1 * stride + 0] = p4 as u8;
    plane[offset + 1 * stride + 1] = p5 as u8;
    plane[offset + 1 * stride + 2] = p6 as u8;
    plane[offset + 1 * stride + 3] = p7 as u8;

    plane[offset + 2 * stride + 0] = p8 as u8;
    plane[offset + 2 * stride + 1] = p0 as u8;
    plane[offset + 2 * stride + 2] = p1 as u8;
    plane[offset + 2 * stride + 3] = p2 as u8;

    plane[offset + 3 * stride + 0] = p9 as u8;
    plane[offset + 3 * stride + 1] = p4 as u8;
    plane[offset + 3 * stride + 2] = p5 as u8;
    plane[offset + 3 * stride + 3] = p6 as u8;
}

fn predict_vl_4x4(plane: &mut [u8], offset: usize, stride: usize) {
    let mut a = [0i32; 8];
    for k in 0..8 {
        a[k] = plane[offset + k - stride] as i32;
    }
    let p0 = (a[0] + a[1] + 1) >> 1;
    let p1 = (a[1] + a[2] + 1) >> 1;
    let p2 = (a[2] + a[3] + 1) >> 1;
    let p3 = (a[3] + a[4] + 1) >> 1;

    let p4 = (a[0] + 2 * a[1] + a[2] + 2) >> 2;
    let p5 = (a[1] + 2 * a[2] + a[3] + 2) >> 2;
    let p6 = (a[2] + 2 * a[3] + a[4] + 2) >> 2;
    let p7 = (a[3] + 2 * a[4] + a[5] + 2) >> 2;

    let p8 = (a[4] + 2 * a[5] + a[6] + 2) >> 2;
    let p9 = (a[5] + 2 * a[6] + a[7] + 2) >> 2;

    plane[offset + 0 * stride + 0] = p0 as u8;
    plane[offset + 0 * stride + 1] = p1 as u8;
    plane[offset + 0 * stride + 2] = p2 as u8;
    plane[offset + 0 * stride + 3] = p3 as u8;

    plane[offset + 1 * stride + 0] = p4 as u8;
    plane[offset + 1 * stride + 1] = p5 as u8;
    plane[offset + 1 * stride + 2] = p6 as u8;
    plane[offset + 1 * stride + 3] = p7 as u8;

    plane[offset + 2 * stride + 0] = p1 as u8;
    plane[offset + 2 * stride + 1] = p2 as u8;
    plane[offset + 2 * stride + 2] = p3 as u8;
    plane[offset + 2 * stride + 3] = p8 as u8;

    plane[offset + 3 * stride + 0] = p5 as u8;
    plane[offset + 3 * stride + 1] = p6 as u8;
    plane[offset + 3 * stride + 2] = p7 as u8;
    plane[offset + 3 * stride + 3] = p9 as u8;
}

fn predict_hd_4x4(plane: &mut [u8], offset: usize, stride: usize) {
    let l0 = plane[offset - 1] as i32;
    let l1 = plane[offset + stride - 1] as i32;
    let l2 = plane[offset + 2 * stride - 1] as i32;
    let l3 = plane[offset + 3 * stride - 1] as i32;
    let a_m1 = plane[offset - stride - 1] as i32;
    let a0 = plane[offset - stride] as i32;
    let a1 = plane[offset - stride + 1] as i32;
    let a2 = plane[offset - stride + 2] as i32;

    let p0 = (l0 + a_m1 + 1) >> 1;
    let p1 = (l0 + 2 * a_m1 + a0 + 2) >> 2;
    let p2 = (a_m1 + 2 * a0 + a1 + 2) >> 2;
    let p3 = (a0 + 2 * a1 + a2 + 2) >> 2;

    let p4 = (l1 + l0 + 1) >> 1;
    let p5 = (l1 + 2 * l0 + a_m1 + 2) >> 2;

    let p6 = (l2 + l1 + 1) >> 1;
    let p7 = (l2 + 2 * l1 + l0 + 2) >> 2;

    let p8 = (l3 + l2 + 1) >> 1;
    let p9 = (l3 + 2 * l2 + l1 + 2) >> 2;

    plane[offset + 0 * stride + 0] = p0 as u8;
    plane[offset + 0 * stride + 1] = p1 as u8;
    plane[offset + 0 * stride + 2] = p2 as u8;
    plane[offset + 0 * stride + 3] = p3 as u8;

    plane[offset + 1 * stride + 0] = p4 as u8;
    plane[offset + 1 * stride + 1] = p5 as u8;
    plane[offset + 1 * stride + 2] = p0 as u8;
    plane[offset + 1 * stride + 3] = p1 as u8;

    plane[offset + 2 * stride + 0] = p6 as u8;
    plane[offset + 2 * stride + 1] = p7 as u8;
    plane[offset + 2 * stride + 2] = p4 as u8;
    plane[offset + 2 * stride + 3] = p5 as u8;

    plane[offset + 3 * stride + 0] = p8 as u8;
    plane[offset + 3 * stride + 1] = p9 as u8;
    plane[offset + 3 * stride + 2] = p6 as u8;
    plane[offset + 3 * stride + 3] = p7 as u8;
}

fn predict_hu_4x4(plane: &mut [u8], offset: usize, stride: usize) {
    let l0 = plane[offset - 1] as i32;
    let l1 = plane[offset + stride - 1] as i32;
    let l2 = plane[offset + 2 * stride - 1] as i32;
    let l3 = plane[offset + 3 * stride - 1] as i32;

    let p0 = (l0 + l1 + 1) >> 1;
    let p1 = (l0 + 2 * l1 + l2 + 2) >> 2;
    let p2 = (l1 + l2 + 1) >> 1;
    let p3 = (l1 + 2 * l2 + l3 + 2) >> 2;
    let p4 = (l2 + l3 + 1) >> 1;
    let p5 = (l2 + 2 * l3 + l3 + 2) >> 2;
    let p6 = l3 as u8;

    plane[offset + 0 * stride + 0] = p0 as u8;
    plane[offset + 0 * stride + 1] = p1 as u8;
    plane[offset + 0 * stride + 2] = p2 as u8;
    plane[offset + 0 * stride + 3] = p3 as u8;

    plane[offset + 1 * stride + 0] = p2 as u8;
    plane[offset + 1 * stride + 1] = p3 as u8;
    plane[offset + 1 * stride + 2] = p4 as u8;
    plane[offset + 1 * stride + 3] = p5 as u8;

    plane[offset + 2 * stride + 0] = p4 as u8;
    plane[offset + 2 * stride + 1] = p5 as u8;
    plane[offset + 2 * stride + 2] = p6;
    plane[offset + 2 * stride + 3] = p6;

    plane[offset + 3 * stride + 0] = p6;
    plane[offset + 3 * stride + 1] = p6;
    plane[offset + 3 * stride + 2] = p6;
    plane[offset + 3 * stride + 3] = p6;
}

fn copy_down(plane: &mut [u8], offset: usize, stride: usize) {
    let src_off = offset + 16 - stride;
    let b0 = plane[src_off];
    let b1 = plane[src_off + 1];
    let b2 = plane[src_off + 2];
    let b3 = plane[src_off + 3];

    for step in 1..4 {
        let dst_off = src_off + step * stride * 4;
        plane[dst_off] = b0;
        plane[dst_off + 1] = b1;
        plane[dst_off + 2] = b2;
        plane[dst_off + 3] = b3;
    }
}



fn fixup_left(plane: &mut [u8], offset: usize, width: usize, stride: usize, row: usize, mode: u8) {
    let mut left_off = offset - 1;
    if mode == DC_PRED && row > 0 {
        let above_off = offset - stride;
        for i in 0..width {
            plane[left_off] = plane[above_off + i];
            left_off += stride;
        }
    } else {
        left_off = left_off.wrapping_sub(stride);
        for _ in 0..=width {
            plane[left_off] = 129;
            left_off += stride;
        }
    }
}

fn fixup_above(plane: &mut [u8], offset: usize, width: usize, stride: usize, col: usize, mode: u8) {
    let above_off = offset - stride;
    if mode == DC_PRED && col > 0 {
        let mut left_off = offset - 1;
        for i in 0..width {
            plane[above_off + i] = plane[left_off];
            left_off += stride;
        }
    } else {
        for i in 0..=width {
            plane[above_off - 1 + i] = 127;
        }
    }
    for i in 0..4 {
        plane[above_off + width + i] = 127;
    }
}

fn decode_mb_tokens(
    bool_dec: &mut BoolDecoder,
    left: &mut [u8; 9],
    above: &mut [u8; 9],
    tokens: &mut [i16],
    mode: u8,
    coeff_probs: &[[[[u8; 11]; 3]; 8]; 4],
    dqf: &DequantFactors,
) -> u32 {
    let mut eob_mask = 0u32;
    let (mut i, mut stop, mut block_type, mut curr_dq) = if mode != B_PRED {
        (24usize, 24usize, 1usize, dqf.y2)
    } else {
        (0usize, 15usize, 3usize, dqf.y1)
    };

    loop {
        let mut t = (left[LEFT_CONTEXT_INDEX[i]] + above[ABOVE_CONTEXT_INDEX[i]]) as usize;
        let first_c = if block_type == 0 { 1usize } else { 0usize };
        let b_tokens = &mut tokens[i * 16..(i + 1) * 16];

        // Zero out block coefficients
        b_tokens.fill(0);

        let mut c = first_c;
        let mut skip = false;
        let mut had_data = false;
        'block: while c < 16 {
            let band = COEFF_BANDS[c] as usize;
            let prob = &coeff_probs[block_type][band][t];

            // EOB test
            if !skip {
                if bool_dec.get(prob[0]) == 0 {
                    break 'block;
                }
            }

            // Zero test
            if bool_dec.get(prob[1]) == 0 {
                skip = true;
                had_data = true;
                t = 0;
                c += 1;
                continue;
            }
            skip = false;
            had_data = true;

            // Decode token value
            let val = if bool_dec.get(prob[2]) == 0 {
                1i32
            } else if bool_dec.get(prob[3]) == 0 {
                if bool_dec.get(prob[4]) == 0 {
                    2i32
                } else if bool_dec.get(prob[5]) == 0 {
                    3i32
                } else {
                    4i32
                }
            } else if bool_dec.get(prob[6]) == 0 {
                if bool_dec.get(prob[7]) == 0 {
                    5i32 + (bool_dec.get(159) as i32)
                } else {
                    let b1 = bool_dec.get(165) as i32;
                    let b0 = bool_dec.get(145) as i32;
                    7i32 + (b1 << 1) + b0
                }
            } else if bool_dec.get(prob[8]) == 0 {
                if bool_dec.get(prob[9]) == 0 {
                    let b2 = bool_dec.get(173) as i32;
                    let b1 = bool_dec.get(148) as i32;
                    let b0 = bool_dec.get(140) as i32;
                    11i32 + (b2 << 2) + (b1 << 1) + b0
                } else {
                    let b3 = bool_dec.get(176) as i32;
                    let b2 = bool_dec.get(155) as i32;
                    let b1 = bool_dec.get(140) as i32;
                    let b0 = bool_dec.get(135) as i32;
                    19i32 + (b3 << 3) + (b2 << 2) + (b1 << 1) + b0
                }
            } else if bool_dec.get(prob[10]) == 0 {
                let b4 = bool_dec.get(180) as i32;
                let b3 = bool_dec.get(157) as i32;
                let b2 = bool_dec.get(141) as i32;
                let b1 = bool_dec.get(134) as i32;
                let b0 = bool_dec.get(130) as i32;
                35i32 + (b4 << 4) + (b3 << 3) + (b2 << 2) + (b1 << 1) + b0
            } else {
                let mut extra = 0i32;
                for b_idx in 0..11 {
                    extra = (extra << 1) | (bool_dec.get(EXTRABITS_CAT6[10 - b_idx]) as i32);
                }
                67i32 + extra
            };

            let sign = bool_dec.get_bit();
            let signed_val = if sign != 0 { -val } else { val };
            let dq = if c == 0 { curr_dq[0] } else { curr_dq[1] };
            b_tokens[ZIGZAG[c]] = (signed_val * (dq as i32)) as i16;

            t = if val == 1 { 1 } else { 2 };
            c += 1;
        }

        if c > 1 {
            eob_mask |= 1 << i;
        }
        if had_data {
            eob_mask |= 1 << 31;
        }
        left[LEFT_CONTEXT_INDEX[i]] = if had_data { 1 } else { 0 };
        above[ABOVE_CONTEXT_INDEX[i]] = if had_data { 1 } else { 0 };

        i += 1;
        if i <= stop {
            continue;
        }

        if i == 25 {
            block_type = 0;
            i = 0;
            stop = 15;
            curr_dq = dqf.y1;
            continue;
        }

        if i == 16 {
            block_type = 2;
            stop = 23;
            curr_dq = dqf.uv;
            continue;
        }

        break;
    }

    eob_mask
}



#[derive(Default)]
pub struct LoopFilterHdr {
    pub use_simple: bool,
    pub level: u8,
    pub sharpness: u8,
    pub delta_enabled: bool,
    pub ref_delta: [i32; 4],
    pub mode_delta: [i32; 4],
}

fn calculate_filter_parameters(
    lf_hdr: &LoopFilterHdr,
    seg: &SegmentHdr,
    mbi: &MbInfo,
    edge_limit_: &mut i32,
    interior_limit_: &mut i32,
    hev_threshold_: &mut i32,
) {
    let mut filter_level = lf_hdr.level as i32;
    if seg.enabled {
        if !seg.abs {
            filter_level += seg.lf_level[mbi.segment_id as usize];
        } else {
            filter_level = seg.lf_level[mbi.segment_id as usize];
        }
    }
    filter_level = filter_level.clamp(0, 63);

    if lf_hdr.delta_enabled {
        filter_level += lf_hdr.ref_delta[0]; // Ref frame 0 (CURRENT_FRAME)
        if mbi.y_mode == B_PRED {
            filter_level += lf_hdr.mode_delta[0];
        } else {
            filter_level += lf_hdr.mode_delta[2];
        }
    }
    filter_level = filter_level.clamp(0, 63);

    let mut interior_limit = filter_level;
    if lf_hdr.sharpness > 0 {
        interior_limit >>= if lf_hdr.sharpness > 4 { 2 } else { 1 };
        let max_int = 9 - lf_hdr.sharpness as i32;
        if interior_limit > max_int {
            interior_limit = max_int;
        }
    }
    if interior_limit < 1 {
        interior_limit = 1;
    }

    let mut hev_threshold = (filter_level >= 15) as i32;
    if filter_level >= 40 {
        hev_threshold += 1;
    }

    *edge_limit_ = filter_level;
    *interior_limit_ = interior_limit;
    *hev_threshold_ = hev_threshold;
}

#[inline(always)]
fn saturate_int8(x: i32) -> i32 {
    if x < -128 { -128 } else if x > 127 { 127 } else { x }
}

#[inline(always)]
fn simple_threshold(plane: &[u8], offset: usize, stride: usize, filter_limit: i32) -> bool {
    let p0 = plane[offset - stride] as i32;
    let p1 = plane[offset - 2 * stride] as i32;
    let q0 = plane[offset] as i32;
    let q1 = plane[offset + stride] as i32;
    ((p0 - q0).abs() * 2 + ((p1 - q1).abs() >> 1)) <= filter_limit
}

#[inline(always)]
fn normal_threshold(plane: &[u8], offset: usize, stride: usize, edge_limit: i32, interior_limit: i32) -> bool {
    let e = edge_limit;
    let i = interior_limit;
    if !simple_threshold(plane, offset, stride, 2 * e + i) {
        return false;
    }
    let p3 = plane[offset - 4 * stride] as i32;
    let p2 = plane[offset - 3 * stride] as i32;
    let p1 = plane[offset - 2 * stride] as i32;
    let p0 = plane[offset - stride] as i32;
    let q0 = plane[offset] as i32;
    let q1 = plane[offset + stride] as i32;
    let q2 = plane[offset + 2 * stride] as i32;
    let q3 = plane[offset + 3 * stride] as i32;

    (p3 - p2).abs() <= i && (p2 - p1).abs() <= i && (p1 - p0).abs() <= i
        && (q3 - q2).abs() <= i && (q2 - q1).abs() <= i && (q1 - q0).abs() <= i
}

#[inline(always)]
fn high_edge_variance(plane: &[u8], offset: usize, stride: usize, hev_threshold: i32) -> bool {
    let p0 = plane[offset - stride] as i32;
    let p1 = plane[offset - 2 * stride] as i32;
    let q0 = plane[offset] as i32;
    let q1 = plane[offset + stride] as i32;
    (p1 - p0).abs() > hev_threshold || (q1 - q0).abs() > hev_threshold
}

#[inline(always)]
fn filter_common(plane: &mut [u8], offset: usize, stride: usize, use_outer_taps: bool) {
    let p1_val = plane[offset - 2 * stride] as i32;
    let p0_val = plane[offset - stride] as i32;
    let q0_val = plane[offset] as i32;
    let q1_val = plane[offset + stride] as i32;

    let mut a = 3 * (q0_val - p0_val);
    if use_outer_taps {
        a += saturate_int8(p1_val - q1_val);
    }
    a = saturate_int8(a);

    let f1 = (if a + 4 > 127 { 127 } else { a + 4 }) >> 3;
    let f2 = (if a + 3 > 127 { 127 } else { a + 3 }) >> 3;

    plane[offset - stride] = clamp_u8(p0_val + f2);
    plane[offset] = clamp_u8(q0_val - f1);

    if !use_outer_taps {
        let a_sub = (f1 + 1) >> 1;
        plane[offset - 2 * stride] = clamp_u8(p1_val + a_sub);
        plane[offset + stride] = clamp_u8(q1_val - a_sub);
    }
}

#[inline(always)]
fn filter_mb_edge(plane: &mut [u8], offset: usize, stride: usize) {
    let p2_val = plane[offset - 3 * stride] as i32;
    let p1_val = plane[offset - 2 * stride] as i32;
    let p0_val = plane[offset - stride] as i32;
    let q0_val = plane[offset] as i32;
    let q1_val = plane[offset + stride] as i32;
    let q2_val = plane[offset + 2 * stride] as i32;

    let w = saturate_int8(saturate_int8(p1_val - q1_val) + 3 * (q0_val - p0_val));

    let a_p0 = (27 * w + 63) >> 7;
    plane[offset - stride] = clamp_u8(p0_val + a_p0);
    plane[offset] = clamp_u8(q0_val - a_p0);

    let a_p1 = (18 * w + 63) >> 7;
    plane[offset - 2 * stride] = clamp_u8(p1_val + a_p1);
    plane[offset + stride] = clamp_u8(q1_val - a_p1);

    let a_p2 = (9 * w + 63) >> 7;
    plane[offset - 3 * stride] = clamp_u8(p2_val + a_p2);
    plane[offset + 2 * stride] = clamp_u8(q2_val - a_p2);
}

fn filter_mb_v_edge(src: &mut [u8], offset: usize, stride: usize, edge_limit: i32, interior_limit: i32, hev_threshold: i32, size: usize) {
    for i in 0..(8 * size) {
        let row_off = offset + i * stride;
        if normal_threshold(src, row_off, 1, edge_limit, interior_limit) {
            if high_edge_variance(src, row_off, 1, hev_threshold) {
                filter_common(src, row_off, 1, true);
            } else {
                filter_mb_edge(src, row_off, 1);
            }
        }
    }
}

fn filter_subblock_v_edge(src: &mut [u8], offset: usize, stride: usize, edge_limit: i32, interior_limit: i32, hev_threshold: i32, size: usize) {
    for i in 0..(8 * size) {
        let row_off = offset + i * stride;
        if normal_threshold(src, row_off, 1, edge_limit, interior_limit) {
            let hev = high_edge_variance(src, row_off, 1, hev_threshold);
            filter_common(src, row_off, 1, hev);
        }
    }
}

fn filter_mb_h_edge(src: &mut [u8], offset: usize, stride: usize, edge_limit: i32, interior_limit: i32, hev_threshold: i32, size: usize) {
    for col in 0..(8 * size) {
        let col_off = offset + col;
        if normal_threshold(src, col_off, stride, edge_limit, interior_limit) {
            if high_edge_variance(src, col_off, stride, hev_threshold) {
                filter_common(src, col_off, stride, true);
            } else {
                filter_mb_edge(src, col_off, stride);
            }
        }
    }
}

fn filter_subblock_h_edge(src: &mut [u8], offset: usize, stride: usize, edge_limit: i32, interior_limit: i32, hev_threshold: i32, size: usize) {
    for col in 0..(8 * size) {
        let col_off = offset + col;
        if normal_threshold(src, col_off, stride, edge_limit, interior_limit) {
            let hev = high_edge_variance(src, col_off, stride, hev_threshold);
            filter_common(src, col_off, stride, hev);
        }
    }
}

fn filter_v_edge_simple(src: &mut [u8], offset: usize, stride: usize, filter_limit: i32) {
    for i in 0..16 {
        let row_off = offset + i * stride;
        if simple_threshold(src, row_off, 1, filter_limit) {
            filter_common(src, row_off, 1, true);
        }
    }
}

fn filter_h_edge_simple(src: &mut [u8], offset: usize, stride: usize, filter_limit: i32) {
    for col in 0..16 {
        let col_off = offset + col;
        if simple_threshold(src, col_off, stride, filter_limit) {
            filter_common(src, col_off, stride, true);
        }
    }
}



pub const YUV_FIX2: i32 = 6;
pub const YUV_MASK2: i32 = (256 << YUV_FIX2) - 1;

#[inline(always)]
fn mult_hi(v: i32, coeff: i32) -> i32 {
    (v * coeff) >> 8
}

#[inline(always)]
fn vp8_clip8(v: i32) -> u8 {
    if (v & !YUV_MASK2) == 0 {
        (v >> YUV_FIX2) as u8
    } else if v < 0 {
        0
    } else {
        255
    }
}

#[inline(always)]
fn vp8_yuv_to_rgba(y: u8, u: u8, v: u8, rgba: &mut [u8]) {
    let y = y as i32;
    let u = u as i32;
    let v = v as i32;
    let y_scaled = mult_hi(y, 19077);
    let r = vp8_clip8(y_scaled + mult_hi(v, 26149) - 14234);
    let g = vp8_clip8(y_scaled - mult_hi(u, 6419) - mult_hi(v, 13320) + 8708);
    let b = vp8_clip8(y_scaled + mult_hi(u, 33050) - 17685);
    rgba[0] = r;
    rgba[1] = g;
    rgba[2] = b;
    rgba[3] = 255;
}

fn upsample_rgba_line_pair(
    top_y: &[u8],
    bottom_y: Option<&[u8]>,
    top_u: &[u8],
    top_v: &[u8],
    cur_u: &[u8],
    cur_v: &[u8],
    top_dst: &mut [u8],
    mut bottom_dst: Option<&mut [u8]>,
    len: usize,
) {
    let last_pixel_pair = (len - 1) >> 1;
    let mut tl_u = top_u[0] as u32;
    let mut tl_v = top_v[0] as u32;
    let mut l_u = cur_u[0] as u32;
    let mut l_v = cur_v[0] as u32;

    {
        let uv0_u = (3 * tl_u + l_u + 2) >> 2;
        let uv0_v = (3 * tl_v + l_v + 2) >> 2;
        vp8_yuv_to_rgba(top_y[0], uv0_u as u8, uv0_v as u8, &mut top_dst[0..4]);
    }
    if let Some(ref mut b_dst) = bottom_dst {
        if let Some(bot_y) = bottom_y {
            let uv0_u = (3 * l_u + tl_u + 2) >> 2;
            let uv0_v = (3 * l_v + tl_v + 2) >> 2;
            vp8_yuv_to_rgba(bot_y[0], uv0_u as u8, uv0_v as u8, &mut b_dst[0..4]);
        }
    }

    for x in 1..=last_pixel_pair {
        let t_u = top_u[x] as u32;
        let t_v = top_v[x] as u32;
        let uv_u = cur_u[x] as u32;
        let uv_v = cur_v[x] as u32;

        let avg_u = tl_u + t_u + l_u + uv_u + 8;
        let avg_v = tl_v + t_v + l_v + uv_v + 8;
        let diag_12_u = (avg_u + 2 * (t_u + l_u)) >> 3;
        let diag_12_v = (avg_v + 2 * (t_v + l_v)) >> 3;
        let diag_03_u = (avg_u + 2 * (tl_u + uv_u)) >> 3;
        let diag_03_v = (avg_v + 2 * (tl_v + uv_v)) >> 3;

        {
            let uv0_u = (diag_12_u + tl_u) >> 1;
            let uv0_v = (diag_12_v + tl_v) >> 1;
            let uv1_u = (diag_03_u + t_u) >> 1;
            let uv1_v = (diag_03_v + t_v) >> 1;
            vp8_yuv_to_rgba(top_y[2 * x - 1], uv0_u as u8, uv0_v as u8, &mut top_dst[(2 * x - 1) * 4..]);
            vp8_yuv_to_rgba(top_y[2 * x], uv1_u as u8, uv1_v as u8, &mut top_dst[2 * x * 4..]);
        }

        if let Some(ref mut b_dst) = bottom_dst {
            if let Some(bot_y) = bottom_y {
                let uv0_u = (diag_03_u + l_u) >> 1;
                let uv0_v = (diag_03_v + l_v) >> 1;
                let uv1_u = (diag_12_u + uv_u) >> 1;
                let uv1_v = (diag_12_v + uv_v) >> 1;
                vp8_yuv_to_rgba(bot_y[2 * x - 1], uv0_u as u8, uv0_v as u8, &mut b_dst[(2 * x - 1) * 4..]);
                vp8_yuv_to_rgba(bot_y[2 * x], uv1_u as u8, uv1_v as u8, &mut b_dst[2 * x * 4..]);
            }
        }

        tl_u = t_u;
        tl_v = t_v;
        l_u = uv_u;
        l_v = uv_v;
    }

    if (len & 1) == 0 {
        let uv0_u = (3 * tl_u + l_u + 2) >> 2;
        let uv0_v = (3 * tl_v + l_v + 2) >> 2;
        vp8_yuv_to_rgba(top_y[len - 1], uv0_u as u8, uv0_v as u8, &mut top_dst[(len - 1) * 4..]);

        if let Some(ref mut b_dst) = bottom_dst {
            if let Some(bot_y) = bottom_y {
                let uv0_u = (3 * l_u + tl_u + 2) >> 2;
                let uv0_v = (3 * l_v + tl_v + 2) >> 2;
                vp8_yuv_to_rgba(bot_y[len - 1], uv0_u as u8, uv0_v as u8, &mut b_dst[(len - 1) * 4..]);
            }
        }
    }
}


fn emit_fancy_rgba(
    cur_y_band: &[u8],
    cur_u_band: &[u8],
    cur_v_band: &[u8],
    mb_row: usize,
    width: usize,
    height: usize,
    tmp_y: &mut [u8],
    tmp_u: &mut [u8],
    tmp_v: &mut [u8],
    out_rgba: &mut [u8],
) {
    let mb_y = mb_row * 16;
    let mb_h = 16.min(height - mb_y);
    let y_end = mb_y + mb_h;
    let out_stride = width * 4;

    let mut y = mb_y;
    let mut local_y = 0usize;
    let mut local_uv_y = 0usize;

    if y == 0 {
        let cur_y_line = &cur_y_band[Y_OFFSET..Y_OFFSET + width];
        let cur_u_line = &cur_u_band[UV_OFFSET..UV_OFFSET + (width + 1) / 2];
        let cur_v_line = &cur_v_band[UV_OFFSET..UV_OFFSET + (width + 1) / 2];
        let dst_line = &mut out_rgba[0..out_stride];
        upsample_rgba_line_pair(
            cur_y_line,
            None,
            cur_u_line,
            cur_v_line,
            cur_u_line,
            cur_v_line,
            dst_line,
            None,
            width,
        );
    } else {
        let cur_y_line = &cur_y_band[Y_OFFSET..Y_OFFSET + width];
        let cur_u_line = &cur_u_band[UV_OFFSET..UV_OFFSET + (width + 1) / 2];
        let cur_v_line = &cur_v_band[UV_OFFSET..UV_OFFSET + (width + 1) / 2];
        let prev_dst_start = (y - 1) * out_stride;
        let cur_dst_start = y * out_stride;
        let (top_slice, bot_slice) = out_rgba.split_at_mut(cur_dst_start);
        let top_dst = &mut top_slice[prev_dst_start..prev_dst_start + out_stride];
        let bot_dst = &mut bot_slice[0..out_stride];
        upsample_rgba_line_pair(
            &tmp_y[..width],
            Some(cur_y_line),
            &tmp_u[..(width + 1) / 2],
            &tmp_v[..(width + 1) / 2],
            cur_u_line,
            cur_v_line,
            top_dst,
            Some(bot_dst),
            width,
        );
    }

    while y + 2 < y_end {
        let top_uv_y = local_uv_y;
        local_uv_y += 1;
        let cur_uv_y = local_uv_y;

        let top_y_idx = local_y + 1;
        let bot_y_idx = local_y + 2;
        local_y += 2;

        let top_y_line = &cur_y_band[Y_OFFSET + top_y_idx * Y_STRIDE..Y_OFFSET + top_y_idx * Y_STRIDE + width];
        let bot_y_line = &cur_y_band[Y_OFFSET + bot_y_idx * Y_STRIDE..Y_OFFSET + bot_y_idx * Y_STRIDE + width];

        let top_u_line = &cur_u_band[UV_OFFSET + top_uv_y * UV_STRIDE..UV_OFFSET + top_uv_y * UV_STRIDE + (width + 1) / 2];
        let top_v_line = &cur_v_band[UV_OFFSET + top_uv_y * UV_STRIDE..UV_OFFSET + top_uv_y * UV_STRIDE + (width + 1) / 2];
        let cur_u_line = &cur_u_band[UV_OFFSET + cur_uv_y * UV_STRIDE..UV_OFFSET + cur_uv_y * UV_STRIDE + (width + 1) / 2];
        let cur_v_line = &cur_v_band[UV_OFFSET + cur_uv_y * UV_STRIDE..UV_OFFSET + cur_uv_y * UV_STRIDE + (width + 1) / 2];

        let top_dst_y = y + 1;
        let bot_dst_y = y + 2;
        y += 2;

        let (first_part, second_part) = out_rgba.split_at_mut(bot_dst_y * out_stride);
        let top_dst = &mut first_part[top_dst_y * out_stride..(top_dst_y + 1) * out_stride];
        let bot_dst = &mut second_part[0..out_stride];

        upsample_rgba_line_pair(
            top_y_line,
            Some(bot_y_line),
            top_u_line,
            top_v_line,
            cur_u_line,
            cur_v_line,
            top_dst,
            Some(bot_dst),
            width,
        );
    }

    local_y += 1;
    if y_end < height {
        let last_y_line = &cur_y_band[Y_OFFSET + local_y * Y_STRIDE..Y_OFFSET + local_y * Y_STRIDE + width];
        let last_u_line = &cur_u_band[UV_OFFSET + local_uv_y * UV_STRIDE..UV_OFFSET + local_uv_y * UV_STRIDE + (width + 1) / 2];
        let last_v_line = &cur_v_band[UV_OFFSET + local_uv_y * UV_STRIDE..UV_OFFSET + local_uv_y * UV_STRIDE + (width + 1) / 2];
        tmp_y[..width].copy_from_slice(last_y_line);
        tmp_u[..(width + 1) / 2].copy_from_slice(last_u_line);
        tmp_v[..(width + 1) / 2].copy_from_slice(last_v_line);
    } else {
        if (y_end & 1) == 0 {
            let last_y_line = &cur_y_band[Y_OFFSET + local_y * Y_STRIDE..Y_OFFSET + local_y * Y_STRIDE + width];
            let last_u_line = &cur_u_band[UV_OFFSET + local_uv_y * UV_STRIDE..UV_OFFSET + local_uv_y * UV_STRIDE + (width + 1) / 2];
            let last_v_line = &cur_v_band[UV_OFFSET + local_uv_y * UV_STRIDE..UV_OFFSET + local_uv_y * UV_STRIDE + (width + 1) / 2];
            let last_dst_y = y_end - 1;
            let dst_line = &mut out_rgba[last_dst_y * out_stride..(last_dst_y + 1) * out_stride];
            upsample_rgba_line_pair(
                last_y_line,
                None,
                last_u_line,
                last_v_line,
                last_u_line,
                last_v_line,
                dst_line,
                None,
                width,
            );
        }
    }
}

pub fn decode(chunk: &[u8], out_rgba: &mut [u8]) -> Result<(u32, u32, usize), ()> {
    let (width, height) = dimensions(chunk)?;
    let required_len = (width as usize).checked_mul(height as usize).and_then(|p| p.checked_mul(4)).ok_or(())?;
    if out_rgba.len() < required_len {
        return Err(());
    }

    let tag = (chunk[0] as u32) | ((chunk[1] as u32) << 8) | ((chunk[2] as u32) << 16);
    let part0_sz = (tag >> 5) as usize;
    if 10 + part0_sz > chunk.len() {
        return Err(());
    }

    let mb_cols = ((width + 15) / 16) as usize;
    let mb_rows = ((height + 15) / 16) as usize;
    if mb_cols > MAX_MB_COLS {
        return Err(());
    }

    let scratch = vp8_scratch();
    let w = width as usize;
    let h = height as usize;

    // Initialize bool decoder on partition 0 (header + modes)
    let mut bool_dec = BoolDecoder::new(&chunk[10..10 + part0_sz]);

    // Check color space and clamp bits (must be 0)
    let color_space = bool_dec.get_bit();
    let clamp_type = bool_dec.get_bit();
    if color_space != 0 || clamp_type != 0 {
        return Err(());
    }

    // Segmentation
    let mut seg = SegmentHdr::default();
    seg.enabled = bool_dec.get_bit() != 0;
    if seg.enabled {
        seg.update_map = bool_dec.get_bit() != 0;
        seg.update_data = bool_dec.get_bit() != 0;
        if seg.update_data {
            seg.abs = bool_dec.get_bit() != 0;
            for i in 0..4 {
                seg.quant_idx[i] = bool_dec.maybe_get_int(7);
            }
            for i in 0..4 {
                seg.lf_level[i] = bool_dec.maybe_get_int(6);
            }
        }
        if seg.update_map {
            for i in 0..3 {
                seg.tree_probs[i] = if bool_dec.get_bit() != 0 {
                    bool_dec.get_uint(8) as u8
                } else {
                    255
                };
            }
        }
    }

    // Loop filter
    let mut lf = LoopFilterHdr::default();
    lf.use_simple = bool_dec.get_bit() != 0;
    lf.level = bool_dec.get_uint(6) as u8;
    lf.sharpness = bool_dec.get_uint(3) as u8;
    lf.delta_enabled = bool_dec.get_bit() != 0;
    if lf.delta_enabled && bool_dec.get_bit() != 0 {
        for i in 0..4 {
            lf.ref_delta[i] = bool_dec.maybe_get_int(6);
        }
        for i in 0..4 {
            lf.mode_delta[i] = bool_dec.maybe_get_int(6);
        }
    }

    // Token partitions
    let num_partitions = 1usize << bool_dec.get_uint(2);
    let token_data_start = 10 + part0_sz;
    if token_data_start > chunk.len() {
        return Err(());
    }
    let mut remaining_sz = chunk.len() - token_data_start;
    let mut partition_sz = [0usize; 8];
    let mut partition_offset = [0usize; 8];

    let mut data_p = token_data_start;
    if num_partitions > 1 {
        let size_header_len = 3 * (num_partitions - 1);
        if remaining_sz < size_header_len {
            return Err(());
        }
        data_p += size_header_len;
        remaining_sz -= size_header_len;

        let mut sz_read_p = token_data_start;
        for i in 0..(num_partitions - 1) {
            let s = (chunk[sz_read_p] as usize)
                | ((chunk[sz_read_p + 1] as usize) << 8)
                | ((chunk[sz_read_p + 2] as usize) << 16);
            sz_read_p += 3;
            if remaining_sz < s {
                return Err(());
            }
            partition_sz[i] = s;
            remaining_sz -= s;
        }
        partition_sz[num_partitions - 1] = remaining_sz;
    } else {
        partition_sz[0] = remaining_sz;
    }

    let mut cur_offset = data_p;
    for i in 0..num_partitions {
        partition_offset[i] = cur_offset;
        cur_offset += partition_sz[i];
    }

    // Quantizer
    let mut quant = QuantHdr::default();
    quant.q_index = bool_dec.get_uint(7) as u8;
    quant.y1_dc_delta = bool_dec.maybe_get_int(4);
    quant.y2_dc_delta = bool_dec.maybe_get_int(4);
    quant.y2_ac_delta = bool_dec.maybe_get_int(4);
    quant.uv_dc_delta = bool_dec.maybe_get_int(4);
    quant.uv_ac_delta = bool_dec.maybe_get_int(4);
    quant.delta_update = true;

    // Reference (refresh entropy)
    let _refresh_entropy = bool_dec.get_bit();

    // Default coefficient probabilities
    scratch.coeff_probs = K_DEFAULT_COEFF_PROBS;

    // Read coefficient probability updates
    for i in 0..4 {
        for j in 0..8 {
            for k in 0..3 {
                for l in 0..11 {
                    if bool_dec.get(K_COEFF_ENTROPY_UPDATE_PROBS[i][j][k][l]) != 0 {
                        scratch.coeff_probs[i][j][k][l] = bool_dec.get_uint(8) as u8;
                    }
                }
            }
        }
    }

    let coeff_skip_enabled = bool_dec.get_bit() != 0;
    let coeff_skip_prob = if coeff_skip_enabled {
        bool_dec.get_uint(8) as u8
    } else {
        0
    };

    // Initialize dequantization factors
    let mut dq_factors = [DequantFactors::default(); 4];
    dequant_init(&mut dq_factors, &seg, &quant);

    // Token decoders for each partition
    let mut token_decoders = [
        BoolDecoder::new(&[]),
        BoolDecoder::new(&[]),
        BoolDecoder::new(&[]),
        BoolDecoder::new(&[]),
        BoolDecoder::new(&[]),
        BoolDecoder::new(&[]),
        BoolDecoder::new(&[]),
        BoolDecoder::new(&[]),
    ];
    for i in 0..num_partitions {
        let start = partition_offset[i];
        let end = start + partition_sz[i];
        if end <= chunk.len() {
            token_decoders[i] = BoolDecoder::new(&chunk[start..end]);
        }
    }

    // Reset contexts
    scratch.above_token_ctx = [[0; 9]; MAX_MB_COLS];
    scratch.mb_info_prev = [MbInfo::default(); MAX_MB_COLS + 1];

    let mut partition_idx = 0usize;

    for row in 0..mb_rows {
        scratch.left_token_ctx = [0; 9];
        scratch.mb_info_curr = [MbInfo::default(); MAX_MB_COLS + 1];

        // 1. Decode Modes for row
        for col in 0..mb_cols {
            let mut mb = MbInfo::default();
            if seg.update_map {
                mb.segment_id = if bool_dec.get(seg.tree_probs[0]) != 0 {
                    2 + bool_dec.get(seg.tree_probs[2])
                } else {
                    bool_dec.get(seg.tree_probs[1])
                };
            }
            if coeff_skip_enabled {
                mb.skip_coeff = bool_dec.get(coeff_skip_prob);
            }

            let y_mode = bool_dec.read_tree(&KF_Y_MODE_TREE, &KF_Y_MODE_PROBS) as u8;
            mb.y_mode = y_mode;

            let above = &scratch.mb_info_prev[col + 1];
            let left = &scratch.mb_info_curr[col];

            if y_mode == B_PRED {
                for b in 0..16 {
                    let a_m = above_block_mode(&mb, above, b) as usize;
                    let l_m = left_block_mode(&mb, left, b) as usize;
                    let b_probs = &KF_B_MODE_PROBS[a_m][l_m];
                    mb.sub_modes[b] = bool_dec.read_tree(&B_MODE_TREE, b_probs) as u8;
                }
            }
            mb.uv_mode = bool_dec.read_tree(&UV_MODE_TREE, &KF_UV_MODE_PROBS) as u8;
            scratch.mb_info_curr[col + 1] = mb;
        }

        // 2. Decode Tokens for row
        let t_dec = &mut token_decoders[partition_idx];
        for col in 0..mb_cols {
            let mb = &mut scratch.mb_info_curr[col + 1];
            let coeffs_start = col * 25 * 16;
            let mb_coeffs = &mut scratch.coeffs[coeffs_start..coeffs_start + 25 * 16];

            if mb.skip_coeff != 0 {
                scratch.left_token_ctx[..8].fill(0);
                scratch.above_token_ctx[col][..8].fill(0);
                if mb.y_mode != B_PRED {
                    scratch.left_token_ctx[8] = 0;
                    scratch.above_token_ctx[col][8] = 0;
                }
                mb.eob_mask = 0;
                mb_coeffs.fill(0);
            } else {
                let dqf = &dq_factors[mb.segment_id as usize];
                mb.eob_mask = decode_mb_tokens(
                    t_dec,
                    &mut scratch.left_token_ctx,
                    &mut scratch.above_token_ctx[col],
                    mb_coeffs,
                    mb.y_mode,
                    &scratch.coeff_probs,
                    dqf,
                );
            }
        }

        partition_idx += 1;
        if partition_idx == num_partitions {
            partition_idx = 0;
        }

        // 3. Intra Predict & Reconstruct for row
        let cur_band = row & 1;
        let _prev_band = (row.wrapping_sub(1)) & 1;

        // If row > 0, copy boundary rows from previous band
        if row > 0 {
            let (y_first, y_second) = scratch.y_bands.split_at_mut(1);
            let (y_cur, y_prev) = if cur_band == 0 {
                (&mut y_first[0], &y_second[0])
            } else {
                (&mut y_second[0], &y_first[0])
            };
            for r in 0..4 {
                let src_y = Y_OFFSET + (12 + r) * Y_STRIDE;
                let dst_y = Y_OFFSET - (4 - r) * Y_STRIDE;
                y_cur[dst_y - 1..dst_y + mb_cols * 16 + 4]
                    .copy_from_slice(&y_prev[src_y - 1..src_y + mb_cols * 16 + 4]);
            }
            let dst_pred_y = Y_OFFSET - Y_STRIDE;
            y_cur[dst_pred_y..dst_pred_y + mb_cols * 16 + 4]
                .copy_from_slice(&scratch.top_pred_y[..mb_cols * 16 + 4]);

            let (u_first, u_second) = scratch.u_bands.split_at_mut(1);
            let (u_cur, u_prev) = if cur_band == 0 {
                (&mut u_first[0], &u_second[0])
            } else {
                (&mut u_second[0], &u_first[0])
            };
            for r in 0..4 {
                let src_uv = UV_OFFSET + (4 + r) * UV_STRIDE;
                let dst_uv = UV_OFFSET - (4 - r) * UV_STRIDE;
                u_cur[dst_uv - 1..dst_uv + mb_cols * 8 + 4]
                    .copy_from_slice(&u_prev[src_uv - 1..src_uv + mb_cols * 8 + 4]);
            }
            let dst_pred_uv = UV_OFFSET - UV_STRIDE;
            u_cur[dst_pred_uv..dst_pred_uv + mb_cols * 8 + 4]
                .copy_from_slice(&scratch.top_pred_u[..mb_cols * 8 + 4]);

            let (v_first, v_second) = scratch.v_bands.split_at_mut(1);
            let (v_cur, v_prev) = if cur_band == 0 {
                (&mut v_first[0], &v_second[0])
            } else {
                (&mut v_second[0], &v_first[0])
            };
            for r in 0..4 {
                let src_uv = UV_OFFSET + (4 + r) * UV_STRIDE;
                let dst_uv = UV_OFFSET - (4 - r) * UV_STRIDE;
                v_cur[dst_uv - 1..dst_uv + mb_cols * 8 + 4]
                    .copy_from_slice(&v_prev[src_uv - 1..src_uv + mb_cols * 8 + 4]);
            }
            v_cur[dst_pred_uv..dst_pred_uv + mb_cols * 8 + 4]
                .copy_from_slice(&scratch.top_pred_v[..mb_cols * 8 + 4]);
        }

        // Reconstruct each macroblock
        for col in 0..mb_cols {
            let mb = scratch.mb_info_curr[col + 1];
            let y_off = Y_OFFSET + col * 16;
            let uv_off = UV_OFFSET + col * 8;
            let coeffs_start = col * 25 * 16;

            if col == 0 {
                fixup_left(&mut scratch.y_bands[cur_band], y_off, 16, Y_STRIDE, row, mb.y_mode);
                fixup_left(&mut scratch.u_bands[cur_band], uv_off, 8, UV_STRIDE, row, mb.uv_mode);
                fixup_left(&mut scratch.v_bands[cur_band], uv_off, 8, UV_STRIDE, row, mb.uv_mode);
                if row == 0 {
                    scratch.y_bands[cur_band][y_off.wrapping_sub(Y_STRIDE + 1)] = 127;
                    scratch.u_bands[cur_band][uv_off.wrapping_sub(UV_STRIDE + 1)] = 127;
                    scratch.v_bands[cur_band][uv_off.wrapping_sub(UV_STRIDE + 1)] = 127;
                }
            }
            if row == 0 {
                fixup_above(&mut scratch.y_bands[cur_band], y_off, 16, Y_STRIDE, col, mb.y_mode);
                fixup_above(&mut scratch.u_bands[cur_band], uv_off, 8, UV_STRIDE, col, mb.uv_mode);
                fixup_above(&mut scratch.v_bands[cur_band], uv_off, 8, UV_STRIDE, col, mb.uv_mode);
            }

            // Luma Prediction & IDCT
            if mb.y_mode == B_PRED {
                let top_right_off = y_off + 16 - Y_STRIDE;
                if row == 0 {
                    scratch.y_bands[cur_band][top_right_off..top_right_off + 4].fill(127);
                } else if col >= mb_cols - 1 {
                    let last_top_val = scratch.y_bands[cur_band][top_right_off - 1];
                    scratch.y_bands[cur_band][top_right_off..top_right_off + 4].fill(last_top_val);
                }
                copy_down(&mut scratch.y_bands[cur_band], y_off, Y_STRIDE);
                for b in 0..16 {
                    let bx = b & 3;
                    let by = b >> 2;
                    let sub_off = y_off + by * 4 * Y_STRIDE + bx * 4;
                    match mb.sub_modes[b] {
                        B_DC_PRED => predict_dc_nxn(&mut scratch.y_bands[cur_band], sub_off, Y_STRIDE, 4),
                        B_TM_PRED => predict_tm_nxn(&mut scratch.y_bands[cur_band], sub_off, Y_STRIDE, 4),
                        B_VE_PRED => predict_ve_4x4(&mut scratch.y_bands[cur_band], sub_off, Y_STRIDE),
                        B_HE_PRED => predict_he_4x4(&mut scratch.y_bands[cur_band], sub_off, Y_STRIDE),
                        B_LD_PRED => predict_ld_4x4(&mut scratch.y_bands[cur_band], sub_off, Y_STRIDE),
                        B_RD_PRED => predict_rd_4x4(&mut scratch.y_bands[cur_band], sub_off, Y_STRIDE),
                        B_VR_PRED => predict_vr_4x4(&mut scratch.y_bands[cur_band], sub_off, Y_STRIDE),
                        B_VL_PRED => predict_vl_4x4(&mut scratch.y_bands[cur_band], sub_off, Y_STRIDE),
                        B_HD_PRED => predict_hd_4x4(&mut scratch.y_bands[cur_band], sub_off, Y_STRIDE),
                        B_HU_PRED => predict_hu_4x4(&mut scratch.y_bands[cur_band], sub_off, Y_STRIDE),
                        _ => predict_dc_nxn(&mut scratch.y_bands[cur_band], sub_off, Y_STRIDE, 4),
                    }
                    let c_off = coeffs_start + b * 16;
                    vp8_idct_add(&mut scratch.y_bands[cur_band], sub_off, Y_STRIDE, &scratch.coeffs[c_off..c_off + 16]);
                }
            } else {
                match mb.y_mode {
                    DC_PRED => predict_dc_nxn(&mut scratch.y_bands[cur_band], y_off, Y_STRIDE, 16),
                    V_PRED => predict_v_nxn(&mut scratch.y_bands[cur_band], y_off, Y_STRIDE, 16),
                    H_PRED => predict_h_nxn(&mut scratch.y_bands[cur_band], y_off, Y_STRIDE, 16),
                    TM_PRED => predict_tm_nxn(&mut scratch.y_bands[cur_band], y_off, Y_STRIDE, 16),
                    _ => predict_dc_nxn(&mut scratch.y_bands[cur_band], y_off, Y_STRIDE, 16),
                }

                // WHT on Y2 block
                let y2_off = coeffs_start + 24 * 16;
                let mut y2_out = [0i16; 16];
                vp8_walsh(&scratch.coeffs[y2_off..y2_off + 16], &mut y2_out);
                for i in 0..16 {
                    scratch.coeffs[coeffs_start + i * 16] = y2_out[i];
                }

                for b in 0..16 {
                    let bx = b & 3;
                    let by = b >> 2;
                    let sub_off = y_off + by * 4 * Y_STRIDE + bx * 4;
                    let c_off = coeffs_start + b * 16;
                    vp8_idct_add(&mut scratch.y_bands[cur_band], sub_off, Y_STRIDE, &scratch.coeffs[c_off..c_off + 16]);
                }
            }

            // Chroma Prediction & IDCT
            match mb.uv_mode {
                DC_PRED => {
                    predict_dc_nxn(&mut scratch.u_bands[cur_band], uv_off, UV_STRIDE, 8);
                    predict_dc_nxn(&mut scratch.v_bands[cur_band], uv_off, UV_STRIDE, 8);
                }
                V_PRED => {
                    predict_v_nxn(&mut scratch.u_bands[cur_band], uv_off, UV_STRIDE, 8);
                    predict_v_nxn(&mut scratch.v_bands[cur_band], uv_off, UV_STRIDE, 8);
                }
                H_PRED => {
                    predict_h_nxn(&mut scratch.u_bands[cur_band], uv_off, UV_STRIDE, 8);
                    predict_h_nxn(&mut scratch.v_bands[cur_band], uv_off, UV_STRIDE, 8);
                }
                TM_PRED => {
                    predict_tm_nxn(&mut scratch.u_bands[cur_band], uv_off, UV_STRIDE, 8);
                    predict_tm_nxn(&mut scratch.v_bands[cur_band], uv_off, UV_STRIDE, 8);
                }
                _ => {
                    predict_dc_nxn(&mut scratch.u_bands[cur_band], uv_off, UV_STRIDE, 8);
                    predict_dc_nxn(&mut scratch.v_bands[cur_band], uv_off, UV_STRIDE, 8);
                }
            }

            for b in 0..4 {
                let bx = b & 1;
                let by = b >> 1;
                let sub_off = uv_off + by * 4 * UV_STRIDE + bx * 4;
                let c_off_u = coeffs_start + (16 + b) * 16;
                let c_off_v = coeffs_start + (20 + b) * 16;
                vp8_idct_add(&mut scratch.u_bands[cur_band], sub_off, UV_STRIDE, &scratch.coeffs[c_off_u..c_off_u + 16]);
                vp8_idct_add(&mut scratch.v_bands[cur_band], sub_off, UV_STRIDE, &scratch.coeffs[c_off_v..c_off_v + 16]);
            }
        }

        // Extend rightmost pixel by 4 for intra prediction
        let end_y = Y_OFFSET + mb_cols * 16 + 15 * Y_STRIDE;
        let last_val = scratch.y_bands[cur_band][end_y - 1];
        scratch.y_bands[cur_band][end_y..end_y + 4].fill(last_val);

        // Stash away unfiltered bottom samples for next row's intra prediction
        let y_last_row = Y_OFFSET + 15 * Y_STRIDE;
        scratch.top_pred_y[..mb_cols * 16]
            .copy_from_slice(&scratch.y_bands[cur_band][y_last_row..y_last_row + mb_cols * 16]);
        scratch.top_pred_y[mb_cols * 16..mb_cols * 16 + 4].fill(last_val);

        let uv_last_row = UV_OFFSET + 7 * UV_STRIDE;
        scratch.top_pred_u[..mb_cols * 8]
            .copy_from_slice(&scratch.u_bands[cur_band][uv_last_row..uv_last_row + mb_cols * 8]);
        let end_u = scratch.top_pred_u[mb_cols * 8 - 1];
        scratch.top_pred_u[mb_cols * 8..mb_cols * 8 + 4].fill(end_u);

        scratch.top_pred_v[..mb_cols * 8]
            .copy_from_slice(&scratch.v_bands[cur_band][uv_last_row..uv_last_row + mb_cols * 8]);
        let end_v = scratch.top_pred_v[mb_cols * 8 - 1];
        scratch.top_pred_v[mb_cols * 8..mb_cols * 8 + 4].fill(end_v);

        // Restore vertically-filtered row -1 from previous band for loop filtering
        if row > 0 {
            let (y_first, y_second) = scratch.y_bands.split_at_mut(1);
            let (y_cur, y_prev) = if cur_band == 0 {
                (&mut y_first[0], &y_second[0])
            } else {
                (&mut y_second[0], &y_first[0])
            };
            let src_y = Y_OFFSET + 15 * Y_STRIDE;
            let dst_y = Y_OFFSET - Y_STRIDE;
            y_cur[dst_y - 1..dst_y + mb_cols * 16 + 4]
                .copy_from_slice(&y_prev[src_y - 1..src_y + mb_cols * 16 + 4]);

            if !lf.use_simple {
                let (u_first, u_second) = scratch.u_bands.split_at_mut(1);
                let (u_cur, u_prev) = if cur_band == 0 {
                    (&mut u_first[0], &u_second[0])
                } else {
                    (&mut u_second[0], &u_first[0])
                };
                let src_uv = UV_OFFSET + 7 * UV_STRIDE;
                let dst_uv = UV_OFFSET - UV_STRIDE;
                u_cur[dst_uv - 1..dst_uv + mb_cols * 8 + 4]
                    .copy_from_slice(&u_prev[src_uv - 1..src_uv + mb_cols * 8 + 4]);

                let (v_first, v_second) = scratch.v_bands.split_at_mut(1);
                let (v_cur, v_prev) = if cur_band == 0 {
                    (&mut v_first[0], &v_second[0])
                } else {
                    (&mut v_second[0], &v_first[0])
                };
                v_cur[dst_uv - 1..dst_uv + mb_cols * 8 + 4]
                    .copy_from_slice(&v_prev[src_uv - 1..src_uv + mb_cols * 8 + 4]);
            }
        }

        // 4. Loop filter row
        if lf.level > 0 {
            for col in 0..mb_cols {
                let mbi = scratch.mb_info_curr[col + 1];
                let mut edge_limit = 0i32;
                let mut interior_limit = 0i32;
                let mut hev_threshold = 0i32;
                calculate_filter_parameters(&lf, &seg, &mbi, &mut edge_limit, &mut interior_limit, &mut hev_threshold);

                if edge_limit > 0 {
                    let y_ptr = Y_OFFSET + col * 16;
                    let uv_ptr = UV_OFFSET + col * 8;
                    let filter_subblocks = mbi.eob_mask != 0 || mbi.y_mode == B_PRED;

                    if lf.use_simple {
                        let mb_limit = (edge_limit + 2) * 2 + interior_limit;
                        let b_limit = edge_limit * 2 + interior_limit;
                        if col > 0 {
                            filter_v_edge_simple(&mut scratch.y_bands[cur_band], y_ptr, Y_STRIDE, mb_limit);
                        }
                        if filter_subblocks {
                            filter_v_edge_simple(&mut scratch.y_bands[cur_band], y_ptr + 4, Y_STRIDE, b_limit);
                            filter_v_edge_simple(&mut scratch.y_bands[cur_band], y_ptr + 8, Y_STRIDE, b_limit);
                            filter_v_edge_simple(&mut scratch.y_bands[cur_band], y_ptr + 12, Y_STRIDE, b_limit);
                        }
                        if row > 0 {
                            filter_h_edge_simple(&mut scratch.y_bands[cur_band], y_ptr, Y_STRIDE, mb_limit);
                        }
                        if filter_subblocks {
                            filter_h_edge_simple(&mut scratch.y_bands[cur_band], y_ptr + 4 * Y_STRIDE, Y_STRIDE, b_limit);
                            filter_h_edge_simple(&mut scratch.y_bands[cur_band], y_ptr + 8 * Y_STRIDE, Y_STRIDE, b_limit);
                            filter_h_edge_simple(&mut scratch.y_bands[cur_band], y_ptr + 12 * Y_STRIDE, Y_STRIDE, b_limit);
                        }
                    } else {
                        if col > 0 {
                            filter_mb_v_edge(&mut scratch.y_bands[cur_band], y_ptr, Y_STRIDE, edge_limit + 2, interior_limit, hev_threshold, 2);
                            filter_mb_v_edge(&mut scratch.u_bands[cur_band], uv_ptr, UV_STRIDE, edge_limit + 2, interior_limit, hev_threshold, 1);
                            filter_mb_v_edge(&mut scratch.v_bands[cur_band], uv_ptr, UV_STRIDE, edge_limit + 2, interior_limit, hev_threshold, 1);
                        }
                        if filter_subblocks {
                            filter_subblock_v_edge(&mut scratch.y_bands[cur_band], y_ptr + 4, Y_STRIDE, edge_limit, interior_limit, hev_threshold, 2);
                            filter_subblock_v_edge(&mut scratch.y_bands[cur_band], y_ptr + 8, Y_STRIDE, edge_limit, interior_limit, hev_threshold, 2);
                            filter_subblock_v_edge(&mut scratch.y_bands[cur_band], y_ptr + 12, Y_STRIDE, edge_limit, interior_limit, hev_threshold, 2);
                            filter_subblock_v_edge(&mut scratch.u_bands[cur_band], uv_ptr + 4, UV_STRIDE, edge_limit, interior_limit, hev_threshold, 1);
                            filter_subblock_v_edge(&mut scratch.v_bands[cur_band], uv_ptr + 4, UV_STRIDE, edge_limit, interior_limit, hev_threshold, 1);
                        }
                        if row > 0 {
                            filter_mb_h_edge(&mut scratch.y_bands[cur_band], y_ptr, Y_STRIDE, edge_limit + 2, interior_limit, hev_threshold, 2);
                            filter_mb_h_edge(&mut scratch.u_bands[cur_band], uv_ptr, UV_STRIDE, edge_limit + 2, interior_limit, hev_threshold, 1);
                            filter_mb_h_edge(&mut scratch.v_bands[cur_band], uv_ptr, UV_STRIDE, edge_limit + 2, interior_limit, hev_threshold, 1);
                        }
                        if filter_subblocks {
                            filter_subblock_h_edge(&mut scratch.y_bands[cur_band], y_ptr + 4 * Y_STRIDE, Y_STRIDE, edge_limit, interior_limit, hev_threshold, 2);
                            filter_subblock_h_edge(&mut scratch.y_bands[cur_band], y_ptr + 8 * Y_STRIDE, Y_STRIDE, edge_limit, interior_limit, hev_threshold, 2);
                            filter_subblock_h_edge(&mut scratch.y_bands[cur_band], y_ptr + 12 * Y_STRIDE, Y_STRIDE, edge_limit, interior_limit, hev_threshold, 2);
                            filter_subblock_h_edge(&mut scratch.u_bands[cur_band], uv_ptr + 4 * UV_STRIDE, UV_STRIDE, edge_limit, interior_limit, hev_threshold, 1);
                            filter_subblock_h_edge(&mut scratch.v_bands[cur_band], uv_ptr + 4 * UV_STRIDE, UV_STRIDE, edge_limit, interior_limit, hev_threshold, 1);
                        }
                    }
                }
            }
        }

        if row > 0 {
            let (y_first, y_second) = scratch.y_bands.split_at_mut(1);
            let (y_cur, y_prev) = if cur_band == 0 {
                (&y_first[0], &mut y_second[0])
            } else {
                (&y_second[0], &mut y_first[0])
            };
            for r in 1..=3 {
                let src_y = Y_OFFSET - r * Y_STRIDE;
                let dst_y = Y_OFFSET + (16 - r) * Y_STRIDE;
                y_prev[dst_y..dst_y + mb_cols * 16]
                    .copy_from_slice(&y_cur[src_y..src_y + mb_cols * 16]);
            }
            if !lf.use_simple {
                let (u_first, u_second) = scratch.u_bands.split_at_mut(1);
                let (u_cur, u_prev) = if cur_band == 0 {
                    (&u_first[0], &mut u_second[0])
                } else {
                    (&u_second[0], &mut u_first[0])
                };
                let (v_first, v_second) = scratch.v_bands.split_at_mut(1);
                let (v_cur, v_prev) = if cur_band == 0 {
                    (&v_first[0], &mut v_second[0])
                } else {
                    (&v_second[0], &mut v_first[0])
                };
                for r in 1..=3 {
                    let src_uv = UV_OFFSET - r * UV_STRIDE;
                    let dst_uv = UV_OFFSET + (8 - r) * UV_STRIDE;
                    u_prev[dst_uv..dst_uv + mb_cols * 8]
                        .copy_from_slice(&u_cur[src_uv..src_uv + mb_cols * 8]);
                    v_prev[dst_uv..dst_uv + mb_cols * 8]
                        .copy_from_slice(&v_cur[src_uv..src_uv + mb_cols * 8]);
                }
            }
        }

        if row > 0 {
            let prev_band = (row - 1) & 1;
            emit_fancy_rgba(
                &scratch.y_bands[prev_band],
                &scratch.u_bands[prev_band],
                &scratch.v_bands[prev_band],
                row - 1,
                w,
                h,
                &mut scratch.tmp_y,
                &mut scratch.tmp_u,
                &mut scratch.tmp_v,
                out_rgba,
            );
        }
        scratch.mb_info_prev = scratch.mb_info_curr;
    }

    // Emit the last macroblock row
    let last_band = (mb_rows - 1) & 1;
    emit_fancy_rgba(
        &scratch.y_bands[last_band],
        &scratch.u_bands[last_band],
        &scratch.v_bands[last_band],
        mb_rows - 1,
        w,
        h,
        &mut scratch.tmp_y,
        &mut scratch.tmp_u,
        &mut scratch.tmp_v,
        out_rgba,
    );

    Ok((width, height, required_len))
}



