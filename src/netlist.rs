use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{Datasheet, Rating};

static NUMBER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[-+]?\d+(?:[.,]\d+)?(?:[eE][-+]?\d+)?").expect("valid number regex")
});
static TEMPERATURE_CONDITION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bT(?:C|A)\s*(?:=|:)?\s*([+−–-]?\d+(?:[.,]\d+)?)\s*°?C?\b")
        .expect("valid temperature condition regex")
});
static IDENTIFIER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z][A-Za-z0-9_+\-]{0,23}").expect("valid identifier regex"));

/// Options for the datasheet-derived custom `.smoke` constraint profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SmokeOptions {
    /// Override the name following `.smoke`. The parsed part name is used by default.
    pub device_name: Option<String>,
    /// Project derating policy. Datasheet limits themselves are emitted unchanged.
    pub derate: f64,
    /// Default case-temperature reference for current and power ratings.
    pub reference_temperature_c: f64,
}

impl Default for SmokeOptions {
    fn default() -> Self {
        Self {
            device_name: None,
            derate: 0.8,
            reference_temperature_c: 25.0,
        }
    }
}

/// Backward-compatible name retained for Rust and C-ABI callers.
pub type NetlistOptions = SmokeOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quantity {
    Voltage,
    Current,
    Power,
    Temperature,
    ThermalResistance,
}

#[derive(Debug, Clone, Copy)]
enum LimitSide {
    Minimum,
    Maximum,
}

#[derive(Debug, Clone)]
struct Constraint {
    value: f64,
    qualifiers: Vec<(String, f64)>,
}

