#include <limits.h>
#include <setjmp.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#include "jerror.h"
#include "jpeglib.h"

enum decode_status { DECODE_OK, DECODE_FORMAT, DECODE_ALLOCATION, DECODE_UNSUPPORTED };

struct decoded_jpeg {
    uint8_t *rgb;
    uint32_t width;
    uint32_t height;
};

struct decoder {
    struct jpeg_decompress_struct image;
    struct jpeg_error_mgr errors;
    jmp_buf recovery;
    uint8_t *rgb;
};

static void fatal_error(j_common_ptr info)
{
    struct decoder *decoder = info->client_data;
    int status = info->err->msg_code == JERR_OUT_OF_MEMORY ? DECODE_ALLOCATION : DECODE_FORMAT;
    longjmp(decoder->recovery, status);
}

static void emit_message(j_common_ptr info, int level)
{
    /* An exhausted memory source inserts a synthetic EOI. Reject that image. */
    if (level < 0 && info->err->msg_code == JWRN_JPEG_EOF)
        fatal_error(info);
}

static int decode(struct decoder *decoder, const uint8_t *bytes, unsigned long length,
                  struct decoded_jpeg *output)
{
    struct jpeg_decompress_struct *image = &decoder->image;
    int error = setjmp(decoder->recovery);
    if (error != 0)
        return error;

    image->err = jpeg_std_error(&decoder->errors);
    image->err->error_exit = fatal_error;
    image->err->emit_message = emit_message;
    image->client_data = decoder;
    jpeg_create_decompress(image);
    jpeg_mem_src(image, bytes, length);
    if (jpeg_read_header(image, TRUE) != JPEG_HEADER_OK)
        return DECODE_FORMAT;

    switch (image->jpeg_color_space) {
    case JCS_GRAYSCALE:
    case JCS_RGB:
    case JCS_YCbCr:
        image->out_color_space = JCS_RGB;
        break;
    default:
        return DECODE_UNSUPPORTED;
    }

    jpeg_calc_output_dimensions(image);
    uint32_t width = image->output_width;
    uint32_t height = image->output_height;
    const size_t sample_limit = 512u * 1024u * 1024u;
    if (width == 0 || height == 0)
        return DECODE_FORMAT;
    if (sample_limit / 3u / width < height)
        return DECODE_ALLOCATION;
    size_t stride = (size_t)width * 3u;
    decoder->rgb = malloc(stride * height);
    if (decoder->rgb == NULL)
        return DECODE_ALLOCATION;

    if (!jpeg_start_decompress(image) || image->output_components != 3)
        return DECODE_FORMAT;
    while (image->output_scanline < height) {
        JSAMPROW row = decoder->rgb + image->output_scanline * stride;
        if (jpeg_read_scanlines(image, &row, 1) != 1)
            return DECODE_FORMAT;
    }
    if (!jpeg_finish_decompress(image))
        return DECODE_FORMAT;

    *output = (struct decoded_jpeg){decoder->rgb, width, height};
    decoder->rgb = NULL;
    return DECODE_OK;
}

int mycela_ijg_decode(const uint8_t *bytes, size_t length, struct decoded_jpeg *output)
{
    if (length == 0 || length > ULONG_MAX)
        return DECODE_FORMAT;
    struct decoder *decoder = calloc(1, sizeof(*decoder));
    if (decoder == NULL)
        return DECODE_ALLOCATION;
    int status = decode(decoder, bytes, (unsigned long)length, output);
    jpeg_destroy_decompress(&decoder->image);
    free(decoder->rgb);
    free(decoder);
    return status;
}

void mycela_ijg_free(uint8_t *rgb)
{
    free(rgb);
}
