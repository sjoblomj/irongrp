use crate::palpng::{read_rgb_palette, ImageWithMetadata};
use crate::png::{png_to_pixels, render_and_save_frames_to_png};
use crate::{list_png_files, log, Args, CompressionType, LogLevel, LOG_LEVEL, UNCOMPRESSED_FILENAME, WAR1_FILENAME};
use clap::ValueEnum;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{Error, ErrorKind, Read, Result, Seek, SeekFrom, Write};

#[derive(Debug)]
pub struct GrpHeader {
    pub frame_count: u16,
    pub max_width:   u16,
    pub max_height:  u16,
}

#[derive(Clone, Debug)]
pub struct GrpFrame {
    pub x_offset: u8,
    pub y_offset: u8,
    pub width:    u8,
    pub height:   u8,
    pub image_data_offset: u32,
    pub image_data: ImageData,
}

#[derive(Clone, Debug)]
pub struct ImageData {
    /// offsets to the rows of raw data, relative to the image_data_offset
    pub row_offsets:  Vec<u16>,
    /// List of rows of raw image data
    pub raw_row_data: Vec<Vec<u8>>,
    /// The raw image data, converted to pixels
    pub converted_pixels: Vec<u8>,
    /// Type of GRP being represented
    pub grp_type: GrpType,
}

#[derive(Clone, ValueEnum, PartialEq, Debug, Copy)]
pub enum GrpType {
    Normal,
    Uncompressed,
    UncompressedExtended,
    War1,
}

#[derive(Hash, Eq, PartialEq)]
struct FrameDedupKey {
    image_data: Vec<u8>,
    x_offset: u8,
    y_offset: u8,
    width:    u16,
    height:   u16,
}

impl GrpFrame {
    /// The length of the frame in bytes, as it would be written to a GRP file
    fn grp_frame_len(&self) -> usize {
        let row_offsets_size     = self.image_data.row_offsets.len() * 2; // u16 = 2 bytes
        let raw_data_size: usize = self.image_data.raw_row_data.iter().map(|row| row.len()).sum();
        row_offsets_size + raw_data_size
    }
}

/// Parses the header of a GRP file. Returns the header and whether
/// it was in WarCraft I style or not.
pub fn read_grp_header<R: Read + Seek>(file: &mut R) -> Result<(GrpHeader, bool)> {
    let mut buf = [0u8; 8];
    file.read_exact(&mut buf)?;

    let frame_count     = u16::from_le_bytes([buf[0], buf[1]]);
    let war1_max_width  = u8 ::from_le_bytes([buf[2]]);
    let war1_max_height = u8 ::from_le_bytes([buf[3]]);
    let max_width       = u16::from_le_bytes([buf[2], buf[3]]);
    let max_height      = u16::from_le_bytes([buf[4], buf[5]]);

    let war1_style = determine_grp_style(
        file,
        frame_count,
        war1_max_width,
        war1_max_height,
    )?;

    let header = if !war1_style {
        GrpHeader {
            frame_count,
            max_width,
            max_height,
        }
    } else {
        GrpHeader {
            frame_count,
            max_width:  war1_max_width  as u16,
            max_height: war1_max_height as u16,
        }
    };

    log(LogLevel::Debug, &format!(
        "Read GRP Header. War1 style: {}, Frame count: {}, max width: {}, max_height: {}",
        war1_style, header.frame_count, header.max_width, header.max_height,
    ));
    Ok((header, war1_style))
}

/// Returns true if the GRP is in War1 style, false otherwise.
/// If it appears to not be a GRP, it throws an error.
fn determine_grp_style<R: Read + Seek>(
    file: &mut R,
    frame_count: u16,
    war1_max_width:  u8,
    war1_max_height: u8,
) -> Result<bool> {

    if war1_max_width != 0 && war1_max_height != 0 {
        // This is true for War1 GRPs and Extended GRPs
        let is_war1_style = try_reading_frame_headers(
            file,
            frame_count,
            get_header_size(true),
        ).is_ok();
        if is_war1_style {
            return Ok(is_war1_style)
        }
    }
    let result = try_reading_frame_headers(file, frame_count, get_header_size(false));
    if result.is_err() {
        return Err(result.err().unwrap());
    }
    Ok(false)
}

/// Reads all frame headers and checks that the offsets are within file boundaries.
/// Returns Error if not.
fn try_reading_frame_headers<R: Read + Seek>(
    file: &mut R,
    frame_count: u16,
    start_pos: usize,
) -> Result<()> {

    let file_len = file.seek(SeekFrom::End(0))?;
    for i in 0..frame_count {
        file.seek(SeekFrom::Start(start_pos as u64 + (i * 8) as u64))?;
        let mut buf = [0u8; 8];
        file.read_exact(&mut buf)?;

        // buf[0] and buf[1] contain x_offset and y_offset, respectively
        let w = u8::from_le_bytes([buf[2]]);
        let height = u8::from_le_bytes([buf[3]]);
        let image_data_offset = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);

        let (width, offset) = adjust_width_and_offset_if_extended_when_decoding(w, image_data_offset);

        if width == 0 || height == 0 {
            return Err(Error::new(ErrorKind::Other, "Frame width or height is zero"));
        }
        if offset > file_len as u32 {
            return Err(Error::new(ErrorKind::Other, "Image data offset is too large"));
        }
    }
    Ok(())
}

fn offset_is_extended(offset: u32) -> bool {
    (offset & EXTENDED_OFFSET_BIT) != 0
}

fn image_should_be_extended(width: u16) -> bool {
    width >= EXTENDED_IMAGE_WIDTH
}

fn adjust_width_and_offset_if_extended_when_decoding(width: u8, image_data_offset: u32) -> (u16, u32) {
    if offset_is_extended(image_data_offset) {
        // If the high bit is set, that means that the frame of the
        // Uncompressed GRP has a width greater than 256 pixels.

        let offset = image_data_offset & EXTENDED_OFFSET_BIT - 1; // clear the highest bit
        return (width as u16 + EXTENDED_IMAGE_WIDTH, offset)
    };
    (width as u16, image_data_offset)
}

fn adjust_width_and_offset_if_extended_when_encoding(width: u16, offset: u32) -> (u16, u32) {
    if image_should_be_extended(width) {
        return (width - EXTENDED_IMAGE_WIDTH, offset | EXTENDED_OFFSET_BIT)
    }
    (width, offset)
}


