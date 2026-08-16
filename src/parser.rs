use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::extract::ExtractedDocument;
use crate::model::{Datasheet, General, ParseMetadata, Pin, Rating};

static GAP_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s{2,}").expect("valid gap regex"));
static LEADING_NUMBER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\d+(?:\.\d+)*[.)]?\s*").expect("valid heading regex"));
static PACKAGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)\b(?P<family>TSDSON|TDSON|HVSOF|WQFN|TQFN|VQFN|UQFN|QFN|TDFN|WSON|XSON|SON|TSSOP|SSOP|MSOP|SOIC|SOP|SOD|SOT|LQFP|TQFP|TFP|LGA|BGA|VMD|SLP|SC|SO)[\s_\-−–]*(?:\(\s*)?(?P<pins>\d{1,4})(?:\s*\))?(?:[\-−–](?P<tail>\d{1,2}))?\b",
    )
    .expect("valid package regex")
});
static TO_PACKAGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bTO[\-−–](?P<pins>\d{1,4})(?:[\-−–](?P<tail>\d{1,2}))?\b")
        .expect("valid TO package regex")
});
static REVERSE_PACKAGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)\b(?P<pins>\d{1,3})[\s-]*(?:pin|lead)s?\s+(?P<family>TSDSON|TDSON|WQFN|TQFN|VQFN|UQFN|QFN|TDFN|WSON|XSON|SON|TSSOP|SSOP|MSOP|SOIC|SOP|LQFP|TQFP|LGA|BGA)\b",
    )
    .expect("valid reverse package regex")
});
static PART_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Z]{1,10}[A-Z0-9]*\d[A-Z0-9]*(?:[-/][A-Z0-9]+)*\b").expect("valid part regex")
});
static TEMPERATURE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bT(?:A|J|AMB|C)\s*(?:=|≤|>=|<|>)?\s*[+−–-]?\d+(?:\s*(?:to|\.\.)\s*[+−–-]?\d+)?\s*°?C\b")
        .expect("valid temperature regex")
});
static DIAGRAM_PINS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*([^\s]+)\s+(\d+)\s{3,}(\d+)\s+([^\s]+)\s*$").expect("valid diagram pin regex")
});
static PAGE_FOOTER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*\d+\s+(?:\d{4}[-/]\d{1,2}[-/]\d{1,2}|Page\s+\d+)\s*$")
        .expect("valid page footer regex")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SectionKind {
    Features,
    Applications,
    Description,
    Pins,
    AbsoluteMaximum,
    Recommended,
    Electrical,
    Thermal,
    Other,
}

#[derive(Debug, Clone)]
struct Heading {
    line: usize,
    start: usize,
    kind: SectionKind,
}

#[derive(Debug, Clone, Copy, Default)]
struct RatingColumns {
    has_min: bool,
    has_typ: bool,
    has_max: bool,
    has_value: bool,
    symbol_first: bool,
    parameter_first: bool,
}

pub(crate) fn parse_document(source: Option<&Path>, extracted: ExtractedDocument) -> Datasheet {
    let lines: Vec<&str> = extracted.text.lines().collect();
    let source_name = source
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned());
    let name = extract_name(source, &lines);
    let packages = extract_packages(&format!(
        "{}\n{}",
        source_name.as_deref().unwrap_or_default(),
        excerpt(&extracted.text, 100_000)
    ));

    let features = extract_list_section(&lines, SectionKind::Features, 48);
    let applications = extract_list_section(&lines, SectionKind::Applications, 32);
    let description = extract_description(&lines);
    let title = extract_title(
        extracted.pdf_metadata.title.as_deref(),
        name.as_deref(),
        &lines,
    );
    let manufacturer = detect_manufacturer(&format!(
        "{}\n{}",
        extracted.pdf_metadata.author.as_deref().unwrap_or_default(),
        excerpt(&extracted.text, 80_000)
    ));

    let pin_configuration = extract_pins(&lines, &packages);
    let absolute_maximum_ratings = extract_ratings(&lines, SectionKind::AbsoluteMaximum);
    let recommended_operating_conditions = extract_ratings(&lines, SectionKind::Recommended);
    let electrical_characteristics = extract_electrical_characteristics(&lines);
    let thermal_characteristics = extract_ratings(&lines, SectionKind::Thermal);

    let populated_groups = usize::from(name.is_some())
        + usize::from(title.is_some())
        + usize::from(manufacturer.is_some())
        + usize::from(!features.is_empty())
        + usize::from(!applications.is_empty())
        + usize::from(description.is_some())
        + usize::from(!packages.is_empty())
        + usize::from(!pin_configuration.is_empty())
        + usize::from(!absolute_maximum_ratings.is_empty())
        + usize::from(!recommended_operating_conditions.is_empty())
        + usize::from(!electrical_characteristics.is_empty())
        + usize::from(!thermal_characteristics.is_empty());
    let mut confidence = populated_groups as f32 / 12.0;
    if extracted.method.contains("ocr") {
        confidence *= 0.9;
    }

    Datasheet {
        schema_version: "1.1".to_owned(),
        general: General {
            name,
            title,
            features,
            mfr: manufacturer,
            packages,
            description,
            applications,
        },
        pin_configuration,
        absolute_maximum_ratings,
        recommended_operating_conditions,
        electrical_characteristics,
        thermal_characteristics,
        metadata: ParseMetadata {
            source_file: source_name,
            total_pages: extracted.total_pages,
            pages_processed: extracted.pages_processed,
            extraction_method: extracted.method,
            ocr_pages: extracted.ocr_pages,
            confidence: (confidence * 100.0).round() / 100.0,
            warnings: extracted.warnings,
        },
    }
}

fn excerpt(text: &str, maximum_chars: usize) -> String {
    text.chars().take(maximum_chars).collect()
}

fn extract_name(source: Option<&Path>, lines: &[&str]) -> Option<String> {
    if let Some(stem) = source
        .and_then(Path::file_stem)
        .map(|value| value.to_string_lossy())
    {
        let candidate = stem
            .rsplit_once('_')
            .map_or(stem.as_ref(), |(_, tail)| tail)
            .trim();
        if !candidate.eq_ignore_ascii_case("input")
            && !candidate.eq_ignore_ascii_case("datasheet")
            && candidate
                .chars()
                .any(|character| character.is_ascii_digit())
        {
            return Some(clean_text(candidate));
        }
    }

    let mut frequencies: HashMap<String, (usize, usize)> = HashMap::new();
    for (line_number, line) in lines.iter().take(500).enumerate() {
        for capture in PART_RE.find_iter(&line.to_ascii_uppercase()) {
            let candidate = capture.as_str();
            if is_part_number_noise(candidate) {
                continue;
            }
            let entry = frequencies
                .entry(candidate.to_owned())
                .or_insert((0, line_number));
            entry.0 += 1;
        }
    }
    frequencies
        .into_iter()
        .max_by_key(|(candidate, (count, first))| {
            (count * 20 + candidate.len(), usize::MAX - first)
        })
        .map(|(candidate, _)| candidate)
}

fn is_part_number_noise(candidate: &str) -> bool {
    const NOISE: &[&str] = &[
        "SOT23", "SOT323", "SOT416", "SOD523", "WSON8", "QFN16", "QFN32", "QFN48", "QFN56",
        "TSSOP8", "SSOP8", "CMOS", "ROHS", "JESD22", "IEC60134", "ISO9001",
    ];
    NOISE.contains(&candidate)
        || candidate.starts_with("REV")
        || candidate.starts_with("PAGE")
        || candidate.ends_with("MHZ")
}

fn extract_title(
    metadata_title: Option<&str>,
    name: Option<&str>,
    lines: &[&str],
) -> Option<String> {
    if let Some(title) = metadata_title.map(clean_text).filter(|title| {
        let credible_for_part = name.is_none_or(|part| {
            let normalized_part = part.replace([' ', '-', '_'], "").to_ascii_uppercase();
            let normalized_title = title
                .replace([' ', '-', '_', '/', ','], "")
                .to_ascii_uppercase();
            normalized_title.contains(&normalized_part)
                || common_prefix_length(&normalized_part, &normalized_title) >= 7
        });
        title.len() >= 8
            && !title.eq_ignore_ascii_case("data sheet")
            && !title.eq_ignore_ascii_case("datasheet")
            && credible_for_part
    }) {
        return Some(title);
    }

    let mut candidates = Vec::new();
    for (index, line) in lines.iter().take(180).enumerate() {
        for (_, _, segment) in segments(line) {
            let value = clean_text(segment);
            if value.len() < 5
                || value.len() > 180
                || is_boilerplate(&value)
                || classify_heading(&value).is_some()
            {
                continue;
            }
            let mut score = 100usize.saturating_sub(index.min(100));
            if name.is_some_and(|part| {
                value
                    .to_ascii_uppercase()
                    .contains(&part.to_ascii_uppercase())
            }) {
                score += 100;
            }
            if value.split_whitespace().count() >= 4 {
                score += 20;
            }
            candidates.push((score, value));
        }
    }
    candidates
        .into_iter()
        .max_by_key(|(score, _)| *score)
        .map(|(_, value)| value)
}

