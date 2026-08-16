use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Datasheet {
    pub schema_version: String,
    pub general: General,
    pub pin_configuration: BTreeMap<String, BTreeMap<String, Pin>>,
    pub absolute_maximum_ratings: Vec<Rating>,
    pub recommended_operating_conditions: Vec<Rating>,
    pub metadata: ParseMetadata,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct General {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mfr: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applications: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Pin {
    pub name: String,
    #[serde(rename = "type")]
    pub pin_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Rating {
    pub parameter: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typ: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ParseMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    pub total_pages: usize,
    pub pages_processed: usize,
    pub extraction_method: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ocr_pages: Vec<usize>,
    pub confidence: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}
