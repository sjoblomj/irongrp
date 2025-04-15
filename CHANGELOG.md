# Changelog

All notable changes to this project will be documented in this file.


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