fn is_boilerplate(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.starts_with("www.")
        || lower.starts_with("http")
        || lower.contains("copyright")
        || lower.contains("all rights reserved")
        || lower.contains("product folder")
        || lower.contains("submit documentation")
        || lower.starts_with("page ")
        || lower.contains("important notice")
        || lower == "product data sheet"
}

fn detect_manufacturer(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    const MANUFACTURERS: &[(&str, &[&str])] = &[
        ("Texas Instruments", &["texas instruments", "www.ti.com"]),
        ("Analog Devices", &["analog devices", "analog.com"]),
        ("Maxim Integrated", &["maxim integrated", "maxim-ic.com"]),
        ("STMicroelectronics", &["stmicroelectronics", "st.com"]),
        ("Infineon Technologies", &["infineon"]),
        ("Toshiba", &["toshiba"]),
        ("Nexperia", &["nexperia"]),
        ("ROHM Semiconductor", &["rohm"]),
        ("onsemi", &["on semiconductor", "onsemi"]),
        ("Fairchild Semiconductor", &["fairchild semiconductor"]),
        ("Microchip Technology", &["microchip"]),
        ("Renesas Electronics", &["renesas"]),
        ("Intel", &["intel corporation", "intel®"]),
        ("NXP Semiconductors", &["nxp semiconductors", "nxp.com"]),
        ("Philips Semiconductors", &["philips semiconductors"]),
        ("Semtech", &["semtech"]),
        ("Comchip Technology", &["comchip"]),
    ];
    MANUFACTURERS
        .iter()
        .find(|(_, aliases)| aliases.iter().any(|alias| lower.contains(alias)))
        .map(|(name, _)| (*name).to_owned())
}

fn extract_packages(text: &str) -> Vec<String> {
    let mut packages = Vec::new();
    for capture in PACKAGE_RE.captures_iter(text) {
        let family = capture.name("family").expect("family capture").as_str();
        let pins = capture.name("pins").expect("pin capture").as_str();
        if pins.parse::<u16>().ok().is_none_or(|count| count == 0) {
            continue;
        }
        let tail = capture.name("tail").map(|value| value.as_str());
        push_unique(&mut packages, canonical_package(family, pins, tail));
    }
    for capture in TO_PACKAGE_RE.captures_iter(text) {
        push_unique(
            &mut packages,
            canonical_package(
                "TO",
                capture.name("pins").expect("pin capture").as_str(),
                capture.name("tail").map(|value| value.as_str()),
            ),
        );
    }
    for capture in REVERSE_PACKAGE_RE.captures_iter(text) {
        push_unique(
            &mut packages,
            canonical_package(
                capture.name("family").expect("family capture").as_str(),
                capture.name("pins").expect("pin capture").as_str(),
                None,
            ),
        );
    }
    packages.truncate(24);
    packages
}

fn canonical_package(family: &str, pins: &str, tail: Option<&str>) -> String {
    match tail {
        Some(tail) => format!("{}-{pins}-{tail}", family.to_ascii_uppercase()),
        None => format!("{}-{pins}", family.to_ascii_uppercase()),
    }
}

fn extract_list_section(lines: &[&str], target: SectionKind, maximum_items: usize) -> Vec<String> {
    let Some(heading) = find_best_heading(lines, target) else {
        return Vec::new();
    };
    let next_heading_on_line = segments(lines[heading.line])
        .into_iter()
        .filter(|(start, _, _)| *start > heading.start)
        .map(|(start, _, _)| start)
        .min();
    let end_column = next_heading_on_line.unwrap_or(usize::MAX);
    let start_column = heading.start.saturating_sub(2);
    let mut items: Vec<String> = Vec::new();

    for line in lines.iter().skip(heading.line + 1).take(180) {
        let piece = char_slice(line, start_column, end_column);
        let trimmed = piece.trim();
        if trimmed.is_empty() {
            continue;
        }
        if heading_segments(&piece)
            .iter()
            .any(|candidate| candidate.kind != target)
        {
            break;
        }
        if is_section_boundary(trimmed) {
            break;
        }

        let bullets = split_bullets(trimmed);
        if !bullets.is_empty() {
            for bullet in bullets {
                let bullet = truncate_at_gap(&bullet);
                if is_useful_list_item(&bullet) {
                    push_unique(&mut items, bullet);
                }
            }
        } else if let Some(last) = items.last_mut() {
            let continuation = truncate_at_gap(trimmed);
            if is_list_continuation(&continuation)
                && !looks_like_table_header(&continuation)
                && !is_boilerplate(&continuation)
                && last.len() + continuation.len() < 400
            {
                join_sentence(last, &continuation);
            }
        }
        if items.len() >= maximum_items {
            break;
        }
    }
    items
}

fn extract_description(lines: &[&str]) -> Option<String> {
    let heading = find_best_heading(lines, SectionKind::Description)?;
    let next_heading_on_line = segments(lines[heading.line])
        .into_iter()
        .filter(|(start, _, _)| *start > heading.start)
        .map(|(start, _, _)| start)
        .min();
    let start_column = heading.start.saturating_sub(2);
    let end_column = next_heading_on_line.unwrap_or(usize::MAX);
    let mut description = String::new();

    for line in lines.iter().skip(heading.line + 1).take(100) {
        let piece = char_slice(line, start_column, end_column);
        let value = clean_text(&piece);
        if value.is_empty() {
            continue;
        }
        if heading_segments(&piece)
            .iter()
            .any(|candidate| candidate.kind != SectionKind::Description)
            || is_section_boundary(&value)
            || value.to_ascii_lowercase().starts_with("device information")
            || value.to_ascii_lowercase().starts_with("table ")
        {
            break;
        }
        if !is_boilerplate(&value) && !looks_like_table_header(&value) {
            join_sentence(&mut description, &value);
        }
        if description.len() >= 3000 {
            break;
        }
    }
    (!description.is_empty()).then_some(description)
}