/// Parses all GRP frames
pub fn read_grp_frames<R: Read + Seek>(
    file: &mut R,
    frame_count: u16,
    grp_type: GrpType,
) -> Result<Vec<GrpFrame>> {

    let pos = get_header_size(grp_type ==  GrpType::War1) as u64;
    let mut frames = Vec::new();
    for i in 0..frame_count {
        log(LogLevel::Debug, &format!("Reading GRP Frame {} / {}", i, frame_count));
        file.seek(SeekFrom::Start(pos + (i * 8) as u64))?;
        let mut buf = [0u8; 8];
        file.read_exact(&mut buf)?;

        let image_data_offset = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let width  = buf[2];
        let height = buf[3];

        let image_data = if grp_type != GrpType::Normal {

            let (w, offset) = adjust_width_and_offset_if_extended_when_decoding(width, image_data_offset);
            let has_extended_size = offset_is_extended(image_data_offset);
            if  has_extended_size {
                log(LogLevel::Debug, &format!(
                    "Reading Uncompressed frame {} with extended size. Width in file: {}, \
                    actual width: {}. Offset in file: 0x{:0>2X}, actual offset: 0x{:0>2X}",
                    i, width, w, image_data_offset, offset,
                ));
            }

            let compression_type = if has_extended_size {
                // There does not seem to be any War1 GRPs with extended size.
                // The code here needs to be changed if there are.
                GrpType::UncompressedExtended
            } else {
                grp_type // Uncompressed or War1
            };
            read_uncompressed_image_data(
                file,
                w,
                height,
                offset,
                compression_type,
            )?
        } else {
            read_image_data(
                file,
                width  as u16,
                height as u16,
                image_data_offset,
            )?
        };

        let grp_frame = GrpFrame {
            x_offset: buf[0],
            y_offset: buf[1],
            width,
            height,
            image_data_offset,
            image_data,
        };
        frames.push(grp_frame.clone());
        log(LogLevel::Debug, &format!(
            "Read GRP Frame {}. x-offset: 0x{:0>2X} ({}), y-offset: 0x{:0>2X} ({}), \
            width: 0x{:0>2X} ({}), height: 0x{:0>2X} ({}), image-data-offset: 0x{:0>4X} ({}), \
            number of pixels: {}",
            i, grp_frame.x_offset, grp_frame.x_offset, grp_frame.y_offset, grp_frame.y_offset,
            grp_frame.width, grp_frame.width, grp_frame.height, grp_frame.height,
            grp_frame.image_data_offset, grp_frame.image_data_offset,
            grp_frame.image_data.converted_pixels.len(),
        ));
        log(LogLevel::Debug, ""); // Give some space in the logs
    }
    Ok(frames)
}

/// Reads row offsets and decodes image data
fn read_uncompressed_image_data<R: Read + Seek>(
    file:   &mut R,
    width:  u16,
    height: u8,
    image_data_offset: u32,
    grp_type: GrpType,
) -> Result<ImageData> {

    let file_len = file.seek(SeekFrom::End(0))?;
    let data_len = file_len
        .checked_sub(image_data_offset as u64)
        .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, "image_data_offset beyond file length"))?;
    if data_len < width as u64 * height as u64 {
        return Err(Error::new(
            ErrorKind::UnexpectedEof,
            format!("Wanted to read {} bytes, but only {} are available in file",
                    width * height as u16, data_len,
            ),
        ));
    }

    file.seek(SeekFrom::Start(image_data_offset as u64))?;
    let mut pixels = vec![0; width as usize * height as usize];
    file.read_exact(&mut pixels)?;

    let raw_row_data = read_uncompressed_pixels(width, height as u16, pixels.clone());

    Ok(ImageData {
        row_offsets: vec![],
        raw_row_data,
        converted_pixels: pixels,
        grp_type,
    })
}

fn read_uncompressed_pixels(width: u16, height: u16, pixels: Vec<u8>) -> Vec<Vec<u8>> {
    let mut raw_row_data = Vec::with_capacity(height as usize);
    for row in 0..height {
        let start = row as usize * width as usize;
        let row_data = pixels[start..start + width as usize].to_vec();
        raw_row_data.push(row_data.clone());
    }
    raw_row_data
}

/// Reads row offsets and decodes image data
fn read_image_data<R: Read + Seek>(
    file:   &mut R,
    width:  u16,
    height: u16,
    image_data_offset: u32,
) -> Result<ImageData> {

    let file_len = file.seek(SeekFrom::End(0))?;
    let data_len = file_len
        .checked_sub(image_data_offset as u64)
        .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, "image_data_offset beyond file length"))?;

    // Seek to the beginning of the row offset table and read the remainder of the file
    file.seek(SeekFrom::Start(image_data_offset as u64))?;
    let mut data_block = vec![0; data_len as usize];
    file.read_exact(&mut data_block)?;

    // Parse row offsets from the beginning of data_block
    let mut row_offsets = Vec::with_capacity(height as usize);
    for i in 0..height {
        let offset_start = (i * 2) as usize;
        if  offset_start + 2 > data_block.len() {
            return Err(Error::new(
                ErrorKind::UnexpectedEof,
                "Not enough data for row offset table",
            ));
        }
        let row_offset = u16::from_le_bytes([data_block[offset_start], data_block[offset_start + 1]]);
        row_offsets.push(row_offset);
    }

    let mut raw_row_data = Vec::with_capacity(height as usize);
    let mut pixels = vec![0; (width * height) as usize];

    for (row, &row_offset) in row_offsets.iter().enumerate() {
        if row_offset as usize >= data_block.len() {
            return Err(Error::new(
                ErrorKind::UnexpectedEof,
                format!("Row data offset {} is beyond end of data_block ({})", row_offset, data_block.len()),
            ));
        }
        let row_data = &data_block[row_offset as usize ..];
        log(LogLevel::Debug, &format!(
            "Decoding row {} of width {} from offset {} (length {})",
            row, width, row_offset, row_data.len(),
        ));

        let (decoded_row, encoded_length) = decode_grp_rle_row(row_data, width);

        if row_offset as usize + encoded_length > data_block.len() {
            return Err(Error::new(
                ErrorKind::UnexpectedEof, format!(
                    "Row {} encoded length goes beyond buffer: {} + {} > {}",
                    row, row_offset, encoded_length, data_block.len(),
                ),
            ));
        }

        raw_row_data.push(row_data[..encoded_length].to_vec());

        let start = row * width as usize;
        pixels[start .. start + decoded_row.len()].copy_from_slice(&decoded_row);
    }

    Ok(ImageData {
        row_offsets,
        raw_row_data,
        converted_pixels: pixels,
        grp_type: GrpType::Normal,
    })
}

