use clap::{Parser, Subcommand, ValueEnum, ValueHint};
use clap_complete::Shell;
use simplelog::LevelFilter;
use std::cmp::Ordering;
use std::fmt;
use std::fs;

pub mod analyse;
pub mod error;
pub mod grp;
pub mod png;

pub use error::{Error, Result};
use error::InFile;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Logging level
    #[arg(long, value_enum, default_value_t = LogLevel::Info, global = true)]
    pub log_level: LogLevel,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Convert a GRP file to PNG images
    GrpToPng(GrpToPngArgs),

    /// Convert a directory of PNG images to a GRP file
    PngToGrp(PngToGrpArgs),

    /// Inspect the structure of a GRP file
    #[command(visible_alias = "analyse-grp")]
    Analyse(AnalyseArgs),

    /// Generate shell completions and write them to stdout
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(clap::Args)]
pub struct GrpToPngArgs {
    /// Path to the GRP file
    #[arg(value_hint = ValueHint::FilePath)]
    pub input: String,

    /// Directory to write the PNG files to. It is created if it does not exist
    #[arg(value_hint = ValueHint::DirPath)]
    pub output: String,

    /// Path to the palette file. A greyscale palette is used if omitted
    #[arg(long, short = 'p', alias = "pal-path", value_hint = ValueHint::FilePath)]
    pub palette: Option<String>,

    /// Output all frames in one image. GRPs cannot be
    /// created back from tiled images.
    #[arg(long)]
    pub tiled: bool,

    /// Maximum width in pixels of the tiled image.
    /// If this is less than the maximum frame width of
    /// the GRP itself, this value will be ignored.
    #[arg(long, requires = "tiled")]
    pub max_width: Option<u32>,

    /// Only output the given frame number (0-indexed)
    #[arg(long, short = 'f', alias = "frame-number", conflicts_with = "tiled")]
    pub frame: Option<u16>,

    /// Make the background of the PNG images transparent.
    /// The default behaviour is to use the colour of
    /// index 0 in the palette.
    #[arg(long, alias = "use-transparency")]
    pub transparent: bool,
}

#[derive(clap::Args)]
pub struct PngToGrpArgs {
    /// Directory containing the PNG files. All PNGs in it are used, in natural sort order
    #[arg(value_hint = ValueHint::DirPath)]
    pub input: String,

    /// Path of the GRP file to write
    #[arg(value_hint = ValueHint::FilePath)]
    pub output: String,

    /// Path to the palette file. A greyscale palette is used if omitted
    #[arg(long, short = 'p', alias = "pal-path", value_hint = ValueHint::FilePath)]
    pub palette: Option<String>,

    /// Compression type to use when creating GRP files.
    /// If omitted or set to 'auto', it will use 'normal'
    /// compression, unless any of the input PNG file names
    /// contains "uncompressed_" or "war1_", as in the names
    /// of PNGs created from such GRPs. If so, it will use the
    /// corresponding compression, with "uncompressed_" taking
    /// precedence if both are found.
    #[arg(long, short = 'c', alias = "compression-type", value_enum, default_value_t = CompressionType::Auto)]
    pub compression: CompressionType,
}

#[derive(clap::Args)]
pub struct AnalyseArgs {
    /// Path to the GRP file
    #[arg(value_hint = ValueHint::FilePath)]
    pub input: String,

    /// Only analyse the given frame number (0-indexed)
    #[arg(long, short = 'f', alias = "frame-number")]
    pub frame: Option<u16>,

    /// Print the data of the given row number (0-indexed) of the given frame
    #[arg(long, short = 'r', alias = "analyse-row-number", requires = "frame")]
    pub row: Option<u8>,
}

#[derive(Clone, ValueEnum, PartialEq, Debug)]
pub enum CompressionType {
    Normal,
    Optimised,
    Uncompressed,
    War1,
    Auto,
}

#[derive(Clone, ValueEnum, Debug)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}
impl fmt::Display for CompressionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl From<LogLevel> for LevelFilter {
    fn from(level: LogLevel) -> LevelFilter {
        match level {
            LogLevel::Error => LevelFilter::Error,
            LogLevel::Warn  => LevelFilter::Warn,
            LogLevel::Info  => LevelFilter::Info,
            LogLevel::Debug => LevelFilter::Debug,
            LogLevel::Trace => LevelFilter::Trace,
        }
    }
}

