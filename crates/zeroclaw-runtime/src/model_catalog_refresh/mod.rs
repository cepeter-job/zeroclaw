//! Opt-in, daemon-owned refresh of provider model catalogs.

mod catalog;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use parking_lot::RwLock;
use zeroclaw_config::schema::Config;

/// Publish only a nonempty successful selection, leaving the last good cache
/// untouched when an upstream catalog is empty or contains no eligible IDs.
fn publish_catalog(
    config: &Config,
    provider_ref: &str,
    models: Vec<String>,
    free_only: bool,
) -> Result<usize> {
    let models = catalog::select_models(models, free_only)
        .context("provider catalog contains no eligible models")?;
    crate::doctor::persist_model_cache(config, provider_ref, &models)?;
    Ok(models.len())
}

pub(crate) async fn run(
    live_config: Arc<RwLock<Config>>,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<()> {
    let mut last_attempt = HashMap::<String, Instant>::new();
    let mut poll = tokio::time::interval(Duration::from_secs(30));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => return Ok(()),
            _ = poll.tick() => {}
        }
        // Never hold a live-config lock across network I/O. A new snapshot also
        // picks up credential and policy changes without a daemon restart.
        let config = live_config.read().clone();
        let targets: Vec<_> = config
            .providers
            .models
            .iter_entries()
            .filter_map(|(family, alias, provider)| {
                let interval = provider.model_refresh_interval_secs?;
                (interval > 0).then(|| {
                    (
                        format!("{family}.{alias}"),
                        interval,
                        provider.model_refresh_free_only,
                    )
                })
            })
            .collect();
        last_attempt.retain(|name, _| targets.iter().any(|(target, _, _)| target == name));
        for (provider_ref, interval, free_only) in targets {
            let now = Instant::now();
            if !catalog::refresh_due(last_attempt.get(&provider_ref).copied(), now, interval) {
                continue;
            }
            // Record failures too: a broken endpoint must not be hammered every
            // poll. Cancellation also interrupts an in-flight network probe.
            last_attempt.insert(provider_ref.clone(), now);
            let fetched = tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(()),
                result = tokio::time::timeout(
                    Duration::from_secs(60),
                    crate::doctor::fetch_provider_catalog(&config, &provider_ref),
                ) => result,
            };
            let published = match fetched {
                Ok(Ok(models)) => {
                    let config = config.clone();
                    let name = provider_ref.clone();
                    tokio::task::spawn_blocking(move || {
                        publish_catalog(&config, &name, models, free_only)
                    })
                    .await
                    .ok()
                    .and_then(Result::ok)
                }
                _ => None,
            };
            if let Some(count) = published {
                ::zeroclaw_log::record!(
                    INFO,
                    ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Note),
                    "model catalog refreshed for {provider_ref}: {count} models"
                );
            } else {
                // Upstream errors can include request material; do not echo
                // their raw text into daemon logs.
                ::zeroclaw_log::record!(
                    WARN,
                    ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Note),
                    "model catalog refresh failed for {provider_ref}; previous cache retained"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_refresher_exits_without_waiting_for_poll() {
        let config = Arc::new(RwLock::new(Config::default()));
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), run(config, cancel))
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn idle_refresher_observes_shutdown() {
        let config = Arc::new(RwLock::new(Config::default()));
        let cancel = tokio_util::sync::CancellationToken::new();
        let task = tokio::spawn(run(config, cancel.clone()));
        tokio::task::yield_now().await;
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn scheduling_interval_has_immediate_first_tick() {
        let mut poll = tokio::time::interval(Duration::from_secs(30));
        tokio::time::timeout(Duration::from_secs(1), poll.tick())
            .await
            .unwrap();
    }

    #[test]
    fn publish_preserves_other_provider_entries_and_filters_paid_models() {
        let folder = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.data_dir = folder.path().to_path_buf();
        crate::doctor::persist_model_cache(&config, "custom.other", &["existing".into()]).unwrap();
        assert_eq!(
            publish_catalog(
                &config,
                "custom.nous_free",
                vec!["paid".into(), "free:free".into()],
                true
            )
            .unwrap(),
            1
        );
        let cache: zeroclaw_config::schema::ModelCacheState = serde_json::from_slice(
            &std::fs::read(config.data_dir.join("state/models_cache.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cache.entries.len(), 2);
        assert!(
            cache
                .entries
                .iter()
                .any(|e| e.model_provider == "custom.other" && e.models == ["existing"])
        );
        assert!(
            cache
                .entries
                .iter()
                .any(|e| e.model_provider == "custom.nous_free" && e.models == ["free:free"])
        );
    }

    #[test]
    fn empty_selection_preserves_previous_cache_bytes() {
        let folder = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.data_dir = folder.path().to_path_buf();
        crate::doctor::persist_model_cache(&config, "custom.nous_free", &["old:free".into()])
            .unwrap();
        let path = config.data_dir.join("state/models_cache.json");
        let before = std::fs::read(&path).unwrap();
        assert!(publish_catalog(&config, "custom.nous_free", vec!["paid".into()], true).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn malformed_cache_is_not_replaced() {
        let folder = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.data_dir = folder.path().to_path_buf();
        std::fs::create_dir_all(config.data_dir.join("state")).unwrap();
        let path = config.data_dir.join("state/models_cache.json");
        std::fs::write(&path, "invalid json").unwrap();
        assert!(
            publish_catalog(&config, "custom.nous_free", vec!["free:free".into()], true).is_err()
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid json");
    }
}
