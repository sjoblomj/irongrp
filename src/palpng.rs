use crate::{log, LogLevel};
use image::{ColorType, DynamicImage, ImageBuffer};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::fmt::Debug;
use std::fs::File;
use std::io::{Error, ErrorKind, Read};
use std::sync::Mutex;

type CacheKey = ([u8; 3], Option<u8>);
static COLOUR_INDEX_CACHE: Lazy<Mutex<HashMap<CacheKey, u8>>> = Lazy::new(|| Mutex::new(HashMap::new()));

pub struct ImageWithMetadata<O, S>
where
    O: TryFrom<u32>, // Offset type
    S: TryFrom<u32>, // Dimension type
{
    pub x_offset: O,
    pub y_offset: O,
    pub width:    S,
    pub height:   S,
    pub original_width:  S,
    pub original_height: S,
    pub image_data: Vec<u8>,
}

/// Reads a Palette file
pub fn read_rgb_palette(pal_path: &str) -> std::io::Result<Vec<[u8; 3]>> {
    let mut file = File::open(pal_path)?;
    let mut buffer = [0u8; 768]; // RGB PAL files contain 256 RGB entries (256 * 3 bytes = 768)
    file.read_exact(&mut buffer)?;

    Ok(buffer.chunks(3).map(|c| [c[0], c[1], c[2]]).collect())
}

/// Saves the given pixel buffer to the given output path.
pub fn save_pixel_buffer_to_image_file(
    buffer: Vec<u8>,
    output_path: &str,
    use_transparency: bool,
    width:  u32,
    height: u32,
) -> Result<(), Error> {
    let image = if use_transparency {
        DynamicImage::ImageRgba8(
            ImageBuffer::from_raw(width, height, buffer)
                .expect("Failed to create RGBA image"),
        )
    } else {
        DynamicImage::ImageRgb8(
            ImageBuffer::from_raw(width, height, buffer)
                .expect("Failed to create RGB image"),
        )
    };
    image.save(&output_path).map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
}

/// Draws an image into a pixel buffer (Vec<u8>).
/// Uses the given palette for colour lookups.
pub fn draw_image_to_pixel_buffer<O, S>(
    image: ImageWithMetadata<O, S>,
    palette: &Vec<[u8; 3]>,
    use_transparency: bool,
) -> std::io::Result<Vec<u8>>
where
    O: TryFrom<u32> + TryInto<u32>, <O as TryInto<u32>>::Error: Debug,
    S: TryFrom<u32> + TryInto<u32>, <S as TryInto<u32>>::Error: Debug,
{
    let height     = image.height  .try_into().unwrap();
    let width      = image.width   .try_into().unwrap();
    let x_offset   = image.x_offset.try_into().unwrap();
    let y_offset   = image.y_offset.try_into().unwrap();
    let max_width  = image.original_width .try_into().unwrap();
    let max_height = image.original_height.try_into().unwrap();

    let mut buffer = vec![0u8; (max_width * max_height * if use_transparency { 4 } else { 3 }) as usize];

    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) as usize;
            let palette_index = image.image_data[idx] as usize;
            let colour = palette[palette_index];

            let out_x = x + x_offset;
            let out_y = y + y_offset;
            let pixel_index = (out_y * max_width + out_x) as usize;

            if use_transparency {
                let base = pixel_index * 4;
                let intensity = if palette_index == 0 {
                    0
                } else {
                    255
                };
                buffer[base..base + 4].copy_from_slice(&[colour[0], colour[1], colour[2], intensity]);
            } else {
                let base = pixel_index * 3;
                buffer[base..base + 3].copy_from_slice(&[colour[0], colour[1], colour[2]]);
            }
        }
    }

    Ok(buffer)
}

