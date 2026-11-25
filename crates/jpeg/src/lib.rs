//! JPEG decoding through the bundled IJG reference implementation.
use std::{ffi::c_int, ptr};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Format,
    Allocation,
    ColorModel,
}

#[repr(C)]
pub struct DecodedJpeg {
    rgb: *mut u8,
    width: u32,
    height: u32,
}

unsafe extern "C" {
    fn mycela_ijg_decode(bytes: *const u8, length: usize, output: *mut DecodedJpeg) -> c_int;
    fn mycela_ijg_free(rgb: *mut u8);
}

impl DecodedJpeg {
    /// Decode an RGB, YCbCr, or grayscale JPEG to interleaved RGB8.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut result = Self {
            rgb: ptr::null_mut(),
            width: 0,
            height: 0,
        };
        // SAFETY: The bridge reads the input slice synchronously and transfers an RGB
        // allocation only on success. Its error recovery stays in C.
        match unsafe { mycela_ijg_decode(bytes.as_ptr(), bytes.len(), &mut result) } {
            0 => Ok(result),
            2 => Err(DecodeError::Allocation),
            3 => Err(DecodeError::ColorModel),
            _ => Err(DecodeError::Format),
        }
    }

    pub fn dimensions(&self) -> [u32; 2] {
        [self.width, self.height]
    }

    pub fn rgb(&self) -> &[u8] {
        let len = self.width as usize * self.height as usize * 3;
        // SAFETY: A successful decode owns exactly width * height RGB pixels.
        // The allocation is initialized and checked against the bridge's size limit.
        unsafe { std::slice::from_raw_parts(self.rgb, len) }
    }
}

impl Drop for DecodedJpeg {
    fn drop(&mut self) {
        // SAFETY: This is the bridge's allocation or null. The same C allocator frees it.
        unsafe { mycela_ijg_free(self.rgb) };
    }
}
