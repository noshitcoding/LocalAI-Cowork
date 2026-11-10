//! Validate original OCR result data; no recognition, publication or IO authority.
use super::*;

fn flag(value: &Value) -> Result<bool> {
    value.as_bool().ok_or_else(invalid)
}
fn list(value: &Value) -> Result<&Vec<Value>> {
    value.as_array().ok_or_else(invalid)
}
fn pin(language: &str) -> Result<&'static str> {
    match language {
        "deu" => Ok("19d219bbb6672c869d20a9636c6816a81eb9a71796cb93ebe0cb1530e2cdb22d"),
        "eng" => Ok("7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2"),
        _ => Err(invalid()),
    }
}
fn joined(words: &[Value]) -> Result<String> {
    let mut result = String::new();
    let mut previous: Option<(u64, u64)> = None;
    for word in words {
        let location = (integer(&word["page"])?, integer(&word["line"])?);
        if let Some(before) = previous {
            result.push_str(if location.0 != before.0 {
                "\n\n"
            } else if location != before {
                "\n"
            } else {
                " "
            });
        }
        result.push_str(text(&word["text"])?);
        previous = Some(location);
    }
    Ok(result)
}

pub(super) fn project(
    source: Value,
    raw: &Value,
    images: &Value,
    options: &Value,
    preview_count: u64,
) -> Result<Value> {
    if raw["format"] != "ocr" || sha(&raw["source_sha256"])? != sha(&source["sha256"])? {
        return Err(invalid());
    }
    let engine = &raw["engine"];
    let languages = list(&options["languages"])?;
    let models = engine["models_sha256"].as_object().ok_or_else(invalid)?;
    if engine["name"] != "tesseract"
        || engine["version"] != "tesseract 5.5.3"
        || engine["binding_version"] != "2.12.0"
        || engine["languages"] != options["languages"]
        || languages.is_empty()
        || languages.len() > 2
        || models.len() != languages.len()
    {
        return Err(invalid());
    }
    let mut seen_languages = std::collections::BTreeSet::new();
    let model_rows = languages
        .iter()
        .map(|language| {
            let language = text(language)?;
            if !seen_languages.insert(language)
                || models.get(language).and_then(Value::as_str) != Some(pin(language)?)
            {
                return Err(invalid());
            }
            Ok(json!({"language":language,"sha256":pin(language)?}))
        })
        .collect::<Result<Vec<_>>>()?;
    let total_pages = integer(&raw["total_pages"])?;
    if total_pages == 0 {
        return Err(invalid());
    }
    let expected_pages = if let Some(pages) = options.get("pages") {
        list(pages)?
            .iter()
            .map(integer)
            .collect::<Result<std::collections::BTreeSet<_>>>()?
            .into_iter()
            .collect::<Vec<_>>()
    } else {
        (1..=total_pages.min(8)).collect()
    };
    let pages = list(&raw["pages"])?;
    if pages.is_empty() || pages.len() > 16 || pages.len() != expected_pages.len() {
        return Err(invalid());
    }
    let pages_truncated = options.get("pages").is_none() && total_pages > 8;
    let expected_next_page = if pages_truncated {
        json!(9)
    } else {
        Value::Null
    };
    if flag(&raw["pages_truncated"])? != pages_truncated || raw["next_page"] != expected_next_page {
        return Err(invalid());
    }
    let mut page_data = BTreeMap::new();
    let mut total_words = 0u64;
    let mut total_pixels = 0u64;
    for (page, expected) in pages.iter().zip(expected_pages) {
        let number = integer(&page["page"])?;
        let width = integer(&page["width"])?;
        let height = integer(&page["height"])?;
        let count = integer(&page["word_count"])?;
        if number != expected
            || number == 0
            || number > total_pages
            || width == 0
            || height == 0
            || width > 100_000
            || height > 100_000
            || width.checked_mul(height).is_none_or(|n| n > 12_000_000)
            || page["dpi"] != options["dpi"]
            || page["rotation"] != options["rotation"]
            || count > 100_000
        {
            return Err(invalid());
        }
        sha(&page["raster_sha256"])?;
        total_pixels = total_pixels
            .checked_add(width * height)
            .ok_or_else(invalid)?;
        if total_pixels > 60_000_000 {
            return Err(invalid());
        }
        if !page["preview_sha256"].is_null() {
            sha(&page["preview_sha256"])?;
        }
        page_data.insert(number, (width, height, count, total_words));
        total_words = total_words.checked_add(count).ok_or_else(invalid)?;
    }
    if total_words > 100_000 || integer(&raw["total_words"])? != total_words {
        return Err(invalid());
    }
    let empty_images = Vec::new();
    let images = if images.is_null() {
        &empty_images
    } else {
        list(images)?
    };
    if preview_count != images.len() as u64 || images.len() > 16 {
        return Err(invalid());
    }
    let mut preview_pages = std::collections::BTreeSet::new();
    for image in images {
        let number = integer(&image["page"])?;
        let page = pages
            .iter()
            .find(|page| page["page"].as_u64() == Some(number))
            .ok_or_else(invalid)?;
        if !preview_pages.insert(number)
            || image["media_type"] != "image/png"
            || image["sha256"] != page["preview_sha256"]
            || integer(&image["width"])? == 0
            || integer(&image["height"])? == 0
        {
            return Err(invalid());
        }
        sha(&image["sha256"])?;
    }
    if pages.iter().any(|page| {
        !page["preview_sha256"].is_null()
            && !preview_pages.contains(&page["page"].as_u64().unwrap_or(0))
    }) {
        return Err(invalid());
    }
    let offset = integer(&raw["offset"])?;
    let words = list(&raw["words"])?;
    let limit = options["limit"].as_u64().unwrap_or(100);
    if offset != options["offset"].as_u64().unwrap_or(0)
        || offset > total_words
        || words.len() > 100
        || words.len() as u64 > limit
        || words.is_empty() && offset < total_words
    {
        return Err(invalid());
    }
    let mut previous: Option<(u64, u64)> = None;
    for (position, word) in words.iter().enumerate() {
        let number = integer(&word["page"])?;
        let (width, height, count, prefix) = *page_data.get(&number).ok_or_else(invalid)?;
        let index = integer(&word["index"])?;
        let line = integer(&word["line"])?;
        let content = text(&word["text"])?;
        let x = integer(&word["x"])?;
        let y = integer(&word["y"])?;
        let w = integer(&word["width"])?;
        let h = integer(&word["height"])?;
        let score = word["score"]
            .as_f64()
            .filter(|n| n.is_finite() && (0.0..=100.0).contains(n))
            .ok_or_else(invalid)?;
        if index >= count
            || prefix + index != offset + position as u64
            || line == 0
            || line > 100_000
            || content.is_empty()
            || content.chars().count() > 256
            || w == 0
            || h == 0
            || x.checked_add(w).is_none_or(|n| n > width)
            || y.checked_add(h).is_none_or(|n| n > height)
            || flag(&word["text_truncated"])? && content.chars().count() != 256
            || previous.is_some_and(|(page, before_line)| page == number && line < before_line)
        {
            return Err(invalid());
        }
        let _ = score;
        previous = Some((number, line));
    }
    let next = offset + words.len() as u64;
    if next > total_words {
        return Err(invalid());
    }
    let expected_next = if next < total_words {
        json!(next)
    } else {
        Value::Null
    };
    let truncated =
        pages_truncated || next < total_words || words.iter().any(|w| w["text_truncated"] == true);
    let content = joined(words)?;
    if raw["next_offset"] != expected_next
        || flag(&raw["truncated"])? != truncated
        || raw["text"].as_str() != Some(content.as_str())
        || content.chars().count() > 16_384
    {
        return Err(invalid());
    }
    Ok(
        json!({"source":source,"engine":{"name":"tesseract","version":engine["version"],"binding_version":engine["binding_version"],"languages":languages,"models":model_rows},
        "total_pages":total_pages,"pages":pages,"pages_truncated":pages_truncated,"next_page":expected_next_page,
        "total_words":total_words,"offset":offset,"next_offset":expected_next,"words":words,"text":content,"truncated":truncated}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn original() -> Value {
        serde_json::from_str(include_str!(
            "../../../../../tests/fixtures/workflow-native-ocr-engine.json"
        ))
        .unwrap()
    }
    fn run(value: &Value) -> Result<Value> {
        project(
            value["source"].clone(),
            &value["raw"],
            &value["images"],
            &value["options"],
            value["images"].as_array().unwrap().len() as u64,
        )
    }
    #[test]
    fn original_actual_ocr_keeps_page_source_scores_and_word_positions() {
        let input = original();
        let mut output = run(&input).unwrap();
        assert_eq!(output["words"], input["raw"]["words"]);
        assert_eq!(output["source"], input["source"]);
        assert!(output["text"].as_str().unwrap().contains("119,00"));
        output["diagnostics"] = json!([]);
        output["diagnostics_truncated"] = json!(false);
        let operation = WorkflowDocumentOperation::Ocr {
            path_input: "path".into(),
            sha256_input: "sha".into(),
            languages: vec!["deu".into(), "eng".into()],
            pages: vec![],
            dpi: 200,
            rotation: 0,
            offset: 0,
            max_words: 100,
        };
        operation.output_type().validate_value(&output).unwrap();
    }
    #[test]
    fn altered_coordinates_scores_pages_cursors_and_language_identity_are_rejected() {
        for (pointer, bad) in [
            ("/raw/words/0/score", json!(101)),
            ("/raw/words/0/x", json!(100_000)),
            ("/raw/words/0/index", json!(3)),
            ("/raw/next_offset", json!(1)),
            ("/raw/total_words", json!(1)),
            ("/raw/pages/0/page", json!(2)),
            ("/raw/pages/0/preview_sha256", json!("0".repeat(64))),
            ("/raw/engine/models_sha256/deu", json!("0".repeat(64))),
            ("/raw/engine/languages", json!(["eng", "deu"])),
            ("/raw/truncated", json!(true)),
        ] {
            let mut input = original();
            *input.pointer_mut(pointer).unwrap() = bad;
            assert!(
                run(&input).is_err(),
                "accepted corrupted original field {pointer}"
            );
        }
    }
}
