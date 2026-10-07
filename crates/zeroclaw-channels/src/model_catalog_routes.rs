//! Session-only routes shared by the Telegram picker and its command resolver.

use zeroclaw_config::schema::{Config, ModelCacheState, ModelRouteConfig};

pub(crate) fn with_cached_models(
    config: &Config,
    routes: &[ModelRouteConfig],
) -> Vec<ModelRouteConfig> {
    let mut result = routes.to_vec();
    let path = config.data_dir.join("state/models_cache.json");
    let Some(cache) = std::fs::read(path)
        .ok()
        .and_then(|raw| serde_json::from_slice::<ModelCacheState>(&raw).ok())
    else {
        return result;
    };
    for entry in cache.entries {
        let Some((family, alias)) = entry.model_provider.split_once('.') else {
            continue;
        };
        let Some(provider) = config.providers.models.find(family, alias) else {
            continue;
        };
        // Cache data cannot grant access to a provider absent from live operator routes.
        // Route-specific credentials are never silently replaced with alias credentials.
        let permitted = routes.iter().any(|route| {
            route.model_provider == entry.model_provider
                && config.model_routes.iter().any(|stored| {
                    stored.hint == route.hint
                        && stored.model == route.model
                        && stored.model_provider == route.model_provider
                        && stored.api_key.is_none()
                })
        });
        if !permitted {
            continue;
        }
        for model in entry.models {
            if !safe_field(&model)
                || !safe_field(&entry.model_provider)
                || (provider.model_refresh_free_only && !model.ends_with(":free"))
            {
                continue;
            }
            if result
                .iter()
                .any(|r| r.model_provider == entry.model_provider && r.model == model)
            {
                continue;
            }
            let hint = format!("catalog:{}:{}", entry.model_provider, model);
            if !safe_field(&hint)
                || result.iter().any(|r| {
                    r.hint.eq_ignore_ascii_case(&hint) || r.model.eq_ignore_ascii_case(&hint)
                })
            {
                continue;
            }
            result.push(ModelRouteConfig {
                hint,
                model_provider: entry.model_provider.clone(),
                model,
                api_key: None,
            });
        }
    }
    result
}

fn safe_field(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.starts_with("--")
        && !value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '`')
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeroclaw_config::schema::{
        CustomModelProviderConfig, ModelCacheEntry, ModelProviderConfig,
    };

    #[test]
    fn cached_routes_preserve_live_access_filter_and_credential_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = Config {
            data_dir: temp.path().into(),
            ..Default::default()
        };
        config.providers.models.custom.insert(
            "free".into(),
            CustomModelProviderConfig {
                base: ModelProviderConfig {
                    model_refresh_free_only: true,
                    ..Default::default()
                },
            },
        );
        config.model_routes.push(ModelRouteConfig {
            hint: "free".into(),
            model_provider: "custom.free".into(),
            model: "old:free".into(),
            api_key: None,
        });
        std::fs::create_dir_all(temp.path().join("state")).unwrap();
        let path = temp.path().join("state/models_cache.json");
        let cache = ModelCacheState {
            entries: vec![
                ModelCacheEntry {
                    model_provider: "custom.free".into(),
                    models: vec![
                        "old:free".into(),
                        "new:free".into(),
                        "paid".into(),
                        "bad\n:free".into(),
                    ],
                },
                ModelCacheEntry {
                    model_provider: "custom.unknown".into(),
                    models: vec!["ghost:free".into()],
                },
            ],
        };
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();
        let expanded = with_cached_models(&config, &config.model_routes);
        assert_eq!(expanded.len(), 2);
        assert_eq!(expanded[1].model, "new:free");
        assert!(expanded[1].api_key.is_none());
        assert_eq!(config.model_routes.len(), 1);
        assert!(with_cached_models(&config, &[]).is_empty());
        config.model_routes[0].api_key = Some("route-only".into());
        assert_eq!(with_cached_models(&config, &config.model_routes).len(), 1);
        config.model_routes[0].api_key = None;
        std::fs::write(&path, "invalid").unwrap();
        assert_eq!(with_cached_models(&config, &config.model_routes).len(), 1);
    }
}