/// Decodes an RLE-compressed row of pixels
fn decode_grp_rle_row(line_data: &[u8], image_width: u16) -> (Vec<u8>, usize) {
    let mut line_pixels = vec![0; image_width as usize]; // Initialize with transparent pixels (palette index 0)
    let mut x = 0; // Position in output row
    let mut data_offset = 0; // Position in input data

    while x < image_width as usize && data_offset < line_data.len() {
        let control_byte = line_data[data_offset];
        data_offset += 1;

        if control_byte & 0x80 != 0 { // Transparent - skip x pixels
            let skip = (control_byte & 0x7F) as usize;
            x += skip;
            log(LogLevel::Debug, &format!(
                "Decoding transparent byte (0x{:0>2X}). Skipping 0x{:0>2X} ({}) pixels.",
                control_byte, skip, skip,
            ));

        } else if control_byte & 0x40 != 0 { // Run-length encoding (repeat same colour X times)
            let run_length  = (control_byte & 0x3F) as usize;
            if data_offset >= line_data.len() { // Safety check
                log(LogLevel::Error, &format!(
                    "Decoding error: Requested offset ({}) is greater than line length ({}).",
                    data_offset, line_data.len(),
                ));
                break;
            }
            let colour_index = line_data[data_offset]; // Colour index from palette
            data_offset += 1;
            log(LogLevel::Debug, &format!(
                "Decoding control byte 0x{:0>2X} 0x{:0>2X}. data_offset: 0x{:0>2X} ({}). \
                Pixel with palette index {} will be repeated {} times.",
                control_byte, colour_index, data_offset, data_offset, colour_index, run_length,
            ));

            for _ in 0..run_length {
                if x >= image_width as usize {
                    log(LogLevel::Error, &format!(
                        "Decoding error: X position ({}) is greater than image width ({}).",
                        x, image_width,
                    ));
                    break;
                }
                line_pixels[x] = colour_index;
                x += 1;
            }

        } else { // Normal - copy x pixels directly
            let copy_length = control_byte as usize;

            log(LogLevel::Debug, &format!(
                "Normal decoding (0x{:0>2X}). Will copy {} pixels.",
                control_byte, copy_length,
            ));
            let mut bytes_for_logging = "".to_string();

            for _ in 0..copy_length {
                if x >= image_width as usize || data_offset >= line_data.len() {
                    log(LogLevel::Error, &format!(
                        "Decoding error: X position ({}) is greater than image width ({}), \
                        or data offset ({}) is greater than line length ({}).",
                        x, image_width, data_offset, line_data.len(),
                    ));
                    break;
                }
                line_pixels[x] = line_data[data_offset];
                bytes_for_logging.push_str(&format!("{:02X} ", line_data[data_offset]));
                x += 1;
                data_offset += 1;
            }
            if copy_length == 0 {
                data_offset += 1;
                log(LogLevel::Error, "Read instruction to copy 0 pixels - Stepping over");
            } else {
                log(LogLevel::Debug, &format!(
                    "Normal decoding of {} bytes: {}",
                    copy_length, bytes_for_logging,
                ));
            }
        }
    }

    (line_pixels, data_offset)
}


/// Encodes an RLE-compressed row of pixels
fn encode_grp_rle_row(row_pixels: &[u8], compression_type: &CompressionType) -> Vec<u8> {
    let mut encoded = Vec::new();
    let mut i = 0;

    if matches!(LOG_LEVEL.get(), Some(LogLevel::Debug)) {
        log(LogLevel::Debug, &format!("Beginning to encode using compression type '{}'", compression_type));
        for x in 0..row_pixels.len() {
            log(LogLevel::Debug, &format!(
                "x: {:2}, row_pixels[i]: {:2X} ({:3})",
                x, row_pixels[x], row_pixels[x],
            ));
        }
    }

    let same_colour_threshold = if let CompressionType::Normal = compression_type {
        3
    } else {
        2
    };

    let mut safety_break = 0;
    while i < row_pixels.len() {
        safety_break += 1;
        if safety_break > 4096 {
            log(LogLevel::Error, "Seems like we're stuck in an infinite encoding loop, after 4096 iterations. Breaking.");
            break;
        }
        let current_colour = row_pixels[i];

        log(LogLevel::Debug, &format!(
            "Encoding pixel at position {} / {} with palette index {}",
            i, row_pixels.len(), current_colour,
        ));
        // Case 1: Transparent run (index 0)
        if current_colour == 0 {
            let mut run_len = 1;
            while i + run_len < row_pixels.len() && row_pixels[i + run_len] == 0 && run_len < 127 {
                run_len += 1;
            }
            log(LogLevel::Debug, &format!(
                "Encoding transparent run of 0x{:0>2X} ({}) => 0x{:0>2X} ({})",
                run_len, run_len, 0x80 | run_len as u8, 0x80 | run_len as u8,
            ));
            encoded.push(0x80 | run_len as u8);
            i += run_len;

        } else { // Case 2: Run of the same colour (but not transparent)
            let mut run_len = 1;
            while i + run_len < row_pixels.len()
                && row_pixels[i + run_len] == current_colour
                && run_len < 63
            {
                run_len += 1;
            }
            log(LogLevel::Debug, &format!(
                "Encoding: Pixels of the same colour: 0x{:0>2X} ({})",
                run_len, run_len,
            ));

            if run_len > same_colour_threshold {
                log(LogLevel::Debug, &format!(
                    "Encoding same colour 0x{:0>2X} ({}) => 0x{:0>2X} 0x{:0>2X}",
                    run_len, run_len, 0x40 | run_len as u8, current_colour,
                ));
                encoded.push(0x40 | run_len as u8);
                encoded.push(current_colour);
                i += run_len;

            } else { // Case 3: Literal copy
                let start = i;
                let mut run_len = 0;
                let mut last_colour = 0;
                let mut last_colour_len = 0;

                // Go through the row until we find a run of same coloured pixels above the threshold
                for x in i..row_pixels.len() {
                    log(LogLevel::Debug, &format!(
                        "Encoding literal copy. x: {:2}, row_pixels[i]: {:2X} ({:3})",
                        x, row_pixels[x], row_pixels[x],
                    ));
                    if row_pixels[x] == 0 {
                        break;
                    }
                    if row_pixels[x] != last_colour || last_colour_len == 0 {
                        // New pixel or first pixel
                        last_colour = row_pixels[x];
                        last_colour_len = 1;
                    } else {
                        // Repetition of last seen pixel
                        last_colour_len += 1;
                    }

                    if compression_type == &CompressionType::Normal {
                        if run_len >= 63 {
                            break;
                        }
                        if last_colour_len > same_colour_threshold {
                            run_len -= same_colour_threshold;
                            break;
                        }
                    } else {
                        // This order can save a few bytes in some cases, but it's not how Blizzard
                        // did things
                        if last_colour_len > same_colour_threshold {
                            run_len -= same_colour_threshold;
                            break;
                        }
                        if run_len >= 63 {
                            break;
                        }
                     }
                    run_len += 1;
                }

                log(LogLevel::Debug, &format!(
                    "Encoding literal copy of 0x{:0>2X} ({}) => 0x{:0>2X} ({})",
                    run_len, run_len, run_len, run_len,
                ));
                encoded.push(run_len as u8);
                encoded.extend_from_slice(&row_pixels[start..start + run_len]);
                i += run_len;
            }
        }
    }

    encoded
}

fn find_longest_overlap(row1: &[u8], row2: &[u8]) -> usize {
    let max_overlap = std::cmp::min(row1.len(), row2.len());

    for overlap_len in (1..=max_overlap).rev() {
        if &row1[row1.len() - overlap_len ..] == &row2[..overlap_len] {
            return overlap_len;
        }
    }
    0
}