/// Generate a compact custom dscapture `.smoke` constraint profile.
///
/// Values are taken from Absolute Maximum Ratings. The only policy value added
/// by the generator is `DERATE`; it is deliberately kept separate from the raw
/// datasheet values. The output is intended for a dscapture-aware preprocessor,
/// not for direct execution by stock NGSpice.
pub fn to_smoke_profile(datasheet: &Datasheet, options: &SmokeOptions) -> Result<String> {
    validate_options(options)?;

    let device_name = options
        .device_name
        .as_deref()
        .or(datasheet.general.name.as_deref())
        .unwrap_or("UNKNOWN_DEVICE");
    let device_name = smoke_identifier(device_name);
    if device_name.is_empty() {
        return Err(Error::InvalidOptions(
            "smoke device name must contain at least one letter or digit".to_owned(),
        ));
    }

    let ratings = &datasheet.absolute_maximum_ratings;
    let mut constraints = BTreeMap::<String, Constraint>::new();
    let mut preferred_order = Vec::<String>::new();
    let mut consumed = HashSet::<usize>::new();

    if let Some((index, rating)) = best_rating(ratings, rating_score_vds)
        && let Some(value) = rating_magnitude(rating, Quantity::Voltage)
    {
        insert_constraint(
            &mut constraints,
            &mut preferred_order,
            "VDS_MAX",
            value.abs(),
            Vec::new(),
        );
        consumed.insert(index);
    }

    if let Some((index, rating)) = best_rating(ratings, rating_score_vgs) {
        if let Some(value) = rating_value(rating, LimitSide::Maximum, Quantity::Voltage) {
            insert_constraint(
                &mut constraints,
                &mut preferred_order,
                "VGS_POS_MAX",
                value.abs(),
                Vec::new(),
            );
        }
        let negative = rating_value(rating, LimitSide::Minimum, Quantity::Voltage);
        if let Some(value) = negative.filter(|value| *value < 0.0) {
            insert_constraint(
                &mut constraints,
                &mut preferred_order,
                "VGS_NEG_MAX",
                value,
                Vec::new(),
            );
        }
        consumed.insert(index);
    }

    if let Some((index, rating)) = best_rating(ratings, rating_score_continuous_drain_current)
        && let Some(value) = rating_magnitude(rating, Quantity::Current)
    {
        insert_constraint(
            &mut constraints,
            &mut preferred_order,
            "ID_CONT_MAX",
            value.abs(),
            vec![(
                "TC_REF".to_owned(),
                rating_reference_temperature(rating).unwrap_or(options.reference_temperature_c),
            )],
        );
        consumed.insert(index);
    }

    if let Some((index, rating)) = best_rating(ratings, rating_score_pulsed_drain_current)
        && let Some(value) = rating_magnitude(rating, Quantity::Current)
    {
        insert_constraint(
            &mut constraints,
            &mut preferred_order,
            "ID_PULSE_MAX",
            value.abs(),
            Vec::new(),
        );
        consumed.insert(index);
    }

    if let Some((index, rating)) = best_rating(ratings, rating_score_power_dissipation)
        && let Some(value) = rating_magnitude(rating, Quantity::Power)
    {
        insert_constraint(
            &mut constraints,
            &mut preferred_order,
            "PD_MAX",
            value.abs(),
            vec![(
                "TC_REF".to_owned(),
                rating_reference_temperature(rating).unwrap_or(options.reference_temperature_c),
            )],
        );
        consumed.insert(index);
    }

    if let Some((index, rating)) = best_rating(ratings, rating_score_junction_temperature)
        && let Some(value) = rating_value(rating, LimitSide::Maximum, Quantity::Temperature)
    {
        insert_constraint(
            &mut constraints,
            &mut preferred_order,
            "TJ_MAX",
            value,
            Vec::new(),
        );
        consumed.insert(index);
    }

    let thermal_ratings = datasheet
        .absolute_maximum_ratings
        .iter()
        .chain(&datasheet.recommended_operating_conditions)
        .chain(&datasheet.thermal_characteristics)
        .chain(&datasheet.electrical_characteristics)
        .collect::<Vec<_>>();
    if let Some((_, rating)) = best_rating_refs(&thermal_ratings, rating_score_rth_jc)
        && let Some(value) = rating_magnitude(rating, Quantity::ThermalResistance)
    {
        insert_constraint(
            &mut constraints,
            &mut preferred_order,
            "RTH_JC",
            value.abs(),
            Vec::new(),
        );
    }

    // ICs and other non-transistor parts often have per-pin voltage limits
    // rather than VDS/VGS/ID. Preserve those limits as SYMBOL_MIN/MAX entries.
    for (index, rating) in ratings.iter().enumerate() {
        if consumed.contains(&index) {
            continue;
        }
        let Some(quantity) = generic_quantity(rating) else {
            continue;
        };
        for identifier in rating_identifiers(rating) {
            if let Some(value) = rating_value(rating, LimitSide::Minimum, quantity) {
                insert_constraint(
                    &mut constraints,
                    &mut preferred_order,
                    &format!("{identifier}_MIN"),
                    value,
                    Vec::new(),
                );
            }
            if let Some(value) = rating_value(rating, LimitSide::Maximum, quantity) {
                insert_constraint(
                    &mut constraints,
                    &mut preferred_order,
                    &format!("{identifier}_MAX"),
                    value,
                    Vec::new(),
                );
            }
        }
    }

    let mut output = String::new();
    push_line(&mut output, "* Generated from datasheet");
    push_line(
        &mut output,
        "* Custom dscapture directive — requires preprocessing",
    );
    push_line(&mut output, &format!(".smoke {device_name}"));
    for key in preferred_order {
        let Some(constraint) = constraints.get(&key) else {
            continue;
        };
        let mut line = format!("+ {key}={}", format_number(constraint.value));
        for (qualifier, value) in &constraint.qualifiers {
            line.push(' ');
            line.push_str(qualifier);
            line.push('=');
            line.push_str(&format_number(*value));
        }
        push_line(&mut output, &line);
    }
    push_line(
        &mut output,
        &format!("+ DERATE={}", format_number(options.derate)),
    );
    Ok(output)
}

/// Backward-compatible function name. It now returns the custom `.smoke`
/// profile rather than a standalone NGSpice testbench.
pub fn to_ngspice_netlist(datasheet: &Datasheet, options: &NetlistOptions) -> Result<String> {
    to_smoke_profile(datasheet, options)
}

