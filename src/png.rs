use crate::grp::{GrpFrame, GrpType, EXTENDED_IMAGE_WIDTH};
use crate::error::{Error, InFile, Result};
use crate::{validate_frame_number, Args, UNCOMPRESSED_FILENAME, WAR1_FILENAME};
use log::{debug, info, warn};
use palpngrs::{draw_image_to_pixel_buffer, read_png, save_pixels_to_image_file, Offset, Palette0Pixels, PalettizedImageWithMetadata, Size};
use std::collections::{HashMap, HashSet};

pub fn render_and_save_frames_to_png(
    frames: &[GrpFrame],
    palette: &[[u8; 3]],
    max_frame_width:  u32,
    max_frame_height: u32,
    args: &Args,
) -> Result<()> {
    let (max_frame_width, max_frame_height) = canvas_size(frames, max_frame_width, max_frame_height);

    if args.tiled && args.frame_number.is_none() {
        if frames.is_empty() {
            return Err(Error::InvalidArgument("The GRP has no frames to draw".to_string()));
        }
        // Tiled mode, so we need to draw all frames into one image.
        // Attempt to set the number of columns to sqrt(number of frames), so e.g., if there
        // are 25 frames, we will attempt to create a 5x5 image.
        // If the user has requested a max_width, then scale down to try to accommodate for that.
        // So, if there are 25 frames, but the user has requested a max_width that only fits
        // 3 frames, then the resulting image would be 3x9
        let mut cols = (frames.len() as f64).sqrt().floor() as u32;
        debug!(
            "Saving all frames as one PNG. Columns: {}, max-frame-size: {}x{}, requested max width: {}",
            cols, max_frame_width, max_frame_height, args.max_width.unwrap_or(0),
        );

        // The user has requested a maximum width in pixels,
        // so we might need to adjust the number of columns down.
        if let Some(max_w) = args.max_width {
            if max_w > max_frame_width && cols * max_frame_width > max_w {
                cols = (max_w as f64 / max_frame_width as f64).floor() as u32;
                debug!("Adjusted number of columns to: {}", cols);
            } else if max_w < max_frame_width {
                cols = 1;
                debug!(
                    "The requested max-width, {}, is smaller than one frame. The resulting image \
                    will have 1 column and it will be {} pixels wide.",
                    max_w, max_frame_width
                );
            }
        }

        let canvas_width = cols * max_frame_width;
        let canvas_height = (frames.len() as f64 / cols as f64).ceil() as u32 * max_frame_height;

        let pixel_length: usize = if args.use_transparency { 4 } else { 3 }; // RGBA or RGB
        let mut buffer = vec![0u8; pixel_length * (canvas_width * canvas_height) as usize];

        for (i, frame) in frames.iter().enumerate() {
            let col = (i as u32) % cols;
            let row = (i as u32) / cols;
            let base_x = col * max_frame_width;
            let base_y = row * max_frame_height;

            let temp_img = image_to_buffer(frame, palette, max_frame_width, max_frame_height, args.use_transparency)?;

            for y in 0..max_frame_height {
                for x in 0..max_frame_width {
                    let dst_index = ((base_y + y) * canvas_width + (base_x + x)) as usize * pixel_length;
                    let src_index = (y * max_frame_width + x) as usize * pixel_length;
                    buffer[dst_index..dst_index + pixel_length]
                        .copy_from_slice(&temp_img[src_index..src_index  + pixel_length]);
                }
            }
        }

        let output_path = format!("{}/all_frames.png", args.output_path.as_deref().unwrap());
        save_pixels_to_image_file(buffer, &output_path, args.use_transparency, canvas_width, canvas_height)
            .in_file(&output_path)?;
        info!("Saved all frames to {}", output_path);

    } else {
        // Non-tiled mode - save each frame as a separate image.
        validate_frame_number(args.frame_number, frames.len())?;
        for (i, frame) in frames.iter().enumerate() {
            if args.frame_number.is_some() && args.frame_number != Some(i as u16) {
                continue;
            }
            let buffer = image_to_buffer(frame, palette, max_frame_width, max_frame_height, args.use_transparency)?;

            let grp_type = if frame.image_data.grp_type == GrpType::Normal {
                ""
            } else if frame.image_data.grp_type == GrpType::War1 {
                &format!("{}_", WAR1_FILENAME)
            } else {
                &format!("{}_", UNCOMPRESSED_FILENAME)
            };

            let output_path = format!("{}/{}frame_{:03}.png", args.output_path.as_deref().unwrap(), grp_type, i);
            save_pixels_to_image_file(buffer, &output_path, args.use_transparency, max_frame_width, max_frame_height)
                .in_file(&output_path)?;
            info!("Saved frame {:2} to {}", i, output_path);
        }

        if args.frame_number.is_none() {
            let duplicates = find_identical_frames(frames);
            for indices in duplicates.shared_image_data {
                info!("Identical frames: {:?}", indices);
            }
            for indices in duplicates.duplicated_image_data {
                info!("Identical frames with duplicated image data in GRP: {:?}", indices);
            }
        }
    }

    Ok(())
}

