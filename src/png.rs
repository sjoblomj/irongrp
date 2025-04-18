use crate::grp::GrpFrame;
use crate::{log, Args, LogLevel};
use image::{ColorType, DynamicImage, ImageBuffer};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;

type CacheKey = ([u8; 3], Option<u8>);
static COLOUR_INDEX_CACHE: Lazy<Mutex<HashMap<CacheKey, u8>>> = Lazy::new(|| Mutex::new(HashMap::new()));

pub struct TrimmedImage {
    pub x_offset: u8,
    pub y_offset: u8,
    pub width:    u8,
    pub height:   u8,
    pub original_width:  u16,
    pub original_height: u16,
    pub image_data: Vec<u8>,
}

fn create_dynamic_image(
    use_transparency: bool,
    max_frame_width:  u32,
    max_frame_height: u32,
    buffer: Vec<u8>,
) -> DynamicImage {
    if use_transparency {
        DynamicImage::ImageRgba8(
            ImageBuffer::from_raw(max_frame_width, max_frame_height, buffer)
                .expect("Failed to create RGBA image"),
        )
    } else {
        DynamicImage::ImageRgb8(
            ImageBuffer::from_raw(max_frame_width, max_frame_height, buffer)
                .expect("Failed to create RGB image"),
        )
    }
}

// Draws a frame into a raw buffer (Vec<u8>)
fn draw_frame_to_raw_buffer(
    frame: &GrpFrame,
    palette: &[[u8; 3]],
    max_width: u32,
    max_height: u32,
    use_transparency: bool,
) -> Vec<u8> {
    let mut buffer = vec![0u8; (max_width * max_height * if use_transparency { 4 } else { 3 }) as usize];

    let x_offset = frame.x_offset as u32;
    let y_offset = frame.y_offset as u32;

    for y in 0..frame.height as u32 {
        for x in 0..frame.width as u32 {
            let idx = (y * frame.width as u32 + x) as usize;
            let palette_index = frame.image_data.converted_pixels[idx] as usize;
            let colour = palette[palette_index];

            let out_x = x + x_offset;
            let out_y = y + y_offset;
            let pixel_index = (out_y * max_width + out_x) as usize;

            if use_transparency {
                let base = pixel_index * 4;
                if palette_index == 0 {
                    buffer[base..base + 4].copy_from_slice(&[0, 0, 0, 0]);
                } else {
                    buffer[base..base + 4].copy_from_slice(&[colour[0], colour[1], colour[2], 255]);
                }
            } else {
                let base = pixel_index * 3;
                buffer[base..base + 3].copy_from_slice(&[colour[0], colour[1], colour[2]]);
            }
        }
    }

    buffer
}