fn validate_options(options: &SmokeOptions) -> Result<()> {
    if !options.derate.is_finite() || options.derate <= 0.0 || options.derate > 1.0 {
        return Err(Error::InvalidOptions(
            "derate must be a finite number greater than 0 and at most 1".to_owned(),
        ));
    }
    if !options.reference_temperature_c.is_finite()
        || options.reference_temperature_c < -273.15
        || options.reference_temperature_c > 1000.0
    {
        return Err(Error::InvalidOptions(
            "reference_temperature_c must be between -273.15 and 1000".to_owned(),
        ));
    }
    if options
        .device_name
        .as_deref()
        .is_some_and(|name| name.contains(['\n', '\r', '\0']))
    {
        return Err(Error::InvalidOptions(
            "device_name must not contain control characters".to_owned(),
        ));
    }
    Ok(())
}

fn best_rating(ratings: &[Rating], score: fn(&Rating) -> usize) -> Option<(usize, &Rating)> {
    ratings
        .iter()
        .enumerate()
        .filter_map(|(index, rating)| {
            let score = score(rating);
            (score > 0).then_some((score, index, rating))
        })
        .max_by_key(|(score, _, _)| *score)
        .map(|(_, index, rating)| (index, rating))
}

fn best_rating_refs<'a>(
    ratings: &[&'a Rating],
    score: fn(&Rating) -> usize,
) -> Option<(usize, &'a Rating)> {
    ratings
        .iter()
        .enumerate()
        .filter_map(|(index, rating)| {
            let score = score(rating);
            (score > 0).then_some((score, index, *rating))
        })
        .max_by_key(|(score, _, _)| *score)
        .map(|(_, index, rating)| (index, rating))
}

fn rating_score_vds(rating: &Rating) -> usize {
    if !rating_has_quantity(rating, Quantity::Voltage) {
        return 0;
    }
    semantic_score(
        rating,
        &["VDS", "VDSS"],
        &["drain source voltage", "drain-source voltage"],
    )
}

fn rating_score_vgs(rating: &Rating) -> usize {
    if !rating_has_quantity(rating, Quantity::Voltage) {
        return 0;
    }
    semantic_score(
        rating,
        &["VGS", "VGSS"],
        &["gate source voltage", "gate-source voltage"],
    )
}

fn rating_score_continuous_drain_current(rating: &Rating) -> usize {
    let evidence = rating_evidence(rating);
    if !rating_has_quantity(rating, Quantity::Current)
        || evidence.contains("pulse")
        || compact_symbol(rating).contains("IDM")
    {
        return 0;
    }
    semantic_score(
        rating,
        &["ID", "IDCONT"],
        &[
            "continuous drain current",
            "drain current continuous",
            "drain current (dc)",
        ],
    )
}

fn rating_score_pulsed_drain_current(rating: &Rating) -> usize {
    let base = semantic_score(
        rating,
        &["IDM", "IDPULSE", "IDPULS"],
        &[
            "pulsed drain current",
            "drain current pulsed",
            "drain current (pulsed)",
            "pulse drain current",
        ],
    );
    if base > 0 && rating_has_quantity(rating, Quantity::Current) {
        base
    } else if rating_evidence(rating).contains("pulsed") && rating.unit.is_none() {
        80
    } else {
        0
    }
}

fn rating_score_power_dissipation(rating: &Rating) -> usize {
    if !rating_has_quantity(rating, Quantity::Power) {
        return 0;
    }
    let base = semantic_score(
        rating,
        &["PD", "PTOT", "PDMAX"],
        &["power dissipation", "total power", "power rating"],
    );
    if base == 0 {
        return 0;
    }
    let evidence = rating_evidence(rating);
    base + usize::from(evidence.contains("tc =") || evidence.contains("tc=")) * 30
}

fn rating_score_junction_temperature(rating: &Rating) -> usize {
    if !rating_has_quantity(rating, Quantity::Temperature) {
        return 0;
    }
    semantic_score(
        rating,
        &["TJ", "TJMAX", "TCH"],
        &[
            "junction temperature",
            "operating junction",
            "channel temperature",
        ],
    )
}

fn rating_score_rth_jc(rating: &Rating) -> usize {
    if !rating_has_quantity(rating, Quantity::ThermalResistance) {
        return 0;
    }
    semantic_score(
        rating,
        &["RTHJC", "RΘJC", "ΘJC"],
        &[
            "junction to case thermal resistance",
            "junction-to-case thermal resistance",
            "thermal resistance junction case",
        ],
    )
}

fn rating_has_quantity(rating: &Rating, quantity: Quantity) -> bool {
    unit_scale(rating.unit.as_deref(), quantity).is_some()
}