fn extract_pins(lines: &[&str], packages: &[String]) -> BTreeMap<String, BTreeMap<String, Pin>> {
    let Some(heading) = find_best_heading(lines, SectionKind::Pins) else {
        return BTreeMap::new();
    };
    let mut output: BTreeMap<String, BTreeMap<String, Pin>> = BTreeMap::new();
    let mut active_packages: Vec<String> = if packages.len() == 1 {
        packages.to_vec()
    } else {
        Vec::new()
    };
    let mut last_keys: Vec<(String, String)> = Vec::new();
    let mut pending_description: Vec<String> = Vec::new();
    let mut parsed_rows = 0usize;
    let heading_text = lines[heading.line].to_ascii_lowercase();
    let start_column = if heading_text.contains("terminal functions")
        || heading_text.contains("pin functions")
        || heading_text.contains("pin description")
    {
        0
    } else {
        heading.start.saturating_sub(2)
    };
    let mut shared_pin_type: Option<String> = None;

    for line in lines.iter().skip(heading.line + 1).take(500) {
        let piece = char_slice(line, start_column, usize::MAX);
        let trimmed = piece.trim();
        if trimmed.is_empty() {
            continue;
        }
        if parsed_rows > 0 && is_pin_section_end(trimmed) {
            break;
        }

        if looks_like_table_header(trimmed) {
            last_keys.clear();
            pending_description.clear();
            continue;
        }

        let type_columns: Vec<&str> = GAP_RE.split(trimmed).collect();
        if type_columns
            .first()
            .is_some_and(|value| is_explicit_pin_type(value))
            && type_columns
                .get(1)
                .is_none_or(|value| !is_pin_number(value))
        {
            let pin_type = clean_text(type_columns[0]);
            for (package, number) in &last_keys {
                if let Some(pin) = output
                    .get_mut(package)
                    .and_then(|pins| pins.get_mut(number))
                {
                    pin.pin_type.clone_from(&pin_type);
                    if type_columns.len() > 1 {
                        let continuation = clean_text(&type_columns[1..].join(" "));
                        if !continuation.is_empty() {
                            let description = pin.description.get_or_insert_with(String::new);
                            join_sentence(description, &continuation);
                        }
                    }
                }
            }
            shared_pin_type = Some(pin_type);
            continue;
        }

        let line_packages = extract_packages(trimmed);
        if !line_packages.is_empty() && looks_like_package_label(trimmed) {
            active_packages = line_packages;
            continue;
        }

        let row_has_explicit_type = pin_line_has_explicit_type(&piece);
        let rows = parse_pin_line(&piece);
        if rows.is_empty() {
            if !last_keys.is_empty() && looks_like_pin_continuation(trimmed) {
                let continuation = clean_text(truncate_at_gap(trimmed).as_str());
                for (package, number) in &last_keys {
                    if let Some(pin) = output
                        .get_mut(package)
                        .and_then(|pins| pins.get_mut(number))
                    {
                        let description = pin.description.get_or_insert_with(String::new);
                        if description.len() + continuation.len() < 1200 {
                            join_sentence(description, &continuation);
                        }
                    }
                }
            } else if last_keys.is_empty() && looks_like_pin_continuation(trimmed) {
                pending_description.push(clean_text(trimmed));
            }
            continue;
        }

        if active_packages.is_empty() {
            active_packages = if packages.is_empty() {
                vec!["UNKNOWN".to_owned()]
            } else {
                packages.iter().take(8).cloned().collect()
            };
        }
        last_keys.clear();
        for (number, mut pin) in rows {
            if !pending_description.is_empty() {
                let pending = pending_description.join(" ");
                if let Some(description) = pin.description.as_mut() {
                    let mut combined = pending;
                    join_sentence(&mut combined, description);
                    *description = combined;
                } else {
                    pin.description = Some(pending);
                }
                pending_description.clear();
            }
            if !row_has_explicit_type && !matches!(pin.pin_type.as_str(), "Ground" | "No Connect") {
                if let Some(pin_type) = shared_pin_type.as_ref() {
                    pin.pin_type.clone_from(pin_type);
                }
            } else if row_has_explicit_type {
                shared_pin_type = Some(pin.pin_type.clone());
            }
            for package in &active_packages {
                if pin_number_fits_package(&number, package) {
                    output
                        .entry(package.clone())
                        .or_default()
                        .entry(number.clone())
                        .or_insert_with(|| pin.clone());
                    last_keys.push((package.clone(), number.clone()));
                }
            }
            parsed_rows += 1;
        }
    }
    output.retain(|_, pins| !pins.is_empty());
    output
}

fn parse_pin_line(line: &str) -> Vec<(String, Pin)> {
    if let Some(capture) = DIAGRAM_PINS_RE.captures(line) {
        let left_name = capture[1].to_owned();
        let left_number = capture[2].to_owned();
        let right_number = capture[3].to_owned();
        let right_name = capture[4].to_owned();
        return vec![
            (left_number, make_pin(&left_name, None, None)),
            (right_number, make_pin(&right_name, None, None)),
        ];
    }

    let columns: Vec<String> = GAP_RE
        .split(line.trim())
        .map(clean_text)
        .filter(|value| !value.is_empty())
        .collect();
    if columns.len() < 2 || looks_like_table_header(&columns.join(" ")) {
        return Vec::new();
    }

    // NAME | PIN | TYPE | DESCRIPTION (TI and many Analog Devices tables)
    if is_pin_number(&columns[1]) && is_pin_name(&columns[0]) {
        let explicit_type = columns.get(2).filter(|value| is_explicit_pin_type(value));
        let description_start = if explicit_type.is_some() { 3 } else { 2 };
        let description =
            (columns.len() > description_start).then(|| columns[description_start..].join(" "));
        return expand_pin_numbers(
            &columns[1],
            make_pin(
                &columns[0],
                explicit_type.map(String::as_str),
                description.as_deref(),
            ),
        );
    }

    // PIN | NAME/description | TYPE? | DESCRIPTION (Nexperia and compact tables)
    if is_pin_number(&columns[0]) {
        let second = &columns[1];
        let (name, mut description) = if is_symbolic_pin_name(second) {
            (second.clone(), None)
        } else {
            (semantic_pin_name(second), Some(second.clone()))
        };
        let explicit_type = columns.get(2).filter(|value| is_explicit_pin_type(value));
        let description_start = if explicit_type.is_some() { 3 } else { 2 };
        if columns.len() > description_start {
            let tail = columns[description_start..].join(" ");
            if let Some(current) = description.as_mut() {
                join_sentence(current, &tail);
            } else {
                description = Some(tail);
            }
        }
        return expand_pin_numbers(
            &columns[0],
            make_pin(
                &name,
                explicit_type.map(String::as_str),
                description.as_deref(),
            ),
        );
    }

    // OCR and non-layout fallback: "3 GND (emitter)".
    let mut words = line.split_whitespace();
    if let (Some(number), Some(name)) = (words.next(), words.next())
        && is_pin_number(number)
        && is_pin_name(name)
    {
        let tail = words.collect::<Vec<_>>().join(" ");
        return expand_pin_numbers(
            number,
            make_pin(name, None, (!tail.is_empty()).then_some(tail.as_str())),
        );
    }
    Vec::new()
}

fn pin_line_has_explicit_type(line: &str) -> bool {
    let columns: Vec<&str> = GAP_RE.split(line.trim()).collect();
    columns.len() >= 3
        && ((is_pin_number(columns[1]) && is_explicit_pin_type(columns[2]))
            || (is_pin_number(columns[0]) && is_explicit_pin_type(columns[2])))
}

fn make_pin(name: &str, explicit_type: Option<&str>, description: Option<&str>) -> Pin {
    let name = clean_pin_name(name);
    Pin {
        pin_type: explicit_type
            .map(clean_text)
            .unwrap_or_else(|| infer_pin_type(&name)),
        name,
        description: description
            .map(clean_text)
            .filter(|value| !value.is_empty()),
    }
}

fn expand_pin_numbers(numbers: &str, pin: Pin) -> Vec<(String, Pin)> {
    let normalized = numbers.replace("and", ",").replace(['/', ';'], ",");
    let mut output = Vec::new();
    for number in normalized.split(',').map(str::trim) {
        if is_pin_number(number) {
            output.push((canonical_pin_number(number), pin.clone()));
        }
    }
    output
}

fn canonical_pin_number(number: &str) -> String {
    match number.trim_matches(['(', ')']).trim() {
        "-" | "—" | "−" => "EP".to_owned(),
        value => value.to_ascii_uppercase(),
    }
}

fn semantic_pin_name(description: &str) -> String {
    let lower = description.to_ascii_lowercase();
    for (needle, name) in [
        ("ground", "GND"),
        ("gnd", "GND"),
        ("supply", "SUPPLY"),
        ("input", "INPUT"),
        ("output", "OUTPUT"),
        ("enable", "ENABLE"),
        ("no connect", "NC"),
    ] {
        if lower.contains(needle) {
            return name.to_owned();
        }
    }
    clean_pin_name(description.split_whitespace().next().unwrap_or("UNKNOWN"))
}

fn clean_pin_name(name: &str) -> String {
    clean_text(name)
        .trim_matches(['(', ')', '[', ']', ',', ':'])
        .to_ascii_uppercase()
}

fn infer_pin_type(name: &str) -> String {
    let upper = name.to_ascii_uppercase();
    if upper == "NC" || upper.contains("NO CONNECT") {
        "No Connect"
    } else if upper.contains("GND")
        || upper.starts_with("VSS")
        || upper == "EP"
        || upper.contains("PAD")
    {
        "Ground"
    } else if upper.starts_with("VCC")
        || upper.starts_with("VDD")
        || upper.starts_with("VIN")
        || upper.starts_with("VBAT")
        || upper == "SUPPLY"
    {
        "Power"
    } else if upper.contains("I/O") || upper.contains("IO") || upper.contains("SDA") {
        "Bidirectional"
    } else if upper.contains("OUT")
        || upper.ends_with('Y')
        || upper == "SW"
        || upper.starts_with("Q")
    {
        "Output"
    } else if upper.contains("IN")
        || upper.starts_with("EN")
        || upper.starts_with("CLK")
        || upper.starts_with("FB")
        || upper == "INPUT"
    {
        "Input"
    } else {
        "Unspecified"
    }
    .to_owned()
}