/// Encodes pixels to an RLE-compressed ImageData
fn encode_grp_rle_data(width: u16, height: u16, pixels: Vec<u8>, compression_type: &CompressionType) -> ImageData {
    let mut raw_row_data = Vec::new();
    let mut rle_data     = Vec::new();
    let mut row_offsets  = Vec::with_capacity(height as usize);
    let mut prev_row: Option<Vec<u8>> = None;

    for row in 0..height {
        let row_start_offset = rle_data.len() as u16 + (height * 2);

        let start = row as usize * width as usize;
        let end = start + width as usize;
        let row_pixels = &pixels[start..end];

        log(LogLevel::Debug, ""); // Give some space in the logs
        log(LogLevel::Debug, &format!(
            "Encoding row {} / {} of width {}. Start: {}, End: {}",
            row, height, width, start, end,
        ));
        let encoded_row = encode_grp_rle_row(row_pixels, compression_type);
        rle_data.extend_from_slice(&encoded_row);
        raw_row_data.push(encoded_row.clone());

        // If the previous row has x bytes in the end that are identical to the x first
        // bytes of the encoded_row, then we can save those x bytes by adjusting the offset.
        let offset_overlap = if prev_row.is_some() && compression_type == &CompressionType::Optimised {
            let overlap = find_longest_overlap(&(prev_row.clone().unwrap().to_vec()), &encoded_row.to_vec());
            if overlap > 1 {
                log(LogLevel::Debug, &format!(
                    "Overlap between row {} and {}: {} bytes",
                    row - 1, row, overlap,
                ));
            }
            overlap as u16
        } else {
            0
        };

        row_offsets.push(row_start_offset - offset_overlap);
        prev_row = Some(encoded_row);
    }

    ImageData {
        row_offsets,
        raw_row_data,
        converted_pixels: pixels,
        grp_type: GrpType::Normal,
    }
}

/// Encodes pixels to an uncompressed ImageData
fn encode_uncompressed_grp(width: u16, height: u16, pixels: Vec<u8>, extended_width: bool) -> ImageData {

    let raw_row_data = read_uncompressed_pixels(width, height, pixels.clone());

    // In uncompressed GRPs, there is no list of row offsets in each frame, unlike in normal GRPs.
    // By setting row_offsets to an empty array, we can avoid it being written later.
    let row_offsets = vec![];
    let grp_type = if extended_width {
        GrpType::UncompressedExtended
    } else {
        GrpType::Uncompressed
    };
    ImageData {
        row_offsets,
        raw_row_data,
        converted_pixels: pixels,
        grp_type,
    }
}

/// Creates a GrpHeader from a set of GrpFrames
fn create_grp_header(frames: &[GrpFrame], max_width: u16, max_height: u16) -> GrpHeader {
    GrpHeader {
        frame_count: frames.len() as u16,
        max_width,
        max_height,
    }
}


/// Given a path, GrpHeader and a set of GrpFrames, this function writes a GRP file
/// to the given path.
fn write_grp_file(path: &str, header: &GrpHeader, frames: &[GrpFrame], compression_type: &CompressionType) -> Result<()> {
    let mut file = File::create(path)?;

    // Write header
    file.write_all(&header.frame_count.to_le_bytes())?;
    if compression_type == &CompressionType::War1 {
        file.write_all(&[header.max_width  as u8])?;
        file.write_all(&[header.max_height as u8])?;
    } else {
        file.write_all(&header.max_width .to_le_bytes())?;
        file.write_all(&header.max_height.to_le_bytes())?;
    }

    // Write frame headers
    for frame in frames {
        file.write_all(&[frame.x_offset])?;
        file.write_all(&[frame.y_offset])?;
        file.write_all(&[frame.width])?;
        file.write_all(&[frame.height])?;
        file.write_all(&frame.image_data_offset.to_le_bytes())?;
    }

    // Frames that share the same image_data_offset are duplicated frames.
    // Only write the image data of those frames once.
    let mut written_frames = HashSet::new();

    // Write image data
    for frame in frames {
        if written_frames.insert(&frame.image_data_offset) {
            // This offset hasn't been written yet — do it now.

            // Write row offset table
            for &offset in &frame.image_data.row_offsets {
                file.write_all(&offset.to_le_bytes())?;
            }

            // Write each row's raw RLE data
            for row in &frame.image_data.raw_row_data {
                file.write_all(row)?;
            }
        }
    }

    Ok(())
}

/// Read the PNG in the given file name, and turn it into a GrpFrame
fn png_to_grpframe(
    image: ImageWithMetadata<u8, u16>,
    image_data_offset: u32,
    compression: &CompressionType,
) -> Result<GrpFrame> {

    let mut offset = image_data_offset;
    let mut width  = image.width as u8;
    let height     = image.height as u8;

    let image_data = if compression == &CompressionType::Normal || compression == &CompressionType::Optimised {

        if image.width > u8::MAX as u16 {
            // The image size was checked when reading the PNGs, but an image width of up to 512
            // is allowed for Extended Uncompressed GRPs. Here, we're dealing with Normal GRPs,
            // which have a max width of 255.
            return Err(Error::new(ErrorKind::InvalidInput, format!(
                "Width ({}) is above limit of {}", image.width, u8::MAX)))
        }
        encode_grp_rle_data(image.width, image.height, image.image_data, compression)

    } else {
        let extended_width = image_should_be_extended(image.width);
        if  extended_width {
            let (w, o) = adjust_width_and_offset_if_extended_when_encoding(image.width, offset);
            log(LogLevel::Debug, &format!(
                "Writing Uncompressed frame with extended size. Actual width: {}, width in file: {}. \
                    Actual offset: 0x{:0>2X}, offset in file: 0x{:0>2X}",
                image.width, w, image_data_offset, o,
            ));
            offset = o;
            width  = w as u8;
        }

        encode_uncompressed_grp(image.width, image.height, image.image_data, extended_width)
    };

    Ok(GrpFrame {
        x_offset: image.x_offset,
        y_offset: image.y_offset,
        width,
        height,
        image_data_offset: offset,
        image_data,
    })
}

