// SPDX-License-Identifier: GPL-3.0-only
//! Gemini conversion shared by single-sheet requests and library jobs.

use serde_json::{Value, json};

pub fn request(chat: &Value) -> Value {
    let parts: Vec<_> = chat["messages"][1]["content"].as_array().unwrap().iter().map(|part| {
        if part["type"] == "text" { json!({"text":part["text"]}) } else {
            let data = part["image_url"]["url"].as_str().unwrap().strip_prefix("data:image/png;base64,").unwrap();
            json!({"inlineData":{"mimeType":"image/png", "data":data}})
        }
    }).collect();
    json!({"systemInstruction":{"parts":[{"text":chat["messages"][0]["content"]}]},
        "contents":[{"role":"user", "parts":parts}], "generationConfig":{"maxOutputTokens":4096,
            "responseMimeType":"application/json", "responseJsonSchema":chat["response_format"]["json_schema"]["schema"]}})
}


/// Keep provider failures specific while sharing the label validator with other providers.
pub fn response(value: &Value) -> Value {
    let error = |message: String| json!({"error":{"message":message}});
    if !value["error"].is_null() { return json!({"error":value["error"]}); }
    if let Some(reason) = value["promptFeedback"]["blockReason"].as_str() {
        return error(format!("Google blocked the request: {reason}."));
    }
    let Some(candidates) = value["candidates"].as_array().filter(|v| v.len() == 1) else {
        return error("Google returned no single candidate.".into());
    };
    let candidate = &candidates[0];
    let reason = candidate["finishReason"].as_str().unwrap_or("UNKNOWN");
    if reason != "STOP" { return error(format!("Google did not finish the label: {reason}.")); }
    let text = candidate["content"]["parts"].as_array().map(|parts| parts.iter()
        .filter(|part| part["thought"] != true).filter_map(|part| part["text"].as_str()).collect::<String>());
    json!({"choices":[{"finish_reason":"stop", "message":{"content":text}}]})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_failures_keep_their_reason() {
        for value in [json!({"promptFeedback":{"blockReason":"SAFETY"}}),
            json!({"candidates":[{"finishReason":"MAX_TOKENS"}]}), json!({"candidates":[]})] {
            let failure = crate::labels::response(&response(&value), &[]).err().unwrap();
            assert!(failure.contains("Google"));
            assert!(!failure.contains("invalid structured"));
        }
    }
    #[test]
    fn thoughts_are_excluded_and_text_parts_are_joined() {
        let reply = json!({"candidates":[{"finishReason":"STOP", "content":{"parts":[
            {"text":"private reasoning", "thought":true},
            {"text":"{\"status\":\"labeled\",\"caption\":\"Torch\","}, {"text":"\"tags\":[]}"}
        ]}}]});
        let response = response(&reply);
        assert!(crate::labels::response(&response, &[]).is_ok());
        assert!(!response.to_string().contains("private reasoning"));
    }
}
