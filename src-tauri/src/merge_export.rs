//! Encode a flattened RGB merge in the format chosen by its extension.
use std::io::Cursor;
use std::path::Path;

pub(crate) fn encode(path: &Path, png: &[u8]) -> anyhow::Result<Vec<u8>> {
    let image = ::image::load_from_memory_with_format(png, ::image::ImageFormat::Png)?.to_rgb8();
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut output = Cursor::new(Vec::new());
    match extension.as_str() {
        "tif" | "tiff" => {
            let mut encoder = tiff::encoder::TiffEncoder::new(&mut output)?;
            encoder.write_image::<tiff::encoder::colortype::RGB8>(image.width(), image.height(), image.as_raw())?;
        }
        "png" | "jpg" | "jpeg" | "bmp" | "webp" => {
            let format = ::image::ImageFormat::from_extension(&extension).unwrap();
            ::image::DynamicImage::ImageRgb8(image).write_to(&mut output, format)?;
        }
        _ => anyhow::bail!("请选择 TIFF、PNG、JPEG、BMP 或 WebP 格式"),
    }
    Ok(output.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiff_and_png_exports_have_their_own_format_and_preserve_rgb() {
        let image = ::image::RgbImage::from_raw(2, 1, vec![255, 0, 0, 0, 127, 255]).unwrap();
        let mut input = Cursor::new(Vec::new());
        image.write_to(&mut input, ::image::ImageFormat::Png).unwrap();
        let tiff = encode(Path::new("merge.tif"), input.get_ref()).unwrap();
        let mut decoder = tiff::decoder::Decoder::new(Cursor::new(tiff)).unwrap();
        assert_eq!(decoder.dimensions().unwrap(), (2, 1));
        match decoder.read_image().unwrap() {
            tiff::decoder::DecodingResult::U8(values) => assert_eq!(values, [255, 0, 0, 0, 127, 255]),
            _ => panic!("expected RGB8"),
        }
        let png = encode(Path::new("merge.PNG"), input.get_ref()).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(::image::load_from_memory(&png).unwrap().to_rgb8(), image);
        for extension in ["jpg", "bmp", "webp"] {
            let bytes = encode(Path::new(&format!("merge.{extension}")), input.get_ref()).unwrap();
            let decoded = ::image::load_from_memory(&bytes).unwrap().to_rgb8();
            assert_eq!(decoded.dimensions(), (2, 1));
            if extension != "jpg" { assert_eq!(decoded, image); }
        }
        assert!(encode(Path::new("merge.psd"), input.get_ref()).is_err());
    }
}
