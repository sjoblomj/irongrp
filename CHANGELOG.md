# Changelog

All notable changes to this project will be documented in this file.

## [0.6.0] - unreleased

### Breaking changes
- The command-line interface uses subcommands instead of `--mode`, and takes the input and output paths as positional arguments:
  - `--mode grp-to-png --input-path X --output-path Y` is now `grp-to-png X Y`
  - `--mode png-to-grp --input-path X --output-path Y` is now `png-to-grp X Y`
  - `--mode analyse-grp --input-path X` is now `analyse X` (`analyse-grp` is accepted as an alias)
  - `--generate-shell-completions SHELL` is now `completions SHELL`

  Flags that do not apply to a subcommand are now rejected, instead of silently ignored.
- Renamed flags: `--pal-path` to `--palette`, `--use-transparency` to `--transparent`, `--frame-number` to `--frame`, `--compression-type` to `--compression` and `--analyse-row-number` to `--row`. The old names are still accepted. New short forms: `-c` for `--compression`, `-f` for `--frame` and `-r` for `--row`.
- Log messages are written to stderr instead of stdout.
- The `analyse` report is written to stdout, independently of `--log-level`, so it can be piped or redirected. Previously parts of it were hidden at `--log-level warn`, and overlapping ranges and the file layout diagram were only shown at `--log-level debug`. Overlapping ranges are now always reported, and the file layout diagram is printed with the new `--layout` flag.

### Added
- Ability to specify and load palettes (up to 9) in the yazi integration.
- Using `tempfile` for tests for added robustness.
- .gitignore file
- GitHub Actions workflows: CI running clippy and the tests on Linux, macOS and Windows, and releases building binaries for Linux, Windows and macOS when a version tag is pushed.
- Icon for the Windows executable.

### Changed
- Better handling of argument validation by using clap.
- Reuse the open file handle in `detect_uncompressed` instead of reopening the GRP a second time.
- Restrict the auto-detect for War1 and Uncompressed to the PNG file names, so a parent directory named like `war1_sprites/` no longer changes the chosen format.
- Updated `palpngrs` to 0.3.0.
- Errors are now reported as a single readable log line naming the file involved, instead of a Rust debug dump, and the program exits with status 1. Internally, the crate uses its own `irongrp::Error` type instead of `std::io::Error`.
- Converting a directory without any PNGs to a GRP is now an error, instead of silently writing an empty GRP.

### Bug fixes
- Corrected bug where specifying a frame to output with `--frame-number` would output all frames except that one.
- Corrected bug where the file layout diagram would not be printed.
- Corrected bug related to boundary checks for Warcraft I style GRPs.
- Fix off-by-one in the PNG width check that rejected 511-pixel-wide images, the actual maximum for extended uncompressed GRPs.
- Corrected crash when analysing a frame number equal to the number of frames, and a row number equal to the frame height is now rejected instead of silently ignored.
- Corrected crash (or, in release builds, a silently corrupt GRP) when converting a large PNG that compresses poorly, such as a 255x255 frame with no repeated pixels. Such frames are now rejected with an error, since their row offsets do not fit in the GRP format.
- Corrected bug where two PNGs with the same pixel data but different dimensions (e.g. a 2x3 and a 3x2 frame of the same colour) were treated as identical when creating a Normal GRP, so the second frame got the dimensions of the first.
- Frames are now only reused when creating a GRP if their data is actually identical, rather than when a 64-bit hash of it matches. The reports of identical frames in `grp-to-png` and `analyse-grp` likewise compare the frames themselves, are listed in frame order, and `analyse-grp` no longer reports frames with the same pixels but different dimensions as identical.
- Corrected bug where `analyse-grp` failed with "failed to fill whole buffer" on Extended Uncompressed GRPs, since the extended bit in the image data offset was treated as part of the offset. The printed image data offsets of such frames are now also correct.
- Corrected bug where creating a WarCraft I style GRP from a PNG wider than 255 pixels gave a broken GRP, with a truncated width in the header and an extended frame width that WarCraft I GRPs do not support. Such PNGs, and PNGs whose canvas is larger than 255x255, are now rejected with an error.
- The max width and height in the header of a created GRP now also take into account the canvas size of frames that reuse the image data of an earlier frame.
- Corrected crash (or, in release builds, reading the wrong frame headers) when reading GRPs with more than 8191 frames.
- Corrected bug where converting PNGs from a GRP with more than 1000 frames back to a GRP put the frames in the wrong order, since `frame_1000.png` was sorted before `frame_999.png`. PNG files are now sorted with numbers in their names ordered by value.
- Corrected bug where the generated shell completion script started with a log line, which broke it when redirected to a file. The message is now written to stderr.
- Corrected bug where `grp-to-png` with a `--frame-number` beyond the last frame succeeded without writing anything. It is now an error, as it already was for `analyse-grp`.
- Corrected bug where `analyse-grp` assumed a 6 byte header for WarCraft I style GRPs, whose header is 4 bytes, so the file layout and the overlap check were off by 2 bytes.
- Corrected bug where `analyse-grp --frame-number` printed wrong absolute row offsets for frames whose image data is more than 64 KiB into the file, or crashed in debug builds.
- Corrected crash in debug builds when analysing a frame of the maximum height of 255 rows with `analyse-grp --frame-number`.
- Corrected the detection of Uncompressed GRPs: it could crash in debug builds on GRPs with very large total frame sizes, and misdetected Uncompressed GRPs whose image data is not stored in frame order as Normal.
- Corrected bug where a GRP whose frame headers could also be read in the WarCraft I style layout, but whose data was not uncompressed in it, was read as a Normal GRP with the max width and height from the WarCraft I style header. WarCraft I style GRPs are now only detected as such if they are uncompressed.
- Corrected bug where `grp-to-png` failed, possibly after writing some of the PNGs, on GRPs whose header gives a max width or height smaller than the frames extend to. The PNGs are now enlarged to fit all frames, with a warning.
- GRPs with 0 frames are now rejected with the error "The GRP has no frames". Previously, such files were rejected as too short if they were 6 bytes, while longer files beginning with two zero bytes, such as many non-GRP game files, were accepted as GRPs.
- Stricter validation of the frame headers when reading a GRP: image data offsets pointing into the header or the frame header table, at the end of the file, or too close to the end of the file to hold the frame's image data are now rejected.
- Malformed image data in Normal GRPs is now reported with one warning per frame, naming the problems and the rows affected, instead of one error log line per problem. The image is still decoded as well as possible.
- Corrected the decoding of instructions to copy 0 pixels in malformed GRPs, which skipped the following byte.
- Converting PNGs to a GRP now fails with an error naming the file if a PNG's file name is not valid UTF-8, instead of silently leaving that frame out of the GRP.
- Corrected the description of the automatic compression type detection in `--help` and the README: the PNG file names must contain "uncompressed_" or "war1_", including the underscore.
- WarCraft I style GRPs with frames of extended width, which neither the games nor IronGRP create, are no longer accepted. They were read with the frames named as Uncompressed, so converting back gave an Uncompressed GRP. When a GRP can be read neither as WarCraft I style nor normally, the error now explains why for both.
- Corrected bug where `analyse-grp` reported overlapping ranges for frames sharing image data, and for rows of a frame sharing row data, which are valid ways of saving space. Such data is now listed once in the file layout, labelled with all the frames or rows using it. The overlap check now also finds ranges overlapping an earlier range other than the one just before.
- Removed the silent-truncation safety_break fallback in the RLE encoder and added proptest coverage for Optimised compression.



