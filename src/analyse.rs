use crate::error::{Error, InFile, Result};
use crate::grp::{get_header_size, read_grp_file, GrpFrame, GrpType, EXTENDED_IMAGE_WIDTH};
use crate::{validate_frame_number, Args};
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

    validate_frame_number(args.frame_number, frames.len())?;
    if let Some(frame_number) = args.frame_number {
        let frame_number = frame_number as usize;
        if args.analyse_row_number.is_some() && is_uncompressed {
            return Err(Error::InvalidArgument(
                "--analyse-row-number is only supported for GRPs of type Normal".to_string(),
            ));
        }
        if let Some(row_number) = args.analyse_row_number {
            if row_number >= frames[frame_number].height {
                return Err(Error::InvalidArgument(format!(
                    "Row number {} is out of range; frame {} has {} row(s)",
                    row_number, frame_number, frames[frame_number].height,
                )));
            }
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
                    absolute_row_offset(&frames[frame_number], i),
                );
            }
        }
        if let Some(row_number) = args.analyse_row_number {
            let i = row_number as usize;
            let row = &frames[frame_number].image_data.raw_row_data[i];
            let start = absolute_row_offset(&frames[frame_number], i);
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
    let used_ranges = used_ranges(&frames, grp_type);

    let duplicates = frames_with_identical_image_data(&frames);
    let duplicates_found = !duplicates.is_empty();
    for indices in duplicates {
        warn!("⚠ Identical image data found in frames: {:?}", indices);
    }
    if !duplicates_found {
        info!("✔ All frames have unique pixel data");
    }
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

/// The offset in the file where row `row` of the given Normal frame starts.
fn absolute_row_offset(frame: &GrpFrame, row: usize) -> u64 {
    frame.decoded_image_data_offset() as u64 + frame.image_data.row_offsets[row] as u64
}

/// Returns the byte ranges of the file that are used by the header, the frame headers and the
/// image data of each frame, as (start, end, label), sorted by start. `end` is exclusive.
fn used_ranges(frames: &[GrpFrame], grp_type: GrpType) -> Vec<(u64, u64, String)> {
    let header_size = get_header_size(grp_type == GrpType::War1) as u64;
    let frame_headers_end = header_size + frames.len() as u64 * 8;

    let mut used_ranges: Vec<(u64, u64, String)> = Vec::new();
    used_ranges.push((0, header_size, format!("GRP Header ({} frames)", frames.len())));
    used_ranges.push((header_size, frame_headers_end, "Frame headers".to_string()));

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
    used_ranges.sort_by_key(|r| r.0);
    used_ranges
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

    /// Writes a WarCraft I style GRP with two 2x2 frames, and returns its path
    fn write_war1_grp(dir: &std::path::Path) -> String {
        let mut data = vec![0x02, 0x00, 0x02, 0x02]; // 2 frames, max width and height 2 as u8
        data.extend([0, 0, 2, 2, 20, 0, 0, 0]); // Frame 0: 2x2 at offset 4 + 2 * 8 = 20
        data.extend([0, 0, 2, 2, 24, 0, 0, 0]); // Frame 1: 2x2 at offset 24
        data.extend([1, 2, 3, 4, 5, 6, 7, 8]);  // Image data
        let path = dir.join("war1.grp");
        std::fs::write(&path, data).unwrap();
        path.to_str().unwrap().to_string()
    }

    /// Asserts that the non-empty ranges cover the file exactly, without gaps or overlaps
    fn assert_ranges_cover_file(ranges: &[(u64, u64, String)], file_len: u64) {
        let mut pos = 0;
        for (start, end, label) in ranges.iter().filter(|(start, end, _)| start != end) {
            assert_eq!(*start, pos, "range '{}' should start where the previous one ended", label);
            pos = *end;
        }
        assert_eq!(pos, file_len, "the ranges should end at the end of the file");
    }

    #[test]
    fn used_ranges_use_four_byte_header_for_war1_grps() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = write_war1_grp(temp_dir.path());
        let (_, grp_type, frames) = read_grp_file(&path).unwrap();
        assert_eq!(grp_type, GrpType::War1);

        let ranges = used_ranges(&frames, grp_type);

        assert_eq!((ranges[0].0, ranges[0].1), (0, 4),  "GRP header");
        assert_eq!((ranges[1].0, ranges[1].1), (4, 20), "frame headers");
        assert_ranges_cover_file(&ranges, std::fs::metadata(&path).unwrap().len());
    }

    #[test]
    fn used_ranges_use_six_byte_header_for_other_grps() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = write_extended_uncompressed_grp(temp_dir.path());
        let (_, grp_type, frames) = read_grp_file(&path).unwrap();

        let ranges = used_ranges(&frames, grp_type);

        assert_eq!((ranges[0].0, ranges[0].1), (0, 6),  "GRP header");
        assert_eq!((ranges[1].0, ranges[1].1), (6, 22), "frame headers");
        assert_ranges_cover_file(&ranges, std::fs::metadata(&path).unwrap().len());
    }

    /// Writes a Normal GRP whose one 2x2 frame has its image data beyond 64 KiB into the file,
    /// and returns its path and the offset of the image data
    fn write_grp_with_image_data_beyond_64_kib(dir: &std::path::Path) -> (String, u32) {
        // Truncated to u16 this is 0xFFFE, so adding a row offset to it would overflow
        let image_data_offset: u32 = 0x1_FFFE;
        let mut data = vec![0x01, 0x00, 0x02, 0x00, 0x02, 0x00]; // 1 frame, max width and height 2
        data.extend([0, 0, 2, 2]);
        data.extend(image_data_offset.to_le_bytes());
        data.resize(image_data_offset as usize, 0); // Unused padding up to the image data
        data.extend([4, 0, 6, 0]);                  // Row offsets: row 0 at +4, row 1 at +6
        data.extend([0x42, 0x07]);                  // Row 0: colour 7 repeated twice
        data.extend([0x02, 0x08, 0x09]);            // Row 1: copy the 2 pixels 8 and 9
        let path = dir.join("far.grp");
        std::fs::write(&path, data).unwrap();
        (path.to_str().unwrap().to_string(), image_data_offset)
    }

    #[test]
    fn absolute_row_offset_does_not_truncate_offsets_beyond_64_kib() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (path, image_data_offset) = write_grp_with_image_data_beyond_64_kib(temp_dir.path());
        let (_, grp_type, frames) = read_grp_file(&path).unwrap();
        assert_eq!(grp_type, GrpType::Normal);
        assert_eq!(frames[0].image_data.converted_pixels, vec![7, 7, 8, 9]);

        assert_eq!(absolute_row_offset(&frames[0], 0), image_data_offset as u64 + 4);
        assert_eq!(absolute_row_offset(&frames[0], 1), image_data_offset as u64 + 6);
    }

    #[test]
    fn analyses_frame_with_image_data_beyond_64_kib() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (path, _) = write_grp_with_image_data_beyond_64_kib(temp_dir.path());
        // The offsets are computed in the arguments to info!, which are only evaluated if the
        // log level is enabled. No logger is installed, so nothing is actually printed.
        log::set_max_level(log::LevelFilter::Info);

        analyse_grp(&make_test_args(&path, None)).expect("expected the whole GRP to be analysed");
        analyse_grp(&make_test_args(&path, Some(0))).expect("expected frame 0 to be analysed");
        for row in [0, 1] {
            let mut args = make_test_args(&path, Some(0));
            args.analyse_row_number = Some(row);
            analyse_grp(&args).unwrap_or_else(|e| panic!("expected row {} to be analysed: {}", row, e));
        }
    }

    /// Writes a Normal GRP with one frame of 1x255 pixels, the maximum height, and returns its path
    fn write_grp_with_max_height_frame(dir: &std::path::Path) -> String {
        let height: u16 = 255;
        let mut data = vec![0x01, 0x00, 0x01, 0x00, height as u8, 0x00]; // 1 frame, 1x255
        data.extend([0, 0, 1, height as u8, 14, 0, 0, 0]);               // Image data at offset 14
        for row in 0..height {
            data.extend((height * 2 + row * 2).to_le_bytes()); // Row offsets
        }
        for row in 0..height {
            data.extend([0x01, (row % 255) as u8 + 1]); // Each row: copy 1 pixel
        }
        let path = dir.join("tall.grp");
        std::fs::write(&path, data).unwrap();
        path.to_str().unwrap().to_string()
    }

    #[test]
    fn analyses_frame_with_max_height() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = write_grp_with_max_height_frame(temp_dir.path());
        log::set_max_level(log::LevelFilter::Info); // Evaluate the arguments to info!

        analyse_grp(&make_test_args(&path, Some(0))).expect("expected frame 0 to be analysed");
        for row in [0, 254] {
            let mut args = make_test_args(&path, Some(0));
            args.analyse_row_number = Some(row);
            analyse_grp(&args).unwrap_or_else(|e| panic!("expected row {} to be analysed: {}", row, e));
        }
    }

    #[test]
    fn rejects_row_number_equal_to_frame_height() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = write_grp_with_max_height_frame(temp_dir.path());

        let mut args = make_test_args(&path, Some(0));
        args.analyse_row_number = Some(255);
        let err = analyse_grp(&args).expect_err("expected row 255 to be out of range");
        assert!(matches!(err, Error::InvalidArgument(_)));
        assert!(err.to_string().contains("frame 0 has 255 row(s)"), "{}", err);
    }

    #[test]
    fn rejects_row_number_for_uncompressed_grps() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = write_extended_uncompressed_grp(temp_dir.path());

        let mut args = make_test_args(&path, Some(0));
        args.analyse_row_number = Some(0);
        let err = analyse_grp(&args).expect_err("expected --analyse-row-number to be rejected");
        assert!(matches!(err, Error::InvalidArgument(_)));
    }

    #[test]
    fn analyses_war1_grp() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = write_war1_grp(temp_dir.path());

        analyse_grp(&make_test_args(&path, None)).expect("expected the whole GRP to be analysed");
        analyse_grp(&make_test_args(&path, Some(1))).expect("expected frame 1 to be analysed");
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