fn extract_ratings(lines: &[&str], target: SectionKind) -> Vec<Rating> {
    let Some(heading) = find_best_heading(lines, target) else {
        return Vec::new();
    };
    extract_ratings_from_heading(lines, target, &heading, false)
}

fn extract_electrical_characteristics(lines: &[&str]) -> Vec<Rating> {
    let mut headings = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !line.contains("....") && !line.contains("……"))
        .flat_map(|(line_number, line)| {
            heading_segments(line)
                .into_iter()
                .filter(|heading| heading.kind == SectionKind::Electrical)
                .filter(|heading| {
                    !char_slice(line, 0, heading.start)
                        .trim()
                        .to_ascii_lowercase()
                        .starts_with("table")
                })
                .map(move |heading| Heading {
                    line: line_number,
                    ..heading
                })
        })
        .collect::<Vec<_>>();
    headings.sort_by_key(|heading| heading.line);
    let heading_lines = headings
        .iter()
        .map(|heading| heading.line)
        .collect::<Vec<_>>();
    headings = headings
        .into_iter()
        .enumerate()
        .filter(|(index, heading)| {
            let next_heading = heading_lines.get(index + 1).copied().unwrap_or(lines.len());
            lines[heading.line + 1..next_heading]
                .iter()
                .any(|line| parse_rating_header(line).is_some())
        })
        .map(|(_, heading)| heading)
        .collect();

    let mut rows = Vec::new();
    for heading in headings {
        rows.extend(extract_ratings_from_heading(
            lines,
            SectionKind::Electrical,
            &heading,
            true,
        ));
    }
    deduplicate_ratings(rows)
}

fn extract_ratings_from_heading(
    lines: &[&str],
    target: SectionKind,
    heading: &Heading,
    continue_across_pages: bool,
) -> Vec<Rating> {
    let section_temperature = lines.iter().skip(heading.line).take(4).find_map(|line| {
        TEMPERATURE_RE
            .find(line)
            .map(|value| clean_text(value.as_str()))
    });
    let mut columns = RatingColumns::default();
    let mut rows = Vec::new();
    let mut current_parameter: Option<String> = None;
    let mut current_symbol: Option<String> = None;
    let mut current_unit: Option<String> = None;
    let mut current_unit_parameter: Option<String> = None;
    let mut header_seen = false;
    let end_column = lines
        .iter()
        .skip(heading.line + 1)
        .take(120)
        .flat_map(|line| heading_segments(line))
        .filter(|candidate| candidate.kind == SectionKind::Pins && candidate.start > heading.start)
        .map(|candidate| candidate.start.saturating_sub(2))
        .min()
        .unwrap_or(usize::MAX);

    let scan_limit = if continue_across_pages { 2400 } else { 420 };
    for line in lines.iter().skip(heading.line + 1).take(scan_limit) {
        if !continue_across_pages && !rows.is_empty() && line.contains('\u{c}') {
            break;
        }
        let piece = char_slice(line, heading.start.saturating_sub(2), end_column);
        let trimmed = piece.trim();
        if trimmed.is_empty() {
            continue;
        }
        if is_footnote(trimmed) {
            if rows.is_empty() {
                continue;
            }
            break;
        }
        if !rows.is_empty() && PAGE_FOOTER_RE.is_match(trimmed) {
            break;
        }
        if !rows.is_empty() && is_rating_section_end(trimmed, target) {
            break;
        }
        if let Some(header) = parse_rating_header(trimmed) {
            columns = RatingColumns {
                has_min: columns.has_min || header.has_min,
                has_typ: columns.has_typ || header.has_typ,
                has_max: columns.has_max || header.has_max,
                has_value: columns.has_value || header.has_value,
                symbol_first: columns.symbol_first || header.symbol_first,
                parameter_first: columns.parameter_first || header.parameter_first,
            };
            header_seen = true;
            continue;
        }
        if !header_seen {
            continue;
        }
        if looks_like_table_header(trimmed) || is_boilerplate(trimmed) {
            continue;
        }

        let cells: Vec<String> = GAP_RE
            .split(trimmed)
            .map(clean_text)
            .filter(|cell| !cell.is_empty())
            .collect();
        if cells.is_empty() {
            continue;
        }
        let allow_symbolic_values = target != SectionKind::Electrical;

        if cells.len() == 1
            && current_parameter.is_some()
            && is_standalone_rating_value(&cells[0])
            && is_rating_value_for(&cells[0], allow_symbolic_values)
        {
            rows.push(Rating {
                parameter: current_parameter.clone().unwrap_or_default(),
                symbol: current_symbol.clone(),
                conditions: None,
                temperature: section_temperature.clone(),
                min: None,
                typ: None,
                max: None,
                value: Some(cells[0].clone()),
                unit: if current_unit_parameter.as_deref() == current_parameter.as_deref() {
                    current_unit.clone()
                } else {
                    None
                },
                source: clean_text(trimmed),
            });
            continue;
        }

        if let Some(mut rating) = parse_rating_row(&cells, columns, trimmed, allow_symbolic_values)
        {
            if rating.symbol.is_none() && looks_like_symbol(&rating.parameter) {
                rating.symbol = Some(rating.parameter.clone());
                if let Some(parameter) = current_parameter.as_ref() {
                    rating.parameter = parameter.clone();
                }
            }
            if rating
                .symbol
                .as_deref()
                .is_some_and(|symbol| !extract_packages(symbol).is_empty())
                && current_parameter.is_some()
            {
                let package_condition = rating.symbol.take().unwrap_or_default();
                let row_condition = std::mem::take(&mut rating.parameter);
                rating.parameter = current_parameter.clone().unwrap_or_default();
                rating.symbol.clone_from(&current_symbol);
                rating.conditions = Some(
                    [
                        Some(package_condition),
                        Some(row_condition),
                        rating.conditions.take(),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("; "),
                );
            }
            if rating.parameter.is_empty() {
                rating.parameter = current_parameter.clone().unwrap_or_else(|| {
                    rating
                        .symbol
                        .clone()
                        .unwrap_or_else(|| "unspecified".to_owned())
                });
            }
            if target == SectionKind::Electrical
                && rating.symbol.is_none()
                && current_symbol.is_some()
                && (looks_like_electrical_condition(&rating.source)
                    || rating
                        .parameter
                        .chars()
                        .next()
                        .is_some_and(|character| character.is_ascii_digit()))
            {
                let row_condition = std::mem::take(&mut rating.parameter);
                rating.parameter = current_parameter.clone().unwrap_or_default();
                rating.symbol.clone_from(&current_symbol);
                rating.conditions = join_conditions(Some(row_condition), rating.conditions.take());
            }
            if rating.symbol.is_none()
                && current_parameter.as_deref() == Some(rating.parameter.as_str())
            {
                rating.symbol.clone_from(&current_symbol);
            }
            if rating.unit.is_none() {
                if current_unit_parameter.as_deref() == Some(rating.parameter.as_str()) {
                    rating.unit.clone_from(&current_unit);
                }
            } else {
                current_unit.clone_from(&rating.unit);
                current_unit_parameter = Some(rating.parameter.clone());
            }
            if rating.temperature.is_none() {
                rating.temperature.clone_from(&section_temperature);
            }
            infer_rating_unit(&mut rating);
            if let Some(previous) = rows.last_mut()
                && previous.symbol.as_deref() == Some(previous.parameter.as_str())
                && previous.unit.is_none()
                && rating.parameter != previous.parameter
            {
                previous.parameter = rating.parameter.clone();
                previous.unit.clone_from(&rating.unit);
            }
            if target == SectionKind::Electrical
                && rating.symbol.is_some()
                && rating.symbol != current_symbol
                && let Some(previous) = rows.last_mut()
                && previous
                    .source
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_ascii_digit())
            {
                previous.parameter = rating.parameter.clone();
                previous.symbol.clone_from(&rating.symbol);
                previous.unit.clone_from(&rating.unit);
            }
            current_parameter = Some(rating.parameter.clone());
            if rating.symbol.is_some() {
                current_symbol.clone_from(&rating.symbol);
            }
            rows.push(rating);
        } else if target == SectionKind::Electrical
            && cells.len() == 1
            && looks_like_electrical_condition(&cells[0])
            && cells[0]
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_digit())
        {
            if let Some(previous) = rows.last_mut()
                && current_parameter.as_deref() == Some(previous.parameter.as_str())
            {
                previous.conditions =
                    join_conditions(previous.conditions.take(), Some(clean_text(&cells[0])));
                join_sentence(&mut previous.source, &cells[0]);
            }
        } else if let Some((symbol, parameter)) =
            parse_rating_group(&cells, columns, allow_symbolic_values)
        {
            if let Some(unit) = cells.last().filter(|cell| is_unit(cell)) {
                current_unit = Some(unit.clone());
                current_unit_parameter = Some(parameter.clone());
            }
            if let Some(previous) = rows.last_mut()
                && (previous.source.chars().next().is_some_and(|character| {
                    character.is_ascii_digit() || "+-−–".contains(character)
                }) || (target == SectionKind::Electrical
                    && symbol.is_some()
                    && looks_like_electrical_condition(&previous.source)))
            {
                previous.parameter = parameter.clone();
                if symbol.is_some() {
                    previous.symbol.clone_from(&symbol);
                }
                if current_unit_parameter.as_deref() == Some(parameter.as_str()) {
                    previous.unit.clone_from(&current_unit);
                }
            }
            current_symbol = symbol;
            current_parameter = Some(parameter);
        }
    }

    deduplicate_ratings(rows)
}

