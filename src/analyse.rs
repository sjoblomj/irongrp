use crate::grp::{detect_uncompressed, read_grp_frames, read_grp_header, GrpType};
use crate::{log, Args, LogLevel, LOG_LEVEL};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};


/// Analyzes a GRP file and prints information about header correctness, unused space, overlapping
/// ranges, and file layout.
pub fn analyse_grp(args: &Args) -> std::io::Result<()> {
    let mut file = File::open(&args.input_path)?;
    let file_len = file.metadata()?.len();

    let (header, war1_style) = read_grp_header(&mut file)?;
    let is_uncompressed = detect_uncompressed(&args.input_path, &header, war1_style)?;

    let grp_type = if is_uncompressed && war1_style {
        GrpType::War1
    } else if is_uncompressed {
        GrpType::Uncompressed
    } else {
        GrpType::Normal
    };
    let frames = read_grp_frames(&mut file, header.frame_count as usize, grp_type)?;

    println!();
    log(LogLevel::Info, &format!("GRP type: {:?}", grp_type));

    if args.frame_number.is_some() {
        let frame_number = args.frame_number.unwrap() as usize;
        if  frame_number > frames.len() {
            log(LogLevel::Error, &format!(
                "Frame number {} is out of range (0-{})",
                frame_number, frames.len() - 1,
            ));
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
        }
        let row_number = if args.analyse_row_number.is_none() || is_uncompressed {
            frames[frame_number].height + 1
        } else {
            args.analyse_row_number.unwrap()
        };
        if row_number > frames[frame_number].height && args.analyse_row_number.is_some() {
            log(LogLevel::Error, &format!(
                "Row number {} is out of range (0-{})",
                row_number, frames[frame_number].height,
            ));
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid arguments"));
        }

        let width = if frames[frame_number].image_data.grp_type != GrpType::UncompressedExtended {
            frames[frame_number].width as u16
        } else {
            frames[frame_number].width as u16 + 256
        };
        let next_offset = if frame_number + 1 < frames.len() {
            frames[frame_number + 1].image_data_offset
        } else {
            file_len as u32
        };
        log(LogLevel::Info, &format!("Analyzing frame {}:", frame_number));
        log(LogLevel::Info, &format!("- GrpType:  {:?}", frames[frame_number].image_data.grp_type));
        log(LogLevel::Info, &format!("- X offset: {}", frames[frame_number].x_offset));
        log(LogLevel::Info, &format!("- Y offset: {}", frames[frame_number].y_offset));
        log(LogLevel::Info, &format!("- Width:    {}", width));
        log(LogLevel::Info, &format!("- Height:   {}", frames[frame_number].height));
        log(LogLevel::Info, &format!("- This frames image data offset: 0x{:0>2X}", frames[frame_number].image_data_offset));
        log(LogLevel::Info, &format!("- Next frames image data offset: 0x{:0>2X}", next_offset));
        if frames[frame_number].image_data.grp_type == GrpType::Normal {
            for (i, _) in frames[frame_number].image_data.raw_row_data.iter().enumerate() {
                log(LogLevel::Info, &format!(
                    "- Row {: >2} (0x{:0>2X}), Relative offset: 0x{:0>4X}, Absolute offset: 0x{:0>6X}",
                    i, i, frames[frame_number].image_data.row_offsets[i],
                    frames[frame_number].image_data.row_offsets[i] + frames[frame_number].image_data_offset as u16,
                ));
            }
        }
        if args.analyse_row_number.is_some() && frames[frame_number].image_data.grp_type == GrpType::Normal {
            for (i, row) in frames[frame_number].image_data.raw_row_data.iter().enumerate() {
                if row_number == i as u8 {
                    let start = frames[frame_number].image_data_offset as u64 + frames[frame_number].image_data.row_offsets[i] as u64;
                    println!();
                    log(LogLevel::Info, &format!(
                        "- Row {: >2} (0x{:0>2X}), Relative offset: 0x{:X}, Absolute offset: 0x{:X}",
                        i, i, frames[frame_number].image_data.row_offsets[i], start,
                    ));

                    let mut bytes = "".to_string();
                    let mut buf = vec![0u8; row.len()];
                    file.seek(SeekFrom::Start(start))?;
                    file.read_exact(&mut buf)?;
                    for b in &buf {
                        bytes.push_str(&format!("{:02X} ", b));
                    }
                    log(LogLevel::Info, &format!("  Data ({} bytes): {}", row.len(), &bytes));
                    break;
                }
            }
        }

        return Ok(());
    }
    println!();
    log(LogLevel::Info, "GRP Header:");
    log(LogLevel::Info, &format!("- Frame count: {}", header.frame_count));
    log(LogLevel::Info, &format!("- Max width:   {}", header.max_width));
    log(LogLevel::Info, &format!("- Max height:  {}", header.max_height));

    let mut actual_max_width  = 0;
    let mut actual_max_height = 0;

    for frame in &frames {
        let width = if frame.image_data.grp_type != GrpType::UncompressedExtended {
            frame.width as u16
        } else {
            frame.width as u16 + 256
        };
        let right  = frame.x_offset as u16 + width;
        let bottom = frame.y_offset as u16 + frame.height as u16;
        actual_max_width  = actual_max_width .max(right);
        actual_max_height = actual_max_height.max(bottom);
    }

    if actual_max_width > header.max_width || actual_max_height > header.max_height {
        log(LogLevel::Warn, "⚠ Header max dimensions are less than the actual frame extents!");
        log(LogLevel::Warn, &format!("- Actual max width:  {}", actual_max_width));
        log(LogLevel::Warn, &format!("- Actual max height: {}", actual_max_height));
    } else {
        log(LogLevel::Info, "✔ Header dimensions correctly describe frame bounds");
    }
    println!();

    // Analyze for gaps
    let mut used_ranges: Vec<(u64, u64, String)> = Vec::new();
    used_ranges.push((0, 6, format!("GRP Header ({} frames)", frames.len())));
    used_ranges.push((6, 6 + (frames.len() * 8) as u64, "Frame headers".to_string()));

    for (frame_index, frame) in frames.iter().enumerate() {
        let data_offset = frame.image_data_offset as u64;
        let row_table_end = data_offset + (frame.image_data.row_offsets.len() * 2) as u64;
        let label = format!("Frame {: >2} row offset table ({} rows)", frame_index, frame.height);
        used_ranges.push((data_offset, row_table_end, label));

        for (i, row) in frame.image_data.raw_row_data.iter().enumerate() {
            let row_offset = if frame.image_data.grp_type == GrpType::Normal {
                frame.image_data.row_offsets[i] as u64
            } else if frame.image_data.grp_type == GrpType::UncompressedExtended {
                (frame.width as u64 + 256) * i as u64
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


    let mut hash_map: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, frame) in frames.iter().enumerate() {
        let mut hasher = DefaultHasher::new();
        frame.image_data.converted_pixels.hash(&mut hasher);
        let hash = hasher.finish();
        hash_map.entry(hash).or_default().push(i);
    }

    let mut duplicates_found = false;
    for (_, indices) in hash_map {
        if indices.len() > 1 {
            duplicates_found = true;
            log(LogLevel::Warn, &format!("⚠ Identical image data found in frames: {:?}", indices));
        }
    }
    if !duplicates_found {
        log(LogLevel::Info, "✔ All frames have unique pixel data");
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
                log(LogLevel::Debug, "⚠ Overlapping ranges detected:");
                has_printed_header = true;
            }
            log(LogLevel::Debug, &format!(
                "[0x{:0>2X}]-[0x{:0>2X}] ({}) overlaps with [0x{:0>2X}]-[0x{:0>2X}] ({})",
                prev_start, prev_end, prev_label, curr_start, curr_end, curr_label,
            ));
            overlap_found = true;
        }
    }
    if !overlap_found {
        log(LogLevel::Info, "✔ No overlapping ranges detected");
    }
    println!();


    has_printed_header = false;
    let mut pos = 0;
    let mut any_gaps = false;
    for (start, end, _) in &used_ranges {
        if pos < *start {
            any_gaps = true;
            if !has_printed_header {
                log(LogLevel::Warn, "⚠ Unused data found between GRP sections:");
                has_printed_header = true;
            }
            log(LogLevel::Warn, &format!(
                "- Gap from [0x{:0>6X}] to [0x{:0>6X}] ({} bytes)",
                pos, start, start - pos,
            ));

            let mut bytes = "".to_string();
            let mut buf = vec![0u8; (start - pos) as usize];
            file.seek(SeekFrom::Start(pos))?;
            file.read_exact(&mut buf)?;
            for b in &buf {
                bytes.push_str(&format!("{:02X} ", b));
            }
            log(LogLevel::Warn, &format!("  Data: {}", &bytes));
        }
        pos = *end;
    }
    if pos < file_len {
        any_gaps = true;
        if !has_printed_header {
            log(LogLevel::Warn, "⚠ Unused data found between GRP sections:");
        }
        log(LogLevel::Warn, &format!(
            "- Trailing data from 0x{:0>6X} to end ({} bytes)",
            pos, file_len - pos,
        ));
    }
    if !any_gaps {
        log(LogLevel::Info, "✔ No unused data found between GRP sections");
    }
    println!();


    if matches!(LOG_LEVEL.get(), Some(LogLevel::Debug)) {
        log(LogLevel::Debug, "File layout diagram:");
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
                log(LogLevel::Debug, &format!(
                    "[0x{:0>6X}]-[0x{:0>6X}] UNUSED ({} bytes){}",
                    pos, start, start - pos, &bytes,
                ));
            }
            log(LogLevel::Debug, &format!("[0x{:0>6X}]-[0x{:0>6X}] {}", start, end - 1, label));
            pos = end;
        }
        if pos < file_len {
            log(LogLevel::Debug, &format!(
                "[0x{:0>6X}]-[0x{:0>6X}] UNUSED ({} bytes)",
                pos, file_len, file_len - pos,
            ));
        }
    }

    Ok(())
}
