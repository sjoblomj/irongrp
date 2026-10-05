use crate::png::{png_to_pixels, render_and_save_frames_to_png};
use crate::error::{Error, InFile, Result};
use crate::{list_png_files, CompressionType, GrpToPngArgs, PngToGrpArgs, UNCOMPRESSED_FILENAME, WAR1_FILENAME};
use clap::ValueEnum;
use log::{debug, info, trace, warn};
use palpngrs::{greyscale_palette, read_rgb_palette, PalettizedImageWithMetadata};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Set in the image data offset of frames of Extended Uncompressed GRPs, which are wider than 255
const EXTENDED_OFFSET_BIT: u32 = 0x8000_0000;
/// Added to the width stored in the frame header of frames of Extended Uncompressed GRPs
pub const EXTENDED_IMAGE_WIDTH: u16 = 256;

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
    width:      u16,
    height:     u16,
    /// (x_offset, y_offset), only included for compression types where they matter for reuse
    offsets:    Option<(u8, u8)>,
}

impl GrpFrame {
    /// The actual width of the frame in pixels, accounting for Extended Uncompressed frames
    pub fn decoded_width(&self) -> u16 {
        if self.image_data.grp_type == GrpType::UncompressedExtended {
            self.width as u16 + EXTENDED_IMAGE_WIDTH
        } else {
            self.width as u16
        }
    }

    /// The actual offset in the file of the frame's image data. For Extended Uncompressed
    /// frames, `image_data_offset` has its highest bit set, which is cleared here.
    pub fn decoded_image_data_offset(&self) -> u32 {
        if self.image_data.grp_type == GrpType::UncompressedExtended {
            self.image_data_offset & !EXTENDED_OFFSET_BIT
        } else {
            self.image_data_offset
        }
    }

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
    let too_short = "File is too short to contain a GRP header";
    read_grp_bytes(file, &mut buf[..2], too_short)?;
    let frame_count = u16::from_le_bytes([buf[0], buf[1]]);
    if frame_count == 0 {
        // Such GRPs are not created by IronGRP either, and with no frame headers to examine,
        // a WarCraft I style header cannot be told apart from a normal one.
        return Err(Error::InvalidGrp("The GRP has no frames".to_string()));
    }
    read_grp_bytes(file, &mut buf[2..], too_short)?;

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

    debug!(
        "Read GRP Header. War1 style: {}, Frame count: {}, max width: {}, max_height: {}",
        war1_style, header.frame_count, header.max_width, header.max_height,
    );
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

    let mut war1_error = None;
    if war1_max_width != 0 && war1_max_height != 0 {
        // This is true for War1 GRPs and Extended GRPs. WarCraft I style GRPs are always
        // uncompressed and have no extended frames, so if the frame headers can be read in the
        // War1 layout but the data is not uncompressed in it, or a frame has an extended width,
        // then the GRP is not WarCraft I style.
        match try_reading_frame_headers(file, frame_count, true) {
            Ok(()) if detect_uncompressed(file, frame_count, true)? => return Ok(true),
            Ok(()) => {},
            Err(e) => war1_error = Some(e),
        }
    }
    match (try_reading_frame_headers(file, frame_count, false), war1_error) {
        (Ok(()), _) => Ok(false),
        // Neither layout works, so report why for both, as the GRP may have been meant as either
        (Err(Error::InvalidGrp(normal)), Some(Error::InvalidGrp(war1))) => Err(Error::InvalidGrp(format!(
            "{}. When read as WarCraft I style instead: {}", normal, war1,
        ))),
        (Err(e), _) => Err(e),
    }
}

/// Reads exactly `buf.len()` bytes, reporting a truncated file as an invalid GRP
/// rather than as an I/O error.
fn read_grp_bytes<R: Read>(file: &mut R, buf: &mut [u8], msg: &str) -> Result<()> {
    file.read_exact(buf).map_err(|e| match e.kind() {
        ErrorKind::UnexpectedEof => Error::InvalidGrp(msg.to_string()),
        _ => e.into(),
    })
}