fn semantic_score(rating: &Rating, symbols: &[&str], phrases: &[&str]) -> usize {
    let symbol = compact_symbol(rating);
    let evidence = rating_evidence(rating);
    if symbols.iter().any(|candidate| symbol == *candidate) {
        200
    } else if symbols
        .iter()
        .any(|candidate| symbol.split(',').any(|part| part == *candidate))
    {
        170
    } else if phrases.iter().any(|phrase| evidence.contains(phrase)) {
        140
    } else {
        0
    }
}

fn compact_symbol(rating: &Rating) -> String {
    rating
        .symbol
        .as_deref()
        .unwrap_or_default()
        .chars()
        .filter(|character| character.is_alphanumeric() || *character == ',' || *character == 'Θ')
        .flat_map(char::to_uppercase)
        .collect()
}

fn rating_evidence(rating: &Rating) -> String {
    format!(
        "{} {} {} {}",
        rating.parameter,
        rating.symbol.as_deref().unwrap_or_default(),
        rating.conditions.as_deref().unwrap_or_default(),
        rating.source
    )
    .replace(['−', '–', '—'], "-")
    .to_ascii_lowercase()
}

fn rating_value(rating: &Rating, side: LimitSide, quantity: Quantity) -> Option<f64> {
    let direct = match side {
        LimitSide::Minimum => rating.min.as_deref(),
        LimitSide::Maximum => rating.max.as_deref(),
    };
    let value = if let Some(raw) = direct {
        parse_number(raw)?
    } else {
        value_bound(rating.value.as_deref()?, side)?
    };
    Some(value * unit_scale(rating.unit.as_deref(), quantity)?)
}

fn rating_magnitude(rating: &Rating, quantity: Quantity) -> Option<f64> {
    let raw = rating
        .max
        .as_deref()
        .or(rating.value.as_deref())
        .or(rating.min.as_deref())?;
    Some(parse_number(raw)?.abs() * unit_scale(rating.unit.as_deref(), quantity)?)
}

fn value_bound(value: &str, side: LimitSide) -> Option<f64> {
    let normalized = value.replace(['−', '–', '—'], "-").replace(',', ".");
    let values = NUMBER_RE
        .find_iter(&normalized)
        .filter_map(|matched| matched.as_str().parse::<f64>().ok())
        .collect::<Vec<_>>();
    if normalized.contains(['±', '∓']) {
        let magnitude = values.first()?.abs();
        return Some(match side {
            LimitSide::Minimum => -magnitude,
            LimitSide::Maximum => magnitude,
        });
    }
    if values.len() >= 2 {
        return match side {
            LimitSide::Minimum => values.iter().copied().reduce(f64::min),
            LimitSide::Maximum => values.iter().copied().reduce(f64::max),
        };
    }
    let value = *values.first()?;
    match side {
        LimitSide::Minimum if value < 0.0 => Some(value),
        LimitSide::Maximum if value >= 0.0 => Some(value),
        _ => None,
    }
}

fn parse_number(value: &str) -> Option<f64> {
    let normalized = value.replace(['−', '–', '—'], "-").replace(',', ".");
    NUMBER_RE.find(&normalized)?.as_str().parse::<f64>().ok()
}

fn unit_scale(unit: Option<&str>, quantity: Quantity) -> Option<f64> {
    let Some(unit) = unit else {
        return Some(1.0);
    };
    let unit = unit
        .trim()
        .replace(['μ', 'µ'], "u")
        .replace('Ω', "ohm")
        .replace('Θ', "theta")
        .to_ascii_lowercase();
    match quantity {
        Quantity::Voltage => match unit.as_str() {
            "v" => Some(1.0),
            "mv" => Some(1e-3),
            "kv" => Some(1e3),
            _ => None,
        },
        Quantity::Current => match unit.as_str() {
            "a" => Some(1.0),
            "ma" => Some(1e-3),
            "ua" => Some(1e-6),
            "ka" => Some(1e3),
            _ => None,
        },
        Quantity::Power => match unit.as_str() {
            "w" => Some(1.0),
            "mw" => Some(1e-3),
            "kw" => Some(1e3),
            _ => None,
        },
        Quantity::Temperature => match unit.as_str() {
            "°c" | "c" | "degc" => Some(1.0),
            _ => None,
        },
        Quantity::ThermalResistance => {
            if unit.contains("/w") || unit.contains("per watt") {
                Some(1.0)
            } else {
                None
            }
        }
    }
}

