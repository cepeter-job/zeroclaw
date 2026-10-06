use zeroclaw_config::schema::ModelProviderConfig;

#[test]
fn provider_catalog_refresh_settings_round_trip() {
    let input = serde_json::json!({
        "model_refresh_interval_secs": 3600,
        "model_refresh_free_only": true
    });
    let provider: ModelProviderConfig = serde_json::from_value(input).unwrap();
    let serialized = serde_json::to_value(provider).unwrap();
    assert_eq!(serialized["model_refresh_interval_secs"], 3600);
    assert_eq!(serialized["model_refresh_free_only"], true);
}

#[test]
fn provider_catalog_refresh_is_opt_in() {
    let serialized = serde_json::to_value(ModelProviderConfig::default()).unwrap();
    assert!(serialized.get("model_refresh_interval_secs").is_none());
    assert!(serialized.get("model_refresh_free_only").is_none());
}