/// Reads all frame headers, in the WarCraft I style layout or the normal one, and checks that
/// their image data can be within the file: after the frame header table, and with room for the
/// smallest possible image data before the end of the file. In the WarCraft I style layout, frames
/// may not have an extended width. Returns Error if not.
fn try_reading_frame_headers<R: Read + Seek>(
    file: &mut R,
    frame_count: u16,
    war1_style: bool,
) -> Result<()> {

    let start_pos = get_header_size(war1_style);
    let file_len = file.seek(SeekFrom::End(0))?;
    let frame_headers_end = start_pos as u64 + frame_count as u64 * 8;
    for i in 0..frame_count {
        file.seek(SeekFrom::Start(start_pos as u64 + i as u64 * 8))?;
        let mut buf = [0u8; 8];
        read_grp_bytes(file, &mut buf, "Frame header table goes beyond end of file")?;

        // buf[0] and buf[1] contain x_offset and y_offset, respectively
        let w = buf[2];
        let height = buf[3];
        let image_data_offset = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);

        let (width, offset) = adjust_width_and_offset_if_extended_when_decoding(w, image_data_offset);
        let offset = offset as u64;

        if width == 0 || height == 0 {
            return Err(Error::InvalidGrp(format!("Frame {} has zero width or height", i)));
        }
        if war1_style && offset_is_extended(image_data_offset) {
            return Err(Error::InvalidGrp(format!(
                "Frame {} has an extended width, which WarCraft I style GRPs do not support", i,
            )));
        }
        if offset < frame_headers_end {
            return Err(Error::InvalidGrp(format!(
                "Frame {} has image data offset 0x{:X}, within the GRP header or frame header table \
                (which ends at 0x{:X})", i, offset, frame_headers_end,
            )));
        }
        // The type of GRP is not known yet. Uncompressed frames take width * height bytes, while
        // Normal frames take at least a row offset table of 2 bytes per row, and 1 byte of row data.
        let min_image_data_len = (width as u64 * height as u64).min(2 * height as u64 + 1);
        if offset + min_image_data_len > file_len {
            return Err(Error::InvalidGrp(format!(
                "Frame {} has image data offset 0x{:X}, leaving too little room for its image data \
                before the end of the file (0x{:X})", i, offset, file_len,
            )));
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

        let offset = image_data_offset & !EXTENDED_OFFSET_BIT; // clear the highest bit
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
        debug!("Reading GRP Frame {} / {}", i, frame_count);
        file.seek(SeekFrom::Start(pos + i as u64 * 8))?;
        let mut buf = [0u8; 8];
        file.read_exact(&mut buf)?;

        let image_data_offset = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let width  = buf[2];
        let height = buf[3];

        let image_data = if grp_type != GrpType::Normal {

            let (w, offset) = adjust_width_and_offset_if_extended_when_decoding(width, image_data_offset);
            let has_extended_size = offset_is_extended(image_data_offset);
            if  has_extended_size {
                debug!(
                    "Reading Uncompressed frame {} with extended size. Width in file: {}, \
                    actual width: {}. Offset in file: 0x{:0>2X}, actual offset: 0x{:0>2X}",
                    i, width, w, image_data_offset, offset,
                );
            }

            let compression_type = if has_extended_size {
                // WarCraft I style GRPs with extended frames are not detected as such
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
            let (image_data, problems) = read_image_data(
                file,
                width  as u16,
                height as u16,
                image_data_offset,
            )?;
            if !problems.is_empty() {
                warn!(
                    "Frame {} has malformed image data, which was decoded as well as possible: {}",
                    i, describe_row_problems(&problems),
                );
            }
            image_data
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
        debug!(
            "Read GRP Frame {}. x-offset: 0x{:0>2X} ({}), y-offset: 0x{:0>2X} ({}), \
            width: 0x{:0>2X} ({}), height: 0x{:0>2X} ({}), image-data-offset: 0x{:0>4X} ({}), \
            number of pixels: {}",
            i, grp_frame.x_offset, grp_frame.x_offset, grp_frame.y_offset, grp_frame.y_offset,
            grp_frame.width, grp_frame.width, grp_frame.height, grp_frame.height,
            grp_frame.image_data_offset, grp_frame.image_data_offset,
            grp_frame.image_data.converted_pixels.len(),
        );
        debug!(""); // Give some space in the logs
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
        .ok_or_else(|| Error::InvalidGrp("Image data offset is beyond end of file".to_string()))?;
    if data_len < width as u64 * height as u64 {
        return Err(Error::InvalidGrp(format!(
            "Wanted to read {} bytes, but only {} are available in file",
            width as u64 * height as u64, data_len,
        )));
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

/// Reads row offsets and decodes image data. Also returns the problems found in rows of
/// malformed image data, as (row, problem).
fn read_image_data<R: Read + Seek>(
    file:   &mut R,
    width:  u16,
    height: u16,
    image_data_offset: u32,
) -> Result<(ImageData, Vec<(usize, RowProblem)>)> {

    let file_len = file.seek(SeekFrom::End(0))?;
    let data_len = file_len
        .checked_sub(image_data_offset as u64)
        .ok_or_else(|| Error::InvalidGrp("Image data offset is beyond end of file".to_string()))?;

    // Seek to the beginning of the row offset table and read the remainder of the file
    file.seek(SeekFrom::Start(image_data_offset as u64))?;
    let mut data_block = vec![0; data_len as usize];
    file.read_exact(&mut data_block)?;

    // Parse row offsets from the beginning of data_block
    let mut row_offsets = Vec::with_capacity(height as usize);
    for i in 0..height {
        let offset_start = i as usize * 2;
        if  offset_start + 2 > data_block.len() {
            return Err(Error::InvalidGrp("Not enough data for row offset table".to_string()));
        }
        let row_offset = u16::from_le_bytes([data_block[offset_start], data_block[offset_start + 1]]);
        row_offsets.push(row_offset);
    }

    let mut raw_row_data = Vec::with_capacity(height as usize);
    let mut pixels = vec![0; (width * height) as usize];
    let mut problems = Vec::new();

    for (row, &row_offset) in row_offsets.iter().enumerate() {
        if row_offset as usize >= data_block.len() {
            return Err(Error::InvalidGrp(format!(
                "Row data offset {} is beyond end of data_block ({})", row_offset, data_block.len(),
            )));
        }
        let row_data = &data_block[row_offset as usize ..];
        debug!(
            "Decoding row {} of width {} from offset {} (length {})",
            row, width, row_offset, row_data.len(),
        );

        let decoded_row = decode_grp_rle_row(row_data, width);
        raw_row_data.push(row_data[..decoded_row.encoded_len].to_vec());
        problems.extend(decoded_row.problems.iter().map(|&problem| (row, problem)));

        let start = row * width as usize;
        pixels[start .. start + decoded_row.pixels.len()].copy_from_slice(&decoded_row.pixels);
    }

    let image_data = ImageData {
        row_offsets,
        raw_row_data,
        converted_pixels: pixels,
        grp_type: GrpType::Normal,
    };
    Ok((image_data, problems))
}

/// A problem found when decoding a row of malformed RLE-compressed image data
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum RowProblem {
    /// A run of pixels extends past the frame width. It is cut off at the frame edge.
    RunPastWidth,
    /// The data ends before an instruction is complete, or before the row is filled.
    /// The rest of the row is left transparent.
    MissingData,
    /// An instruction to copy 0 pixels. It is skipped.
    ZeroLengthCopy,
}

impl RowProblem {
    fn description(self) -> &'static str {
        match self {
            RowProblem::RunPastWidth   => "a run of pixels extends past the frame width and was cut off",
            RowProblem::MissingData    => "the image data ends before the row is complete, so the rest is transparent",
            RowProblem::ZeroLengthCopy => "an instruction to copy 0 pixels was skipped",
        }
    }
}

/// A row of pixels decoded from RLE-compressed data
struct DecodedRow {
    pixels: Vec<u8>,
    /// The number of bytes of encoded data that the row took up
    encoded_len: usize,
    /// The problems found if the data is malformed, each listed once
    problems: Vec<RowProblem>,
}

/// Decodes an RLE-compressed row of pixels. Malformed data is decoded as well as possible, and
/// the problems found are returned along with the pixels.
fn decode_grp_rle_row(line_data: &[u8], image_width: u16) -> DecodedRow {
    let width = image_width as usize;
    let mut pixels = vec![0; width]; // Initialize with transparent pixels (palette index 0)
    let mut problems = Vec::new();
    let mut add_problem = |problem| if !problems.contains(&problem) { problems.push(problem) };
    let mut x = 0;   // Position in output row
    let mut pos = 0; // Position in input data

    while x < width {
        let Some(&control_byte) = line_data.get(pos) else {
            add_problem(RowProblem::MissingData);
            break;
        };
        pos += 1;

        if control_byte & 0x80 != 0 { // Transparent - skip x pixels
            let skip = (control_byte & 0x7F) as usize;
            trace!(
                "Decoding transparent byte (0x{:0>2X}). Skipping 0x{:0>2X} ({}) pixels.",
                control_byte, skip, skip,
            );
            if x + skip > width {
                add_problem(RowProblem::RunPastWidth);
            }
            x += skip;

        } else if control_byte & 0x40 != 0 { // Run-length encoding (repeat same colour X times)
            let run_length = (control_byte & 0x3F) as usize;
            let Some(&colour_index) = line_data.get(pos) else {
                add_problem(RowProblem::MissingData);
                break;
            };
            pos += 1;
            trace!(
                "Decoding control byte 0x{:0>2X} 0x{:0>2X}. Pixel with palette index {} will be repeated {} times.",
                control_byte, colour_index, colour_index, run_length,
            );
            if x + run_length > width {
                add_problem(RowProblem::RunPastWidth);
            }
            pixels[x..(x + run_length).min(width)].fill(colour_index);
            x += run_length;

        } else { // Normal - copy x pixels directly
            let copy_length = control_byte as usize;
            if copy_length == 0 {
                trace!("Read instruction to copy 0 pixels - skipping it");
                add_problem(RowProblem::ZeroLengthCopy);
                continue;
            }
            let available = &line_data[pos..(pos + copy_length).min(line_data.len())];
            if available.len() < copy_length {
                add_problem(RowProblem::MissingData);
            }
            if x + copy_length > width {
                add_problem(RowProblem::RunPastWidth);
            }
            let copied = available.len().min(width - x);
            pixels[x..x + copied].copy_from_slice(&available[..copied]);
            if log::log_enabled!(log::Level::Trace) {
                let bytes: Vec<String> = available.iter().map(|b| format!("{:02X}", b)).collect();
                trace!("Normal decoding of {} bytes: {}", copy_length, bytes.join(" "));
            }
            pos += available.len();
            x += copy_length;
        }
    }

    DecodedRow { pixels, encoded_len: pos, problems }
}

/// Describes the problems found when decoding the rows of a frame, given as (row, problem)
fn describe_row_problems(problems: &[(usize, RowProblem)]) -> String {
    const MAX_ROWS_LISTED: usize = 10;
    let mut kinds: Vec<RowProblem> = problems.iter().map(|&(_, problem)| problem).collect();
    kinds.sort();
    kinds.dedup();

    kinds.iter().map(|&kind| {
        let rows: Vec<usize> = problems.iter().filter(|&&(_, p)| p == kind).map(|&(row, _)| row).collect();
        let mut listed: Vec<String> = rows.iter().take(MAX_ROWS_LISTED).map(|row| row.to_string()).collect();
        if rows.len() > MAX_ROWS_LISTED {
            listed.push("...".to_string());
        }
        format!(
            "{} ({} {}: {})",
            kind.description(), rows.len(), if rows.len() == 1 { "row" } else { "rows" }, listed.join(", "),
        )
    }).collect::<Vec<String>>().join("; ")
}


/// Encodes an RLE-compressed row of pixels
fn encode_grp_rle_row(row_pixels: &[u8], compression_type: &CompressionType) -> Vec<u8> {
    let mut encoded = Vec::new();
    let mut i = 0;

    debug!("Beginning to encode using compression type '{}'", compression_type);
    if log::log_enabled!(log::Level::Trace) {
        for (x, pixel) in row_pixels.iter().enumerate() {
            trace!("x: {:2}, row_pixels[x]: {:2X} ({:3})", x, pixel, pixel);
        }
    }

    let same_colour_threshold = if let CompressionType::Optimised = compression_type {
        2
    } else {
        3
    };

    while i < row_pixels.len() {
        let prev_i = i;
        let current_colour = row_pixels[i];

        trace!(
            "Encoding pixel at position {} / {} with palette index {}",
            i, row_pixels.len(), current_colour,
        );
        // Case 1: Transparent run (index 0)
        if current_colour == 0 {
            let mut run_len = 1;
            while i + run_len < row_pixels.len() && row_pixels[i + run_len] == 0 && run_len < 127 {
                run_len += 1;
            }
            trace!(
                "Encoding transparent run of 0x{:0>2X} ({}) => 0x{:0>2X} ({})",
                run_len, run_len, 0x80 | run_len as u8, 0x80 | run_len as u8,
            );
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
            trace!("Encoding: Pixels of the same colour: 0x{:0>2X} ({})", run_len, run_len);

            if run_len > same_colour_threshold {
                trace!(
                    "Encoding same colour 0x{:0>2X} ({}) => 0x{:0>2X} 0x{:0>2X}",
                    run_len, run_len, 0x40 | run_len as u8, current_colour,
                );
                encoded.push(0x40 | run_len as u8);
                encoded.push(current_colour);
                i += run_len;

            } else { // Case 3: Literal copy
                let start = i;
                let mut run_len = 0;
                let mut last_colour = 0;
                let mut last_colour_len = 0;

                // Go through the row until we find a run of same coloured pixels above the threshold
                for (x, &pixel) in row_pixels.iter().enumerate().skip(i) {
                    trace!(
                        "Encoding literal copy. x: {:2}, row_pixels[x]: {:2X} ({:3})",
                        x, pixel, pixel,
                    );
                    if pixel == 0 {
                        break;
                    }
                    if pixel != last_colour || last_colour_len == 0 {
                        // New pixel or first pixel
                        last_colour = pixel;
                        last_colour_len = 1;
                    } else {
                        // Repetition of last seen pixel
                        last_colour_len += 1;
                    }

                    if run_len >= 63 {
                        break;
                    }
                    if last_colour_len > same_colour_threshold {
                        run_len -= same_colour_threshold;
                        break;
                    }
                    run_len += 1;
                }

                trace!(
                    "Encoding literal copy of 0x{:0>2X} ({}) => 0x{:0>2X} ({})",
                    run_len, run_len, run_len, run_len,
                );
                encoded.push(run_len as u8);
                encoded.extend_from_slice(&row_pixels[start..start + run_len]);
                i += run_len;
            }
        }
        // Each branch above must advance `i` by at least one pixel, otherwise the outer loop
        // would never terminate. Asserting in debug builds turns any future regression into an
        // immediate test failure instead of silently looping
        debug_assert!(
            i > prev_i,
            "encode_grp_rle_row failed to advance at position {} (row length {})",
            prev_i, row_pixels.len(),
        );
    }

    encoded
}

/// Encodes pixels to an RLE-compressed ImageData. Fails if the encoded data is too large
/// for the row offsets, which are stored as u16 relative to the start of the frame data.
fn encode_grp_rle_data(width: u16, height: u16, pixels: Vec<u8>, compression_type: &CompressionType) -> Result<ImageData> {
    let mut raw_row_data = Vec::new();
    let mut rle_data     = Vec::new();
    let mut row_offsets  = Vec::with_capacity(height as usize);

    for row in 0..height {
        let row_start_offset = rle_data.len() + height as usize * 2;
        let row_start_offset = u16::try_from(row_start_offset).map_err(|_| Error::CannotEncode(format!(
            "Frame of size {}x{} is too complex to compress: row {} would start at offset {}, \
            above the limit of {}. Try reducing the frame size or the number of colours.",
            width, height, row, row_start_offset, u16::MAX,
        )))?;

        let start = row as usize * width as usize;
        let end = start + width as usize;
        let row_pixels = &pixels[start..end];

        trace!(""); // Give some space in the logs
        trace!(
            "Encoding row {} / {} of width {}. Start: {}, End: {}",
            row, height, width, start, end,
        );
        let encoded_row = encode_grp_rle_row(row_pixels, compression_type);

        rle_data.extend_from_slice(&encoded_row);
        raw_row_data.push(encoded_row.clone());
        row_offsets.push(row_start_offset);
    }

    Ok(ImageData {
        row_offsets,
        raw_row_data,
        converted_pixels: pixels,
        grp_type: GrpType::Normal,
    })
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
    image: PalettizedImageWithMetadata<u8, u16>,
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
            return Err(Error::CannotEncode(format!(
                "Width ({}) is above limit of {}", image.width, u8::MAX)))
        }
        encode_grp_rle_data(image.width, image.height, image.palettized_image, compression)?

    } else {
        let extended_width = image_should_be_extended(image.width);
        if  extended_width && compression == &CompressionType::War1 {
            return Err(Error::CannotEncode(format!(
                "Width ({}) is above limit of {} for compression type {}", image.width, u8::MAX, compression,
            )))
        }
        if  extended_width {
            let (w, o) = adjust_width_and_offset_if_extended_when_encoding(image.width, offset);
            debug!(
                "Writing Uncompressed frame with extended size. Actual width: {}, width in file: {}. \
                    Actual offset: 0x{:0>2X}, offset in file: 0x{:0>2X}",
                image.width, w, image_data_offset, o,
            );
            offset = o;
            width  = w as u8;
        }

        encode_uncompressed_grp(image.width, image.height, image.palettized_image, extended_width)
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
    palette: &[[u8; 3]],
    compression_type: &CompressionType,
) -> Result<(Vec<GrpFrame>, u16, u16)> {

    let mut grp_frames: Vec<GrpFrame> = Vec::with_capacity(png_files.len());
    let mut seen_frames: HashMap<FrameDedupKey, usize> = HashMap::new();

    let header_len = get_header_size(*compression_type == CompressionType::War1);
    let mut image_data_offset = (header_len + png_files.len() * 8) as u32; // Initialize to GRP header size
    let mut max_width  = 0;
    let mut max_height = 0;

    for (index, png_file) in png_files.iter().enumerate() {
        let image = png_to_pixels(png_file.as_str(), palette).in_file(png_file)?;
        validate_war1_frame_size(compression_type, &image).in_file(png_file)?;
        let reuse_key = make_frame_reuse_key(compression_type, &image);

        max_width  = std::cmp::max(max_width,  image.original_width);
        max_height = std::cmp::max(max_height, image.original_height);

        if let Some(&existing_index) = seen_frames.get(&reuse_key) {
            let reused: GrpFrame = grp_frames[existing_index].clone();
            info!("Frame {} is identical to frame {} — reusing image data", index, existing_index);

            grp_frames.push(GrpFrame {
                x_offset: image.x_offset,
                y_offset: image.y_offset,
                width:    reused.width,
                height:   reused.height,
                image_data_offset: reused.image_data_offset,
                image_data: reused.image_data.clone(),
            });

        } else {
            let grp_frame = png_to_grpframe(image, image_data_offset, compression_type).in_file(png_file)?;

            image_data_offset += grp_frame.grp_frame_len() as u32;
            if offset_is_extended(image_data_offset) {
                return Err(Error::CannotEncode(
                    "The image data offset is already too big to add more frames".to_string(),
                )).in_file(png_file);
            }

            seen_frames.insert(reuse_key, grp_frames.len());
            grp_frames.push(grp_frame);
        }
    }

    Ok((grp_frames, max_width, max_height))
}

pub(crate) fn get_header_size(war1_style: bool) -> usize {
    if war1_style {
        4
    } else {
        6
    }
}

/// For War1 GRPs, the header stores `max_width` and `max_height` as single bytes, and the frames
/// cannot use the extended width of Uncompressed GRPs. Hence the canvas size of the PNG, and the
/// extent of the frame within it (offset + size), must fit in a u8. Other compression types use
/// u16 in the header and impose no such restriction here.
fn validate_war1_frame_size(
    compression_type: &CompressionType,
    image: &PalettizedImageWithMetadata<u8, u16>,
) -> Result<()> {
    if *compression_type != CompressionType::War1 {
        return Ok(());
    }
    let max    = u8::MAX as u16;
    let right  = image.width  + image.x_offset as u16;
    let bottom = image.height + image.y_offset as u16;
    if right > max || bottom > max || image.original_width > max || image.original_height > max {
        return Err(Error::CannotEncode(format!(
            "For compression type {}, the image size must be at most {}x{}, but it is {}x{}. \
            The non-transparent part of the image extends to x = {} and y = {}.",
            compression_type, max, max, image.original_width, image.original_height, right, bottom,
        )));
    }
    Ok(())
}

fn determine_compression_type(png_files: &[String], compression_type: &CompressionType) -> CompressionType {
    let compression = if *compression_type != CompressionType::Auto {
        compression_type.clone()
    } else {
        // Only inspect the file name component, so an ancestor directory like
        // `/home/me/war1_sprites/` cannot force a compression type.
        let any_file_name_contains = |needle: &str| -> bool {
            png_files.iter().any(|p| {
                std::path::Path::new(p)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains(needle))
                    .unwrap_or(false)
            })
        };
        if        any_file_name_contains(&format!("{}_", UNCOMPRESSED_FILENAME)) {
            CompressionType::Uncompressed
        } else if any_file_name_contains(&format!("{}_", WAR1_FILENAME)) {
            CompressionType::War1
        } else {
            CompressionType::Normal
        }
    };
    debug!("Will use compression type {}", compression);
    compression
}

/// Make a key of the data that is relevant for determining whether to reuse a frame or not.
/// Two frames may share image data if and only if their keys are equal.
fn make_frame_reuse_key(compression_type: &CompressionType, image: &PalettizedImageWithMetadata<u8, u16>) -> FrameDedupKey {
    // For normal GRPs, we reference a previous frame if the current image data and its
    // dimensions are identical to a frame we've already seen. The dimensions are needed since
    // e.g. a 2x3 and a 3x2 frame of the same colour have identical image data.
    // For uncompressed GRPs, the x and y offsets must also be identical.
    let include_offsets = *compression_type != CompressionType::Normal
        && *compression_type != CompressionType::Optimised;

    FrameDedupKey {
        image_data: image.palettized_image.clone(),
        width:      image.width,
        height:     image.height,
        offsets:    include_offsets.then_some((image.x_offset, image.y_offset)),
    }
}

/// Detects whether the given GRP is uncompressed (unusual) or not (normal).
pub fn detect_uncompressed<R: Read + Seek>(file: &mut R, frame_count: u16, war1_style: bool) -> Result<bool> {

    let file_len = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(get_header_size(war1_style) as u64))?;

    // In uncompressed GRPs, the image data of all frames (shared data counted once) fills the
    // file exactly, from the lowest image data offset to the end.
    let mut seen_offsets = HashSet::new();
    let mut min_offset: Option<u64> = None;
    let mut total_frame_size: u64 = 0;

    for _ in 0..frame_count {

        let mut buf = [0u8; 8];
        file.read_exact(&mut buf)?;

        let w      = buf[2];
        let height = buf[3];
        let image_data_offset = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);

        let (width, offset) = adjust_width_and_offset_if_extended_when_decoding(w, image_data_offset);
        let offset = offset as u64;

        if seen_offsets.insert(offset) {
            total_frame_size += width as u64 * height as u64;
        }
        min_offset = Some(min_offset.map_or(offset, |min| min.min(offset)));
    }

    let is_uncompressed = min_offset.is_some_and(|min| min + total_frame_size == file_len);
    let msg = format!(
        "Is uncompressed when read as {} style: {}",
        if war1_style { "WarCraft I" } else { "normal" }, is_uncompressed,
    );
    if is_uncompressed {
        warn!("{}", msg);
    } else {
        debug!("{}", msg);
    };

    Ok(is_uncompressed)
}