fn rating_reference_temperature(rating: &Rating) -> Option<f64> {
    [
        rating.conditions.as_deref(),
        rating.temperature.as_deref(),
        Some(rating.source.as_str()),
    ]
    .into_iter()
    .flatten()
    .find_map(|value| {
        let captures = TEMPERATURE_CONDITION_RE.captures(value)?;
        parse_number(captures.get(1)?.as_str())
    })
}

fn generic_quantity(rating: &Rating) -> Option<Quantity> {
    let unit = rating.unit.as_deref()?.trim().to_ascii_lowercase();
    match unit.as_str() {
        "v" | "mv" | "kv" => Some(Quantity::Voltage),
        "a" | "ma" | "µa" | "μa" | "ua" | "ka" => Some(Quantity::Current),
        "w" | "mw" | "kw" => Some(Quantity::Power),
        "°c" | "c" | "degc" => Some(Quantity::Temperature),
        _ => None,
    }
}

fn rating_identifiers(rating: &Rating) -> Vec<String> {
    if rating.parameter.to_ascii_lowercase().contains("(to ")
        || rating.parameter.to_ascii_lowercase().contains("(wrt ")
    {
        let mut relation = Vec::new();
        for matched in IDENTIFIER_RE.find_iter(&rating.parameter) {
            let raw = matched.as_str();
            let uppercase = raw.to_ascii_uppercase();
            if raw == uppercase || matches!(uppercase.as_str(), "TO" | "WRT") {
                relation.push(uppercase);
            }
        }
        if relation.len() >= 3 {
            return vec![relation.join("_")];
        }
    }

    let mut result = Vec::new();
    let source_text = rating.source.to_ascii_uppercase();
    for (source, is_symbol_field) in [
        (rating.symbol.as_deref(), true),
        (Some(rating.parameter.as_str()), false),
    ] {
        let Some(source) = source else {
            continue;
        };
        for matched in IDENTIFIER_RE.find_iter(source) {
            let raw = matched.as_str();
            let uppercase = raw.to_ascii_uppercase();
            let symbol_like = raw == uppercase
                || is_symbol_field
                || matches!(uppercase.as_str(), "TJ" | "TSTG" | "TA" | "TC");
            if symbol_like
                && !matches!(uppercase.as_str(), "V" | "A" | "W" | "C" | "TO")
                && source_text.contains(&uppercase)
                && !result.contains(&uppercase)
            {
                result.push(uppercase);
            }
        }
    }
    result
}

fn insert_constraint(
    constraints: &mut BTreeMap<String, Constraint>,
    order: &mut Vec<String>,
    key: &str,
    value: f64,
    qualifiers: Vec<(String, f64)>,
) {
    if !value.is_finite() || constraints.contains_key(key) {
        return;
    }
    constraints.insert(key.to_owned(), Constraint { value, qualifiers });
    order.push(key.to_owned());
}

fn smoke_identifier(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_owned()
}

fn format_number(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    let mut rendered = format!("{value:.9}");
    while rendered.contains('.') && rendered.ends_with('0') {
        rendered.pop();
    }
    if rendered.ends_with('.') {
        rendered.pop();
    }
    rendered
}