/// The size of the canvas to draw each frame on: the max width and height from the GRP header,
/// enlarged if any frame extends beyond them, so that every frame fits.
fn canvas_size(frames: &[GrpFrame], header_width: u32, header_height: u32) -> (u32, u32) {
    let extent_width  = frames.iter().map(|f| f.x_offset as u32 + f.decoded_width() as u32).max().unwrap_or(0);
    let extent_height = frames.iter().map(|f| f.y_offset as u32 + f.height as u32).max().unwrap_or(0);

    if extent_width > header_width || extent_height > header_height {
        warn!(
            "The frames extend to {}x{}, beyond the max size of {}x{} given in the GRP header. \
            The images will be enlarged so that all frames fit.",
            extent_width, extent_height, header_width, header_height,
        );
    }
    (header_width.max(extent_width), header_height.max(extent_height))
}

/// Groups of frames that are identical, each group sorted by frame index,
/// and the groups sorted by their first frame index.
#[derive(Debug, PartialEq)]
struct IdenticalFrames {
    /// Frames that refer to the same image data in the GRP
    shared_image_data: Vec<Vec<usize>>,
    /// Frames whose images are identical, but whose image data is stored more than once in the GRP
    duplicated_image_data: Vec<Vec<usize>>,
}

/// Frames with equal keys render to identical images
#[derive(Hash, Eq, PartialEq)]
struct RenderedImageKey<'a> {
    x_offset: u8,
    y_offset: u8,
    width:    u16,
    height:   u8,
    pixels:   &'a [u8],
}

fn find_identical_frames(frames: &[GrpFrame]) -> IdenticalFrames {
    let mut offset_map: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut image_map: HashMap<RenderedImageKey, Vec<usize>> = HashMap::new();

    for (i, frame) in frames.iter().enumerate() {
        offset_map.entry(frame.image_data_offset).or_default().push(i);

        let key = RenderedImageKey {
            x_offset: frame.x_offset,
            y_offset: frame.y_offset,
            width:    frame.decoded_width(),
            height:   frame.height,
            pixels:   &frame.image_data.converted_pixels,
        };
        image_map.entry(key).or_default().push(i);
    }

    let groups = |map: Vec<Vec<usize>>| -> Vec<Vec<usize>> {
        let mut groups: Vec<Vec<usize>> = map.into_iter().filter(|indices| indices.len() > 1).collect();
        groups.sort();
        groups
    };
    let shared_image_data = groups(offset_map.into_values().collect());
    // Only report identical images whose data is actually stored more than once
    let duplicated_image_data = groups(image_map.into_values().filter(|indices| {
        indices.iter().map(|&i| frames[i].image_data_offset).collect::<HashSet<_>>().len() > 1
    }).collect());

    IdenticalFrames { shared_image_data, duplicated_image_data }
}

