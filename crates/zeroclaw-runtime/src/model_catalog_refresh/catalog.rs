use std::time::{Duration, Instant};

pub(super) fn select_models(mut models: Vec<String>, free_only: bool) -> Option<Vec<String>> {
    if free_only {
        models.retain(|id| id.ends_with(":free"));
    }
    models.sort();
    models.dedup();
    (!models.is_empty()).then_some(models)
}

pub(super) fn refresh_due(last: Option<Instant>, now: Instant, interval_secs: u64) -> bool {
    interval_secs > 0
        && last.is_none_or(|last| {
            now.saturating_duration_since(last) >= Duration::from_secs(interval_secs.max(60))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_only_excludes_paid_and_router_models() {
        let models = vec![
            "paid/model".into(),
            "vendor/free:free".into(),
            "openrouter/free".into(),
        ];
        assert_eq!(
            select_models(models, true),
            Some(vec!["vendor/free:free".into()])
        );
    }

    #[test]
    fn selection_sorts_and_deduplicates() {
        let models = vec!["z:free".into(), "a:free".into(), "z:free".into()];
        assert_eq!(
            select_models(models, true),
            Some(vec!["a:free".into(), "z:free".into()])
        );
    }

    #[test]
    fn empty_or_paid_only_catalog_is_not_published() {
        assert_eq!(select_models(vec![], true), None);
        assert_eq!(select_models(vec!["paid/model".into()], true), None);
    }

    #[test]
    fn unfiltered_catalog_keeps_paid_models() {
        assert_eq!(
            select_models(vec!["paid/model".into()], false),
            Some(vec!["paid/model".into()])
        );
    }

    #[test]
    fn zero_interval_disables_refresh() {
        assert!(!refresh_due(None, Instant::now(), 0));
    }

    #[test]
    fn enabled_provider_refreshes_immediately_and_then_on_interval() {
        let now = Instant::now();
        assert!(refresh_due(None, now, 3600));
        assert!(!refresh_due(
            Some(now),
            now + Duration::from_secs(3599),
            3600
        ));
        assert!(refresh_due(
            Some(now),
            now + Duration::from_secs(3600),
            3600
        ));
    }

    #[test]
    fn small_intervals_are_clamped_to_one_minute() {
        let now = Instant::now();
        assert!(!refresh_due(Some(now), now + Duration::from_secs(59), 1));
        assert!(refresh_due(Some(now), now + Duration::from_secs(60), 1));
    }
}