fn push_line(output: &mut String, line: &str) {
    output.push_str(line);
    output.push('\n');
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::model::{General, ParseMetadata};

    use super::*;

    fn rating(
        parameter: &str,
        symbol: &str,
        min: Option<&str>,
        max: &str,
        unit: &str,
        conditions: Option<&str>,
    ) -> Rating {
        Rating {
            parameter: parameter.to_owned(),
            symbol: Some(symbol.to_owned()),
            conditions: conditions.map(str::to_owned),
            min: min.map(str::to_owned),
            max: Some(max.to_owned()),
            unit: Some(unit.to_owned()),
            source: format!("{symbol} {} to {max} {unit}", min.unwrap_or("0")),
            ..Rating::default()
        }
    }

    fn sample_datasheet() -> Datasheet {
        Datasheet {
            schema_version: "1.1".to_owned(),
            general: General {
                name: Some("MOS TEST-1".to_owned()),
                ..General::default()
            },
            pin_configuration: BTreeMap::new(),
            absolute_maximum_ratings: vec![
                rating("Drain-source voltage", "VDS", None, "650", "V", None),
                rating("Gate-source voltage", "VGS", Some("-20"), "20", "V", None),
                rating(
                    "Continuous drain current",
                    "ID",
                    None,
                    "60000",
                    "mA",
                    Some("TC = 25 °C"),
                ),
                rating("Pulsed drain current", "IDM", None, "240", "A", None),
                rating(
                    "Power dissipation",
                    "PD",
                    None,
                    "150",
                    "W",
                    Some("TC = 25 °C"),
                ),
                rating(
                    "Operating junction temperature",
                    "TJ",
                    Some("-55"),
                    "175",
                    "°C",
                    None,
                ),
            ],
            recommended_operating_conditions: Vec::new(),
            electrical_characteristics: Vec::new(),
            thermal_characteristics: vec![rating(
                "Junction-to-case thermal resistance",
                "RthJC",
                None,
                "1.0",
                "°C/W",
                None,
            )],
            metadata: ParseMetadata::default(),
        }
    }

    #[test]
    fn generates_compact_smoke_profile() {
        let profile = to_smoke_profile(&sample_datasheet(), &SmokeOptions::default()).unwrap();
        assert_eq!(
            profile,
            concat!(
                "* Generated from datasheet\n",
                "* Custom dscapture directive — requires preprocessing\n",
                ".smoke MOS_TEST-1\n",
                "+ VDS_MAX=650\n",
                "+ VGS_POS_MAX=20\n",
                "+ VGS_NEG_MAX=-20\n",
                "+ ID_CONT_MAX=60 TC_REF=25\n",
                "+ ID_PULSE_MAX=240\n",
                "+ PD_MAX=150 TC_REF=25\n",
                "+ TJ_MAX=175\n",
                "+ RTH_JC=1\n",
                "+ DERATE=0.8\n",
            )
        );
        assert!(!profile.contains(".tran"));
        assert!(!profile.contains("XUUT"));
    }

    #[test]
    fn emits_generic_ic_voltage_limits() {
        let mut datasheet = sample_datasheet();
        datasheet.absolute_maximum_ratings = vec![rating(
            "Input voltage range",
            "VIN",
            Some("-0.3"),
            "30",
            "V",
            None,
        )];
        datasheet.electrical_characteristics.clear();
        let profile = to_smoke_profile(&datasheet, &SmokeOptions::default()).unwrap();
        assert!(profile.contains("- VIN_MIN=-0.3\n"));
        assert!(profile.contains("- VIN_MAX=30\n"));
        assert!(profile.ends_with("- DERATE=0.8\n"));
    }

    #[test]
    fn validates_derating_policy() {
        let options = SmokeOptions {
            derate: 1.1,
            ..SmokeOptions::default()
        };
        assert!(to_smoke_profile(&sample_datasheet(), &options).is_err());
    }

    #[test]
    fn handles_asymmetric_p_channel_limits_and_continuation_rows() {
        let mut datasheet = sample_datasheet();
        datasheet.absolute_maximum_ratings = vec![
            Rating {
                parameter: "Gate-source voltage".to_owned(),
                symbol: Some("VGSS".to_owned()),
                value: Some("-25/+20".to_owned()),
                unit: Some("V".to_owned()),
                source: "Gate-source voltage VGSS -25/+20 V".to_owned(),
                ..Rating::default()
            },
            Rating {
                parameter: "−Pulsed".to_owned(),
                value: Some("60".to_owned()),
                source: "−Pulsed 60".to_owned(),
                ..Rating::default()
            },
        ];
        datasheet.thermal_characteristics.clear();
        let profile = to_smoke_profile(&datasheet, &SmokeOptions::default()).unwrap();
        assert!(profile.contains("+ VGS_POS_MAX=20\n"));
        assert!(profile.contains("+ VGS_NEG_MAX=-25\n"));
        assert!(profile.contains("+ ID_PULSE_MAX=60\n"));
    }
}