/// Turn all the given PNG files into a set of GrpFrames.
fn files_to_grp(
    png_files: Vec<String>,
    palette: &Vec<[u8; 3]>,
    compression_type: &CompressionType,
) -> Result<(Vec<GrpFrame>, u16, u16)> {

    let mut grp_frames: Vec<GrpFrame> = Vec::with_capacity(png_files.len());
    let mut seen_frames: HashMap<u64, usize> = HashMap::new();

    let header_len = get_header_size(*compression_type == CompressionType::War1);
    let mut image_data_offset = (header_len + png_files.len() * 8) as u32; // Initialize to GRP header size
    let mut max_width  = 0;
    let mut max_height = 0;

    for (index, png_file) in png_files.iter().enumerate() {
        let image = png_to_pixels(png_file.as_str(), palette)?;
        let reuse_key = make_frame_reuse_key(&compression_type, &image);

        if let Some(&existing_index) = seen_frames.get(&reuse_key) {
            let reused: GrpFrame = grp_frames[existing_index].clone();
            log(LogLevel::Info, &format!(
                "Frame {} is identical to frame {} — reusing image data",
                index, existing_index,
            ));

            grp_frames.push(GrpFrame {
                x_offset: image.x_offset,
                y_offset: image.y_offset,
                width:    reused.width,
                height:   reused.height,
                image_data_offset: reused.image_data_offset,
                image_data: reused.image_data.clone(),
            });

        } else {
            let orig_width  = image.original_width;
            let orig_height = image.original_height;
            let grp_frame = png_to_grpframe(image, image_data_offset, &compression_type)?;

            image_data_offset += grp_frame.grp_frame_len() as u32;
            if offset_is_extended(image_data_offset) {
                return Err(Error::new(ErrorKind::InvalidInput,
                    "The image data offset is already too big to add more GRPs!",
                ));
            }
            if *compression_type == CompressionType::War1 &&
                (grp_frame.width  as u16 + grp_frame.x_offset as u16) > u8::MAX as u16 ||
                (grp_frame.height as u16 + grp_frame.y_offset as u16) > u8::MAX as u16 {
                return Err(Error::new(ErrorKind::InvalidInput, format!(
                    "For compression type {}: \
                    width ({}) added to x-offset ({}) is {} and must be below {}, or \
                    height ({}) added to y-offset ({}) is {} and must be below {}. \
                    Try making the number of rows and columns of all-transparent pixels fewer.",
                    compression_type, grp_frame.width, grp_frame.x_offset, grp_frame.width + grp_frame.x_offset, u8::MAX,
                    grp_frame.height, grp_frame.y_offset, grp_frame.height + grp_frame.y_offset, u8::MAX,
                )));
            }

            seen_frames.insert(reuse_key, grp_frames.len());
            grp_frames.push(grp_frame);

            max_width  = std::cmp::max(max_width,  orig_width);
            max_height = std::cmp::max(max_height, orig_height);
        }
    }

    Ok((grp_frames, max_width, max_height))
}

fn get_header_size(war1_style: bool) -> usize {
    if war1_style {
        4
    } else {
        6
    }
}

fn determine_compression_type(png_files: &Vec<String>, compression_type: &CompressionType) -> CompressionType {
    let compression = if *compression_type != CompressionType::Auto {
        compression_type.clone()
    } else {
        if png_files.iter().any(|p| p.contains(&format!("{}_", UNCOMPRESSED_FILENAME))) {
            CompressionType::Uncompressed
        } else if png_files.iter().any(|p| p.contains(&format!("{}_", WAR1_FILENAME))) {
            CompressionType::War1
        } else {
            CompressionType::Normal
        }
    };
    log(LogLevel::Debug, &format!("Will use compression type {}", compression));
    compression
}

/// Make a hash of the data that is relevant for determining whether to reuse a frame or not
fn make_frame_reuse_key(compression_type: &CompressionType, image: &ImageWithMetadata<u8, u16>) -> u64 {
    if (*compression_type == CompressionType::Normal) || (*compression_type == CompressionType::Optimised) {
        // For normal GRPs, we reference a previous frame if the current image data
        // is identical to a frame we've already seen.
        let mut hasher = DefaultHasher::new();
        image.image_data.hash(&mut hasher);
        hasher.finish()

    } else {
        // For uncompressed GRPs, we reference a previous frame if both the
        // current image data, and the metadata (x and y offsets, width, height)
        // is identical to a frame we've already seen.
        let key = FrameDedupKey {
            image_data: image.image_data.clone(),
            x_offset:   image.x_offset,
            y_offset:   image.y_offset,
            width:      image.width,
            height:     image.height,
        };

        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish()
    }
}

/// Detects whether the given GRP is uncompressed (unusual) or not (normal)
pub fn detect_uncompressed(input_path: &String, header: &GrpHeader, war1_style: bool) -> Result<bool> {

    let mut file = File::open(input_path)?;
    let file_len = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(get_header_size(war1_style) as u64))?;

    let mut seen_offsets  = HashSet::new();
    let mut first_offset  = 0;
    let mut total_frame_size = 0;

    for _ in 0..header.frame_count {

        let mut buf = [0u8; 8];
        file.read_exact(&mut buf)?;

        let w      = buf[2];
        let height = buf[3];
        let image_data_offset = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);

        let (width, offset) = adjust_width_and_offset_if_extended_when_decoding(w, image_data_offset);

        if seen_offsets.insert(offset) {
            total_frame_size += width as u32 * height as u32;
        }

        if first_offset == 0 {
            first_offset = offset;
        }
    }

    let is_uncompressed = first_offset + total_frame_size == file_len as u32;
    let log_level = if is_uncompressed {
        LogLevel::Warn
    } else {
        LogLevel::Debug
    };
    log(log_level, &format!("Is uncompressed: {}. Is WarCraft I style: {}", is_uncompressed, war1_style));

    Ok(is_uncompressed)
}

/// Converts a GRP to PNGs
pub fn grp_to_png(args: &Args) -> Result<()> {
    let pal_path = &args.pal_path.as_deref().unwrap();
    let palette  = read_rgb_palette(pal_path)?;

    let input_path = &args.input_path.clone().unwrap();
    let mut f  = File::open(input_path)?;
    let (header, war1_style) = read_grp_header(&mut f)?;
    let is_uncompressed = detect_uncompressed(input_path, &header, war1_style)?;

    let grp_type = if is_uncompressed && war1_style {
        GrpType::War1
    } else if is_uncompressed {
        GrpType::Uncompressed
    } else {
        GrpType::Normal
    };

    let frames = read_grp_frames(&mut f, header.frame_count, grp_type)?;

    render_and_save_frames_to_png(
        &frames,
        &palette,
        header.max_width  as u32,
        header.max_height as u32,
        &args,
    )
}

/// Converts PNGs to a GRP
pub fn png_to_grp(args: &Args) -> Result<()> {
    let out_path  = &args.output_path.as_deref().unwrap();
    let pal_path  = &args.pal_path.as_deref().unwrap();

    let palette   = read_rgb_palette(pal_path)?;
    let png_files = list_png_files(&args.input_path.clone().unwrap())?;
    let compression_type = determine_compression_type(&png_files, &args.compression_type);

    let (grp_frames, max_width, max_height) = files_to_grp(png_files, &palette, &compression_type)?;
    let grp_header = create_grp_header(&grp_frames, max_width, max_height);
    write_grp_file(out_path, &grp_header, &grp_frames, &compression_type)
}


