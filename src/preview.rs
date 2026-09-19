use crate::error::CompileError;
use image::{GenericImageView, ImageBuffer, Pixel, RgbImage, Rgb};
use std::path::{Path, PathBuf};

const CANVAS_SIZE: u32 = 512;

pub fn resolve_preview(
    explicit: Option<&Path>,
    map_dir: &Path,
) -> Result<Option<PreviewData>, CompileError> {
    let preview_path = if let Some(p) = explicit {
        if !p.is_file() {
            return Err(CompileError::InvalidPreview(
                p.to_path_buf(),
                "File does not exist".to_string(),
            ));
        }
        Some(p.to_path_buf())
    } else {
        let default = map_dir.join("preview.png");
        if default.is_file() {
            Some(default)
        } else {
            None
        }
    };

    match preview_path {
        Some(path) => {
            let img = image::open(&path).map_err(|e| {
                CompileError::InvalidPreview(path.clone(), e.to_string())
            })?;
            let normalized = normalize_preview(&img);
            Ok(Some(PreviewData {
                path,
                normalized,
            }))
        }
        None => Ok(None),
    }
}

pub struct PreviewData {
    pub path: PathBuf,
    pub normalized: RgbImage,
}

fn normalize_preview(img: &image::DynamicImage) -> RgbImage {
    let (w, h) = img.dimensions();
    let scale = (CANVAS_SIZE as f64 / w.max(h) as f64).min(1.0);
    let new_w = (w as f64 * scale) as u32;
    let new_h = (h as f64 * scale) as u32;

    let resized = img.resize_exact(new_w, new_h, image::imageops::FilterType::Lanczos3);

    let mut canvas: RgbImage = ImageBuffer::from_pixel(CANVAS_SIZE, CANVAS_SIZE, Rgb([0, 0, 0]));

    let x_offset = (CANVAS_SIZE - new_w) / 2;
    let y_offset = (CANVAS_SIZE - new_h) / 2;

    for y in 0..new_h {
        for x in 0..new_w {
            let pixel = resized.get_pixel(x, y);
            let rgb = pixel.to_rgb();
            canvas.put_pixel(x + x_offset, y + y_offset, rgb);
        }
    }

    canvas
}

pub fn encode_png(img: &RgbImage) -> Vec<u8> {
    let mut buf = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut buf);
    image::ImageEncoder::write_image(
        encoder,
        img.as_raw(),
        CANVAS_SIZE,
        CANVAS_SIZE,
        image::ExtendedColorType::Rgb8,
    ).expect("PNG encoding should not fail");
    buf
}