/// Opens and parses the GRP at the given path. Returns its header, its type and its frames.
pub fn read_grp_file(path: impl AsRef<Path>) -> Result<(GrpHeader, GrpType, Vec<GrpFrame>)> {
    let read = || -> Result<_> {
        let mut f = File::open(&path)?;
        let (header, war1_style) = read_grp_header(&mut f)?;

        // WarCraft I style GRPs are only detected as such if they are uncompressed
        let grp_type = if war1_style {
            GrpType::War1
        } else if detect_uncompressed(&mut f, header.frame_count, false)? {
            GrpType::Uncompressed
        } else {
            GrpType::Normal
        };

        let frames = read_grp_frames(&mut f, header.frame_count, grp_type)?;
        Ok((header, grp_type, frames))
    };
    read().in_file(&path)
}

/// Converts a GRP to PNGs
pub fn grp_to_png(args: &GrpToPngArgs) -> Result<()> {
    let palette = get_palette(args.palette.as_deref())?;
    let (header, _, frames) = read_grp_file(&args.input)?;

    render_and_save_frames_to_png(
        &frames,
        &palette,
        header.max_width  as u32,
        header.max_height as u32,
        args,
    )
}

fn get_palette(palette_path: Option<&str>) -> Result<Vec<[u8; 3]>> {
    if let Some(path) = palette_path {
        read_rgb_palette(path).in_file(path)
    } else {
        warn!("No palette given - defaulting to greyscale palette");
        Ok(greyscale_palette())
    }
}

