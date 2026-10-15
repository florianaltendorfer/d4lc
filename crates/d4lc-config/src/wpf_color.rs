//! The JSON shape of a WPF `System.Windows.Media.Color`.
//!
//! Upstream stores colors as `Color` values (`SettingsD4.cs:18-24`,
//! `MultiBuild.cs:7`, `ItemAffix.cs:12`) and writes them with
//! `System.Text.Json` defaults (`SettingsManager.cs:66`). That serializer
//! writes every public readable property of the struct, in declaration order
//! (dotnet/wpf `PresentationCore/System/Windows/Media/Color.cs`):
//!
//! ```json
//! {
//!   "ColorContext": null,
//!   "A": 255, "R": 0, "G": 128, "B": 0,
//!   "ScA": 1, "ScR": 0, "ScG": 0.21586053, "ScB": 0
//! }
//! ```
//!
//! `ColorContext` is the read-only ICC profile and is `null` for every color
//! built from bytes (`Colors.Green`, `FromRgb`). The `Sc*` values are the
//! linear scRGB channels `Color` derives from the sRGB bytes
//! (`sRgbToScRgb` in the same file), written as `float` shortest round-trip
//! numbers. Reading goes through the byte properties, which also reproduce
//! the scRGB values, so only `R`, `G`, `B` (and `A`) are needed.

use crate::json;
use crate::presets::PresetColor;
use serde_json::Value;

/// The upstream JSON object of `color`.
pub(crate) fn color_object(color: PresetColor) -> json::Object {
    json::Object::new()
        .field("ColorContext", Value::Null)
        .field("A", color.a)
        .field("R", color.r)
        .field("G", color.g)
        .field("B", color.b)
        .field("ScA", sc_value(f32::from(color.a) / 255.0))
        .field("ScR", sc_value(srgb_to_scrgb(color.r)))
        .field("ScG", sc_value(srgb_to_scrgb(color.g)))
        .field("ScB", sc_value(srgb_to_scrgb(color.b)))
}

/// A color from the upstream object (`A`/`R`/`G`/`B` bytes) or from d4lc's
/// older string form (`#RRGGBB` or a basic color name). `None` when the value
/// is neither.
pub(crate) fn color_from_json(value: &Value) -> Option<PresetColor> {
    match value {
        Value::Object(object) => {
            let byte = |key: &str| {
                object
                    .get(key)
                    .and_then(Value::as_u64)
                    .and_then(|value| u8::try_from(value).ok())
            };
            Some(PresetColor {
                a: byte("A").unwrap_or(255),
                r: byte("R")?,
                g: byte("G")?,
                b: byte("B")?,
            })
        }
        Value::String(text) => PresetColor::try_parse(text).ok(),
        _ => None,
    }
}

/// `Color.sRgbToScRgb`: the linear value of an sRGB byte, computed in `float`
/// with the power in `double` as the WPF source does.
fn srgb_to_scrgb(byte: u8) -> f32 {
    let value = f32::from(byte) / 255.0;
    if value <= 0.0 {
        0.0
    } else if value <= 0.04045 {
        value / 12.92
    } else if value < 1.0 {
        ((f64::from(value) + 0.055) / 1.055).powf(2.4) as f32
    } else {
        1.0
    }
}

/// A `float` as the shortest decimal that round-trips, the way .NET writes it
/// (`1`, not `1.0`).
fn sc_value(value: f32) -> Value {
    json::number(value.to_string().parse().unwrap_or(0.0))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn green_is_written_as_the_media_color_object() {
        let text = json::to_file_text(&color_object(PresetColor::GREEN));
        assert_eq!(
            text,
            "{\n  \"ColorContext\": null,\n  \"A\": 255,\n  \"R\": 0,\n  \"G\": 128,\n  \"B\": 0,\n  \"ScA\": 1,\n  \"ScR\": 0,\n  \"ScG\": 0.21586053,\n  \"ScB\": 0\n}\n"
        );
    }

    #[test]
    fn scrgb_channels_follow_the_wpf_conversion() {
        let value = serde_json::to_value(color_object(PresetColor {
            a: 128,
            r: 255,
            g: 170,
            b: 10,
        }))
        .expect("color object serializes");
        assert_eq!(value["ScA"], serde_json::json!(0.5019608));
        assert_eq!(value["ScR"], serde_json::json!(1));
        assert_eq!(value["ScG"], serde_json::json!(0.4019778));
        assert_eq!(value["ScB"], serde_json::json!(0.00303527));
    }

    #[test]
    fn object_and_string_forms_read_back() {
        let object = serde_json::to_value(color_object(PresetColor::from_rgb(255, 170, 0)))
            .expect("color object serializes");
        assert_eq!(
            color_from_json(&object),
            Some(PresetColor::from_rgb(255, 170, 0))
        );
        assert_eq!(
            color_from_json(&serde_json::json!("#FFAA00")),
            Some(PresetColor::from_rgb(255, 170, 0))
        );
        assert_eq!(
            color_from_json(&serde_json::json!("Blue")),
            Some(PresetColor::from_rgb(0, 0, 255))
        );
        assert_eq!(
            color_from_json(&serde_json::json!({ "A": 10, "R": 1, "G": 2, "B": 3 })),
            Some(PresetColor {
                a: 10,
                r: 1,
                g: 2,
                b: 3
            })
        );
        assert_eq!(color_from_json(&serde_json::json!({})), None);
        assert_eq!(color_from_json(&serde_json::json!("ultramarine")), None);
        assert_eq!(color_from_json(&serde_json::json!(3)), None);
    }
}