pub fn render_and_save_frames_to_png(
    frames: &[GrpFrame],
    palette: &[[u8; 3]],
    max_frame_width: u32,
    max_frame_height: u32,
    args: &Args,
) -> std::io::Result<()> {
    if args.tiled && args.frame_number.is_none() {
        // Tiled mode, so we need to draw all frames into one image.
        // Attempt to set the number of columns to sqrt(number of frames), so e.g., if there
        // are 25 frames, we will attempt to create a 5x5 image.
        // If the user has requested a max_width, then scale down to try to accommodate for that.
        // So, if there are 25 frames, but the user has requested a max_width that only fits
        // 3 frames, then the resulting image would be 3x9
        let mut cols = (frames.len() as f64).sqrt().floor() as u32;
        log(LogLevel::Debug, &format!(
            "Saving all frames as one PNG. Columns: {}, max-frame-size: {}x{}, requested max width: {}",
            cols, max_frame_width, max_frame_height, args.max_width.unwrap_or(0),
        ));

        // The user has requested a maximum width in pixels,
        // so we might need to adjust the number of columns down.
        if let Some(max_w) = args.max_width {
            if max_w > max_frame_width && cols * max_frame_width > max_w {
                cols = (max_w as f64 / max_frame_width as f64).floor() as u32;
                log(LogLevel::Debug, &format!("Adjusted number of columns to: {}", cols));
            } else if max_w < max_frame_width {
                cols = 1;
                log(LogLevel::Debug, &format!(
                    "The requested max-width, {}, is smaller than one frame. The resulting image \
                    will have 1 column and it will be {} pixels wide.",
                    max_w, max_frame_width));
            }
        }

        let canvas_width = cols * max_frame_width;
        let canvas_height = (frames.len() as f64 / cols as f64).ceil() as u32 * max_frame_height;

        let mut buffer = draw_frame_to_raw_buffer(&frames[0], palette, canvas_width, canvas_height, args.use_transparency);
        for (i, frame) in frames.iter().enumerate() {
            let col = (i as u32) % cols;
            let row = (i as u32) / cols;
            let base_x = col * max_frame_width;
            let base_y = row * max_frame_height;
            let temp_img = draw_frame_to_raw_buffer(frame, palette, max_frame_width, max_frame_height, args.use_transparency);

            for y in 0..max_frame_height {
                for x in 0..max_frame_width {
                    let dst_index = ((base_y + y) * canvas_width + (base_x + x)) as usize * if args.use_transparency { 4 } else { 3 };
                    let src_index = (y * max_frame_width + x) as usize * if args.use_transparency { 4 } else { 3 };
                    buffer[dst_index..dst_index + if args.use_transparency { 4 } else { 3 }]
                        .copy_from_slice(&temp_img[src_index..src_index + if args.use_transparency { 4 } else { 3 }]);
                }
            }
        }

        let image = create_dynamic_image(args.use_transparency, canvas_width, canvas_height, buffer);

        let output_path = format!("{}/all_frames.png", args.output_path.as_deref().unwrap());
        image.save(&output_path).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        log(LogLevel::Info, &format!("Saved all frames to {}", output_path));

    } else {
        // Non-tiled mode - save each frame as a separate image.

        // The following two HashMaps are used for printing duplicates
        // Map: image_data_offset -> list of frame indices
        let mut offset_map: HashMap<u32, Vec<usize>> = HashMap::new();
        // Map: image hash -> list of frame indices
        let mut image_hash_map: HashMap<u64, Vec<usize>> = HashMap::new();

        for (i, frame) in frames.iter().enumerate() {
            if args.frame_number == Some(i as u16) {
                continue;
            }
            offset_map.entry(frame.image_data_offset)
                .or_default()
                .push(i);

            let buffer = draw_frame_to_raw_buffer(frame, palette, max_frame_width, max_frame_height, args.use_transparency);
            let image = create_dynamic_image(args.use_transparency, max_frame_width, max_frame_height, buffer.clone());

            let mut hasher = DefaultHasher::new();
            buffer.hash(&mut hasher); // Hash the raw RGB(A) buffer
            let image_hash = hasher.finish();

            image_hash_map.entry(image_hash)
                .or_default()
                .push(i);

            let output_path = format!("{}/frame_{:03}.png", args.output_path.as_deref().unwrap(), i);
            image.save(&output_path).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
            log(LogLevel::Info, &format!("Saved frame {:2} to {}", i, output_path));
        }

        let mut offset_duplicates_vec: Vec<(&u32, &Vec<usize>)> = offset_map
            .iter()
            .filter(|(_, indices)| indices.len() > 1)
            .collect();
        // Sort by the lowest frame index in each group
        offset_duplicates_vec.sort_by_key(|(_, indices)| *indices.iter().min().unwrap());

        let mut offset_duplicates: HashSet<usize> = HashSet::new();
        for (_, indices) in offset_duplicates_vec {
            log(LogLevel::Info, &format!("Identical frames: {:?}", indices));
            offset_duplicates.extend(indices);
        }

        for (_, indices) in &image_hash_map {
            if indices.len() > 1 {
                let overlap = indices.iter().any(|idx| offset_duplicates.contains(idx));
                if !overlap {
                    log(LogLevel::Info, &format!("Identical frames with duplicated image data in GRP: {:?}", indices));
                }
            }
        }
    }

    Ok(())
}


