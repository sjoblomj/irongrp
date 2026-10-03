use crate::error::{Error, InFile, Result};
use crate::grp::{read_grp_file, GrpFrame, GrpType, EXTENDED_IMAGE_WIDTH};
use crate::Args;
use log::{debug, info, warn};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

/// Analyzes a GRP file and prints information about header correctness, unused space, overlapping
/// ranges, and file layout.
pub fn analyse_grp(args: &Args) -> Result<()> {
    let input_path = args.input_path.as_deref().unwrap();
    let (header, grp_type, frames) = read_grp_file(input_path)?;
    let is_uncompressed = grp_type != GrpType::Normal;

    let mut file = File::open(input_path).in_file(input_path)?;
    let file_len = file.metadata().in_file(input_path)?.len();

    println!();
    info!("GRP type: {:?}", grp_type);

    if args.frame_number.is_some() {
        let frame_number = args.frame_number.unwrap() as usize;
        if  frame_number >= frames.len() {
            return Err(Error::InvalidArgument(format!(
                "Frame number {} is out of range; the GRP has {} frame(s)", frame_number, frames.len(),
            )));
        }
        if args.analyse_row_number.is_some() && is_uncompressed {
            return Err(Error::InvalidArgument(
                "--analyse-row-number is only supported for GRPs of type Normal".to_string(),
            ));
        }
        let row_number = if args.analyse_row_number.is_none() || is_uncompressed {
            frames[frame_number].height + 1
        } else {
            args.analyse_row_number.unwrap()
        };
        if row_number >= frames[frame_number].height && args.analyse_row_number.is_some() {
            return Err(Error::InvalidArgument(format!(
                "Row number {} is out of range; frame {} has {} row(s)",
                row_number, frame_number, frames[frame_number].height,
            )));
        }

        let width = if frames[frame_number].image_data.grp_type != GrpType::UncompressedExtended {
            frames[frame_number].width as u16
        } else {
            frames[frame_number].width as u16 + EXTENDED_IMAGE_WIDTH
        };
        let next_offset = if frame_number + 1 < frames.len() {
            frames[frame_number + 1].decoded_image_data_offset()
        } else {
            file_len as u32
        };
        info!("Analyzing frame {}:", frame_number);
        info!("- GrpType:  {:?}", frames[frame_number].image_data.grp_type);
        info!("- X offset: {}", frames[frame_number].x_offset);
        info!("- Y offset: {}", frames[frame_number].y_offset);
        info!("- Width:    {}", width);
        info!("- Height:   {}", frames[frame_number].height);
        info!("- This frames image data offset: 0x{:0>2X}", frames[frame_number].decoded_image_data_offset());
        info!("- Next frames image data offset: 0x{:0>2X}", next_offset);
        if frames[frame_number].image_data.grp_type == GrpType::Normal {
            for (i, _) in frames[frame_number].image_data.raw_row_data.iter().enumerate() {
                info!(
                    "- Row {: >2} (0x{:0>2X}), Relative offset: 0x{:0>4X}, Absolute offset: 0x{:0>6X}",
                    i, i, frames[frame_number].image_data.row_offsets[i],
                    frames[frame_number].image_data.row_offsets[i] + frames[frame_number].decoded_image_data_offset() as u16,
                );
            }
        }
        if args.analyse_row_number.is_some() && frames[frame_number].image_data.grp_type == GrpType::Normal {
            for (i, row) in frames[frame_number].image_data.raw_row_data.iter().enumerate() {
                if row_number == i as u8 {
                    let start = frames[frame_number].decoded_image_data_offset() as u64 + frames[frame_number].image_data.row_offsets[i] as u64;
                    println!();
                    info!(
                        "- Row {: >2} (0x{:0>2X}), Relative offset: 0x{:X}, Absolute offset: 0x{:X}",
                        i, i, frames[frame_number].image_data.row_offsets[i], start,
                    );

                    let mut bytes = "".to_string();
                    let mut buf = vec![0u8; row.len()];
                    file.seek(SeekFrom::Start(start))?;
                    file.read_exact(&mut buf)?;
                    for b in &buf {
                        bytes.push_str(&format!("{:02X} ", b));
                    }
                    info!("  Data ({} bytes): {}", row.len(), &bytes);
                    break;
                }
            }
        }

        return Ok(());
    }
    println!();
    info!("GRP Header:");
    info!("- Frame count: {}", header.frame_count);
    info!("- Max width:   {}", header.max_width);
    info!("- Max height:  {}", header.max_height);

    let mut actual_max_width  = 0;
    let mut actual_max_height = 0;

    for frame in &frames {
        let width = if frame.image_data.grp_type != GrpType::UncompressedExtended {
            frame.width as u16
        } else {
            frame.width as u16 + EXTENDED_IMAGE_WIDTH
        };
        let right  = frame.x_offset as u16 + width;
        let bottom = frame.y_offset as u16 + frame.height as u16;
        actual_max_width  = actual_max_width .max(right);
        actual_max_height = actual_max_height.max(bottom);
    }

    if actual_max_width > header.max_width || actual_max_height > header.max_height {
        warn!("⚠ Header max dimensions are less than the actual frame extents!");
        warn!("- Actual max width:  {}", actual_max_width);
        warn!("- Actual max height: {}", actual_max_height);
    } else {
        info!("✔ Header dimensions correctly describe frame bounds");
    }
    println!();

    // Analyze for gaps
    let mut used_ranges: Vec<(u64, u64, String)> = Vec::new();
    used_ranges.push((0, 6, format!("GRP Header ({} frames)", frames.len())));
    used_ranges.push((6, 6 + (frames.len() * 8) as u64, "Frame headers".to_string()));

    for (frame_index, frame) in frames.iter().enumerate() {
        let data_offset = frame.decoded_image_data_offset() as u64;
        let row_table_end = data_offset + (frame.image_data.row_offsets.len() * 2) as u64;
        let label = format!("Frame {: >2} row offset table ({} rows)", frame_index, frame.height);
        used_ranges.push((data_offset, row_table_end, label));

        for (i, row) in frame.image_data.raw_row_data.iter().enumerate() {
            let row_offset = if frame.image_data.grp_type == GrpType::Normal {
                frame.image_data.row_offsets[i] as u64
            } else if frame.image_data.grp_type == GrpType::UncompressedExtended {
                (frame.width as u64 + EXTENDED_IMAGE_WIDTH as u64) * i as u64
            } else {
                frame.width as u64 * i as u64
            };

            let start = data_offset + row_offset;
            let end = start + row.len() as u64;
            used_ranges.push((start, end, format!(
                "Frame {: >2}: Image data for row {: >2} ({} bytes)",
                frame_index, i, end - start,
            )));
        }
    }


    let duplicates = frames_with_identical_image_data(&frames);
    let duplicates_found = !duplicates.is_empty();
    for indices in duplicates {
        warn!("⚠ Identical image data found in frames: {:?}", indices);
    }
    if !duplicates_found {
        info!("✔ All frames have unique pixel data");
    }
    used_ranges.sort_by_key(|r| r.0);
    println!();


    // Check for overlapping ranges
    let mut has_printed_header = false;
    let mut overlap_found = false;
    for i in 1..used_ranges.len() {
        let (prev_start, prev_end, prev_label) = &used_ranges[i - 1];
        let (curr_start, curr_end, curr_label) = &used_ranges[i];
        if curr_start < prev_end {
            if !has_printed_header {
                debug!("⚠ Overlapping ranges detected:");
                has_printed_header = true;
            }
            debug!(
                "[0x{:0>2X}]-[0x{:0>2X}] ({}) overlaps with [0x{:0>2X}]-[0x{:0>2X}] ({})",
                prev_start, prev_end, prev_label, curr_start, curr_end, curr_label,
            );
            overlap_found = true;
        }
    }
    if !overlap_found {
        info!("✔ No overlapping ranges detected");
    }
    println!();


    has_printed_header = false;
    let mut pos = 0;
    let mut any_gaps = false;
    for (start, end, _) in &used_ranges {
        if pos < *start {
            any_gaps = true;
            if !has_printed_header {
                warn!("⚠ Unused data found between GRP sections:");
                has_printed_header = true;
            }
            warn!(
                "- Gap from [0x{:0>6X}] to [0x{:0>6X}] ({} bytes)",
                pos, start, start - pos,
            );

            let mut bytes = "".to_string();
            let mut buf = vec![0u8; (start - pos) as usize];
            file.seek(SeekFrom::Start(pos))?;
            file.read_exact(&mut buf)?;
            for b in &buf {
                bytes.push_str(&format!("{:02X} ", b));
            }
            warn!("  Data: {}", &bytes);
        }
        pos = *end;
    }
    if pos < file_len {
        any_gaps = true;
        if !has_printed_header {
            warn!("⚠ Unused data found between GRP sections:");
        }
        warn!(
            "- Trailing data from 0x{:0>6X} to end ({} bytes)",
            pos, file_len - pos,
        );
    }
    if !any_gaps {
        info!("✔ No unused data found between GRP sections");
    }
    println!();


    if log::log_enabled!(log::Level::Debug) {
        debug!("File layout diagram:");
        let mut pos = 0;
        for (start, end, label) in used_ranges {
            if pos < start {
                let mut bytes = "".to_string();
                if start - pos < 32 { // Don't print excessive amounts of data
                    bytes.push_str(": ");
                    let mut buf = vec![0u8; (start - pos) as usize];
                    file.seek(SeekFrom::Start(pos))?;
                    file.read_exact(&mut buf)?;
                    for b in &buf {
                        bytes.push_str(&format!("{:02X} ", b));
                    }
                }
                debug!(
                    "[0x{:0>6X}]-[0x{:0>6X}] UNUSED ({} bytes){}",
                    pos, start, start - pos, &bytes,
                );
            }
            debug!("[0x{:0>6X}]-[0x{:0>6X}] {}", start, end - 1, label);
            pos = end;
        }
        if pos < file_len {
            debug!(
                "[0x{:0>6X}]-[0x{:0>6X}] UNUSED ({} bytes)",
                pos, file_len, file_len - pos,
            );
        }
    }

    Ok(())
}