fn parse_rating_header(line: &str) -> Option<RatingColumns> {
    let upper = line.to_ascii_uppercase();
    let has_min = contains_word(&upper, "MIN");
    let has_typ = contains_word(&upper, "TYP") || contains_word(&upper, "NOM");
    let has_max = contains_word(&upper, "MAX") || contains_word(&upper, "LIMIT");
    let has_value = contains_word(&upper, "VALUE") || contains_word(&upper, "RATING");
    let has_field_names = upper.contains("SYMBOL")
        && (upper.contains("PARAMETER") || upper.contains("CHARACTERISTIC"));
    let tableish = (upper.contains("UNIT") && (has_min || has_typ || has_max || has_value))
        || (upper.contains("UNIT") && has_field_names)
        || (has_min && has_max);
    if !tableish {
        return None;
    }
    let symbol_at = upper.find("SYMBOL");
    let parameter_at = upper
        .find("PARAMETER")
        .or_else(|| upper.find("CHARACTERISTIC"));
    Some(RatingColumns {
        has_min,
        has_typ,
        has_max,
        has_value,
        symbol_first: matches!((symbol_at, parameter_at), (Some(symbol), Some(parameter)) if symbol < parameter),
        parameter_first: matches!((symbol_at, parameter_at), (Some(symbol), Some(parameter)) if parameter < symbol),
    })
}

fn parse_rating_row(
    cells: &[String],
    columns: RatingColumns,
    source: &str,
    allow_symbolic_values: bool,
) -> Option<Rating> {
    let mut cells = cells.to_vec();
    let unit_index = cells.iter().rposition(|cell| is_unit(cell));
    let (unit, trailing_conditions) = if let Some(index) = unit_index {
        let unit = canonical_unit(&cells.remove(index));
        let conditions = cells.split_off(index);
        (Some(unit), conditions)
    } else {
        (None, Vec::new())
    };

    let expected_values = if columns.has_value && !columns.has_min && !columns.has_max {
        1
    } else {
        usize::from(columns.has_min) + usize::from(columns.has_typ) + usize::from(columns.has_max)
    };
    let mut values = Vec::new();
    while let Some(last) = cells.last() {
        if values.len() >= expected_values.max(1)
            || !is_rating_value_for(last, allow_symbolic_values)
        {
            break;
        }
        values.push(cells.pop().expect("last cell exists"));
    }
    values.reverse();

    // Tables with one "Rating" column frequently use a range such as -0.5 to 7.0.
    if values.is_empty()
        && columns.has_value
        && let Some(index) = cells
            .iter()
            .rposition(|cell| is_rating_value_for(cell, allow_symbolic_values))
    {
        values.push(cells.remove(index));
    }
    if values.is_empty() || values.iter().all(|value| is_dash(value)) || cells.is_empty() {
        return None;
    }

    let (symbol, parameter, mut conditions) = identify_rating_fields(&cells, columns);
    if !trailing_conditions.is_empty() {
        let tail = trailing_conditions.join(" ");
        if let Some(existing) = conditions.as_mut() {
            if !existing.is_empty() {
                existing.push_str("; ");
            }
            existing.push_str(&tail);
        } else {
            conditions = Some(tail);
        }
    }
    let mut rating = Rating {
        parameter,
        symbol,
        conditions,
        temperature: TEMPERATURE_RE
            .find(source)
            .map(|value| clean_text(value.as_str())),
        min: None,
        typ: None,
        max: None,
        value: None,
        unit,
        source: clean_text(source),
    };

    if values.len() == 1 && values[0].to_ascii_lowercase().contains(" to ") {
        let bounds: Vec<&str> = Regex::new(r"(?i)\s+to\s+")
            .expect("range regex")
            .split(&values[0])
            .collect();
        if bounds.len() == 2 {
            rating.min = Some(bounds[0].trim().to_owned());
            rating.max = Some(bounds[1].trim().to_owned());
        } else {
            rating.value = values.into_iter().next();
        }
    } else if columns.has_value && !columns.has_min && !columns.has_max {
        rating.value = Some(values.join(" "));
    } else {
        assign_min_typ_max(&mut rating, &values, columns);
    }
    Some(rating)
}

fn identify_rating_fields(
    cells: &[String],
    columns: RatingColumns,
) -> (Option<String>, String, Option<String>) {
    if cells.len() == 1
        && let Some((parameter, symbol)) = cells[0].split_once(',')
        && looks_like_symbol(symbol.trim())
    {
        return (
            Some(strip_reference(symbol)),
            strip_reference(parameter),
            None,
        );
    }
    if columns.symbol_first && cells.len() >= 2 && looks_like_symbol(&cells[0]) {
        return (
            Some(strip_reference(&cells[0])),
            strip_reference(&cells[1]),
            join_optional(&cells[2..]),
        );
    }
    if columns.parameter_first && cells.len() >= 2 && looks_like_symbol(&cells[1]) {
        return (
            Some(strip_reference(&cells[1])),
            strip_reference(&cells[0]),
            join_optional(&cells[2..]),
        );
    }
    if cells.len() >= 2 {
        if looks_like_symbol(&cells[0]) && !looks_like_symbol(&cells[1]) {
            return (
                Some(strip_reference(&cells[0])),
                strip_reference(&cells[1]),
                join_optional(&cells[2..]),
            );
        }
        if !looks_like_symbol(&cells[0]) && looks_like_symbol(&cells[1]) {
            return (
                Some(strip_reference(&cells[1])),
                strip_reference(&cells[0]),
                join_optional(&cells[2..]),
            );
        }
    }
    (None, strip_reference(&cells[0]), join_optional(&cells[1..]))
}

fn assign_min_typ_max(rating: &mut Rating, values: &[String], columns: RatingColumns) {
    match values {
        [one] if columns.has_max && !columns.has_min && !columns.has_typ => {
            rating.max = Some(one.clone())
        }
        [one] if columns.has_typ && !columns.has_min && !columns.has_max => {
            rating.typ = Some(one.clone())
        }
        [one] if columns.has_min && columns.has_max => rating.max = dash_to_none(one),
        [one] => rating.value = Some(one.clone()),
        [min, max] if columns.has_min && columns.has_max && !columns.has_typ => {
            rating.min = dash_to_none(min);
            rating.max = dash_to_none(max);
        }
        [min, typ] if columns.has_min && columns.has_typ && !columns.has_max => {
            rating.min = dash_to_none(min);
            rating.typ = dash_to_none(typ);
        }
        [typ, max] if columns.has_typ && columns.has_max && !columns.has_min => {
            rating.typ = dash_to_none(typ);
            rating.max = dash_to_none(max);
        }
        [min, max] if columns.has_min && columns.has_max => {
            rating.min = dash_to_none(min);
            rating.max = dash_to_none(max);
        }
        [min, typ, max, ..] => {
            rating.min = dash_to_none(min);
            rating.typ = dash_to_none(typ);
            rating.max = dash_to_none(max);
        }
        _ => rating.value = Some(values.join(" ")),
    }
}

