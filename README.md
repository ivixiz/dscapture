# dscapture

`dscapture` преобразует PDF-даташит электронного компонента в типизированный JSON. Парсер рассчитан на документы разных производителей и не привязан к фиксированному шаблону страницы.

Конвейер извлечения:

1. Poppler извлекает текст с сохранением физической разметки (`pdftotext -layout`).
2. Если Poppler отсутствует, используется встроенный pure-Rust backend на базе `lopdf`.
3. Страницы без пригодного текстового слоя автоматически растеризуются через `pdftoppm` и распознаются Tesseract.
4. При feature `opencv-preprocessing` перед OCR применяются median denoise и adaptive Gaussian threshold из OpenCV.
5. Эвристический слой распознаёт общие сведения, корпуса, pin tables, Absolute Maximum Ratings и Recommended Operating Conditions. Исходная строка каждой записи рейтинга остаётся в поле `source`, поэтому частично распознанные таблицы можно проверить или обработать дальше.

## Сборка и запуск

Для обычных PDF рекомендуются системные пакеты `poppler-utils` и `tesseract-ocr`:

```bash
cargo build --release
./target/release/dscapture input.pdf output.json
```

Полезные варианты:

```bash
# Отключить OCR и обработать максимум 64 страницы
dscapture input.pdf output.json --no-ocr --max-pages 64

# Принудительно распознать скан
dscapture scan.pdf output.json --backend ocr --ocr-max-pages 20 --ocr-dpi 350

# Обработать весь документ (по умолчанию лимит равен 128 страницам)
dscapture input.pdf output.json --max-pages 0
```

`--backend` принимает `auto`, `poppler`, `native` или `ocr`. Языки Tesseract задаются через `--ocr-language`, например `eng+deu`.

## OpenCV для сложных сканов

После установки development-пакетов OpenCV и libclang сборка выглядит так (например, в Debian/Ubuntu нужны `libopencv-dev` и `libclang-dev`):

```bash
cargo build --release --features opencv-preprocessing
```

OpenCV используется только на OCR-страницах и не добавляет накладных расходов для PDF с нормальным текстовым слоем.

## Rust API

```rust
use dscapture::{ParseOptions, parse_file, to_json};

let result = parse_file("input.pdf", &ParseOptions::default())?;
println!("{}", to_json(&result, true)?);
# Ok::<(), dscapture::Error>(())
```

`parse_bytes` принимает PDF из памяти и удобен для серверов или плагинов.

## Shared library / C ABI

`crate-type = ["rlib", "cdylib"]`, поэтому release-сборка создаёт `target/release/libdscapture.so`. Заголовок находится в `include/dscapture.h`.

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

Поля, отсутствующие в исходном документе, не выдумываются и не выводятся. `metadata.confidence` — доля успешно заполненных смысловых групп, а не статистическая вероятность. Для очень длинных reference manuals в `metadata.warnings` будет явно указан применённый лимит страниц.