/// Converts PNGs to a GRP
pub fn png_to_grp(args: &PngToGrpArgs) -> Result<()> {
    let out_path  = args.output.as_str();
    let palette   = get_palette(args.palette.as_deref())?;
    let png_files = list_png_files(&args.input)?;
    let compression_type = determine_compression_type(&png_files, &args.compression);

    let (grp_frames, max_width, max_height) = files_to_grp(png_files, &palette, &compression_type)?;
    let grp_header = create_grp_header(&grp_frames, max_width, max_height);
    write_grp_file(out_path, &grp_header, &grp_frames, &compression_type).in_file(out_path)
}


#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn create_test_png(path: &str, colour: [u8; 3], width: u32, height: u32) {
        use image::{Rgb, RgbImage};
        let mut img = RgbImage::new(width, height);
        for pixel in img.pixels_mut() {
            *pixel = Rgb(colour);
        }
        img.save(path).expect("Failed to save test PNG");
    }


    #[test]
    fn test_malformed_header() {
        use std::io::Cursor;
        let data = vec![0u8; 3]; // too short for a valid header
        let mut cursor = Cursor::new(data);

        let result = read_grp_header(&mut cursor);

        assert!(matches!(result, Err(Error::InvalidGrp(_))));
    }

    #[test]
    fn rejects_grp_without_frames() {
        use std::io::Cursor;
        // A header of 2, 4 (War1 style), 6 (normal) and more bytes, all with a frame count of 0
        for len in [2, 4, 6, 8, 100] {
            let mut data = vec![0u8; len];
            if len >= 6 {
                data[2..6].copy_from_slice(&[0x10, 0x00, 0x10, 0x00]); // Max size 16x16
            }
            let err = read_grp_header(&mut Cursor::new(data)).expect_err("expected 0 frames to be rejected");
            assert!(matches!(err, Error::InvalidGrp(_)), "for {} bytes", len);
            assert_eq!(err.to_string(), "invalid GRP: The GRP has no frames", "for {} bytes", len);
        }
    }

    #[test]
    fn rejects_grp_too_short_for_frame_count_or_header() {
        use std::io::Cursor;
        // Too short for the frame count, or a frame count of 1 but too short for the rest
        for data in [vec![], vec![0x01], vec![0x01, 0x00], vec![0x01, 0x00, 0x01, 0x00, 0x01]] {
            let err = read_grp_header(&mut Cursor::new(data.clone())).expect_err("expected an error");
            assert_eq!(err.to_string(), "invalid GRP: File is too short to contain a GRP header", "for {:?}", data);
        }
    }

    /// Checks frame headers of the given (width, height, image data offset) frames, after a
    /// header of `header_len` bytes, in a file of `file_len` bytes
    fn check_frame_headers(header_len: usize, frames: &[(u8, u8, u32)], file_len: usize) -> Result<()> {
        use std::io::Cursor;
        let mut data = vec![0u8; header_len];
        for &(width, height, offset) in frames {
            data.extend([0, 0, width, height]);
            data.extend(offset.to_le_bytes());
        }
        assert!(file_len >= data.len(), "test setup: the file must fit the frame headers");
        data.resize(file_len, 0);
        try_reading_frame_headers(&mut Cursor::new(data), frames.len() as u16, header_len == get_header_size(true))
    }

    fn assert_invalid_grp(result: Result<()>, expected_message: &str) {
        let err = result.expect_err("expected the frame headers to be rejected");
        assert!(matches!(err, Error::InvalidGrp(_)), "{}", err);
        assert!(err.to_string().contains(expected_message), "{}", err);
    }

    #[test]
    fn frame_headers_accept_image_data_right_after_the_frame_header_table() {
        for header_len in [4, 6] {
            let offset = header_len as u32 + 2 * 8;
            let frames = [(1, 1, offset), (1, 1, offset + 1)];
            assert!(check_frame_headers(header_len, &frames, offset as usize + 2).is_ok());
        }
    }

    #[test]
    fn frame_headers_reject_image_data_within_the_header_or_frame_header_table() {
        for header_len in [4, 6] {
            let table_end = header_len as u32 + 2 * 8;
            for offset in [0, header_len as u32, table_end - 1] {
                let frames = [(1, 1, table_end), (1, 1, offset)];
                assert_invalid_grp(
                    check_frame_headers(header_len, &frames, 100),
                    "within the GRP header or frame header table",
                );
            }
        }
    }

    #[test]
    fn frame_headers_reject_image_data_offset_at_end_of_file() {
        // Previously accepted, since only offsets beyond the end of the file were rejected
        assert_invalid_grp(check_frame_headers(6, &[(1, 1, 14)], 14), "too little room");
        assert_invalid_grp(check_frame_headers(6, &[(1, 1, 20)], 14), "too little room");
    }

    #[test]
    fn frame_headers_require_room_for_the_smallest_possible_image_data() {
        // A 2x2 frame takes 4 bytes uncompressed, or at least 2 * 2 + 1 = 5 bytes compressed
        assert!(check_frame_headers(6, &[(2, 2, 14)], 14 + 4).is_ok());
        assert_invalid_grp(check_frame_headers(6, &[(2, 2, 14)], 14 + 3), "too little room");

        // A 10x2 frame takes 20 bytes uncompressed, or at least 5 bytes compressed
        assert!(check_frame_headers(6, &[(10, 2, 14)], 14 + 5).is_ok());
        assert_invalid_grp(check_frame_headers(6, &[(10, 2, 14)], 14 + 4), "too little room");
    }

    #[test]
    fn frame_headers_check_extended_offsets_without_the_extended_bit() {
        // A 300x1 extended frame (width 44 in the file) with 300 bytes of image data at offset 14
        let extended = |offset: u32| [(44, 1, offset | EXTENDED_OFFSET_BIT)];
        assert!(check_frame_headers(6, &extended(14), 14 + 300).is_ok());
        assert_invalid_grp(check_frame_headers(6, &extended(13), 14 + 300), "frame header table");
        // Compressed, a 1 row frame needs at least 3 bytes, and Uncompressed, 300 bytes
        assert_invalid_grp(check_frame_headers(6, &extended(14), 14 + 2), "too little room");
    }

    #[test]
    fn frame_headers_reject_extended_frames_only_in_the_war1_layout() {
        // A 300x1 extended frame (width 44 in the file) with 300 bytes of image data
        for (header_len, war1_style) in [(4, true), (6, false)] {
            let offset = header_len as u32 + 8;
            let frames = [(44, 1, offset | EXTENDED_OFFSET_BIT)];
            let result = check_frame_headers(header_len, &frames, offset as usize + 300);
            if war1_style {
                assert_invalid_grp(result, "Frame 0 has an extended width, which WarCraft I style GRPs do not support");
            } else {
                assert!(result.is_ok(), "{:?}", result);
            }
        }
    }

    #[test]
    fn rejects_war1_grp_with_extended_frame_explaining_both_layouts() {
        // A War1 header with one 300x2 frame: width 44 with the extended bit set
        let mut data = vec![0x01, 0x00, 0xFF, 0x02]; // 1 frame, max size 255x2 as u8
        data.extend([0, 0, 44, 2]);
        data.extend((12u32 | EXTENDED_OFFSET_BIT).to_le_bytes());
        data.extend([7; 600]);

        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("war1_extended.grp");
        std::fs::write(&path, data).unwrap();

        let err = read_grp_file(&path).expect_err("expected the GRP to be rejected");
        assert!(matches!(err.root(), Error::InvalidGrp(_)));
        let message = err.to_string();
        // In the normal layout, the frame header starts 2 bytes later, giving a height of 0
        assert!(message.contains("invalid GRP: Frame 0 has zero width or height. "), "{}", message);
        assert!(message.contains("When read as WarCraft I style instead: Frame 0 has an extended width"), "{}", message);
    }

    #[test]
    fn reports_only_the_normal_layout_error_when_war1_layout_was_not_tried() {
        use std::io::Cursor;
        // Max width 0x0100 has a zero low byte, so the War1 layout is not tried
        let mut data = vec![0x01, 0x00, 0x00, 0x01, 0x01, 0x00];
        data.extend([0, 0, 0, 1, 14, 0, 0, 0]); // Frame 0 with width 0
        data.extend([0; 10]);
        let err = read_grp_header(&mut Cursor::new(data)).expect_err("expected the GRP to be rejected");
        assert_eq!(err.to_string(), "invalid GRP: Frame 0 has zero width or height");
    }

    #[test]
    fn read_grp_file_names_the_file_in_errors() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("broken.grp");
        std::fs::write(&path, [0u8; 3]).unwrap();

        let err = read_grp_file(&path).expect_err("expected an invalid GRP");
        assert!(matches!(err.root(), Error::InvalidGrp(_)));
        assert!(err.to_string().starts_with(&format!("{}: invalid GRP", path.display())));
    }

    #[test]
    fn files_to_grp_names_the_png_in_errors() {
        let palette = greyscale_palette();
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("too_wide.png").to_str().unwrap().to_string();
        // Allowed for Extended Uncompressed GRPs, but too wide for Normal ones
        create_test_png(&file, [42, 42, 42], 300, 10);

        let err = files_to_grp(vec![file.clone()], &palette, &CompressionType::Normal)
            .expect_err("expected a too-wide image to be rejected");
        assert!(matches!(err.root(), Error::CannotEncode(_)));
        assert!(err.to_string().starts_with(&file));
    }

    /// Pixels where every row is 1, 2, ..., 255, so no two neighbours are the same and
    /// nothing is transparent. Each 255-pixel row RLE-encodes to 260 bytes (5 literal copies).
    fn incompressible_pixels(width: u16, height: u16) -> Vec<u8> {
        (0..height).flat_map(|_| (0..width).map(|x| (x % 255) as u8 + 1)).collect()
    }

    #[test]
    fn encode_rle_data_accepts_frame_whose_last_row_offset_fits_in_u16() {
        // Last row starts at 251 * 2 + 250 * 260 = 65502
        let (width, height) = (255, 251);
        let image_data = encode_grp_rle_data(
            width, height, incompressible_pixels(width, height), &CompressionType::Normal,
        ).expect("expected the frame to fit");

        assert_eq!(*image_data.row_offsets.last().unwrap(), 65502);
    }

    #[test]
    fn encode_rle_data_rejects_frame_whose_row_offsets_overflow_u16() {
        // Last row would start at 252 * 2 + 251 * 260 = 65764
        for compression_type in [CompressionType::Normal, CompressionType::Optimised] {
            let (width, height) = (255, 252);
            let result = encode_grp_rle_data(
                width, height, incompressible_pixels(width, height), &compression_type,
            );
            assert!(matches!(result, Err(Error::CannotEncode(_))), "for {}", compression_type);
        }
    }

    #[test]
    fn files_to_grp_rejects_incompressible_max_size_png() {
        let palette = greyscale_palette();
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("noise.png").to_str().unwrap().to_string();

        let (width, height) = (255, 255);
        let pixels = incompressible_pixels(width, height);
        let img = image::RgbImage::from_fn(width as u32, height as u32, |x, y| {
            let v = pixels[(y * width as u32 + x) as usize];
            image::Rgb([v, v, v])
        });
        img.save(&file).unwrap();

        let err = files_to_grp(vec![file.clone()], &palette, &CompressionType::Normal)
            .expect_err("expected an incompressible 255x255 image to be rejected");
        assert!(matches!(err.root(), Error::CannotEncode(_)));
        assert!(err.to_string().starts_with(&file));
    }

    #[test]
    fn list_png_files_rejects_directory_without_pngs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let err = list_png_files(temp_dir.path().to_str().unwrap())
            .expect_err("expected an empty directory to be rejected");
        assert!(matches!(err, Error::InvalidArgument(_)));
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
        assert!(!war1_style);
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
        assert!(war1_style);
        assert_eq!(header.frame_count, 1);
        assert_eq!(header.max_width,   1);
        assert_eq!(header.max_height,  1);
        assert!(result.is_ok());
        Ok(())
    }

    #[test]
    fn reads_grp_with_more_frame_headers_than_fit_in_u16_bytes() {
        // u16::MAX frames, so the frame header table (8 bytes per frame) is far beyond 64 KiB.
        // All frames are 1x1 pixels and share the same image data, but each frame header has
        // a unique combination of x and y offsets, so we can tell that each was read correctly.
        let frame_count = u16::MAX;
        for war1_style in [false, true] {
            let header: Vec<u8> = if war1_style {
                vec![0, 0, 1, 1]       // frame count, then max width and height as u8
            } else {
                vec![0, 0, 1, 0, 1, 0] // frame count, then max width and height as u16
            };
            let mut data = header;
            data[0..2].copy_from_slice(&frame_count.to_le_bytes());

            let image_data_offset = (data.len() + frame_count as usize * 8) as u32;
            for i in 0..frame_count {
                data.extend([i as u8, (i >> 8) as u8, 1, 1]);
                data.extend(image_data_offset.to_le_bytes());
            }
            data.push(0x71); // The 1 pixel of image data shared by all frames

            let temp_dir = tempfile::tempdir().unwrap();
            let path = temp_dir.path().join("many_frames.grp");
            std::fs::write(&path, data).unwrap();

            let (header, grp_type, frames) = read_grp_file(&path).unwrap();
            let expected_type = if war1_style { GrpType::War1 } else { GrpType::Uncompressed };
            assert_eq!(grp_type, expected_type);
            assert_eq!(header.frame_count, frame_count);
            assert_eq!(frames.len(), frame_count as usize);
            for (i, frame) in frames.iter().enumerate() {
                assert_eq!((frame.x_offset, frame.y_offset), (i as u8, (i >> 8) as u8), "frame {}", i);
                assert_eq!(frame.image_data.converted_pixels, vec![0x71], "frame {}", i);
            }
        }
    }

    #[test]
    fn normal_grp_whose_frame_headers_also_fit_the_war1_layout_is_read_as_normal() {
        // A Normal GRP with one 2x2 frame at x = 1, y = 1, whose image data is at 0x10000.
        // Read in the War1 layout (4 byte header), the bytes form a valid frame header too:
        // max width 0x0202 gives a War1 max size of 2x2, and the frame header is read from
        // bytes 4..12, giving a 1x1 frame with image data offset 0x0202, within the file.
        // But the data is not uncompressed in that layout, so the GRP is not War1 style.
        let image_data_offset: u32 = 0x1_0000;
        let mut data = vec![0x01, 0x00, 0x02, 0x02, 0x02, 0x00]; // 1 frame, max size 514x2
        data.extend([1, 1, 2, 2]);                               // x, y, width, height
        data.extend(image_data_offset.to_le_bytes());
        data.resize(image_data_offset as usize, 0);              // Padding up to the image data
        data.extend([4, 0, 6, 0]);                               // Row offsets
        data.extend([0x42, 7, 0x42, 8]);                         // Rows: 7, 7 and 8, 8

        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("looks_like_war1.grp");
        std::fs::write(&path, &data).unwrap();

        // The War1 layout is valid as far as the frame headers go
        let mut cursor = std::io::Cursor::new(&data);
        assert!(try_reading_frame_headers(&mut cursor, 1, true).is_ok());

        let (header, grp_type, frames) = read_grp_file(&path).unwrap();
        assert_eq!(grp_type, GrpType::Normal);
        assert_eq!((header.max_width, header.max_height), (514, 2));
        assert_eq!((frames[0].x_offset, frames[0].y_offset), (1, 1));
        assert_eq!((frames[0].width, frames[0].height), (2, 2));
        assert_eq!(frames[0].image_data.converted_pixels, vec![7, 7, 8, 8]);
    }

    /// A frame of the given position and size, whose pixels are derived from `seed`
    fn seeded_image(x_offset: u8, y_offset: u8, width: u16, height: u16, seed: u8) -> PalettizedImageWithMetadata<u8, u16> {
        let pixels = (0..width as usize * height as usize)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect();
        PalettizedImageWithMetadata::new(
            palpngrs::Offset::new(x_offset, y_offset),
            palpngrs::Size::new(width, height),
            palpngrs::Size::new(width, height),
            pixels,
        )
    }

    /// Encodes the images as a GRP with the given header max size, writes it to a temporary
    /// file, reads it back and checks that the type, header and frames are as expected.
    fn assert_grp_roundtrip(
        images: Vec<PalettizedImageWithMetadata<u8, u16>>,
        max_size: (u16, u16),
        compression_type: CompressionType,
        expected_type: GrpType,
    ) {
        let mut offset = (get_header_size(compression_type == CompressionType::War1) + images.len() * 8) as u32;
        let mut frames = Vec::new();
        for image in images.iter().cloned() {
            let frame = png_to_grpframe(image, offset, &compression_type).unwrap();
            offset += frame.grp_frame_len() as u32;
            frames.push(frame);
        }
        let header = create_grp_header(&frames, max_size.0, max_size.1);

        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("roundtrip.grp");
        let path = path.to_str().unwrap();
        write_grp_file(path, &header, &frames, &compression_type).unwrap();

        let (read_header, grp_type, read_frames) = read_grp_file(path).unwrap();
        let context = format!("max size {:?}, {} frame(s)", max_size, images.len());
        assert_eq!(grp_type, expected_type, "{}", context);
        assert_eq!((read_header.max_width, read_header.max_height), max_size, "{}", context);
        assert_eq!(read_frames.len(), images.len(), "{}", context);
        for (i, (frame, image)) in read_frames.iter().zip(&images).enumerate() {
            assert_eq!((frame.x_offset, frame.y_offset), (image.x_offset, image.y_offset), "frame {}, {}", i, context);
            assert_eq!((frame.decoded_width(), frame.height as u16), (image.width, image.height), "frame {}, {}", i, context);
            assert_eq!(frame.image_data.converted_pixels, image.palettized_image, "frame {}, {}", i, context);
        }
    }

    #[test]
    fn extended_uncompressed_grps_with_two_byte_max_width_are_not_read_as_war1() {
        // The low and high bytes of these max widths are both non-zero, so reading the GRP
        // first tries the War1 layout, where they would be the max width and height.
        for max_width in [257, 300, 384, 511] {
            for max_height in [1, 2, 40, 255] {
                let images = vec![
                    seeded_image(0, 0, max_width, max_height.min(3), 1),         // Extended
                    seeded_image(3, 1, 10, 2, 2),                                // Not extended
                    seeded_image(1, 0, max_width - 1, max_height.min(2), 3),     // Extended
                ];
                assert_grp_roundtrip(
                    images, (max_width, max_height), CompressionType::Uncompressed, GrpType::Uncompressed,
                );
            }
        }
    }

    #[test]
    fn normal_grps_with_two_byte_max_width_are_not_read_as_war1() {
        for max_width in [257, 300, 384, 511] {
            let images = vec![
                seeded_image(0, 0, 255, 3, 1),
                seeded_image(200, 1, 10, 2, 2),
            ];
            assert_grp_roundtrip(images, (max_width, 40), CompressionType::Normal, GrpType::Normal);
        }
    }

    #[test]
    fn war1_grps_are_read_as_war1() {
        for (max_width, max_height) in [(1, 1), (2, 30), (255, 255)] {
            let images = vec![
                seeded_image(0, 0, max_width, max_height, 1),
                seeded_image(0, 0, 1, 1, 2),
            ];
            assert_grp_roundtrip(images, (max_width, max_height), CompressionType::War1, GrpType::War1);
        }
    }

    /// Frames as (x_offset, y_offset, width, height, seed), with widths up to `max_width`
    fn frames_strategy(max_width: u16) -> impl Strategy<Value = Vec<(u8, u8, u16, u16, u8)>> {
        proptest::collection::vec((any::<u8>(), any::<u8>(), 1..=max_width, 1u16..=4, any::<u8>()), 1..5)
    }

    proptest! {
        // Every GRP must be read back as the type it was written as, with the same header and
        // frames, whatever the header's max size is. In particular, max widths whose low and
        // high bytes are both non-zero make the reader try the War1 layout first.
        #[test]
        fn prop_uncompressed_grps_are_read_back_as_uncompressed(
            max_width in 1u16..=511, max_height in 1u16..=255, frames in frames_strategy(511),
        ) {
            let images = frames.into_iter().map(|(x, y, w, h, seed)| seeded_image(x, y, w, h, seed)).collect();
            assert_grp_roundtrip(images, (max_width, max_height), CompressionType::Uncompressed, GrpType::Uncompressed);
        }

        #[test]
        fn prop_normal_grps_are_read_back_as_normal(
            max_width in 1u16..=511, max_height in 1u16..=255, frames in frames_strategy(255),
        ) {
            let images = frames.into_iter().map(|(x, y, w, h, seed)| seeded_image(x, y, w, h, seed)).collect();
            assert_grp_roundtrip(images, (max_width, max_height), CompressionType::Normal, GrpType::Normal);
        }

        #[test]
        fn prop_war1_grps_are_read_back_as_war1(
            max_width in 1u16..=255, max_height in 1u16..=255, frames in frames_strategy(255),
        ) {
            let images = frames.into_iter().map(|(x, y, w, h, seed)| seeded_image(x, y, w, h, seed)).collect();
            assert_grp_roundtrip(images, (max_width, max_height), CompressionType::War1, GrpType::War1);
        }
    }

    #[test]
    fn detects_uncompressed_grp_with_image_data_not_in_frame_order() {
        // Two 2x2 frames, where the image data of frame 1 comes before that of frame 0
        let mut data = vec![0x02, 0x00, 0x02, 0x00, 0x02, 0x00]; // 2 frames, max size 2x2
        data.extend([0, 0, 2, 2, 26, 0, 0, 0]); // Frame 0 at offset 6 + 2 * 8 + 4 = 26
        data.extend([0, 0, 2, 2, 22, 0, 0, 0]); // Frame 1 at offset 22
        data.extend([5, 6, 7, 8]);              // Frame 1
        data.extend([1, 2, 3, 4]);              // Frame 0

        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("reordered.grp");
        std::fs::write(&path, data).unwrap();

        let (_, grp_type, frames) = read_grp_file(&path).unwrap();
        assert_eq!(grp_type, GrpType::Uncompressed);
        assert_eq!(frames[0].image_data.converted_pixels, vec![1, 2, 3, 4]);
        assert_eq!(frames[1].image_data.converted_pixels, vec![5, 6, 7, 8]);
    }

    #[test]
    fn detect_uncompressed_does_not_overflow_for_huge_total_frame_size() {
        use std::io::Cursor;
        // u16::MAX extended frames of 511x255 pixels, each with its own image data offset,
        // add up to more than u32::MAX bytes of image data
        let frame_count = u16::MAX;
        let mut data = vec![0u8; 6];
        for i in 0..frame_count as u32 {
            data.extend([0, 0, 255, 255]);
            data.extend((i | EXTENDED_OFFSET_BIT).to_le_bytes());
        }
        let header = GrpHeader { frame_count, max_width: 511, max_height: 255 };

        let result = detect_uncompressed(&mut Cursor::new(data), header.frame_count, false);

        assert!(matches!(result, Ok(false)));
    }

    #[test]
    fn detect_uncompressed_is_false_for_grp_without_frames() {
        use std::io::Cursor;
        let header = GrpHeader { frame_count: 0, max_width: 0, max_height: 0 };
        let result = detect_uncompressed(&mut Cursor::new(vec![0u8; 6]), header.frame_count, false);
        assert!(matches!(result, Ok(false)));
    }

    /// Decodes a row of valid RLE-compressed data, asserting that no problems were found
    fn decode_valid_row(line_data: &[u8], image_width: u16) -> (Vec<u8>, usize) {
        let decoded = decode_grp_rle_row(line_data, image_width);
        assert_eq!(decoded.problems, vec![], "unexpected problems decoding {:02X?}", line_data);
        (decoded.pixels, decoded.encoded_len)
    }

    #[test]
    fn test_decode_transparent_only() {
        let data = vec![0x85]; // skip 5 transparent pixels

        let (result, encoded_length) = decode_valid_row(&data, 5);

        assert_eq!(result, vec![0, 0, 0, 0, 0]);
        assert_eq!(encoded_length, data.len());
    }

    #[test]
    fn test_decode_solid_colour_run() {
        let data = vec![0x42, 7]; // repeat colour 7 for 2 pixels

        let (result, encoded_length) = decode_valid_row(&data, 2);

        assert_eq!(result, vec![7, 7]);
        assert_eq!(encoded_length, data.len());
    }

    #[test]
    fn test_decode_raw_pixels() {
        let data = vec![3, 5, 6, 7]; // copy 3 pixels directly

        let (result, encoded_length) = decode_valid_row(&data, 3);

        assert_eq!(result, vec![5, 6, 7]);
        assert_eq!(encoded_length, data.len());
    }

    #[test]
    fn test_decode_mixed_sequence() {
        let data = vec![0x81, 0x43, 9, 2, 8, 7];
        // skip 1 transparent, repeat 9 for 3, then copy 2 pixels (8, 7)

        let (result, encoded_length) = decode_valid_row(&data, 6);

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

        let (decoded, encoded_length) = decode_valid_row(&original, width);
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

        let (decoded, encoded_length) = decode_valid_row(&original, width);
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

        let (decoded, encoded_length) = decode_valid_row(&original, width);
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
        let (decoded_normal, encoded_normal_length) = decode_valid_row(&encoded_normal, width);
        let (decoded_optim , encoded_optim_length)  = decode_valid_row(&encoded_optim,  width);

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

        let decoded = decode_grp_rle_row(&data, 1);

        // The pixel is left transparent (0)
        assert_eq!(decoded.pixels, vec![0]);
        assert_eq!(decoded.encoded_len, data.len());
        assert_eq!(decoded.problems, vec![RowProblem::MissingData]);
    }

    #[test]
    fn test_decode_run_exceeds_width() {
        // Claims to repeat 5 pixels but only room for 3
        let data = vec![0x45, 7]; // run-length of 5 with colour 7

        let decoded = decode_grp_rle_row(&data, 3);

        // Should clamp at width
        assert_eq!(decoded.pixels, vec![7, 7, 7]);
        assert_eq!(decoded.encoded_len, data.len());
        assert_eq!(decoded.problems, vec![RowProblem::RunPastWidth]);
    }

    #[test]
    fn test_decode_raw_exceeds_data() {
        // Claims to copy 3 pixels but only 2 are present
        let data = vec![3, 1, 2];

        let decoded = decode_grp_rle_row(&data, 3);

        assert_eq!(decoded.pixels, vec![1, 2, 0]);
        assert_eq!(decoded.encoded_len, data.len());
        assert_eq!(decoded.problems, vec![RowProblem::MissingData]);
    }

    #[test]
    fn decode_literal_copy_past_width_consumes_the_whole_instruction() {
        // Copies 4 pixels into a row 3 wide, followed by data of the next row
        let data = vec![4, 1, 2, 3, 4, 0x85];

        let decoded = decode_grp_rle_row(&data, 3);

        assert_eq!(decoded.pixels, vec![1, 2, 3]);
        assert_eq!(decoded.encoded_len, 5);
        assert_eq!(decoded.problems, vec![RowProblem::RunPastWidth]);
    }

    #[test]
    fn decode_transparent_run_past_width_is_a_problem() {
        let decoded = decode_grp_rle_row(&[0x41, 9, 0x83], 3);
        assert_eq!(decoded.pixels, vec![9, 0, 0]);
        assert_eq!(decoded.problems, vec![RowProblem::RunPastWidth]);
    }

    #[test]
    fn decode_data_ending_before_row_is_filled_is_a_problem() {
        // Data for 2 pixels in a row 5 wide
        let decoded = decode_grp_rle_row(&[0x42, 9], 5);
        assert_eq!(decoded.pixels, vec![9, 9, 0, 0, 0]);
        assert_eq!(decoded.encoded_len, 2);
        assert_eq!(decoded.problems, vec![RowProblem::MissingData]);

        let decoded = decode_grp_rle_row(&[], 2);
        assert_eq!(decoded.pixels, vec![0, 0]);
        assert_eq!(decoded.problems, vec![RowProblem::MissingData]);
    }

    #[test]
    fn decode_zero_length_copy_is_skipped_without_consuming_the_next_byte() {
        // Copy 0 pixels, then repeat colour 9 three times. Previously, the byte after the
        // copy instruction (0x43) was skipped too, so 9 was read as the next instruction.
        let data = vec![0x00, 0x43, 9];

        let decoded = decode_grp_rle_row(&data, 3);

        assert_eq!(decoded.pixels, vec![9, 9, 9]);
        assert_eq!(decoded.encoded_len, data.len());
        assert_eq!(decoded.problems, vec![RowProblem::ZeroLengthCopy]);
    }

    #[test]
    fn decode_lists_each_problem_once_in_the_order_found() {
        // Two 0-length copies, then a run past the width
        let decoded = decode_grp_rle_row(&[0x00, 0x00, 0x45, 7], 3);
        assert_eq!(decoded.pixels, vec![7, 7, 7]);
        assert_eq!(decoded.problems, vec![RowProblem::ZeroLengthCopy, RowProblem::RunPastWidth]);
    }

    #[test]
    fn describes_row_problems_by_kind_with_the_rows_affected() {
        let problems = vec![
            (16, RowProblem::RunPastWidth),
            (3,  RowProblem::ZeroLengthCopy),
            (17, RowProblem::RunPastWidth),
        ];
        assert_eq!(
            describe_row_problems(&problems),
            "a run of pixels extends past the frame width and was cut off (2 rows: 16, 17); \
            an instruction to copy 0 pixels was skipped (1 row: 3)",
        );
    }

    #[test]
    fn describes_at_most_ten_rows_per_kind_of_problem() {
        let problems: Vec<(usize, RowProblem)> = (0..12).map(|row| (row, RowProblem::MissingData)).collect();
        assert_eq!(
            describe_row_problems(&problems),
            "the image data ends before the row is complete, so the rest is transparent \
            (12 rows: 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, ...)",
        );
    }

    #[test]
    fn reads_frames_with_malformed_rows_as_well_as_possible() {
        // A Normal GRP with one 3x2 frame. Row 0 has a run past the width, row 1 a 0-length copy.
        let mut data = vec![0x01, 0x00, 0x03, 0x00, 0x02, 0x00]; // 1 frame, max size 3x2
        data.extend([0, 0, 3, 2, 14, 0, 0, 0]);
        data.extend([4, 0, 6, 0]);       // Row offsets
        data.extend([0x44, 7]);          // Row 0: colour 7 four times, in a row 3 wide
        data.extend([0x00, 0x43, 8]);    // Row 1: copy 0 pixels, then colour 8 three times

        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("malformed.grp");
        std::fs::write(&path, data).unwrap();

        let (_, grp_type, frames) = read_grp_file(&path).unwrap();
        assert_eq!(grp_type, GrpType::Normal);
        assert_eq!(frames[0].image_data.converted_pixels, vec![7, 7, 7, 8, 8, 8]);
        assert_eq!(frames[0].image_data.raw_row_data, vec![vec![0x44, 7], vec![0x00, 0x43, 8]]);
    }


    #[test]
    fn detects_duplicate_frames() {
        let palette = greyscale_palette();
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();

        let file1 = dir.join("frame1.png").to_str().unwrap().to_string();
        let file2 = dir.join("frame2.png").to_str().unwrap().to_string();
        let file3 = dir.join("frame3.png").to_str().unwrap().to_string();

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
    }

    #[test]
    fn does_not_deduplicate_different_frames() {
        let palette = greyscale_palette();
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();

        let file_a = dir.join("frameA.png").to_str().unwrap().to_string();
        let file_b = dir.join("frameB.png").to_str().unwrap().to_string();

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
    }

    /// Creates a PNG on a canvas of palette index 0 (transparent when read back), with a
    /// rectangle of the given colour at the given position.
    fn create_test_png_with_rect(path: &str, canvas: (u32, u32), rect: (u32, u32, u32, u32), colour: [u8; 3]) {
        let (rx, ry, rw, rh) = rect;
        let img = image::RgbImage::from_fn(canvas.0, canvas.1, |x, y| {
            if x >= rx && x < rx + rw && y >= ry && y < ry + rh {
                image::Rgb(colour)
            } else {
                image::Rgb([0, 0, 0])
            }
        });
        img.save(path).expect("Failed to save test PNG");
    }

    #[test]
    fn does_not_deduplicate_frames_with_same_pixels_but_different_dimensions() {
        let palette = greyscale_palette();
        let temp_dir = tempfile::tempdir().unwrap();
        let file_a = temp_dir.path().join("frame_000.png").to_str().unwrap().to_string();
        let file_b = temp_dir.path().join("frame_001.png").to_str().unwrap().to_string();

        // Both have the image data [5, 5, 5, 5, 5, 5]
        create_test_png(&file_a, [5, 5, 5], 2, 3);
        create_test_png(&file_b, [5, 5, 5], 3, 2);

        for compression_type in [
            CompressionType::Normal, CompressionType::Optimised,
            CompressionType::Uncompressed, CompressionType::War1,
        ] {
            let (frames, _, _) = files_to_grp(
                vec![file_a.clone(), file_b.clone()], &palette, &compression_type,
            ).unwrap();

            assert_ne!(frames[0].image_data_offset, frames[1].image_data_offset, "for {}", compression_type);
            assert_eq!((frames[0].width, frames[0].height), (2, 3), "for {}", compression_type);
            assert_eq!((frames[1].width, frames[1].height), (3, 2), "for {}", compression_type);
        }
    }

    #[test]
    fn frames_with_same_pixels_but_different_dimensions_survive_grp_roundtrip() {
        let palette = greyscale_palette();
        let temp_dir = tempfile::tempdir().unwrap();
        let file_a = temp_dir.path().join("frame_000.png").to_str().unwrap().to_string();
        let file_b = temp_dir.path().join("frame_001.png").to_str().unwrap().to_string();
        let grp_path = temp_dir.path().join("out.grp").to_str().unwrap().to_string();

        create_test_png(&file_a, [5, 5, 5], 2, 3);
        create_test_png(&file_b, [5, 5, 5], 3, 2);

        let compression_type = CompressionType::Normal;
        let (frames, max_width, max_height) = files_to_grp(
            vec![file_a, file_b], &palette, &compression_type,
        ).unwrap();
        let header = create_grp_header(&frames, max_width, max_height);
        write_grp_file(&grp_path, &header, &frames, &compression_type).unwrap();

        let (_, _, read_frames) = read_grp_file(&grp_path).unwrap();
        assert_eq!(read_frames.len(), 2);
        assert_eq!((read_frames[0].width, read_frames[0].height), (2, 3));
        assert_eq!((read_frames[1].width, read_frames[1].height), (3, 2));
        assert_eq!(read_frames[0].image_data.converted_pixels, vec![5; 6]);
        assert_eq!(read_frames[1].image_data.converted_pixels, vec![5; 6]);
    }

    #[test]
    fn decoded_width_and_offset_account_for_extended_frames() {
        let palette = greyscale_palette();
        let temp_dir = tempfile::tempdir().unwrap();
        let narrow = temp_dir.path().join("frame_000.png").to_str().unwrap().to_string();
        let wide   = temp_dir.path().join("frame_001.png").to_str().unwrap().to_string();
        create_test_png(&narrow, [7, 7, 7], 10, 2);
        create_test_png(&wide,   [8, 8, 8], 300, 2);

        let (frames, _, _) = files_to_grp(
            vec![narrow, wide], &palette, &CompressionType::Uncompressed,
        ).unwrap();

        // 6 byte header, 2 frame headers of 8 bytes each, then 10x2 bytes of image data for frame 0
        let (frame0_offset, frame1_offset) = (6 + 2 * 8, 6 + 2 * 8 + 10 * 2);

        assert_eq!(frames[0].image_data_offset, frame0_offset);
        assert_eq!(frames[0].decoded_image_data_offset(), frame0_offset);
        assert_eq!(frames[0].decoded_width(), 10);

        assert_eq!(frames[1].image_data_offset, frame1_offset | EXTENDED_OFFSET_BIT);
        assert_eq!(frames[1].decoded_image_data_offset(), frame1_offset);
        assert_eq!(frames[1].width, 44); // 300 - 256
        assert_eq!(frames[1].decoded_width(), 300);
    }

    #[test]
    fn frame_reuse_keys_are_equal_only_for_identical_frames() {
        let image = |x_offset: u8, width: u16, height: u16, pixels: Vec<u8>| PalettizedImageWithMetadata::new(
            palpngrs::Offset::new(x_offset, 0), palpngrs::Size::new(width, height), palpngrs::Size::new(8, 8), pixels,
        );
        let base = image(0, 2, 2, vec![1, 2, 3, 4]);

        for compression_type in [CompressionType::Normal, CompressionType::Uncompressed] {
            let key = |img: &PalettizedImageWithMetadata<u8, u16>| make_frame_reuse_key(&compression_type, img);
            assert!(key(&base) == key(&image(0, 2, 2, vec![1, 2, 3, 4])), "for {}", compression_type);
            assert!(key(&base) != key(&image(0, 2, 2, vec![1, 2, 3, 5])), "for {}", compression_type);
            assert!(key(&base) != key(&image(0, 4, 1, vec![1, 2, 3, 4])), "for {}", compression_type);
        }
        let moved = image(3, 2, 2, vec![1, 2, 3, 4]);
        assert!(make_frame_reuse_key(&CompressionType::Normal,       &base) == make_frame_reuse_key(&CompressionType::Normal,       &moved));
        assert!(make_frame_reuse_key(&CompressionType::Uncompressed, &base) != make_frame_reuse_key(&CompressionType::Uncompressed, &moved));
    }

    #[test]
    fn reuses_frames_at_different_offsets_only_for_normal_grps() {
        let palette = greyscale_palette();
        let temp_dir = tempfile::tempdir().unwrap();
        let file_a = temp_dir.path().join("frame_000.png").to_str().unwrap().to_string();
        let file_b = temp_dir.path().join("frame_001.png").to_str().unwrap().to_string();

        // Same 2x2 image, placed at different positions on the canvas
        create_test_png_with_rect(&file_a, (8, 8), (1, 1, 2, 2), [9, 9, 9]);
        create_test_png_with_rect(&file_b, (8, 8), (4, 5, 2, 2), [9, 9, 9]);

        for (compression_type, should_reuse) in [
            (CompressionType::Normal,       true),
            (CompressionType::Optimised,    true),
            (CompressionType::Uncompressed, false),
            (CompressionType::War1,         false),
        ] {
            let (frames, _, _) = files_to_grp(
                vec![file_a.clone(), file_b.clone()], &palette, &compression_type,
            ).unwrap();

            let reused = frames[0].image_data_offset == frames[1].image_data_offset;
            assert_eq!(reused, should_reuse, "for {}", compression_type);
            assert_eq!((frames[0].x_offset, frames[0].y_offset), (1, 1), "for {}", compression_type);
            assert_eq!((frames[1].x_offset, frames[1].y_offset), (4, 5), "for {}", compression_type);
        }
    }

    /// An image of the given size at the given offset, on a canvas of the given size
    fn test_image(offset: (u8, u8), size: (u16, u16), canvas: (u16, u16)) -> PalettizedImageWithMetadata<u8, u16> {
        PalettizedImageWithMetadata::new(
            palpngrs::Offset::new(offset.0, offset.1),
            palpngrs::Size::new(size.0, size.1),
            palpngrs::Size::new(canvas.0, canvas.1),
            vec![1; size.0 as usize * size.1 as usize],
        )
    }

    #[test]
    fn war1_size_accepts_frame_within_bounds() {
        let image = test_image((50, 60), (100, 80), (200, 200));
        assert!(validate_war1_frame_size(&CompressionType::War1, &image).is_ok());
    }

    #[test]
    fn war1_size_accepts_frame_exactly_at_boundary() {
        // width + x_offset == height + y_offset == canvas width == canvas height == u8::MAX (255).
        // The check rejects only when a value is strictly greater than u8::MAX.
        let image = test_image((55, 105), (200, 150), (255, 255));
        assert!(validate_war1_frame_size(&CompressionType::War1, &image).is_ok());
    }

    #[test]
    fn war1_size_rejects_width_overflow() {
        // 200 + 56 = 256 > 255
        let image = test_image((56, 10), (200, 10), (256, 255));
        let err = validate_war1_frame_size(&CompressionType::War1, &image)
            .expect_err("expected width rejection");
        assert!(matches!(err, Error::CannotEncode(_)));
        assert!(err.to_string().contains("x = 256"));
    }

    #[test]
    fn war1_size_rejects_height_overflow() {
        // 150 + 200 = 350 > 255
        let image = test_image((10, 200), (10, 150), (255, 350));
        let err = validate_war1_frame_size(&CompressionType::War1, &image)
            .expect_err("expected height rejection");
        assert!(matches!(err, Error::CannotEncode(_)));
        assert!(err.to_string().contains("y = 350"));
    }

    #[test]
    fn war1_size_rejects_frame_wider_than_u8() {
        // Allowed for Extended Uncompressed GRPs, but War1 GRPs have no extended width
        let image = test_image((0, 0), (300, 10), (300, 10));
        let err = validate_war1_frame_size(&CompressionType::War1, &image)
            .expect_err("expected width rejection");
        assert!(matches!(err, Error::CannotEncode(_)));
    }

    #[test]
    fn war1_size_rejects_canvas_larger_than_u8_even_if_frame_fits() {
        // The canvas size becomes max_width and max_height of the header, which are single bytes
        for canvas in [(256, 10), (10, 256)] {
            let image = test_image((0, 0), (10, 10), canvas);
            let err = validate_war1_frame_size(&CompressionType::War1, &image)
                .expect_err("expected canvas rejection");
            assert!(matches!(err, Error::CannotEncode(_)), "for canvas {:?}", canvas);
        }
    }

    #[test]
    fn war1_size_ignored_for_non_war1_compression_types() {
        // Same dimensions that fail for War1 must pass for all other compressions,
        // because their headers store max_width and max_height as u16.
        let image = test_image((56, 200), (300, 150), (400, 400));
        for compression in [
            CompressionType::Normal,
            CompressionType::Optimised,
            CompressionType::Uncompressed,
            CompressionType::Auto,
        ] {
            assert!(
                validate_war1_frame_size(&compression, &image).is_ok(),
                "compression {:?} should not enforce the War1 size check",
                compression,
            );
        }
    }

    #[test]
    fn png_to_grpframe_rejects_extended_width_for_war1() {
        let image = test_image((0, 0), (300, 10), (300, 10));
        let result = png_to_grpframe(image, 12, &CompressionType::War1);
        assert!(matches!(result, Err(Error::CannotEncode(_))));
    }

    #[test]
    fn files_to_grp_rejects_war1_png_wider_than_u8() {
        let palette = greyscale_palette();
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("war1_frame_000.png").to_str().unwrap().to_string();
        create_test_png(&file, [9, 9, 9], 300, 10);

        let err = files_to_grp(vec![file.clone()], &palette, &CompressionType::War1)
            .expect_err("expected a 300 pixel wide War1 frame to be rejected");
        assert!(matches!(err.root(), Error::CannotEncode(_)));
        assert!(err.to_string().starts_with(&file));
    }

    #[test]
    fn files_to_grp_rejects_war1_png_with_canvas_wider_than_u8() {
        let palette = greyscale_palette();
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("war1_frame_000.png").to_str().unwrap().to_string();
        // A 10x10 frame in the corner of a 300x10 canvas
        create_test_png_with_rect(&file, (300, 10), (0, 0, 10, 10), [9, 9, 9]);

        let err = files_to_grp(vec![file.clone()], &palette, &CompressionType::War1)
            .expect_err("expected a War1 canvas wider than 255 to be rejected");
        assert!(matches!(err.root(), Error::CannotEncode(_)));
    }

    #[test]
    fn files_to_grp_max_dimensions_include_reused_frames() {
        let palette = greyscale_palette();
        let temp_dir = tempfile::tempdir().unwrap();
        let file_a = temp_dir.path().join("frame_000.png").to_str().unwrap().to_string();
        let file_b = temp_dir.path().join("frame_001.png").to_str().unwrap().to_string();
        // Identical frames, but the second is on a larger canvas
        create_test_png_with_rect(&file_a, (8, 8),   (1, 1, 2, 2), [9, 9, 9]);
        create_test_png_with_rect(&file_b, (12, 10), (1, 1, 2, 2), [9, 9, 9]);

        let (frames, max_width, max_height) = files_to_grp(
            vec![file_a, file_b], &palette, &CompressionType::Normal,
        ).unwrap();

        assert_eq!(frames[0].image_data_offset, frames[1].image_data_offset, "expected the frame to be reused");
        assert_eq!((max_width, max_height), (12, 10));
    }

    #[test]
    fn determine_compression_type_returns_explicit_choice_unchanged() {
        let files = vec!["frame_000.png".to_string()];
        for explicit in [
            CompressionType::Normal,
            CompressionType::Optimised,
            CompressionType::Uncompressed,
            CompressionType::War1,
        ] {
            assert_eq!(determine_compression_type(&files, &explicit), explicit);
        }
    }

    #[test]
    fn determine_compression_type_detects_war1_prefix_in_file_name() {
        let files = vec!["sprites/war1_frame_000.png".to_string()];
        assert_eq!(
            determine_compression_type(&files, &CompressionType::Auto),
            CompressionType::War1,
        );
    }

    #[test]
    fn determine_compression_type_detects_uncompressed_prefix_in_file_name() {
        let files = vec!["sprites/uncompressed_frame_000.png".to_string()];
        assert_eq!(
            determine_compression_type(&files, &CompressionType::Auto),
            CompressionType::Uncompressed,
        );
    }

    #[test]
    fn determine_compression_type_requires_the_underscore() {
        for name in ["uncompressed.png", "uncompressedframe_000.png", "war1.png", "war1frame_000.png"] {
            let files = vec![format!("sprites/{}", name)];
            assert_eq!(
                determine_compression_type(&files, &CompressionType::Auto),
                CompressionType::Normal,
                "for {}", name,
            );
        }
    }

    #[test]
    fn determine_compression_type_matches_anywhere_in_the_file_name() {
        for (name, expected) in [
            ("my_uncompressed_frame.png", CompressionType::Uncompressed),
            ("orc_war1_frame_000.png",    CompressionType::War1),
        ] {
            let files = vec![format!("sprites/{}", name)];
            assert_eq!(determine_compression_type(&files, &CompressionType::Auto), expected, "for {}", name);
        }
    }

    #[test]
    fn determine_compression_type_uses_any_matching_file_and_prefers_uncompressed() {
        let files = vec![
            "sprites/frame_000.png".to_string(),
            "sprites/war1_frame_001.png".to_string(),
            "sprites/uncompressed_frame_002.png".to_string(),
        ];
        assert_eq!(
            determine_compression_type(&files, &CompressionType::Auto),
            CompressionType::Uncompressed,
        );
        assert_eq!(
            determine_compression_type(&files[..2], &CompressionType::Auto),
            CompressionType::War1,
        );
    }

    #[test]
    fn determine_compression_type_defaults_to_normal_for_plain_file_names() {
        let files = vec!["sprites/frame_000.png".to_string()];
        assert_eq!(
            determine_compression_type(&files, &CompressionType::Auto),
            CompressionType::Normal,
        );
    }

    #[test]
    fn determine_compression_type_ignores_match_in_ancestor_directory() {
        // Regression test: a directory named war1_sprites or uncompressed_dumps
        // must not force a compression type when the file names themselves are plain.
        let war1_dir = vec!["/home/me/war1_sprites/frame_000.png".to_string()];
        assert_eq!(
            determine_compression_type(&war1_dir, &CompressionType::Auto),
            CompressionType::Normal,
        );

        let uncompressed_dir = vec!["/home/me/uncompressed_dumps/frame_000.png".to_string()];
        assert_eq!(
            determine_compression_type(&uncompressed_dir, &CompressionType::Auto),
            CompressionType::Normal,
        );
    }


    fn perform_row_tests(test_cases: Vec<Vec<u8>>) {
        for row in test_cases {
            let encoded_normal = encode_grp_rle_row(&row, &CompressionType::Normal);
            let encoded_optim  = encode_grp_rle_row(&row, &CompressionType::Optimised);
            let (decoded_normal, encoded_normal_length) = decode_valid_row(&encoded_normal, row.len() as u16);
            let (decoded_optim , encoded_optim_length)  = decode_valid_row(&encoded_optim,  row.len() as u16);

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
    // Width range covers from empty rows up to the EXTENDED_IMAGE_WIDTH (256) limit, exercising
    // the literal-copy run-length cap (63) several times over. Both Normal and Optimised
    // thresholds are tested so the more aggressive Optimised path (threshold 2) is also fuzzed.
    proptest! {
        #[test]
        fn prop_encode_decode_roundtrip_normal(row in proptest::collection::vec(0u8..=255, 0..256)) {
            let width = row.len();
            let encoded = encode_grp_rle_row(&row, &CompressionType::Normal);
            let (decoded, encoded_length) = decode_valid_row(&encoded, width as u16);
            prop_assert_eq!(decoded, row);
            prop_assert_eq!(encoded_length, encoded.len());
        }

        #[test]
        fn prop_encode_decode_roundtrip_optimised(row in proptest::collection::vec(0u8..=255, 0..256)) {
            let width = row.len();
            let encoded = encode_grp_rle_row(&row, &CompressionType::Optimised);
            let (decoded, encoded_length) = decode_valid_row(&encoded, width as u16);
            prop_assert_eq!(decoded, row);
            prop_assert_eq!(encoded_length, encoded.len());
        }
    }
}