fn parse_rating_group(
    cells: &[String],
    columns: RatingColumns,
    allow_symbolic_values: bool,
) -> Option<(Option<String>, String)> {
    let mut end = cells.len();
    if cells.last().is_some_and(|cell| is_unit(cell)) {
        end -= 1;
    }
    while end > 0 && is_dash(&cells[end - 1]) {
        end -= 1;
    }
    let cells = &cells[..end];
    if cells.is_empty() {
        return None;
    }
    if cells
        .iter()
        .any(|cell| is_rating_value_for(cell, allow_symbolic_values) || is_unit(cell))
    {
        return None;
    }
    if columns.symbol_first && cells.len() >= 2 && looks_like_symbol(&cells[0]) {
        Some((Some(strip_reference(&cells[0])), strip_reference(&cells[1])))
    } else if columns.parameter_first && cells.len() >= 2 && looks_like_symbol(&cells[1]) {
        Some((Some(strip_reference(&cells[1])), strip_reference(&cells[0])))
    } else if cells.len() >= 2 && looks_like_symbol(&cells[0]) {
        Some((Some(strip_reference(&cells[0])), strip_reference(&cells[1])))
    } else if cells.len() <= 2 && cells[0].chars().any(char::is_alphabetic) {
        Some((None, strip_reference(&cells[0])))
    } else {
        None
    }
}

fn infer_rating_unit(rating: &mut Rating) {
    let evidence = format!("{} {}", rating.parameter, rating.source).to_ascii_lowercase();
    let compact_symbol = rating
        .symbol
        .as_deref()
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    if evidence.contains("thermal resistance") || compact_symbol.starts_with("rth") {
        if rating.unit.is_none() || rating.unit.as_deref() == Some("°C") {
            rating.unit = Some("°C/W".to_owned());
        }
        return;
    }
    if evidence.contains("temperature")
        || evidence.contains("junction")
        || evidence.contains("storage")
        || evidence.contains("ambient")
    {
        rating.unit = Some("°C".to_owned());
        if evidence.contains("junction")
            || evidence.contains("storage")
            || evidence.contains("ambient")
        {
            rating.parameter = "Temperature".to_owned();
        }
        return;
    }
    if rating.unit.is_some() {
        return;
    }
    let voltage_symbol = rating.symbol.as_deref().is_some_and(|symbol| {
        symbol
            .split([',', '/', ' '])
            .filter(|part| !part.is_empty())
            .any(|part| part.to_ascii_uppercase().starts_with('V'))
    });
    rating.unit = if evidence.contains("voltage") || voltage_symbol {
        Some("V".to_owned())
    } else if evidence.contains("current") && !evidence.contains("gain") {
        Some("A".to_owned())
    } else {
        None
    };
    if rating.unit.as_deref() == Some("V")
        && rating
            .symbol
            .as_deref()
            .is_some_and(|symbol| rating.parameter.eq_ignore_ascii_case(symbol))
    {
        rating.parameter = "Voltage".to_owned();
    }
}

fn deduplicate_ratings(rows: Vec<Rating>) -> Vec<Rating> {
    let mut seen = HashSet::new();
    rows.into_iter()
        .filter(|rating| {
            seen.insert(format!(
                "{}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
                rating.parameter,
                rating.symbol,
                rating.conditions,
                rating.min,
                rating.typ,
                rating.max,
                rating.value
            ))
        })
        .collect()
}

fn find_best_heading(lines: &[&str], target: SectionKind) -> Option<Heading> {
    let mut candidates = Vec::new();
    for (line_number, line) in lines.iter().enumerate() {
        if matches!(
            target,
            SectionKind::Features | SectionKind::Applications | SectionKind::Description
        ) && line_number > 400
        {
            break;
        }
        if line.contains("....") || line.contains("……") {
            continue;
        }
        for heading in heading_segments(line) {
            if heading.kind != target {
                continue;
            }
            let context = lines
                .iter()
                .skip(line_number + 1)
                .take(50)
                .copied()
                .collect::<Vec<_>>()
                .join("\n")
                .to_ascii_uppercase();
            let mut score = 1000usize.saturating_sub(line_number.min(1000));
            match target {
                SectionKind::Features | SectionKind::Applications => {
                    score += context.matches(['•', '■']).count() * 15;
                }
                SectionKind::Pins => {
                    score += usize::from(context.contains("PIN")) * 60;
                    score += usize::from(context.contains("DESCRIPTION")) * 60;
                    let heading_text = line.to_ascii_uppercase();
                    if heading_text.contains("TERMINAL FUNCTIONS")
                        || heading_text.contains("PIN FUNCTIONS")
                        || heading_text.contains("PIN DESCRIPTION")
                    {
                        score += 200;
                    }
                }
                SectionKind::AbsoluteMaximum
                | SectionKind::Recommended
                | SectionKind::Electrical
                | SectionKind::Thermal => {
                    score += usize::from(context.contains("UNIT")) * 80;
                    score += usize::from(context.contains("MAX")) * 40;
                }
                SectionKind::Description => {
                    score += context.split_whitespace().count().min(80);
                }
                SectionKind::Other => {}
            }
            candidates.push((
                score,
                Heading {
                    line: line_number,
                    ..heading
                },
            ));
        }
    }
    candidates
        .into_iter()
        .max_by_key(|(score, _)| *score)
        .map(|(_, heading)| heading)
}

fn heading_segments(line: &str) -> Vec<Heading> {
    segments(line)
        .into_iter()
        .filter_map(|(start, _end, segment)| {
            classify_heading(segment).map(|kind| Heading {
                line: 0,
                start,
                kind,
            })
        })
        .collect()
}

fn classify_heading(value: &str) -> Option<SectionKind> {
    if value.trim().ends_with('.') {
        return None;
    }
    let stripped = LEADING_NUMBER_RE.replace(value.trim(), "");
    let normalized = stripped
        .trim_matches([':', '.', '-', '–', '—'])
        .trim()
        .to_ascii_lowercase();
    let base = normalized.split('(').next().unwrap_or(&normalized).trim();
    if matches!(
        base,
        "features" | "key features" | "benefits and features" | "product features"
    ) {
        Some(SectionKind::Features)
    } else if matches!(
        base,
        "applications" | "typical applications" | "application"
    ) {
        Some(SectionKind::Applications)
    } else if matches!(
        base,
        "description" | "general description" | "product description" | "overview"
    ) {
        Some(SectionKind::Description)
    } else if matches!(
        base,
        "pin configuration and functions"
            | "pin configuration"
            | "pin configurations"
            | "pin functions"
            | "pin description"
            | "pin descriptions"
            | "pinning information"
            | "pin assignment"
            | "pin assignments"
            | "terminal configuration and functions"
            | "terminal functions"
            | "pinout"
    ) {
        Some(SectionKind::Pins)
    } else if matches!(
        base,
        "absolute maximum ratings"
            | "absolute maximum rating"
            | "maximum ratings"
            | "maximum rating"
            | "limiting values"
            | "limiting value"
    ) {
        Some(SectionKind::AbsoluteMaximum)
    } else if matches!(
        base,
        "recommended operating conditions"
            | "recommended operating condition"
            | "recommended operation conditions"
            | "operating ranges"
            | "operating range"
    ) {
        Some(SectionKind::Recommended)
    } else if matches!(
        base,
        "electrical characteristics"
            | "electrical characteristic"
            | "electrical specifications"
            | "electrical specification"
            | "dc electrical characteristics"
            | "dc electrical characteristic"
            | "ac electrical characteristics"
            | "ac electrical characteristic"
            | "dc characteristics"
            | "ac characteristics"
            | "static characteristics"
            | "dynamic characteristics"
            | "characteristics"
            | "electrical data"
    ) || base.starts_with("electrical characteristics ")
        || base.starts_with("dc characteristics ")
        || base.starts_with("ac characteristics ")
    {
        Some(SectionKind::Electrical)
    } else if matches!(
        base,
        "thermal information"
            | "thermal characteristics"
            | "thermal characteristic"
            | "thermal resistance"
            | "thermal data"
    ) {
        Some(SectionKind::Thermal)
    } else if base.ends_with("maximum ratings") || base.ends_with("maximum rating") {
        Some(SectionKind::AbsoluteMaximum)
    } else if is_generic_heading(base) {
        Some(SectionKind::Other)
    } else {
        None
    }
}

fn is_generic_heading(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "ordering information",
        "device information",
        "functional description",
        "typical characteristics",
        "mechanical data",
        "package information",
        "marking",
        "revision history",
        "esd ratings",
        "quick reference data",
    ]
    .iter()
    .any(|heading| lower == *heading)
}

