# dscapture

`dscapture` converts an electronic component PDF datasheet into typed JSON. The parser is designed for documents from different manufacturers and does not depend on a fixed page template.

The CLI also generates a compact, datasheet-derived smoke/derating constraint profile next to the JSON output. For example, `output.json` is accompanied by `output.cir`. The profile is intended to be included in a larger simulation flow and processed by a dscapture-aware preprocessor.

Extraction pipeline:

1. Poppler extracts text while preserving the physical layout (`pdftotext -layout`).
2. If Poppler is unavailable, the built-in pure-Rust backend based on `lopdf` is used.
3. Pages without a usable text layer are automatically rasterized with `pdftoppm` and recognized by Tesseract.
4. When the `opencv-preprocessing` feature is enabled, OpenCV median denoising and adaptive Gaussian thresholding are applied before OCR.
5. The heuristic layer recognizes general information, packages, pin tables, Absolute Maximum Ratings, Recommended Operating Conditions, Electrical Characteristics, and Thermal Characteristics. Electrical tables continued across multiple pages are combined automatically. The original text of every rating entry is retained in the `source` field, allowing partially recognized tables to be reviewed or processed further.

## Build and run

The `poppler-utils` and `tesseract-ocr` system packages are recommended for regular PDF documents:

```bash
cargo build --release
./target/release/dscapture input.pdf output.json
```

This command writes both `output.json` and `output.cir`.

Useful options:

```bash
# Disable OCR and process at most 64 pages
dscapture input.pdf output.json --no-ocr --max-pages 64

# Force OCR for a scanned document
dscapture scan.pdf output.json --backend ocr --ocr-max-pages 20 --ocr-dpi 350

# Process the entire document (the default limit is 128 pages)
dscapture input.pdf output.json --max-pages 0

# Select another smoke-profile path and derating policy
dscapture input.pdf output.json --smoke-output limits.smoke --derate 0.75

# Override the name following .smoke and the default case-temperature reference
dscapture input.pdf output.json --smoke-device M1 --reference-temperature 50

# Generate JSON only
dscapture input.pdf output.json --no-smoke
```

`--backend` accepts `auto`, `poppler`, `native`, or `ocr`. Tesseract languages are selected with `--ocr-language`, for example `eng+deu`.

## OpenCV for difficult scans

After installing the OpenCV and libclang development packages, build the project as follows (on Debian/Ubuntu, install `libopencv-dev` and `libclang-dev`):

```bash
cargo build --release --features opencv-preprocessing
```

OpenCV is used only for OCR pages and adds no overhead when processing PDFs with a usable text layer.

## Smoke/derating constraint profile

The generated file is intentionally compact:

```text
- Generated from datasheet
- Custom dscapture directive — requires preprocessing
.smoke MOSFET_NAME
- VDS_MAX=650
- VGS_POS_MAX=20
- VGS_NEG_MAX=-20
- ID_CONT_MAX=60 TC_REF=25
- ID_PULSE_MAX=240
- PD_MAX=150 TC_REF=25
- TJ_MAX=175
- RTH_JC=1
- DERATE=0.8
```

Only values found in the parsed datasheet are emitted. Missing MOSFET fields are omitted rather than invented. For ICs and other parts with per-pin limits, the generator emits generic constraints such as `VIN_MIN`, `VIN_MAX`, or `VBST_MAX`. Units are normalized to V, A, W, °C, and °C/W.

`DERATE` is a project policy and defaults to `0.8`; the raw datasheet limits remain unchanged. `TC_REF` is taken from a rating's conditions when available and otherwise defaults to 25 °C. Both values can be overridden:

```bash
dscapture input.pdf output.json --derate 0.7 --reference-temperature 50
```

The `.smoke` directive and the `- KEY=VALUE` records are custom dscapture syntax, not native NGSpice statements. They require preprocessing before the profile is included in a stock NGSpice netlist.

## Rust API

```rust
use dscapture::{ParseOptions, SmokeOptions, parse_file, to_json, to_smoke_profile};

let result = parse_file("input.pdf", &ParseOptions::default())?;
println!("{}", to_json(&result, true)?);
println!("{}", to_smoke_profile(&result, &SmokeOptions::default())?);
# Ok::<(), dscapture::Error>(())
```

`parse_bytes` accepts a PDF from memory and is convenient for servers and plugins.

## Shared library / C ABI

Because `crate-type = ["rlib", "cdylib"]`, a release build produces `target/release/libdscapture.so`. The C header is located at `include/dscapture.h`.

The C ABI provides JSON functions and the corresponding `dscapture_parse_file_smoke` / `dscapture_parse_bytes_smoke` functions. The older `*_ngspice` symbols remain as compatibility aliases and now return the same compact smoke profile.

```c
#include <stdio.h>
#include "dscapture.h"

int main(void) {
    const char *options = "{\"max_pages\":64,\"ocr_enabled\":true}";
    char *json = dscapture_parse_file_json("input.pdf", options);
    puts(json);
    dscapture_free_string(json);
    return 0;
}
```

Fields that are missing from the source document are neither inferred nor included in the output. `metadata.confidence` is the fraction of successfully populated semantic groups, not a statistical probability. For very long reference manuals, `metadata.warnings` explicitly reports the applied page limit.