fn cached_map_colour_to_palette_index(
    colour: [u8; 3],
    alpha: Option<u8>,
    palette: &[[u8; 3]]
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

fn map_colour_to_palette_index(colour: [u8; 3], alpha: Option<u8>, palette: &[[u8; 3]]) -> u8 {
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

fn trim_away_transparency(pixels_2d: &Vec<Vec<u8>>, width: u32, height: u32) -> (usize, usize, usize, usize) {
    // Determine how many rows/columns to trim from each edge
    let mut trim_top: usize    = 0;
    let mut trim_bottom: usize = 0;
    let mut trim_left: usize   = 0;
    let mut trim_right: usize  = 0;

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
    let new_width = if width as usize > trim_left + trim_right {
        width as usize - trim_left - trim_right
    } else {
        log(LogLevel::Error, "Image is too small to trim. Setting width to 0");
        0
    };
    let new_height = if height as usize > trim_top + trim_bottom {
        height as usize - trim_top - trim_bottom
    } else {
        log(LogLevel::Error, "Image is too small to trim. Setting height to 0");
        0
    };

    log(LogLevel::Debug, &format!(
        "width:  0x{:0>2X} ({}),  new_width: 0x{:0>2X} ({}), x_offset: 0x{:0>2X} ({})",
        width, width, new_width, new_width,
        (width as usize - new_width) / 2, (width as usize - new_width) / 2,
    ));
    log(LogLevel::Debug, &format!(
        "height: 0x{:0>2X} ({}), new_height: 0x{:0>2X} ({}), y_offset: 0x{:0>2X} ({})",
        height, height, new_height, new_height,
        (height as usize - new_height) / 2, (height as usize - new_height) / 2,
    ));

    (new_width, new_height, trim_left, trim_top)
}

pub fn png_to_pixels(png_file_name: &str, palette: &[[u8; 3]]) -> std::io::Result<TrimmedImage> {
    let img = image::open(png_file_name)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    let has_alpha = match img.color() {
        ColorType::Rgba8 | ColorType::La8 | ColorType::Rgba16 | ColorType::La16 => true,
        _ => false,
    };
    let img_data = img.to_rgba8();

    let (width, height) = img_data.dimensions();
    log(LogLevel::Debug, "");
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

    let (new_width, new_height, trim_left, trim_top) =
        trim_away_transparency(&pixels_2d, width, height);

    let mut trimmed_pixels = Vec::with_capacity(new_width * new_height);
    for row in pixels_2d.iter().skip(trim_top).take(new_height) {
        trimmed_pixels.extend(&row[trim_left .. (trim_left + new_width)]);
    }

    Ok(TrimmedImage {
        x_offset: trim_left     as u8,
        y_offset: trim_top      as u8,
        width:    new_width     as u8,
        height:   new_height    as u8,
        original_width:  width  as u16,
        original_height: height as u16,
        image_data: trimmed_pixels,
    })
}


#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage, Rgba, RgbaImage};
    use std::fs;

    fn dummy_palette() -> [[u8; 3]; 256] {
        let mut palette = [[0u8; 3]; 256];
        for (i, rgb) in palette.iter_mut().enumerate() {
            rgb[0] = i as u8;
            rgb[1] = i as u8;
            rgb[2] = i as u8;
        }
        palette
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

        let trimmed_image = png_to_pixels(path_rgb, &palette).unwrap();
        for i in 0..trimmed_image.image_data.len() {
            assert_eq!(trimmed_image.image_data[i], 100);
        }
        fs::remove_file(path_rgb).unwrap();


        let path_rgba = "test_rgba.png";
        save_test_png_rgba(path_rgba, [100, 100, 100, 255], 8, 8);

        let trimmed_image = png_to_pixels(path_rgba, &palette).unwrap();
        for i in 0..trimmed_image.image_data.len() {
            assert_eq!(trimmed_image.image_data[i], 100);
        }
        fs::remove_file(path_rgba).unwrap();
    }

    #[test]
    fn drops_alpha_channel_if_not_0() {
        let palette = dummy_palette();
        let path_rgba = "test_rgba_alpha.png";
        save_test_png_rgba(path_rgba, [100, 100, 100, 71], 8, 8);

        let trimmed_image = png_to_pixels(path_rgba, &palette).unwrap();
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

        let trimmed_image = png_to_pixels(path, &palette).unwrap();
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

        let trimmed_image = png_to_pixels(path, &palette).unwrap();

        assert_eq!(trimmed_image.image_data[0], 100); // Closest match
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn whole_image_is_transparent_and_trimmed_away() {
        let palette = dummy_palette();
        let path = "test_transparency.png";
        save_test_png_rgba(path, [0, 0, 0, 0], 1, 1); // Fully transparent

        let trimmed_image = png_to_pixels(path, &palette).unwrap();

        assert_eq!(trimmed_image.image_data.len(), 0);
        fs::remove_file(path).unwrap();
    }
}