pub fn read_png<O, S>(
    png_file_name: &str,
    palette: &Vec<[u8; 3]>,
    trim_transparent_pixels: bool,
) -> std::io::Result<ImageWithMetadata<O, S>>
where
    O: TryFrom<u32>,
    S: TryFrom<u32>,
{
    let img = image::open(png_file_name)
        .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?;
    let has_alpha = match img.color() {
        ColorType::Rgba8 | ColorType::La8 | ColorType::Rgba16 | ColorType::La16 => true,
        _ => false,
    };
    let img_data = img.to_rgba8();

    let (width, height) = img_data.dimensions();
    log(LogLevel::Info, &format!(
        "Reading image {}. Has alpha channel: {}. Dimensions: 0x{:0>2X} * 0x{:0>2X} ({} * {})",
        png_file_name, has_alpha, width, height, width, height,
    ));

    let mut pixels_2d = vec![vec![0u8; width as usize]; height as usize];
    for (y, row) in img_data.rows().enumerate() {
        for (x, pixel) in row.enumerate() {
            let rgb = [pixel[0], pixel[1], pixel[2]];
            let alpha = if has_alpha {
                Some(pixel[3])
            } else {
                None
            };
            let index = cached_map_colour_to_palette_index(rgb, alpha, palette);
            pixels_2d[y][x] = index;
        }
    }

    let (new_width, new_height, trim_left, trim_top) = if trim_transparent_pixels {
        trim_away_transparency(&pixels_2d, width, height)
    } else {
        (width, height, 0, 0)
    };

    let mut pixels = Vec::with_capacity((new_width * new_height) as usize);
    for row in pixels_2d.iter().skip(trim_top as usize).take(new_height as usize) {
        pixels.extend(&row[trim_left as usize .. (trim_left + new_width) as usize]);
    }

    Ok(ImageWithMetadata {
        x_offset: cast::<O>(trim_left,  "x_offset")?,
        y_offset: cast::<O>(trim_top,   "y_offset")?,
        width:    cast::<S>(new_width,  "width")?,
        height:   cast::<S>(new_height, "height")?,
        original_width:  cast::<S>(width,  "original_width")?,
        original_height: cast::<S>(height, "original_height")?,
        image_data: pixels,
    })
}

fn cached_map_colour_to_palette_index(
    colour: [u8; 3],
    alpha: Option<u8>,
    palette: &Vec<[u8; 3]>,
) -> u8 {
    let key = (colour, alpha);

    // Attempt to get cached result
    if let Some(result) = COLOUR_INDEX_CACHE.lock().unwrap().get(&key) {
        return *result;
    }

    // Compute if not cached
    let result = map_colour_to_palette_index(colour, alpha, palette);

    // Insert into cache
    COLOUR_INDEX_CACHE.lock().unwrap().insert(key, result);

    result
}

fn map_colour_to_palette_index(colour: [u8; 3], alpha: Option<u8>, palette: &Vec<[u8; 3]>) -> u8 {
    if alpha == Some(0) {
        return 0; // Transparent
    }
    if alpha != Some(255) && alpha != None {
        log(LogLevel::Warn, &format!(
            "Pixel [{}, {}, {}, {}] is neither fully transparent nor fully opaque. Will drop the alpha channel.",
            colour[0], colour[1], colour[2], alpha.unwrap(),
        ));
    }
    let mut best_index = 0;
    let mut best_distance = u32::MAX;

    for (i, &pal_colour) in palette.iter().enumerate() {
        let dr = colour[0] as i32 - pal_colour[0]  as i32;
        let dg = colour[1] as i32 - pal_colour[1]  as i32;
        let db = colour[2] as i32 - pal_colour[2]  as i32;
        let dist = (dr * dr + dg * dg + db * db) as u32;

        if dist < best_distance {
            best_distance = dist;
            best_index = i;
        }
    }

    if best_distance != 0 {
        log(LogLevel::Warn, &format!(
            "Non-exact colour match for pixel [{}, {}, {}] — using palette index {} (distance = {})",
            colour[0], colour[1], colour[2], best_index, best_distance,
        ));
    }

    best_index as u8
}

