#ifndef DSCAPTURE_H
#define DSCAPTURE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Static version string; do not free. */
const char *dscapture_version(void);

/*
 * Parse a PDF file. options_json may be NULL. The result is a UTF-8 JSON
 * datasheet or {"error":"..."}. Free it with dscapture_free_string().
 */
char *dscapture_parse_file_json(const char *input_path,
                                const char *options_json);

/* filename_hint and options_json may be NULL. */
char *dscapture_parse_bytes_json(const uint8_t *data,
                                 size_t length,
                                 const char *filename_hint,
                                 const char *options_json);

void dscapture_free_string(char *value);

#ifdef __cplusplus
}
#endif

#endif
