// Match the existing bridge's default client identity, not a fabricated latest version.
pub(super) const CLIENT_VERSION: &str = "1.0";

pub(super) fn client_version(value: &str) -> Result<String, &'static str> {
    let mut parts: Vec<&str> = value
        .trim()
        .split('-')
        .next()
        .unwrap_or_default()
        .split('.')
        .collect();
    if parts.len() == 2 {
        parts.push("0");
    }
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err("invalid Codex catalog client version");
    }
    Ok(parts.join("."))
}

pub(super) fn catalog_url(responses_url: &str) -> Result<reqwest::Url, &'static str> {
    let mut url = reqwest::Url::parse(responses_url).map_err(|_| "invalid Codex endpoint")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("invalid Codex endpoint");
    }
    let prefix = url
        .path()
        .trim_end_matches('/')
        .strip_suffix("/responses")
        .ok_or("Codex endpoint must end in /responses")?;
    let path = format!("{prefix}/models");
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

pub(super) fn parse_models(payload: &serde_json::Value) -> Result<Vec<String>, &'static str> {
    let entries = payload
        .get("models")
        .or_else(|| payload.get("data"))
        .and_then(serde_json::Value::as_array)
        .ok_or("invalid Codex model catalog")?;
    let codex_shape = payload.get("models").is_some();
    let mut models = Vec::new();
    for entry in entries {
        let id = if codex_shape {
            let slug = entry
                .get("slug")
                .and_then(serde_json::Value::as_str)
                .ok_or("invalid Codex model entry")?;
            let supported = entry
                .get("supported_in_api")
                .and_then(serde_json::Value::as_bool)
                .ok_or("invalid Codex model entry")?;
            let visibility = entry
                .get("visibility")
                .and_then(serde_json::Value::as_str)
                .filter(|value| matches!(*value, "list" | "hide" | "none"))
                .ok_or("invalid Codex model entry")?;
            if !supported || visibility != "list" {
                continue;
            }
            slug
        } else {
            entry
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or("invalid gateway model entry")?
        };
        if !id.is_empty() {
            models.push(id.to_owned());
        }
    }
    models.sort();
    models.dedup();
    if models.is_empty() {
        return Err("Codex model catalog is empty");
    }
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn client_version_matches_bridge_semver_normalization() {
        assert_eq!(client_version(CLIENT_VERSION).unwrap(), "1.0.0");
        assert_eq!(client_version(" 1.0 ").unwrap(), "1.0.0");
        assert_eq!(client_version("0.134.0-alpha.2").unwrap(), "0.134.0");
        for invalid in ["", "1", "1.2.3.4", "1.a.0", "１.0.0"] {
            assert!(client_version(invalid).is_err());
        }
    }

    #[test]
    fn direct_catalog_is_sibling_of_responses() {
        assert_eq!(
            catalog_url("https://chatgpt.com/backend-api/codex/responses")
                .unwrap()
                .as_str(),
            "https://chatgpt.com/backend-api/codex/models"
        );
    }

    #[test]
    fn gateway_catalog_never_falls_back_to_chatgpt() {
        assert_eq!(
            catalog_url("https://gateway.example/v1/responses?ignored=1#fragment")
                .unwrap()
                .as_str(),
            "https://gateway.example/v1/models"
        );
    }

    #[test]
    fn invalid_or_non_responses_endpoint_is_rejected() {
        assert!(catalog_url("not a URL").is_err());
        assert!(catalog_url("https://gateway.example/v1/chat/completions").is_err());
    }

    #[test]
    fn codex_slugs_are_preserved_sorted_and_deduplicated() {
        let payload = json!({"models":[
            {"slug":"gpt-6-sol", "supported_in_api":true, "visibility":"list"},
            {"slug":"gpt-6.1-sol-900k", "supported_in_api":true, "visibility":"list"},
            {"slug":"gpt-6-sol", "supported_in_api":true, "visibility":"list"}
        ]});
        assert_eq!(
            parse_models(&payload).unwrap(),
            ["gpt-6-sol", "gpt-6.1-sol-900k"]
        );
    }

    #[test]
    fn codex_catalog_only_includes_visible_api_models() {
        let payload = json!({"models": [
            {"slug": "eligible", "supported_in_api": true, "visibility": "list"},
            {"slug": "hidden", "supported_in_api": true, "visibility": "hide"},
            {"slug": "unlisted", "supported_in_api": true, "visibility": "none"},
            {"slug": "unsupported", "supported_in_api": false, "visibility": "list"}
        ]});
        assert_eq!(parse_models(&payload).unwrap(), ["eligible"]);
    }

    #[test]
    fn gateways_can_return_openai_model_ids() {
        assert_eq!(
            parse_models(&json!({"data":[{"id":"vendor/exact-model"}]})).unwrap(),
            ["vendor/exact-model"]
        );
    }

    #[test]
    fn malformed_entry_rejects_the_whole_catalog() {
        for invalid in [
            json!({"slug":"missing-fields"}),
            json!({"slug":"wrong-type", "supported_in_api":"true", "visibility":"list"}),
            json!({"slug":"unknown-visibility", "supported_in_api":true, "visibility":"unknown"}),
            json!(null),
        ] {
            let payload = json!({"models":[
                {"slug":"eligible", "supported_in_api":true, "visibility":"list"},
                invalid
            ]});
            assert!(parse_models(&payload).is_err());
        }
    }

    #[test]
    fn ineligible_only_catalog_is_not_a_successful_empty_refresh() {
        assert!(
            parse_models(&json!({"models":[
                {"slug":"hidden", "supported_in_api":true, "visibility":"hide"},
                {"slug":"unsupported", "supported_in_api":false, "visibility":"list"}
            ]}))
            .is_err()
        );
    }

    #[test]
    fn malformed_and_empty_catalogs_fail_without_invented_models() {
        for payload in [
            json!({}),
            json!({"models":[]}),
            json!({"models":[{"slug":""},{"slug":1}]}),
            json!({"error":"denied"}),
        ] {
            assert!(parse_models(&payload).is_err());
        }
    }
}