fn trim_away_transparency(pixels_2d: &Vec<Vec<u8>>, width: u32, height: u32) -> (u32, u32, u32, u32) {
    // Determine how many rows/columns to trim from each edge
    let mut trim_top:    u32 = 0;
    let mut trim_bottom: u32 = 0;
    let mut trim_left:   u32 = 0;
    let mut trim_right:  u32 = 0;

    // Top
    for row in pixels_2d {
        if row.iter().all(|&p| p == 0) {
            trim_top += 1;
        } else {
            break;
        }
    }

    // Bottom
    for row in pixels_2d.iter().rev() {
        if row.iter().all(|&p| p == 0) {
            trim_bottom += 1;
        } else {
            break;
        }
    }

    // Left
    for x in 0..width as usize {
        if pixels_2d.iter().all(|row| row[x] == 0) {
            trim_left += 1;
        } else {
            break;
        }
    }

    // Right
    for x in (0..width as usize).rev() {
        if pixels_2d.iter().all(|row| row[x] == 0) {
            trim_right += 1;
        } else {
            break;
        }
    }
    log(LogLevel::Debug, &format!(
        "Trimming 0x{:0>2X} ({}) rows from top, 0x{:0>2X} ({}) from bottom, \
        0x{:0>2X} ({}) from left, 0x{:0>2X} ({}) from right",
        trim_top, trim_top, trim_bottom, trim_bottom, trim_left, trim_left, trim_right, trim_right,
    ));


    // Clamp dimensions
    let new_width = if width > trim_left + trim_right {
        width - trim_left - trim_right
    } else {
        log(LogLevel::Error, "Image is too small to trim. Setting width to 0");
        0
    };
    let new_height = if height > trim_top + trim_bottom {
        height - trim_top - trim_bottom
    } else {
        log(LogLevel::Error, "Image is too small to trim. Setting height to 0");
        0
    };

    log(LogLevel::Debug, &format!(
        "width:  0x{:0>2X} ({}),  new_width: 0x{:0>2X} ({}), x_offset: 0x{:0>2X} ({})",
        width, width, new_width, new_width,
        (width - new_width) / 2, (width - new_width) / 2,
    ));
    log(LogLevel::Debug, &format!(
        "height: 0x{:0>2X} ({}), new_height: 0x{:0>2X} ({}), y_offset: 0x{:0>2X} ({})",
        height, height, new_height, new_height,
        (height - new_height) / 2, (height - new_height) / 2,
    ));

    (new_width, new_height, trim_left, trim_top)
}

fn cast<T: TryFrom<u32>>(value: u32, name: &str) -> Result<T, Error> {
    T::try_from(value).map_err(|_| Error::new(ErrorKind::InvalidInput, format!("{} out of range", name)))
}