/// Returns all PNG files in the given directory.
pub fn list_png_files(dir: &str) -> Result<Vec<String>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).in_file(dir)? {
        let path = entry.in_file(dir)?.path();
        let is_png = path.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png"));
        if is_png {
            let Some(path_str) = path.to_str() else {
                return Err(Error::InvalidArgument(format!(
                    "The file name of '{}' is not valid UTF-8, which is not supported. Please rename it.",
                    path.display(),
                )));
            };
            entries.push(path_str.to_string());
        }
    }

    if entries.is_empty() {
        return Err(Error::InvalidArgument(format!("No PNG files found in directory '{}'", dir)));
    }
    if entries.len() > u16::MAX as usize {
        return Err(Error::InvalidArgument(format!(
            "Too many PNGs found in directory '{}'! Found {} PNGs, but cannot handle more than {}",
            dir, entries.len(), u16::MAX)))
    }
    entries.sort_by(|a, b| natural_cmp(a, b));
    Ok(entries)
}

/// Checks that the frame number, if given, refers to one of the `frame_count` frames.
pub(crate) fn validate_frame_number(frame_number: Option<u16>, frame_count: usize) -> Result<()> {
    match frame_number {
        Some(n) if n as usize >= frame_count => Err(Error::InvalidArgument(format!(
            "Frame number {} is out of range; the GRP has {} frame(s)", n, frame_count,
        ))),
        _ => Ok(()),
    }
}

