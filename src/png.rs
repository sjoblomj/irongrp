use crate::grp::{GrpFrame, GrpType, EXTENDED_IMAGE_WIDTH};
use crate::error::{Error, InFile, Result};
use crate::{validate_frame_number, GrpToPngArgs, UNCOMPRESSED_FILENAME, WAR1_FILENAME};
use log::{debug, info, warn};
use palpngrs::{draw_image_to_pixel_buffer, read_png, save_pixels_to_image_file, Offset, Palette0Pixels, PalettizedImageWithMetadata, Size};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

pub fn render_and_save_frames_to_png(
    frames: &[GrpFrame],
    palette: &[[u8; 3]],
    max_frame_width:  u32,
    max_frame_height: u32,
    args: &GrpToPngArgs,
) -> Result<()> {
    let (max_frame_width, max_frame_height) = canvas_size(frames, max_frame_width, max_frame_height);

    if args.tiled && args.frame.is_none() {
        if frames.is_empty() {
            return Err(Error::InvalidArgument("The GRP has no frames to draw".to_string()));
        }
        // Tiled mode, so we need to draw all frames into one image.
        // Attempt to set the number of columns to sqrt(number of frames), so e.g., if there
        // are 25 frames, we will attempt to create a 5x5 image.
        // If the user has requested a max_width, then scale down to try to accommodate for that.
        // So, if there are 25 frames, but the user has requested a max_width that only fits
        // 3 frames, then the resulting image would be 3x9
        check_output_files(args, &[TILED_FILENAME.to_string()])?;

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

        let pixel_length: usize = if args.transparent { 4 } else { 3 }; // RGBA or RGB
        let mut buffer = vec![0u8; pixel_length * (canvas_width * canvas_height) as usize];

        for (i, frame) in frames.iter().enumerate() {
            let col = (i as u32) % cols;
            let row = (i as u32) / cols;
            let base_x = col * max_frame_width;
            let base_y = row * max_frame_height;

            let temp_img = image_to_buffer(frame, palette, max_frame_width, max_frame_height, args.transparent)?;

            for y in 0..max_frame_height {
                for x in 0..max_frame_width {
                    let dst_index = ((base_y + y) * canvas_width + (base_x + x)) as usize * pixel_length;
                    let src_index = (y * max_frame_width + x) as usize * pixel_length;
                    buffer[dst_index..dst_index + pixel_length]
                        .copy_from_slice(&temp_img[src_index..src_index  + pixel_length]);
                }
            }
        }

        let output_path = format!("{}/{}", args.output, TILED_FILENAME);
        save_pixels_to_image_file(buffer, &output_path, args.transparent, canvas_width, canvas_height)
            .in_file(&output_path)?;
        info!("Saved all frames to {}", output_path);

    } else {
        // Non-tiled mode - save each frame as a separate image.
        validate_frame_number(args.frame, frames.len())?;
        let frames_to_save: Vec<(usize, &GrpFrame)> = frames.iter().enumerate()
            .filter(|(i, _)| args.frame.is_none() || args.frame == Some(*i as u16))
            .collect();
        let file_names: Vec<String> = frames_to_save.iter().map(|(i, frame)| frame_png_name(frame, *i)).collect();
        check_output_files(args, &file_names)?;

        for ((i, frame), file_name) in frames_to_save.into_iter().zip(file_names) {
            let buffer = image_to_buffer(frame, palette, max_frame_width, max_frame_height, args.transparent)?;

            let output_path = format!("{}/{}", args.output, file_name);
            save_pixels_to_image_file(buffer, &output_path, args.transparent, max_frame_width, max_frame_height)
                .in_file(&output_path)?;
            info!("Saved frame {:2} to {}", i, output_path);
        }

        if args.frame.is_none() {
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

const TILED_FILENAME: &str = "all_frames.png";

/// The name of the PNG that frame number `index` is saved as, e.g. "uncompressed_frame_007.png".
/// The prefix tells `png-to-grp` which compression type to use when converting it back.
fn frame_png_name(frame: &GrpFrame, index: usize) -> String {
    let grp_type = match frame.image_data.grp_type {
        GrpType::Normal => "".to_string(),
        GrpType::War1   => format!("{}_", WAR1_FILENAME),
        GrpType::Uncompressed | GrpType::UncompressedExtended => format!("{}_", UNCOMPRESSED_FILENAME),
    };
    format!("{}frame_{:03}.png", grp_type, index)
}

/// Whether `file_name` looks like the name of a PNG written by [`frame_png_name`].
fn is_frame_png_name(file_name: &str) -> bool {
    let name = file_name.to_ascii_lowercase();
    let name = name.strip_prefix(&format!("{}_", UNCOMPRESSED_FILENAME))
        .or_else(|| name.strip_prefix(&format!("{}_", WAR1_FILENAME)))
        .unwrap_or(&name);
    name.strip_prefix("frame_")
        .and_then(|rest| rest.strip_suffix(".png"))
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
}

/// Checks that saving `file_names` in the output directory does not overwrite existing files,
/// and, when all frames are saved, that the directory has no frame PNGs from before. Such
/// leftovers, e.g. from a GRP with more frames, would be picked up by `png-to-grp` together with
/// the new frames. With `--force`, existing files are overwritten and leftovers are warned about.
fn check_output_files(args: &GrpToPngArgs, file_names: &[String]) -> Result<()> {
    let dir = Path::new(&args.output);
    let mut conflicts: Vec<String> = file_names.iter()
        .filter(|name| dir.join(name).exists())
        .cloned()
        .collect();

    let mut leftovers = Vec::new();
    if !args.tiled && args.frame.is_none() {
        let new_names: HashSet<&str> = file_names.iter().map(String::as_str).collect();
        for entry in fs::read_dir(dir).in_file(dir)? {
            let name = entry.in_file(dir)?.file_name().to_string_lossy().into_owned();
            if is_frame_png_name(&name) && !new_names.contains(name.as_str()) {
                leftovers.push(name);
            }
        }
        leftovers.sort();
    }

    if args.force {
        if let Some(example) = leftovers.first() {
            warn!(
                "'{}' contains {} frame PNG(s) that are not part of this GRP, such as '{}'. \
                They are left as they are, but png-to-grp would include them.",
                args.output, leftovers.len(), example,
            );
        }
        return Ok(());
    }
    conflicts.append(&mut leftovers);
    conflicts.sort();
    match conflicts.first() {
        None => Ok(()),
        Some(example) => Err(Error::InvalidArgument(format!(
            "'{}' already contains {} frame PNG(s), such as '{}'. Use --force to overwrite them, \
            or choose an empty output directory",
            args.output, conflicts.len(), example,
        ))),
    }
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

    fn make_test_args(output_path: &str, frame: Option<u16>) -> GrpToPngArgs {
        GrpToPngArgs {
            input:       String::new(),
            output:      output_path.to_string(),
            palette:     None,
            tiled:       false,
            max_width:   None,
            frame,
            transparent: false,
            force:       false,
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
    fn recognises_names_of_frame_pngs() {
        for name in ["frame_000.png", "frame_1000.png", "uncompressed_frame_007.png", "war1_frame_12.PNG"] {
            assert!(is_frame_png_name(name), "{}", name);
        }
        for name in ["all_frames.png", "frame_.png", "frame_00a.png", "frame_000.png.bak", "my_frame_000.png", "f.png"] {
            assert!(!is_frame_png_name(name), "{}", name);
        }
    }

    fn file_names_in(dir: &str) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir).unwrap()
            .map(|e| e.unwrap().file_name().to_str().unwrap().to_string())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn refuses_to_overwrite_existing_pngs_without_force() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path().to_str().unwrap();
        let palette = greyscale_palette();
        let frames = vec![make_test_frame(10, 4, 4), make_test_frame(20, 4, 4)];
        fs::write(temp_dir.path().join("frame_001.png"), "not a PNG").unwrap();

        for frame in [None, Some(1)] {
            let err = render_and_save_frames_to_png(&frames, &palette, 4, 4, &make_test_args(dir, frame))
                .expect_err("expected the existing PNG not to be overwritten");
            assert!(err.to_string().contains("Use --force"), "{}", err);
        }
        assert_eq!(file_names_in(dir), vec!["frame_001.png"], "expected nothing to be written");
        assert_eq!(fs::read(temp_dir.path().join("frame_001.png")).unwrap(), b"not a PNG");

        let mut args = make_test_args(dir, None);
        args.force = true;
        render_and_save_frames_to_png(&frames, &palette, 4, 4, &args).unwrap();
        assert_eq!(png_size(&format!("{}/frame_001.png", dir)), (4, 4));
    }

    #[test]
    fn refuses_to_mix_new_frames_with_leftover_frames_without_force() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path().to_str().unwrap();
        let palette = greyscale_palette();
        let frames = vec![make_test_frame(10, 4, 4), make_test_frame(20, 4, 4)];
        // Left from converting a GRP with more frames
        fs::write(temp_dir.path().join("frame_002.png"), "old").unwrap();
        fs::write(temp_dir.path().join("notes.txt"), "unrelated").unwrap();

        let err = render_and_save_frames_to_png(&frames, &palette, 4, 4, &make_test_args(dir, None))
            .expect_err("expected the leftover frame to be reported");
        assert!(err.to_string().contains("'frame_002.png'"), "{}", err);

        // A single frame, or a tiled image, does not replace the whole set of frames
        render_and_save_frames_to_png(&frames, &palette, 4, 4, &make_test_args(dir, Some(0))).unwrap();
        let mut args = make_test_args(dir, None);
        args.tiled = true;
        render_and_save_frames_to_png(&frames, &palette, 4, 4, &args).unwrap();
        assert_eq!(file_names_in(dir), vec!["all_frames.png", "frame_000.png", "frame_002.png", "notes.txt"]);

        // With --force, the frames are written and the leftover is kept
        let mut args = make_test_args(dir, None);
        args.force = true;
        render_and_save_frames_to_png(&frames, &palette, 4, 4, &args).unwrap();
        assert_eq!(
            file_names_in(dir),
            vec!["all_frames.png", "frame_000.png", "frame_001.png", "frame_002.png", "notes.txt"],
        );
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

        let mut args = make_test_args(dir, None);
        args.force = true; // The second iteration overwrites the PNGs of the first
        for (header_width, header_height) in [(2, 2), (0, 0)] {
            render_and_save_frames_to_png(&frames, &palette, header_width, header_height, &args).unwrap();

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
