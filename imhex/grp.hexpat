import std.io;

struct GrpHeader {
    u16 frame_count;
    u16 max_width;
    u16 max_height;
};
GrpHeader grpHeader @ 0x00;

struct RowOffset {
    u16 row_offset;
};

struct GrpFrameHeader {
    u8 x_offset [[color("FFF")]];
    u8 y_offset [[color("000")]];
    u8 width  [[color("FF00FF")]];
    u8 height [[color("00FFFF")]];
    u32 image_data_offset [[color("FAFAFA")]];
    RowOffset row_offset[height] @ image_data_offset;
};

GrpFrameHeader grpFrameHeader[grpHeader.frame_count] @ 0x06;

std::print("Frames {} ", grpHeader.frame_count);
