# Changelog

All notable changes to this project will be documented in this file.

## [0.6] - unreleased

### Added
- Ability to specify and load palettes (up to 9) in the yazi integration.
- Using `tempfile` for tests for added robustness.
- .gitignore file

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