/// Returns groups of frames that have identical dimensions and pixels. Each group is sorted
/// by frame index, and the groups are sorted by their first frame index.
fn frames_with_identical_image_data(frames: &[GrpFrame]) -> Vec<Vec<usize>> {
    let mut map: HashMap<(u16, u8, &[u8]), Vec<usize>> = HashMap::new();
    for (i, frame) in frames.iter().enumerate() {
        let key = (frame.decoded_width(), frame.height, frame.image_data.converted_pixels.as_slice());
        map.entry(key).or_default().push(i);
    }
    let mut groups: Vec<Vec<usize>> = map.into_values().filter(|indices| indices.len() > 1).collect();
    groups.sort();
    groups
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::grp::ImageData;
    use crate::{CompressionType, LogLevel, OperationMode};

    fn make_test_args(input_path: &str, frame_number: Option<u16>) -> Args {
        Args {
            input_path:         Some(input_path.to_string()),
            pal_path:           None,
            output_path:        None,
            mode:               Some(OperationMode::AnalyseGrp),
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

    fn make_test_frame(pixel_value: u8, width: u8, height: u8, grp_type: GrpType) -> GrpFrame {
        let actual_width = if grp_type == GrpType::UncompressedExtended {
            width as usize + EXTENDED_IMAGE_WIDTH as usize
        } else {
            width as usize
        };
        GrpFrame {
            x_offset: 0,
            y_offset: 0,
            width,
            height,
            image_data_offset: 0,
            image_data: ImageData {
                row_offsets:      vec![],
                raw_row_data:     vec![],
                converted_pixels: vec![pixel_value; actual_width * height as usize],
                grp_type,
            },
        }
    }

    #[test]
    fn identical_image_data_requires_same_pixels_and_dimensions() {
        let frames = vec![
            make_test_frame(5, 2, 3, GrpType::Normal),
            make_test_frame(5, 3, 2, GrpType::Normal), // Same pixels, different dimensions
            make_test_frame(6, 2, 3, GrpType::Normal),
            make_test_frame(5, 2, 3, GrpType::Normal), // Identical to 0
        ];
        assert_eq!(frames_with_identical_image_data(&frames), vec![vec![0, 3]]);
    }

    #[test]
    fn identical_image_data_distinguishes_extended_width() {
        // Width 4 normally and 4 + 256 in an extended frame. Height 0 gives identical (empty) pixels.
        let frames = vec![
            make_test_frame(5, 4, 0, GrpType::Uncompressed),
            make_test_frame(5, 4, 0, GrpType::UncompressedExtended),
        ];
        assert!(frames_with_identical_image_data(&frames).is_empty());
    }

    #[test]
    fn identical_image_data_returns_groups_in_frame_order() {
        let frames: Vec<GrpFrame> = (0..40)
            .map(|i| make_test_frame((i % 20) as u8, 2, 2, GrpType::Normal))
            .collect();
        let expected: Vec<Vec<usize>> = (0..20).map(|i| vec![i, i + 20]).collect();
        assert_eq!(frames_with_identical_image_data(&frames), expected);
    }

    /// Writes an Extended Uncompressed GRP with two 257x2 frames, and returns its path
    fn write_extended_uncompressed_grp(dir: &std::path::Path) -> String {
        let (width, height) = (257usize, 2usize);
        let frame_len = (width * height) as u32;
        let first_offset = 6 + 2 * 8;

        // Max width 512 rather than 257, since the low byte of 257 is non-zero,
        // which would make the reader attempt to parse the GRP as WarCraft I style.
        let mut data = vec![0x02, 0x00, 0x00, 0x02, 0x02, 0x00];
        for i in 0..2 {
            let offset = (first_offset + i * frame_len) | 0x8000_0000; // Extended bit
            data.extend([0, 0, (width - 256) as u8, height as u8]);
            data.extend(offset.to_le_bytes());
        }
        data.extend(vec![0x11; width * height]);
        data.extend(vec![0x22; width * height]);

        let path = dir.join("extended.grp");
        std::fs::write(&path, data).unwrap();
        path.to_str().unwrap().to_string()
    }

    #[test]
    fn analyses_extended_uncompressed_grp() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = write_extended_uncompressed_grp(temp_dir.path());

        let (_, grp_type, frames) = read_grp_file(&path).unwrap();
        assert_eq!(grp_type, GrpType::Uncompressed);
        assert_eq!(frames[0].image_data.grp_type, GrpType::UncompressedExtended);

        analyse_grp(&make_test_args(&path, None)).expect("expected the whole GRP to be analysed");
        analyse_grp(&make_test_args(&path, Some(0))).expect("expected frame 0 to be analysed");
        analyse_grp(&make_test_args(&path, Some(1))).expect("expected frame 1 to be analysed");
    }

    #[test]
    fn rejects_frame_number_equal_to_frame_count() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("one_frame.grp");
        let mut data = vec![0x01, 0x00, 0x01, 0x00, 0x01, 0x00]; // 1 frame, 1x1 size
        data.extend([0, 0, 1, 1, 14, 0, 0, 0]); // frame header (offset 14)
        data.push(0x71); // 1 pixel image data
        std::fs::write(&path, data).unwrap();
        let path = path.to_str().unwrap();

        assert!(analyse_grp(&make_test_args(path, Some(0))).is_ok());
        let err = analyse_grp(&make_test_args(path, Some(1)))
            .expect_err("expected frame 1 to be out of range");
        assert!(matches!(err, Error::InvalidArgument(_)));
    }
}