#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::fs;

    fn create_test_png(path: &str, colour: [u8; 3], width: u32, height: u32) {
        use image::{Rgb, RgbImage};
        let mut img = RgbImage::new(width, height);
        for pixel in img.pixels_mut() {
            *pixel = Rgb(colour);
        }
        img.save(path).expect("Failed to save test PNG");
    }

    fn dummy_palette() -> Vec<[u8; 3]> {
        let mut palette = [[0u8; 3]; 256];
        for (i, rgb) in palette.iter_mut().enumerate() {
            rgb[0] = i as u8;
            rgb[1] = i as u8;
            rgb[2] = i as u8;
        }
        Vec::from(palette)
    }


    #[test]
    fn test_malformed_header() {
        use std::io::Cursor;
        let data = vec![0u8; 3]; // too short for a valid header
        let mut cursor = Cursor::new(data);

        let result = read_grp_header(&mut cursor);

        assert!(result.is_err());
    }

    #[test]
    fn test_incomplete_frame_header() {
        use std::io::Cursor;
        let mut data = vec![0x01, 0x00, 0x01, 0x00, 0x01, 0x00]; // 1 frame, 1x1 size
        data.extend(vec![0; 4]); // only half a frame header
        let mut cursor = Cursor::new(data);

        let _ = read_grp_header(&mut cursor); // skip header
        let result = read_grp_frames(&mut cursor, 1, GrpType::Normal);

        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_row_offset() {
        use std::io::Cursor;
        // Valid header + 1 frame header
        let mut data = vec![0x01, 0x00, 0x01, 0x00, 0x01, 0x00]; // 1 frame, 1x1 size
        data.extend(vec![0, 0, 1, 1, 14, 0, 0, 0]); // frame header (offset 14)
        data.extend(vec![0xFF, 0xFF]); // row offset points far beyond file

        let mut cursor = Cursor::new(data);
        let _ = read_grp_header(&mut cursor);
        let result = read_grp_frames(&mut cursor, 1, GrpType::Normal);
        assert!(result.is_err());
    }

    #[test]
    fn test_uncompressed_style_header() -> Result<()> {
        use std::io::Cursor;
        // Valid header + 1 frame header
        let raw_header = vec![0x01, 0x00, 0x01, 0x00, 0x01, 0x00]; // 1 frame, 1x1 size
        let header_len = raw_header.len() as u64;
        let mut data = vec![];
        data.extend(raw_header);
        data.extend(vec![0, 0, 1, 1, 14, 0, 0, 0]); // frame header (offset 14)
        data.extend(vec![0x71]); // 1 pixel image data

        let mut cursor = Cursor::new(data);
        let (header, war1_style) = read_grp_header(&mut cursor)?;
        cursor.seek(SeekFrom::Start(header_len))?;
        let result = read_grp_frames(&mut cursor, 1, GrpType::Uncompressed);
        assert_eq!(war1_style, false);
        assert_eq!(header.frame_count, 1);
        assert_eq!(header.max_width,   1);
        assert_eq!(header.max_height,  1);
        assert!(result.is_ok());
        Ok(())
    }

    #[test]
    fn test_war1_style_header() -> Result<()> {
        use std::io::Cursor;
        // Valid header + 1 frame header
        let raw_header = vec![0x01, 0x00, 0x01, 0x01]; // 1 frame, 1x1 size
        let header_len = raw_header.len() as u64;
        let mut data = vec![];
        data.extend(raw_header);
        data.extend(vec![0, 0, 1, 1, 12, 0, 0, 0]); // frame header (offset 12)
        data.extend(vec![0x71]); // 1 pixel image data

        let mut cursor = Cursor::new(data);
        let (header, war1_style) = read_grp_header(&mut cursor)?;
        cursor.seek(SeekFrom::Start(header_len))?;
        let result = read_grp_frames(&mut cursor, 1, GrpType::War1);
        assert_eq!(war1_style, true);
        assert_eq!(header.frame_count, 1);
        assert_eq!(header.max_width,   1);
        assert_eq!(header.max_height,  1);
        assert!(result.is_ok());
        Ok(())
    }

    #[test]
    fn test_decode_transparent_only() {
        let data = vec![0x85]; // skip 5 transparent pixels

        let (result, encoded_length) = decode_grp_rle_row(&data, 5);

        assert_eq!(result, vec![0, 0, 0, 0, 0]);
        assert_eq!(encoded_length, data.len());
    }

    #[test]
    fn test_decode_solid_colour_run() {
        let data = vec![0x42, 7]; // repeat colour 7 for 2 pixels

        let (result, encoded_length) = decode_grp_rle_row(&data, 2);

        assert_eq!(result, vec![7, 7]);
        assert_eq!(encoded_length, data.len());
    }

    #[test]
    fn test_decode_raw_pixels() {
        let data = vec![3, 5, 6, 7]; // copy 3 pixels directly

        let (result, encoded_length) = decode_grp_rle_row(&data, 3);

        assert_eq!(result, vec![5, 6, 7]);
        assert_eq!(encoded_length, data.len());
    }

    #[test]
    fn test_decode_mixed_sequence() {
        let data = vec![0x81, 0x43, 9, 2, 8, 7];
        // skip 1 transparent, repeat 9 for 3, then copy 2 pixels (8, 7)

        let (result, encoded_length) = decode_grp_rle_row(&data, 6);

        assert_eq!(result, vec![0, 9, 9, 9, 8, 7]);
        assert_eq!(encoded_length, data.len());
    }


    #[test]
    fn test_encode_transparent_only() {
        // A row with 5 transparent pixels (palette index 0)
        let row = vec![0; 5];

        let encoded_normal = encode_grp_rle_row(&row, &CompressionType::Normal);
        let encoded_optim  = encode_grp_rle_row(&row, &CompressionType::Optimised);

        // 0x80 means transparent run; 0x80 | 5 = 0x85
        assert_eq!(encoded_normal, vec![0x85]);
        assert_eq!(encoded_optim,  vec![0x85]);
    }

    #[test]
    fn test_encode_solid_colour_run() {
        // A row with 4 pixels of the same colour (e.g. 7)
        let row = vec![7; 4];

        let encoded_normal = encode_grp_rle_row(&row, &CompressionType::Normal);
        let encoded_optim  = encode_grp_rle_row(&row, &CompressionType::Optimised);

        // 0x40 means repeated colour; 0x40 | 4 = 0x44, followed by the colour
        assert_eq!(encoded_normal, vec![0x44, 7]);
        assert_eq!(encoded_optim,  vec![0x44, 7]);
    }

    #[test]
    fn test_encode_raw_pixels() {
        // A row with 3 different pixels (no repetition)
        let row = vec![5, 6, 7];

        let encoded_normal = encode_grp_rle_row(&row, &CompressionType::Normal);
        let encoded_optim  = encode_grp_rle_row(&row, &CompressionType::Optimised);

        // No compression, just copy 3 pixels: [3, 5, 6, 7]
        assert_eq!(encoded_normal, vec![0x03, 5, 6, 7]);
        assert_eq!(encoded_optim,  vec![0x03, 5, 6, 7]);
    }

    #[test]
    fn test_encode_mixed_sequence() {
        // Mixed content:
        // 1 transparent pixel, 3 repeated 9s, and then 2 different pixels
        let row = vec![0, 9, 9, 9, 8, 7];

        let encoded_normal = encode_grp_rle_row(&row, &CompressionType::Normal);
        let encoded_optim  = encode_grp_rle_row(&row, &CompressionType::Optimised);

        // Breakdown:
        // - 0x81: skip 1 transparent
        // - 0x43, 9: repeat 9 for 3 times
        // - 0x02, 8, 7: copy 2 pixels
        assert_eq!(encoded_normal, vec![0x81, 0x05, 9, 9, 9, 8, 7]);
        assert_eq!(encoded_optim,  vec![0x81, 0x43, 9, 0x02, 8, 7]);
    }


    #[test]
    fn test_encode_max_transparent_run() {
        let row = vec![0; 127];

        let encoded_normal = encode_grp_rle_row(&row, &CompressionType::Normal);
        let encoded_optim  = encode_grp_rle_row(&row, &CompressionType::Optimised);

        assert_eq!(encoded_normal, vec![0xFF]); // 0x80 | 127
        assert_eq!(encoded_optim,  vec![0xFF]); // 0x80 | 127
    }

    #[test]
    fn test_encode_max_solid_colour_run() {
        let row = vec![12; 63];

        let encoded_normal = encode_grp_rle_row(&row, &CompressionType::Normal);
        let encoded_optim  = encode_grp_rle_row(&row, &CompressionType::Optimised);

        assert_eq!(encoded_normal, vec![0x7F, 12]); // 0x40 | 63 = 0x7F
        assert_eq!(encoded_optim,  vec![0x7F, 12]); // 0x40 | 63 = 0x7F
    }

    #[test]
    fn test_encode_max_raw_copy() {
        let row: Vec<u8> = (1..63).collect();

        let encoded_normal = encode_grp_rle_row(&row, &CompressionType::Normal);
        let encoded_optim  = encode_grp_rle_row(&row, &CompressionType::Optimised);

        let mut expected = vec![62];
        expected.extend(row.iter());
        assert_eq!(encoded_normal, expected);
        assert_eq!(encoded_optim,  expected);
    }

    #[test]
    fn test_encode_alternating_transparency() {
        let row = vec![0, 1, 0, 2, 0, 3];

        let encoded_normal = encode_grp_rle_row(&row, &CompressionType::Normal);
        let encoded_optim  = encode_grp_rle_row(&row, &CompressionType::Optimised);

        // Should encode as a series of transparent skips and literal copies.
        // Before each literal copy there is a number (here 1 in each case)
        // denoting how many pixels of that copy.
        assert_eq!(encoded_normal, vec![0x81, 0x01, 1, 0x81, 0x01, 2, 0x81, 0x01, 3]);
        assert_eq!(encoded_optim,  vec![0x81, 0x01, 1, 0x81, 0x01, 2, 0x81, 0x01, 3]);
    }

    #[test]
    fn test_encode_then_decode_roundtrip_with_differences_between_compression_types() {
        let original = vec![0x8F, 0x02, 0x8A, 0x40, 0x48, 0x8B, 0x04, 0x40, 0x40, 0x40, 0x8A, 0x8F];
        let width = 44;

        let (decoded, encoded_length) = decode_grp_rle_row(&original, width);
        let encoded_normal = encode_grp_rle_row(&decoded, &CompressionType::Normal);
        let encoded_optim  = encode_grp_rle_row(&decoded, &CompressionType::Optimised);

        assert_eq!(encoded_normal, original);
        assert_eq!(encoded_optim,  vec![0x8F, 0x02, 138, 64, 0x48, 139, 0x43, 64, 0x01, 138, 0x8F]);
        assert_eq!(encoded_length, original.len());
    }

    #[test]
    fn test_encode_then_decode_longer_roundtrip_with_differences_between_compression_types() {
        let original =vec![
            0x81, 0x06, 0x0D, 0x43, 0x40, 0x8C, 0xA3, 0x09, 0x44, 0x08, 0x16, 0x0C, 0x42, 0x77,
            0x2C, 0x8A, 0x8A, 0x8A, 0x8B, 0x28, 0x28, 0x91, 0x43, 0x28, 0x8A, 0x40, 0x40, 0x8A,
            0x77, 0x2B, 0x42, 0x43, 0x0A, 0x44, 0x08, 0x06, 0x0A, 0xA1, 0x8C, 0x40, 0x0B, 0x0F, 0x81];
        let width = 44;

        let (decoded, encoded_length) = decode_grp_rle_row(&original, width);
        let encoded_normal = encode_grp_rle_row(&decoded, &CompressionType::Normal);
        let encoded_optim  = encode_grp_rle_row(&decoded, &CompressionType::Optimised);

        let expected_optim = vec![
            0x81, 0x06, 0x0D, 0x43, 0x40, 0x8C, 0xA3, 0x09, 0x44, 0x08, 0x4, 0x0C, 0x42, 0x77,
            0x2C, 0x43, 0x8A, 0x0F, 0x8B, 0x28, 0x28, 0x91, 0x43, 0x28, 0x8A, 0x40, 0x40, 0x8A,
            0x77, 0x2B, 0x42, 0x43, 0x0A, 0x44, 0x08, 0x06, 0x0A, 0xA1, 0x8C, 0x40, 0x0B, 0x0F, 0x81];
        assert_eq!(encoded_normal, original);
        assert_eq!(encoded_optim, expected_optim);
        assert_eq!(encoded_normal.len(), 43);
        assert_eq!(encoded_optim .len(), 43);
        assert_eq!(encoded_length, original.len());
    }

    #[test]
    fn test_battlecruiser_frame8_row36() {
        let original = vec![
            0x82, 0x3F, 0x8A, 0x8A, 0x40, 0x8A, 0x40, 0x8B, 0x8B, 0x8B, 0x40, 0x40, 0x8B, 0x8B,
            0x40, 0x40, 0x8A, 0x8A, 0xA8, 0x0C, 0x0C, 0x09, 0x09, 0x08, 0x95, 0x95, 0x95, 0x7D,
            0x7D, 0x97, 0x97, 0x45, 0x45, 0x45, 0x91, 0x91, 0x92, 0x9B, 0x2C, 0x8A, 0x8B, 0x40,
            0x8B, 0x40, 0x8D, 0x92, 0x47, 0x91, 0x49, 0x49, 0x47, 0x40, 0x8B, 0x8B, 0x40, 0x42,
            0x92, 0x49, 0x91, 0x91, 0x49, 0x49, 0x40, 0x40, 0x40, 0x15, 0x40, 0x45, 0x49, 0x47,
            0x91, 0x91, 0x92, 0x43, 0x8A, 0x8A, 0x8A, 0x95, 0x51, 0x9A, 0x9A, 0x9A, 0x7D, 0x7D,
            0x97, 0x95, 0x8A, 0x81];
        let width = 87;

        let (decoded, encoded_length) = decode_grp_rle_row(&original, width);
        let encoded_normal = encode_grp_rle_row(&decoded, &CompressionType::Normal);
        let encoded_optim  = encode_grp_rle_row(&decoded, &CompressionType::Optimised);

        let expected_optim = vec![
            130, 5, 138, 138, 64, 138, 64, 67, 139, 14, 64, 64, 139, 139, 64, 64, 138, 138,
            168, 12, 12, 9, 9, 8, 67, 149, 4, 125, 125, 151, 151, 67, 69, 28, 145, 145, 146,
            155, 44, 138, 139, 64, 139, 64, 141, 146, 71, 145, 73, 73, 71, 64, 139, 139, 64,
            66, 146, 73, 145, 145, 73, 73, 68, 64, 7, 69, 73, 71, 145, 145, 146, 67, 67, 138,
            2, 149, 81, 67, 154, 5, 125, 125, 151, 149, 138, 129];
        assert_eq!(encoded_normal, original);
        assert_eq!(encoded_optim, expected_optim);
        assert_eq!(encoded_normal.len(), 88);
        assert_eq!(encoded_optim .len(), 86);
        assert_eq!(encoded_length, original.len());
    }

    #[test]
    fn test_encode_then_decode_roundtrip() {
        let original = vec![0, 0, 7, 7, 7, 8, 9];
        let width = original.len() as u16;

        let encoded_normal = encode_grp_rle_row(&original, &CompressionType::Normal);
        let encoded_optim  = encode_grp_rle_row(&original, &CompressionType::Optimised);
        let (decoded_normal, encoded_normal_length) = decode_grp_rle_row(&encoded_normal, width);
        let (decoded_optim , encoded_optim_length)  = decode_grp_rle_row(&encoded_optim,  width);

        assert_eq!(original, decoded_normal);
        assert_eq!(original, decoded_optim);
        assert_eq!(encoded_normal_length, encoded_normal.len());
        assert_eq!(encoded_optim_length,  encoded_optim.len());
    }

    #[test]
    fn test_roundtrip_various_patterns() {
        let test_rows = vec![
            vec![0, 0, 0, 0, 0],
            vec![1, 2, 3, 4, 5],
            vec![5, 5, 5, 5, 5],
            vec![0, 1, 1, 1, 0, 2, 2],
            vec![1, 2, 2, 2, 3, 0, 0],
        ];

        perform_row_tests(test_rows);
    }

    #[test]
    fn test_roundtrip_edge_cases() {
        let max_transparent  = vec![0; 127];
        let max_solid_colour = vec![42; 63];
        let max_raw_copy: Vec<u8> = (0..63).collect();
        let combo = [&[0; 3][..], &[5; 5][..], &[1, 2, 3][..]].concat();

        let edge_cases = vec![
            max_transparent,
            max_solid_colour,
            max_raw_copy,
            combo,
        ];

        perform_row_tests(edge_cases);
    }

    #[test]
    fn test_decode_truncated_run_length() {
        // Claims to repeat a colour, but colour byte is missing
        let data = vec![0x41]; // run-length of 1, but no colour follows

        let (result, encoded_length) = decode_grp_rle_row(&data, 1);

        // Expect a fallback to default pixel value (0)
        assert_eq!(result, vec![0]);
        assert_eq!(encoded_length, data.len());
    }

    #[test]
    fn test_decode_run_exceeds_width() {
        // Claims to repeat 5 pixels but only room for 3
        let data = vec![0x45, 7]; // run-length of 5 with colour 7

        let (result, encoded_length) = decode_grp_rle_row(&data, 3);

        // Should clamp at width
        assert_eq!(result, vec![7, 7, 7]);
        assert_eq!(encoded_length, data.len());
    }

    #[test]
    fn test_decode_raw_exceeds_data() {
        // Claims to copy 3 pixels but only 2 are present
        let data = vec![3, 1, 2];

        let (result, encoded_length) = decode_grp_rle_row(&data, 3);

        assert_eq!(result, vec![1, 2, 0]);
        assert_eq!(encoded_length, data.len());
    }


    #[test]
    fn detects_duplicate_frames() {
        let palette = dummy_palette();
        let temp_dir = "temp_test_output";
        fs::create_dir_all(temp_dir).unwrap();

        let file1 = format!("{}/frame1.png", temp_dir);
        let file2 = format!("{}/frame2.png", temp_dir);
        let file3 = format!("{}/frame3.png", temp_dir);

        create_test_png(&file1, [71, 71, 71], 16, 16);
        create_test_png(&file2, [42, 42, 42], 16, 16);
        create_test_png(&file3, [71, 71, 71], 16, 16); // identical to file1

        let result = files_to_grp(
            vec![file1.clone(), file2.clone(), file3.clone()],
            &palette,
            &CompressionType::Normal,
        ).unwrap();
        let frames = result.0;

        assert_eq!(frames.len(), 3, "Should create three GrpFrames");
        assert_ne!(
            frames[0].image_data_offset,
            frames[1].image_data_offset,
            "The first two frames should differ",
        );
        assert_eq!(
            frames[0].image_data_offset,
            frames[2].image_data_offset,
            "Duplicate frames should be identical"
        );

        fs::remove_dir_all(temp_dir).unwrap();
    }

    #[test]
    fn does_not_deduplicate_different_frames() {
        let palette = dummy_palette();
        let temp_dir = "temp_test_output2";
        fs::create_dir_all(temp_dir).unwrap();

        let file_a = format!("{}/frameA.png", temp_dir);
        let file_b = format!("{}/frameB.png", temp_dir);

        create_test_png(&file_a, [10, 10, 10], 16, 16);
        create_test_png(&file_b, [11, 11, 11], 16, 16);

        let result = files_to_grp(
            vec![file_a.clone(), file_b.clone()],
            &palette,
            &CompressionType::Normal,
        ).unwrap();
        let frames = result.0;

        assert_eq!(frames.len(), 2, "Should create two GrpFrames");
        assert_ne!(
            frames[0].image_data_offset,
            frames[1].image_data_offset,
            "Different frames should not share the same image_data_offset"
        );

        fs::remove_dir_all(temp_dir).unwrap();
    }

    fn perform_row_tests(test_cases: Vec<Vec<u8>>) {
        for row in test_cases {
            let encoded_normal = encode_grp_rle_row(&row, &CompressionType::Normal);
            let encoded_optim  = encode_grp_rle_row(&row, &CompressionType::Optimised);
            let (decoded_normal, encoded_normal_length) = decode_grp_rle_row(&encoded_normal, row.len() as u16);
            let (decoded_optim , encoded_optim_length)  = decode_grp_rle_row(&encoded_optim,  row.len() as u16);

            assert_eq!(decoded_normal, row);
            assert_eq!(decoded_optim,  row);
            assert_eq!(encoded_normal_length, encoded_normal.len());
            assert_eq!(encoded_optim_length,  encoded_optim.len());
        }
    }


    // Property-based test: for any randomly generated row of pixel values (between 0 and 255),
    // the function encodes the row with GRP RLE compression, then decodes it back again.
    // The output must exactly match the original input.
    // This ensures our encoder and decoder are inverses of each other and that the RLE logic
    // works across a wide variety of input patterns, including edge cases we might not think to test manually.
    //
    // proptest generates hundreds of random rows (length 0 to 127) and runs the test for each.
    proptest! {
        #[test]
        fn prop_encode_decode_roundtrip(row in proptest::collection::vec(0u8..=255, 0..128)) {
            let width = row.len();
            let encoded = encode_grp_rle_row(&row, &CompressionType::Normal);
            let (decoded, encoded_length) = decode_grp_rle_row(&encoded, width as u16);
            prop_assert_eq!(decoded, row);
            prop_assert_eq!(encoded_length, encoded.len());
        }
    }
}

const EXTENDED_OFFSET_BIT: u32 = 0x8000_0000;
pub const EXTENDED_IMAGE_WIDTH: u16 = 256;