fn segments(line: &str) -> Vec<(usize, usize, &str)> {
    let mut result = Vec::new();
    let mut cursor = 0usize;
    for gap in GAP_RE.find_iter(line) {
        if gap.start() > cursor {
            let value = &line[cursor..gap.start()];
            let leading = value
                .chars()
                .take_while(|character| character.is_whitespace())
                .count();
            let trailing = value
                .chars()
                .rev()
                .take_while(|character| character.is_whitespace())
                .count();
            let start = line[..cursor].chars().count() + leading;
            let end = line[..gap.start()].chars().count().saturating_sub(trailing);
            if start < end {
                result.push((start, end, value.trim()));
            }
        }
        cursor = gap.end();
    }
    if cursor < line.len() {
        let value = &line[cursor..];
        let leading = value
            .chars()
            .take_while(|character| character.is_whitespace())
            .count();
        let start = line[..cursor].chars().count() + leading;
        let end = line.chars().count();
        if start < end {
            result.push((start, end, value.trim()));
        }
    }
    result
}

fn char_slice(value: &str, start: usize, end: usize) -> String {
    value
        .chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn split_bullets(value: &str) -> Vec<String> {
    let normalized = value.replace(['●', '▪', '◆'], "•").replace('■', "•");
    if normalized.contains('•') {
        return normalized
            .split('•')
            .skip(1)
            .map(|item| item.trim().to_owned())
            .filter(|item| !item.is_empty())
            .collect();
    }
    let trimmed = normalized.trim_start();
    if trimmed.starts_with("- ") || trimmed.starts_with("+ ") || trimmed.starts_with("* ") {
        return vec![trimmed[2..].trim().to_owned()];
    }
    Vec::new()
}

fn truncate_at_gap(value: &str) -> String {
    GAP_RE
        .split(value)
        .next()
        .map(clean_text)
        .unwrap_or_default()
}

fn is_useful_list_item(value: &str) -> bool {
    value.len() >= 3 && value.chars().any(char::is_alphabetic) && !is_boilerplate(value)
}

fn is_list_continuation(value: &str) -> bool {
    let first = value.chars().next();
    let alphabetic = value
        .chars()
        .filter(|character| character.is_alphabetic())
        .count();
    let short_uppercase_label = value.split_whitespace().count() == 1
        && value.len() <= 8
        && value
            .chars()
            .filter(char::is_ascii_alphabetic)
            .all(|character| character.is_ascii_uppercase());
    value.len() >= 3
        && !short_uppercase_label
        && first.is_some_and(|character| character.is_alphabetic() || character == '(')
        && alphabetic * 3 >= value.chars().count().max(1)
}

fn is_section_boundary(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.starts_with("table ")
        || lower.starts_with("figure ")
        || lower.starts_with("typical application schematic")
        || lower.starts_with("device information")
        || lower.starts_with("table of contents")
}

fn is_pin_section_end(value: &str) -> bool {
    classify_heading(value).is_some_and(|kind| kind != SectionKind::Pins)
        || Regex::new(r"^\d+(?:\.\d+)*\s+(?:Ordering|Specifications|Electrical|Functional|Limiting|Absolute|Maximum)")
            .expect("pin end regex")
            .is_match(value)
}

fn is_rating_section_end(value: &str, target: SectionKind) -> bool {
    if let Some(kind) = classify_heading(value) {
        return kind != target && kind != SectionKind::Pins;
    }
    let lower = LEADING_NUMBER_RE.replace(value, "").to_ascii_lowercase();
    [
        "esd ratings",
        "electrical characteristics",
        "typical characteristics",
        "rating and characteristic curves",
        "dc characteristics",
        "ac characteristics",
    ]
    .iter()
    .any(|heading| lower.trim().starts_with(heading))
}

fn looks_like_package_label(value: &str) -> bool {
    let packages = extract_packages(value);
    if packages.is_empty() {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    value.len() < 100
        && (lower.contains("package")
            || lower.contains("-pin")
            || lower.contains("top view")
            || lower.contains("variant")
            || lower.contains(',')
            || packages
                .iter()
                .any(|package| value.trim().eq_ignore_ascii_case(&package.replace('-', ""))))
}

fn looks_like_pin_continuation(value: &str) -> bool {
    value.len() >= 12
        && value.len() <= 220
        && value.chars().any(char::is_alphabetic)
        && !looks_like_table_header(value)
        && !is_boilerplate(value)
        && classify_heading(value).is_none()
}

fn is_pin_number(value: &str) -> bool {
    let value = value.trim_matches(['(', ')']).trim();
    value == "-"
        || value == "—"
        || value == "−"
        || matches!(
            value.to_ascii_uppercase().as_str(),
            "EP" | "PAD" | "DAP" | "TAB"
        )
        || value
            .split([',', '/', ';'])
            .all(|piece| piece.trim().parse::<u16>().is_ok())
}

fn is_pin_name(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 40
        && !value.contains('.')
        && value.split_whitespace().count() <= 3
        && value.chars().any(char::is_alphabetic)
}

fn is_symbolic_pin_name(value: &str) -> bool {
    is_pin_name(value)
        && !value.contains(['(', ')'])
        && (value.split_whitespace().count() == 1
            || value
                .chars()
                .filter(char::is_ascii_alphabetic)
                .all(|character| character.is_ascii_uppercase()))
}

fn is_explicit_pin_type(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_uppercase().as_str(),
        "I" | "O" | "I/O" | "IO" | "PWR" | "POWER" | "GND" | "OD" | "NC" | "ANALOG" | "DIGITAL"
    )
}

fn looks_like_table_header(value: &str) -> bool {
    let upper = clean_text(value).to_ascii_uppercase();
    (upper.contains("PIN") && upper.contains("DESCRIPTION"))
        || (upper.contains("SYMBOL") && upper.contains("UNIT"))
        || (upper.contains("PARAMETER") && upper.contains("UNIT"))
        || (upper.contains("CHARACTERISTIC") && upper.contains("RATING"))
        || upper == "NAME NO."
        || upper == "I/O DESCRIPTION"
        || upper == "TYPE DESCRIPTION"
}

fn is_footnote(value: &str) -> bool {
    value.starts_with("Note")
        || Regex::new(r"^\(\d+\)\s+\S")
            .expect("footnote regex")
            .is_match(value)
        || Regex::new(r"^\[\d+\]\s+\S")
            .expect("footnote regex")
            .is_match(value)
}

fn is_unit(value: &str) -> bool {
    let compact = value
        .replace(' ', "")
        .replace('Ω', "ω")
        .replace('μ', "µ")
        .to_ascii_lowercase();
    matches!(
        compact.as_str(),
        "v" | "vv"
            | "mv"
            | "kv"
            | "a"
            | "ma"
            | "µa"
            | "μa"
            | "na"
            | "w"
            | "ww"
            | "mw"
            | "µw"
            | "μw"
            | "°c"
            | "c"
            | "k"
            | "k/w"
            | "°c/w"
            | "ω"
            | "kω"
            | "mω"
            | "mq"
            | "µh"
            | "μh"
            | "nh"
            | "f"
            | "mf"
            | "µf"
            | "μf"
            | "nf"
            | "pf"
            | "hz"
            | "khz"
            | "mhz"
            | "ghz"
            | "s"
            | "ms"
            | "µs"
            | "μs"
            | "ns"
            | "%"
            | "db"
            | "dbm"
            | "ns/v"
            | "v/µs"
            | "v/μs"
            | "ha"
    )
}

fn canonical_unit(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "vv" => "V".to_owned(),
        "ww" => "W".to_owned(),
        "ha" => "μA".to_owned(),
        "mq" => "mΩ".to_owned(),
        _ => clean_text(value),
    }
}

fn is_rating_value(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || value.len() > 45 {
        return false;
    }
    if matches!(value, "-" | "—" | "−" | "–" | "⎯") {
        return true;
    }
    let lower = value.to_ascii_lowercase();
    if lower.contains("internally limited") || lower.contains("not limited") {
        return true;
    }
    let begins_like_value = value
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_digit() || "+-−–±.<>".contains(character));
    begins_like_value
        || (lower.contains(" to ") && value.chars().any(|character| character.is_ascii_digit()))
        || ((value.contains("VIN") || value.contains("VCC") || value.contains("VDD"))
            && value.chars().any(|character| character.is_ascii_digit()))
}