#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage, Rgba, RgbaImage};
    use std::fs;

    fn dummy_palette() -> Vec<[u8; 3]> {
        let mut palette = [[0u8; 3]; 256];
        for (i, rgb) in palette.iter_mut().enumerate() {
            rgb[0] = i as u8;
            rgb[1] = i as u8;
            rgb[2] = i as u8;
        }
        Vec::from(palette)
    }

    fn save_test_png_rgb(path: &str, colour: [u8; 3], width: u32, height: u32) {
        let mut img = RgbImage::new(width, height);
        for pixel in img.pixels_mut() {
            *pixel = Rgb(colour);
        }
        let _ = fs::remove_file(path); // Remove if it already exists
        img.save(path).unwrap();
    }

    fn save_test_png_rgba(path: &str, colour: [u8; 4], width: u32, height: u32) {
        let mut img = RgbaImage::new(width, height);
        for pixel in img.pixels_mut() {
            *pixel = Rgba(colour);
        }
        let _ = fs::remove_file(path); // Remove if it already exists
        img.save(path).unwrap();
    }


    #[test]
    fn detects_alpha_correctly() {
        let palette = dummy_palette();
        let path_rgb = "test_rgb.png";
        save_test_png_rgb(path_rgb, [100, 100, 100], 8, 8);

        let result_rgb: ImageWithMetadata<u8, u16> = read_png(path_rgb, &palette, true).unwrap();
        for i in 0..result_rgb.image_data.len() {
            assert_eq!(result_rgb.image_data[i], 100);
        }
        fs::remove_file(path_rgb).unwrap();


        let path_rgba = "test_rgba.png";
        save_test_png_rgba(path_rgba, [100, 100, 100, 255], 8, 8);

        let result_rgba: ImageWithMetadata<u8, u16> = read_png(path_rgba, &palette, true).unwrap();
        for i in 0..result_rgba.image_data.len() {
            assert_eq!(result_rgba.image_data[i], 100);
        }
        fs::remove_file(path_rgba).unwrap();
    }

    #[test]
    fn drops_alpha_channel_if_not_0() {
        let palette = dummy_palette();
        let path_rgba = "test_rgba_alpha.png";
        save_test_png_rgba(path_rgba, [100, 100, 100, 71], 8, 8);

        let trimmed_image: ImageWithMetadata<u8, u8> = read_png(path_rgba, &palette, true).unwrap();
        for i in 0..trimmed_image.image_data.len() {
            assert_eq!(trimmed_image.image_data[i], 100);
        }
        fs::remove_file(path_rgba).unwrap();
    }

    #[test]
    fn trims_transparent_rows_and_columns() {
        let palette = dummy_palette();
        let path = "test_trim.png";
        let mut img = RgbaImage::new(3, 3);

        // Center is visible, borders are fully transparent
        for y in 0..3 {
            for x in 0..3 {
                let alpha = if x == 1 && y == 1 { 255 } else { 0 };
                img.put_pixel(x, y, Rgba([100, 100, 100, alpha]));
            }
        }
        img.save(path).unwrap();

        let trimmed_image: ImageWithMetadata<u8, u8> = read_png(path, &palette, true).unwrap();
        assert_eq!(trimmed_image.width,    1);
        assert_eq!(trimmed_image.height,   1);
        assert_eq!(trimmed_image.x_offset, 1);
        assert_eq!(trimmed_image.y_offset, 1);

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn maps_non_exact_colours() {
        let palette = dummy_palette();
        let path = "test_colour.png";
        save_test_png_rgb(path, [100, 100, 101], 1, 1);

        let result: ImageWithMetadata<u8, u16> = read_png(path, &palette, false).unwrap();

        assert_eq!(result.image_data[0], 100); // Closest match
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn whole_image_is_transparent_and_trimmed_away() {
        let palette = dummy_palette();
        let path = "test_transparency.png";
        save_test_png_rgba(path, [0, 0, 0, 0], 1, 1); // Fully transparent

        let trimmed_image: ImageWithMetadata<u8, u16> = read_png(path, &palette, true).unwrap();

        assert_eq!(trimmed_image.image_data.len(), 0);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn whole_image_is_transparent_but_not_trimmed_away() {
        let palette = dummy_palette();
        let path = "test_transparency_without_trimming.png";
        save_test_png_rgba(path, [0, 0, 0, 0], 1, 1); // Fully transparent

        let trimmed_image: ImageWithMetadata<u8, u16> = read_png(path, &palette, false).unwrap();

        assert_eq!(trimmed_image.image_data.len(), 1);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn image_exactly_255x255() {
        let palette = dummy_palette();
        let path = "test_image_exactly_255x255.png";
        let mut img = RgbaImage::new(255, 255);
        for pixel in img.pixels_mut() {
            *pixel = Rgba([100, 100, 100, 255]);
        }
        img.save(&path).unwrap();

        let result: ImageWithMetadata<u8, u8> = read_png(path, &palette, true).unwrap();
        assert_eq!(result.width  + result.x_offset, 255);
        assert_eq!(result.height + result.y_offset, 255);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn image_just_above_255x255() {
        let palette = dummy_palette();
        let path = "test_image_just_above_255x255.png";
        let mut img = RgbaImage::new(256, 256);
        for pixel in img.pixels_mut() {
            *pixel = Rgba([100, 100, 100, 255]);
        }
        img.save(&path).unwrap();

        let result: Result<ImageWithMetadata<u8, u8>, Error> = read_png(path, &palette, false);
        assert!(result.is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn image_too_many_transparent_pixes() {
        let palette = dummy_palette();
        let path = "test_image_too_many_transparent_pixels.png";
        let mut img = RgbaImage::new(300, 300);

        // 260 pixels transparent on the top and left
        for y in 0..3 {
            for x in 0..3 {
                let alpha = if x > 260 && y > 260 { 255 } else { 0 };
                img.put_pixel(x, y, Rgba([100, 100, 100, alpha]));
            }
        }
        img.save(&path).unwrap();

        let result: Result<ImageWithMetadata<u8, u16>, Error> = read_png(path, &palette, true);
        assert!(result.is_err());
        fs::remove_file(path).unwrap();
    }
}
