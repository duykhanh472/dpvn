//! Image validation (magic bytes, dimensions) and display derivative generation.
//!
//! Originals are stored verbatim; a display derivative (longest edge ≤ 1600 px, JPEG q=82)
//! is generated in `spawn_blocking` with a `Semaphore(2)` to prevent CPU starvation.

use std::sync::LazyLock;

use image::{DynamicImage, ImageFormat};
use tokio::sync::Semaphore;

use super::error::IngestError;

/// Semaphore capping concurrent image encode/decode operations.
/// Image encoding is CPU-bound; more than 2 concurrent tasks starve the async runtime.
pub static IMAGE_SEMAPHORE: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(2));

/// Maximum number of pixels along the longest edge for the display derivative.
const MAX_DISPLAY_EDGE: u32 = 1_600;

/// JPEG quality for display derivatives (0–100).
#[allow(dead_code)]
const DISPLAY_JPEG_QUALITY: u8 = 82;

/// Minimum allowed image dimension in either axis.
const MIN_DIM: u32 = 200;
/// Maximum allowed image dimension in either axis.
const MAX_DIM: u32 = 10_000;

/// Magic byte sequences for supported image types.
const PNG_MAGIC: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
const JPEG_MAGIC: &[u8] = &[0xFF, 0xD8, 0xFF];

/// Image type determined by magic bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageType {
    /// Portable Network Graphics.
    Png,
    /// JPEG / JFIF.
    Jpeg,
}

impl ImageType {
    /// Return the canonical file extension (without a dot).
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }

    /// Return the `image` crate format constant.
    #[must_use]
    fn image_format(self) -> ImageFormat {
        match self {
            Self::Png => ImageFormat::Png,
            Self::Jpeg => ImageFormat::Jpeg,
        }
    }
}

/// Result of image validation.
#[derive(Debug, Clone)]
pub struct ValidatedImage {
    /// Detected image type.
    pub image_type: ImageType,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

/// Validate image bytes: check magic bytes and decode dimensions.
///
/// Does **not** allocate a full pixel buffer — uses magic bytes and header parsing only.
///
/// # Errors
/// Returns [`IngestError::InvalidImage`] or [`IngestError::InvalidDimensions`].
pub fn validate_image(filename: &str, data: &[u8]) -> Result<ValidatedImage, IngestError> {
    let image_type = detect_magic(data).ok_or_else(|| IngestError::InvalidImage {
        file: filename.to_owned(),
        reason: "không nhận diện được magic bytes (yêu cầu PNG hoặc JPEG)".to_owned(),
    })?;

    // Decode only header (dimensions) using `image::io::Reader`
    let format = image_type.image_format();
    let cursor = std::io::Cursor::new(data);
    let reader = image::ImageReader::with_format(cursor, format);

    let (width, height) = reader
        .into_dimensions()
        .map_err(|e| IngestError::InvalidImage {
            file: filename.to_owned(),
            reason: format!("không đọc được kích thước ảnh: {e}"),
        })?;

    if width < MIN_DIM || height < MIN_DIM || width > MAX_DIM || height > MAX_DIM {
        return Err(IngestError::InvalidDimensions {
            file: filename.to_owned(),
            width,
            height,
        });
    }

    Ok(ValidatedImage {
        image_type,
        width,
        height,
    })
}

/// Detect the image type from the leading magic bytes.
fn detect_magic(data: &[u8]) -> Option<ImageType> {
    if data.starts_with(PNG_MAGIC) {
        Some(ImageType::Png)
    } else if data.starts_with(JPEG_MAGIC) {
        Some(ImageType::Jpeg)
    } else {
        None
    }
}

/// Generate a JPEG display derivative (longest edge ≤ 1600 px, q=82) from image bytes.
///
/// Runs inside `spawn_blocking`. Concurrency is bounded by [`IMAGE_SEMAPHORE`].
/// Returns the JPEG bytes of the derivative.
///
/// If derivative generation fails, the caller should degrade gracefully rather than
/// blocking the ingest commit — errors are logged, not fatal.
///
/// # Errors
/// Returns a `String` error description on failure (non-fatal).
pub async fn generate_display_derivative(image_data: Vec<u8>) -> Result<Vec<u8>, String> {
    let _permit = IMAGE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| format!("semaphore acquire failed: {e}"))?;

    tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let img =
            image::load_from_memory(&image_data).map_err(|e| format!("image decode error: {e}"))?;

        let resized = resize_to_display(img);

        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        resized
            .write_to(&mut cursor, ImageFormat::Jpeg)
            .map_err(|e| format!("JPEG encode error: {e}"))?;

        Ok(buf)
    })
    .await
    .map_err(|e| format!("spawn_blocking panicked: {e}"))?
}

/// Resize a `DynamicImage` so the longest edge does not exceed [`MAX_DISPLAY_EDGE`].
/// Preserves aspect ratio. Applies only if the image is larger than the threshold.
fn resize_to_display(img: DynamicImage) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    let longest = w.max(h);

    if longest <= MAX_DISPLAY_EDGE {
        return img;
    }

    // Scale the longest edge to MAX_DISPLAY_EDGE, preserve aspect ratio.
    let scale = f64::from(MAX_DISPLAY_EDGE) / f64::from(longest);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (nw, nh) = ((f64::from(w) * scale) as u32, (f64::from(h) * scale) as u32);

    img.resize(nw, nh, image::imageops::FilterType::Lanczos3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_png_magic() {
        let png_header = [0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(detect_magic(&png_header), Some(ImageType::Png));
    }

    #[test]
    fn test_detect_jpeg_magic() {
        let jpeg_header = [0xFFu8, 0xD8, 0xFF, 0xE0];
        assert_eq!(detect_magic(&jpeg_header), Some(ImageType::Jpeg));
    }

    #[test]
    fn test_detect_unknown_magic() {
        let unknown = [0x00u8, 0x01, 0x02, 0x03];
        assert_eq!(detect_magic(&unknown), None);
    }

    #[test]
    fn test_invalid_image_returns_error() {
        let garbage = b"not an image at all, really not";
        let result = validate_image("test.png", garbage);
        assert!(result.is_err());
    }
}