fn is_rating_value_for(value: &str, allow_symbolic_values: bool) -> bool {
    if !allow_symbolic_values {
        let has_letters = value.chars().any(char::is_alphabetic);
        let starts_with_letter = value.chars().next().is_some_and(char::is_alphabetic);
        if starts_with_letter
            || (has_letters && (value.contains('<') || value.contains('>') || value.contains('=')))
        {
            return false;
        }
    }
    is_rating_value(value)
}

fn is_standalone_rating_value(value: &str) -> bool {
    let value = value.trim();
    matches!(value, "-" | "—" | "−" | "–" | "⎯")
        || value.to_ascii_lowercase().contains("internally limited")
        || value
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit() || "+-−–±.<>".contains(character))
}

fn looks_like_symbol(value: &str) -> bool {
    let value = strip_reference(value);
    let word_count = value.split_whitespace().count();
    let compact_symbol = word_count == 1 && value.chars().count() <= 4;
    let uppercase_symbol = value
        .chars()
        .filter(char::is_ascii_alphabetic)
        .all(|character| character.is_ascii_uppercase());
    value.len() <= 24
        && !value.contains(['=', '<', '>'])
        && !value.contains("°C")
        && word_count <= 3
        && (compact_symbol || uppercase_symbol)
        && value.chars().any(char::is_alphabetic)
        && !value.to_ascii_lowercase().contains("voltage")
        && !value.to_ascii_lowercase().contains("current")
        && !value.to_ascii_lowercase().contains("temperature")
        && !value.to_ascii_lowercase().contains("power")
}

fn looks_like_electrical_condition(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.contains(['=', '<', '>'])
        || TEMPERATURE_RE.is_match(value)
        || [
            "source,",
            "sink,",
            "turning on",
            "turning off",
            "hysteresis",
            "falling",
            "rising",
            "wake up",
            "no load",
            "continuous",
            "auto skip",
            "delay for",
            "pg in",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

fn join_conditions(first: Option<String>, second: Option<String>) -> Option<String> {
    let values = [first, second]
        .into_iter()
        .flatten()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.join("; "))
}

fn contains_word(haystack: &str, needle: &str) -> bool {
    haystack
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|word| word == needle || (needle == "TYP" && word == "TYP."))
}

fn dash_to_none(value: &str) -> Option<String> {
    (!is_dash(value)).then(|| value.trim().to_owned())
}

fn is_dash(value: &str) -> bool {
    matches!(value.trim(), "-" | "—" | "−" | "–" | "⎯")
}

fn join_optional(values: &[String]) -> Option<String> {
    (!values.is_empty()).then(|| values.join(" "))
}

fn strip_reference(value: &str) -> String {
    Regex::new(r"\s*(?:\(\d+\)|\[\d+\])\s*$")
        .expect("reference regex")
        .replace(value.trim(), "")
        .trim()
        .to_owned()
}

fn clean_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(['|', ':'])
        .trim()
        .to_owned()
}

fn join_sentence(target: &mut String, continuation: &str) {
    if continuation.is_empty() {
        return;
    }
    if target.ends_with('-') && !target.ends_with(" -") {
        target.pop();
    } else if !target.is_empty() {
        target.push(' ');
    }
    target.push_str(continuation.trim());
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !value.is_empty()
        && !values
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&value))
    {
        values.push(value);
    }
}

fn pin_number_fits_package(number: &str, package: &str) -> bool {
    let Some(number) = number.parse::<u16>().ok() else {
        return true;
    };
    let declared = package
        .rsplit('-')
        .next()
        .and_then(|value| value.parse::<u16>().ok());
    declared.is_none_or(|maximum| number <= maximum)
}

fn common_prefix_length(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names_are_canonicalized_and_unique() {
        assert_eq!(
            extract_packages("up to 97% in 8-Pin WSON, WSON (8), SOT-23-6 and TSSOP8"),
            vec!["WSON-8", "SOT-23-6", "TSSOP-8"]
        );
    }

    #[test]
    fn parses_name_first_and_pin_first_rows() {
        let ti = parse_pin_line("AGND                  3           I      Analog GND supply pin.");
        assert_eq!(ti[0].0, "3");
        assert_eq!(ti[0].1.name, "AGND");
        assert_eq!(ti[0].1.pin_type, "I");

        let nxp = parse_pin_line("1               input (base)");
        assert_eq!(nxp[0].0, "1");
        assert_eq!(nxp[0].1.name, "INPUT");
        assert_eq!(nxp[0].1.pin_type, "Input");
    }

    #[test]
    fn parses_min_typ_max_rating() {
        let header = parse_rating_header("Symbol Parameter Conditions Min Typ Max Unit").unwrap();
        let cells = vec!["R1", "bias resistor", "70", "100", "130", "kΩ"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let row = parse_rating_row(&cells, header, "R1 bias resistor 70 100 130 kΩ", true).unwrap();
        assert_eq!(row.symbol.as_deref(), Some("R1"));
        assert_eq!(row.min.as_deref(), Some("70"));
        assert_eq!(row.typ.as_deref(), Some("100"));
        assert_eq!(row.max.as_deref(), Some("130"));
    }

    #[test]
    fn parses_ocr_rating_with_conditions_after_unit() {
        let columns = RatingColumns {
            has_min: true,
            has_typ: true,
            has_max: true,
            symbol_first: false,
            parameter_first: true,
            ..RatingColumns::default()
        };
        let cells = GAP_RE
            .split("Continuous drain current    ID    -    -    39    A    VGS=4.5 V, TC=25 °C")
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let row = parse_rating_row(&cells, columns, &cells.join(" "), false).unwrap();
        assert_eq!(row.symbol.as_deref(), Some("ID"));
        assert_eq!(row.max.as_deref(), Some("39"));
        assert_eq!(row.unit.as_deref(), Some("A"));
        assert_eq!(row.conditions.as_deref(), Some("VGS=4.5 V, TC=25 °C"));
    }

    #[test]
    fn recognizes_numbered_headings() {
        assert_eq!(
            classify_heading("7.1 Absolute Maximum Ratings"),
            Some(SectionKind::AbsoluteMaximum)
        );
        assert_eq!(
            classify_heading("2. Pinning information"),
            Some(SectionKind::Pins)
        );
        assert_eq!(
            classify_heading("TERMINAL FUNCTIONS (continued)"),
            Some(SectionKind::Pins)
        );
        assert_eq!(classify_heading("PINOUT"), Some(SectionKind::Pins));
        assert_eq!(
            classify_heading("MOSFET MAXIMUM RATINGS (TC = 25°C, unless otherwise noted)"),
            Some(SectionKind::AbsoluteMaximum)
        );
        assert_eq!(
            classify_heading("THERMAL CHARACTERISTICS"),
            Some(SectionKind::Thermal)
        );
        assert_eq!(
            classify_heading("ELECTRICAL CHARACTERISTICS (continued)"),
            Some(SectionKind::Electrical)
        );
        assert_eq!(
            classify_heading("7. Characteristics"),
            Some(SectionKind::Electrical)
        );
    }

    #[test]
    fn parses_multi_page_electrical_characteristics() {
        let text = concat!(
            "ELECTRICAL CHARACTERISTICS\n",
            "SYMBOL    PARAMETER    TEST CONDITIONS    MIN    TYP    MAX    UNIT\n",
            "IQ    Quiescent supply current    VIN = 5 V    1    2    3    mA\n",
            "\u{c}ELECTRICAL CHARACTERISTICS (continued)\n",
            "SYMBOL    PARAMETER    TEST CONDITIONS    MIN    TYP    MAX    UNIT\n",
            "VOH    High-level output voltage    IOH = -4 mA    2.4    3.0    3.3    V\n"
        );
        let lines = text.lines().collect::<Vec<_>>();
        let rows = extract_electrical_characteristics(&lines);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].symbol.as_deref(), Some("IQ"));
        assert_eq!(rows[0].typ.as_deref(), Some("2"));
        assert_eq!(rows[1].symbol.as_deref(), Some("VOH"));
        assert_eq!(rows[1].max.as_deref(), Some("3.3"));
    }

    #[test]
    fn standalone_reference_marker_does_not_end_a_rating_table() {
        assert!(!is_footnote("(4)"));
        assert!(is_footnote(
            "(4) Stresses beyond those listed may cause damage."
        ));
    }
}