## [0.5] - 2025-06-19

### Added
- ImHex pattern language definitions for Normal GRPs, Uncompressed GRPs and WarCraft I style GRPs.
- Added yazi integration.
- Included fallback greyscale palette.
- Added a Readme file.
- Added a logo.

### Changed
- Moved the PNG handling to an external library.
- Better logging: Introduced logging library and the log level 'trace'.

### Removed
- Removed some Optimisation schemes from CompressionType Optimised. This makes it slightly less optimised but identical to how Blizzard did it, and the code is less complex.



## [0.4] - 2025-05-10

### Added
- Support for Extended Uncompressed GRPs. This allows for Uncompressed GRPs to have frames with a width up to 512 pixels.
- Support for WarCraft I Uncompressed GRPs.
- Shell completion.
- More tests.
- Boundary checks for width, height, offsets and frame count.

### Changed
- When extracting PNGs from an Uncompressed GRP or WarCraft I style GRP, IronGRP will now name them "uncompressed_frame_xxx.png" or "war1_frame_xxx.png", respectively. When converting PNGs to GRP, if no CompressionType is given (or if CompressionType Auto is given), IronGRP will create an Uncompressed GRP if any of the input filenames contains "uncompressed", and create a WarCraft I style GRP if any of the input filenames contains "war1".
- Renamed the values of the CompressionType to make more sense.
- Some refactoring to make code more reusable.



## [0.3] - 2025-04-19

### Added
- Support for converting to and from Uncompressed GRPs.
- Will now print which frames are identical when extracting frames from a GRP.
- Caching of palette lookups. This gives a speedup of over 80% on larger GRPs.

### Changed
- If requesting a tiled image with a max-width that is too low to fit one frame, the resulting image will now be 1 column wide and as big as the frame. Previously, it would in this case behave as if no max-width was given.



## [0.2] - 2025-04-15

### Added
- Now detects duplicated GRP frames and reuses them to save space.
- Will now reuse data row overlaps when using CompressionType Optimised.
- Added `--frame-number` and `--analyse-row-number` options. The former allows for only outputting the given frame, or to do more thorough analysis of the given frame. The latter is for analysing a specific row in the given frame.

### Changed
- Fixed a bug where the decoding would sometimes be fed too little data and thus decode incorrectly.
- Fixed an integer overflow bug.
- Fixed bug where reused frames would erroneously reuse offsets.
- Made the encoding algorithm closer to Blizzard's original algorithm in some cases.
- More efficient IO handling of GRP files.
- Now handles PNGs with alpha channels, in the sense that fully transparent pixels will be set to use palette index 0, and any non-opaque pixels will have its alpha value ignored.
- Improved and more consistent logging. Also prints how much time an operation took.



## [0.1] - 2025-04-03

### Added
- First version of program. Can convert from GRP to PNGs, and from PNGs to GRP. Can create tiled PNGs. Can analyse GRPs.