/// Compares strings so that runs of digits are ordered by their numeric value, so that e.g.
/// "frame_999.png" comes before "frame_1000.png". Strings that only differ in leading zeros,
/// such as "frame_01" and "frame_1", fall back to ordinary string comparison.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a_rest, mut b_rest) = (a, b);
    loop {
        let (Some(a_char), Some(b_char)) = (a_rest.chars().next(), b_rest.chars().next()) else {
            return a_rest.len().cmp(&b_rest.len()).then_with(|| a.cmp(b));
        };
        let ordering = if a_char.is_ascii_digit() && b_char.is_ascii_digit() {
            let (a_digits, a_tail) = split_leading_digits(a_rest);
            let (b_digits, b_tail) = split_leading_digits(b_rest);
            a_rest = a_tail;
            b_rest = b_tail;
            let (a_num, b_num) = (a_digits.trim_start_matches('0'), b_digits.trim_start_matches('0'));
            a_num.len().cmp(&b_num.len()).then_with(|| a_num.cmp(b_num))
        } else {
            a_rest = &a_rest[a_char.len_utf8()..];
            b_rest = &b_rest[b_char.len_utf8()..];
            a_char.cmp(&b_char)
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
}

fn split_leading_digits(s: &str) -> (&str, &str) {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    s.split_at(end)
}

const UNCOMPRESSED_FILENAME: &str = "uncompressed";
const WAR1_FILENAME: &str = "war1";


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_frame_number_accepts_frames_in_range_or_no_frame() {
        assert!(validate_frame_number(None,    0).is_ok());
        assert!(validate_frame_number(None,    3).is_ok());
        assert!(validate_frame_number(Some(0), 3).is_ok());
        assert!(validate_frame_number(Some(2), 3).is_ok());
    }

    #[test]
    fn validate_frame_number_rejects_frames_out_of_range() {
        for (frame_number, frame_count) in [(3, 3), (7, 3), (0, 0), (u16::MAX, u16::MAX as usize)] {
            let err = validate_frame_number(Some(frame_number), frame_count)
                .expect_err("expected the frame number to be rejected");
            assert!(matches!(err, Error::InvalidArgument(_)));
            assert!(err.to_string().contains(&format!("the GRP has {} frame(s)", frame_count)));
        }
    }

    #[test]
    fn natural_cmp_orders_numbers_by_value() {
        assert_eq!(natural_cmp("frame_999.png",  "frame_1000.png"), Ordering::Less);
        assert_eq!(natural_cmp("frame_1000.png", "frame_999.png"),  Ordering::Greater);
        assert_eq!(natural_cmp("frame_002.png",  "frame_010.png"),  Ordering::Less);
        assert_eq!(natural_cmp("2.png",          "10.png"),         Ordering::Less);
        assert_eq!(natural_cmp("frame_10_b",     "frame_10_a"),     Ordering::Greater);
    }

    #[test]
    fn natural_cmp_compares_non_digits_as_usual() {
        assert_eq!(natural_cmp("a",       "b"),       Ordering::Less);
        assert_eq!(natural_cmp("frame",   "frame_1"), Ordering::Less);
        assert_eq!(natural_cmp("frame_1", "frame"),   Ordering::Greater);
        assert_eq!(natural_cmp("same_1",  "same_1"),  Ordering::Equal);
        assert_eq!(natural_cmp("åäö_2",   "åäö_10"),  Ordering::Less);
    }

    #[test]
    fn natural_cmp_is_a_total_order_for_leading_zeros() {
        // Numerically equal, so they fall back to string comparison rather than being Equal
        assert_eq!(natural_cmp("frame_01", "frame_1"),  Ordering::Less);
        assert_eq!(natural_cmp("frame_1",  "frame_01"), Ordering::Greater);
        // Leading zeros only decide when the strings are otherwise equal
        assert_eq!(natural_cmp("frame_1a", "frame_01b"), Ordering::Less);
    }

    #[test]
    fn natural_cmp_handles_numbers_too_large_for_integers() {
        assert_eq!(natural_cmp("99999999999999999999999", "100000000000000000000000"), Ordering::Less);
    }

    #[test]
    // Linux only: macOS does not allow file names that are not valid UTF-8, nor does Windows
    #[cfg(target_os = "linux")]
    fn list_png_files_rejects_png_whose_name_is_not_valid_utf8() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("frame_000.png"), []).unwrap();
        let invalid_name = OsStr::from_bytes(b"frame_\xFF01.png");
        std::fs::write(temp_dir.path().join(invalid_name), []).unwrap();

        let err = list_png_files(temp_dir.path().to_str().unwrap())
            .expect_err("expected the PNG with an invalid UTF-8 name to be rejected");
        assert!(matches!(err, Error::InvalidArgument(_)));
        assert!(err.to_string().contains("frame_\u{FFFD}01.png"), "{}", err);
        assert!(err.to_string().contains("not valid UTF-8"), "{}", err);
    }

    #[test]
    // Linux only: macOS does not allow file names that are not valid UTF-8, nor does Windows
    #[cfg(target_os = "linux")]
    fn list_png_files_ignores_non_png_whose_name_is_not_valid_utf8() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("frame_000.png"), []).unwrap();
        std::fs::write(temp_dir.path().join(OsStr::from_bytes(b"notes_\xFF.txt")), []).unwrap();
        std::fs::write(temp_dir.path().join(OsStr::from_bytes(b"image.\xFFpng")), []).unwrap();

        let files = list_png_files(temp_dir.path().to_str().unwrap()).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("frame_000.png"));
    }

    #[test]
    fn list_png_files_accepts_any_case_of_the_png_extension() {
        let temp_dir = tempfile::tempdir().unwrap();
        for name in ["a.png", "b.PNG", "c.Png", "d.png.txt", "e"] {
            std::fs::write(temp_dir.path().join(name), []).unwrap();
        }
        let files = list_png_files(temp_dir.path().to_str().unwrap()).unwrap();
        let names: Vec<&str> = files.iter()
            .map(|f| std::path::Path::new(f).file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, vec!["a.png", "b.PNG", "c.Png"]);
    }

    #[test]
    fn list_png_files_orders_frames_numerically() {
        let temp_dir = tempfile::tempdir().unwrap();
        // The same naming scheme as when writing PNGs, with more frames than 3 digits can hold
        for i in (0..=1001).rev() {
            std::fs::write(temp_dir.path().join(format!("frame_{:03}.png", i)), []).unwrap();
        }
        std::fs::write(temp_dir.path().join("not_a_png.txt"), []).unwrap();

        let files = list_png_files(temp_dir.path().to_str().unwrap()).unwrap();

        let names: Vec<String> = files.iter()
            .map(|f| std::path::Path::new(f).file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        let expected: Vec<String> = (0..=1001).map(|i| format!("frame_{:03}.png", i)).collect();
        assert_eq!(names, expected);
    }
}