fn image_to_buffer(
    frame: &GrpFrame,
    palette: &[[u8; 3]],
    max_frame_width:  u32,
    max_frame_height: u32,
    use_transparency: bool,
) -> Result<Vec<u8>> {

    let image = PalettizedImageWithMetadata::new(
        Offset::new(frame.x_offset as u32, frame.y_offset as u32),
        Size::new(frame.decoded_width() as u32, frame.height as u32),
        Size::new(max_frame_width, max_frame_height),
        frame.image_data.converted_pixels.clone(),
    );

    let buffer = draw_image_to_pixel_buffer(image, palette, use_transparency)?;
    Ok(buffer)
}

pub fn png_to_pixels(png_file_name: &str, palette: &[[u8; 3]]) -> Result<PalettizedImageWithMetadata<u8, u16>> {
    debug!(""); // Give some space in the logs
    // PNGs exported without --use-transparency have their transparent pixels drawn as palette[0],
    // so treat that colour as transparent when reading them back.
    let png: PalettizedImageWithMetadata<u8, u16> = read_png(png_file_name, palette, true, Palette0Pixels::Transparent)?;

    // UncompressedExtended GRPs store width as `actual_width - EXTENDED_IMAGE_WIDTH` in a u8,
    // so the maximum representable width is EXTENDED_IMAGE_WIDTH + u8::MAX (= 511).
    let max_width = EXTENDED_IMAGE_WIDTH as u32 + u8::MAX as u32;
    if png.width as u32 > max_width || png.height as u32 > u8::MAX as u32 {
        return Err(Error::CannotEncode(format!(
            "Width ({}) is above limit of {}, or height ({}) is above limit of {}",
            png.width, max_width, png.height, u8::MAX,
        )))
    }
    Ok(png)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::grp::ImageData;
    use crate::{CompressionType, LogLevel};
    use palpngrs::greyscale_palette;
    use std::path::Path;

    fn make_test_frame(pixel_value: u8, width: u8, height: u8) -> GrpFrame {
        GrpFrame {
            x_offset: 0,
            y_offset: 0,
            width,
            height,
            image_data_offset: 0,
            image_data: ImageData {
                row_offsets:      vec![],
                raw_row_data:     vec![],
                converted_pixels: vec![pixel_value; width as usize * height as usize],
                grp_type:         GrpType::Normal,
            },
        }
    }

    fn make_test_args(output_path: &str, frame_number: Option<u16>) -> Args {
        Args {
            input_path:         None,
            pal_path:           None,
            output_path:        Some(output_path.to_string()),
            mode:               None,
            compression_type:   CompressionType::Auto,
            tiled:              false,
            max_width:          None,
            frame_number,
            analyse_row_number: None,
            use_transparency:   false,
            log_level:          LogLevel::Info,
            generator:          None,
        }
    }

    #[test]
    fn saves_all_frames_when_no_frame_number_given() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path().to_str().unwrap();
        let palette = greyscale_palette();

        let frames = vec![
            make_test_frame(10, 4, 4),
            make_test_frame(20, 4, 4),
            make_test_frame(30, 4, 4),
        ];
        let args = make_test_args(dir, None);

        render_and_save_frames_to_png(&frames, &palette, 4, 4, &args).unwrap();

        for i in 0..frames.len() {
            let path = format!("{}/frame_{:03}.png", dir, i);
            assert!(Path::new(&path).exists(), "Expected {} to exist", path);
        }
    }

    #[test]
    fn saves_only_requested_frame_when_frame_number_given() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path().to_str().unwrap();
        let palette = greyscale_palette();

        let frames = vec![
            make_test_frame(10, 4, 4),
            make_test_frame(20, 4, 4),
            make_test_frame(30, 4, 4),
        ];
        let args = make_test_args(dir, Some(1));

        render_and_save_frames_to_png(&frames, &palette, 4, 4, &args).unwrap();

        assert!(!Path::new(&format!("{}/frame_000.png", dir)).exists());
        assert!( Path::new(&format!("{}/frame_001.png", dir)).exists());
        assert!(!Path::new(&format!("{}/frame_002.png", dir)).exists());
    }

    #[test]
    fn rejects_frame_number_out_of_range_and_saves_nothing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path().to_str().unwrap();
        let palette = greyscale_palette();

        let frames = vec![
            make_test_frame(10, 4, 4),
            make_test_frame(20, 4, 4),
            make_test_frame(30, 4, 4),
        ];

        for frame_number in [3, 7] {
            let err = render_and_save_frames_to_png(&frames, &palette, 4, 4, &make_test_args(dir, Some(frame_number)))
                .expect_err("expected the frame number to be rejected");
            assert!(matches!(err, Error::InvalidArgument(_)), "for frame {}", frame_number);
        }
        assert_eq!(std::fs::read_dir(dir).unwrap().count(), 0, "expected no files to be saved");

        // The last frame is still accepted
        render_and_save_frames_to_png(&frames, &palette, 4, 4, &make_test_args(dir, Some(2))).unwrap();
        assert!(Path::new(&format!("{}/frame_002.png", dir)).exists());
    }

    fn make_test_frame_with_offsets(pixel_value: u8, width: u8, height: u8, x_offset: u8, y_offset: u8) -> GrpFrame {
        let mut frame = make_test_frame(pixel_value, width, height);
        frame.x_offset = x_offset;
        frame.y_offset = y_offset;
        frame
    }

    fn png_size(path: &str) -> (u32, u32) {
        image::image_dimensions(path).unwrap_or_else(|e| panic!("could not read {}: {}", path, e))
    }

    #[test]
    fn canvas_size_is_header_size_when_all_frames_fit() {
        let frames = vec![make_test_frame_with_offsets(1, 2, 2, 3, 4)]; // Extends to 5x6
        assert_eq!(canvas_size(&frames, 5, 6),   (5, 6));
        assert_eq!(canvas_size(&frames, 10, 10), (10, 10));
        assert_eq!(canvas_size(&[], 7, 8),       (7, 8));
    }

    #[test]
    fn canvas_size_is_enlarged_to_fit_all_frames() {
        let frames = vec![
            make_test_frame_with_offsets(1, 2, 2, 3, 0), // Extends to 5x2
            make_test_frame_with_offsets(1, 1, 3, 0, 4), // Extends to 1x7
        ];
        assert_eq!(canvas_size(&frames, 4, 4), (5, 7));
        assert_eq!(canvas_size(&frames, 0, 0), (5, 7));
        assert_eq!(canvas_size(&frames, 9, 0), (9, 7));
    }

    #[test]
    fn canvas_size_accounts_for_extended_width() {
        let mut frame = make_test_frame_with_offsets(1, 44, 1, 2, 0);
        frame.image_data.grp_type = GrpType::UncompressedExtended;
        frame.image_data.converted_pixels = vec![1; 300];
        assert_eq!(canvas_size(&[frame], 256, 1), (302, 1)); // 2 + 44 + 256
    }

    #[test]
    fn saves_frames_extending_beyond_header_size() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path().to_str().unwrap();
        let palette = greyscale_palette();
        let frames = vec![
            make_test_frame_with_offsets(10, 2, 2, 0, 0),
            make_test_frame_with_offsets(20, 2, 2, 1, 1), // Extends to 3x3
        ];

        for (header_width, header_height) in [(2, 2), (0, 0)] {
            render_and_save_frames_to_png(&frames, &palette, header_width, header_height, &make_test_args(dir, None))
                .unwrap();

            for i in 0..frames.len() {
                let path = format!("{}/frame_{:03}.png", dir, i);
                assert_eq!(png_size(&path), (3, 3), "for header size {}x{}", header_width, header_height);
            }
            let png = png_to_pixels(&format!("{}/frame_001.png", dir), &palette).unwrap();
            assert_eq!((png.x_offset, png.y_offset, png.width, png.height), (1, 1, 2, 2));
            assert_eq!(png.palettized_image, vec![20; 4]);
        }
    }

    #[test]
    fn saves_tiled_frames_extending_beyond_header_size() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path().to_str().unwrap();
        let palette = greyscale_palette();
        let frames: Vec<GrpFrame> = (0..4).map(|i| make_test_frame_with_offsets(10, 2, 2, i, 0)).collect();

        let mut args = make_test_args(dir, None);
        args.tiled = true;
        render_and_save_frames_to_png(&frames, &palette, 2, 2, &args).unwrap();

        // 2x2 frames of 5x2 pixels each, since the last frame extends to x = 3 + 2
        assert_eq!(png_size(&format!("{}/all_frames.png", dir)), (10, 4));
    }

    #[test]
    fn tiled_mode_rejects_grp_without_frames() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path().to_str().unwrap();
        let mut args = make_test_args(dir, None);
        args.tiled = true;

        let err = render_and_save_frames_to_png(&[], &greyscale_palette(), 4, 4, &args)
            .expect_err("expected an error for a GRP without frames");
        assert!(matches!(err, Error::InvalidArgument(_)));
    }

    fn make_test_frame_at(pixel_value: u8, width: u8, height: u8, image_data_offset: u32) -> GrpFrame {
        let mut frame = make_test_frame(pixel_value, width, height);
        frame.image_data_offset = image_data_offset;
        frame
    }

    #[test]
    fn find_identical_frames_groups_frames_by_shared_and_duplicated_image_data() {
        let frames = vec![
            make_test_frame_at(10, 4, 4, 100), // 0: shares image data with 2
            make_test_frame_at(20, 4, 4, 200), // 1: same image as 3, stored twice
            make_test_frame_at(10, 4, 4, 100), // 2
            make_test_frame_at(20, 4, 4, 300), // 3
            make_test_frame_at(30, 4, 4, 400), // 4: unique
        ];

        assert_eq!(find_identical_frames(&frames), IdenticalFrames {
            shared_image_data:     vec![vec![0, 2]],
            duplicated_image_data: vec![vec![1, 3]],
        });
    }

    #[test]
    fn find_identical_frames_reports_copy_of_shared_image_data_as_duplicated() {
        let frames = vec![
            make_test_frame_at(10, 4, 4, 100),
            make_test_frame_at(10, 4, 4, 100),
            make_test_frame_at(10, 4, 4, 200), // Same image, but its data is stored again
        ];

        assert_eq!(find_identical_frames(&frames), IdenticalFrames {
            shared_image_data:     vec![vec![0, 1]],
            duplicated_image_data: vec![vec![0, 1, 2]],
        });
    }

    #[test]
    fn find_identical_frames_requires_same_dimensions_and_offsets() {
        let mut moved = make_test_frame_at(5, 2, 3, 300);
        moved.x_offset = 1;
        let frames = vec![
            make_test_frame_at(5, 2, 3, 100),
            make_test_frame_at(5, 3, 2, 200), // Same pixels, different dimensions
            moved,                             // Same pixels and dimensions, different offset
        ];

        assert_eq!(find_identical_frames(&frames), IdenticalFrames {
            shared_image_data:     vec![],
            duplicated_image_data: vec![],
        });
    }

    #[test]
    fn find_identical_frames_returns_groups_in_frame_order() {
        // Many groups, so that HashMap iteration order would be likely to differ from frame order
        let frames: Vec<GrpFrame> = (0..40)
            .map(|i| make_test_frame_at((i % 20) as u8, 2, 2, i * 10))
            .collect();

        let duplicated = find_identical_frames(&frames).duplicated_image_data;
        let expected: Vec<Vec<usize>> = (0..20).map(|i| vec![i, i + 20]).collect();
        assert_eq!(duplicated, expected);
    }

    #[test]
    fn png_without_transparency_reads_back_with_transparent_background() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path().to_str().unwrap();
        let palette = greyscale_palette();

        // A 2x2 frame at (3, 1) on a 6x4 canvas. The background is drawn as
        // opaque palette[0] since use_transparency is false.
        let mut frame = make_test_frame(10, 2, 2);
        frame.x_offset = 3;
        frame.y_offset = 1;
        let args = make_test_args(dir, None);
        render_and_save_frames_to_png(&[frame], &palette, 6, 4, &args).unwrap();

        let png = png_to_pixels(&format!("{}/frame_000.png", dir), &palette).unwrap();
        assert_eq!((png.x_offset, png.y_offset), (3, 1));
        assert_eq!((png.width, png.height), (2, 2));
        assert_eq!(png.palettized_image, vec![10; 4]);
    }
}
