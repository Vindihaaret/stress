//! How readings look in the window: the tint, the status words and how much
//! room is left to the limits. Same steps as Vifte, so both apps agree.

use crate::i18n::n_;

/// Borders between the tints (°C).
const HEAT_STEPS: [f64; 3] = [50.0, 65.0, 80.0];
const HEAT_NAMES: [&str; 4] = ["cool", "mild", "warm", "hot"];
/// A tint only steps down this far below its border, so it does not flicker.
const HEAT_MARGIN: f64 = 2.0;
const CALM_BELOW: f64 = 60.0;
const HOT_FROM: f64 = 80.0;

/// The window's tint for the hottest temperature.
pub fn heat_step(temperature: Option<f64>, previous: Option<&'static str>) -> Option<&'static str> {
    let temperature = temperature?;
    let step = HEAT_STEPS.iter().filter(|limit| temperature >= **limit).count();
    if let Some(old) = previous.and_then(|p| HEAT_NAMES.iter().position(|n| *n == p)) {
        if step + 1 == old && temperature >= HEAT_STEPS[step] - HEAT_MARGIN {
            return previous;
        }
    }
    Some(HEAT_NAMES[step])
}

/// CSS class and (untranslated) word for a temperature.
pub fn temperature_status(temperature: f64) -> (&'static str, &'static str) {
    if temperature >= HOT_FROM {
        ("hot", n_("Hot"))
    } else if temperature >= CALM_BELOW {
        ("warm", n_("Warm"))
    } else {
        ("calm", n_("Calm"))
    }
}

/// The limit closest to being reached, as (limit, degrees left). A device
/// without a temperature does not count.
pub fn nearest_limit(readings: &[(Option<f64>, f64)]) -> Option<(f64, f64)> {
    readings
        .iter()
        .filter_map(|(temperature, limit)| temperature.map(|t| (*limit, (limit - t).max(0.0))))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tint_follows_the_steps() {
        assert_eq!(heat_step(None, Some("hot")), None);
        assert_eq!(heat_step(Some(40.0), None), Some("cool"));
        assert_eq!(heat_step(Some(50.0), None), Some("mild"));
        assert_eq!(heat_step(Some(70.0), None), Some("warm"));
        assert_eq!(heat_step(Some(80.0), None), Some("hot"));
    }

    #[test]
    fn tint_waits_before_stepping_down() {
        assert_eq!(heat_step(Some(79.0), Some("hot")), Some("hot"));
        assert_eq!(heat_step(Some(77.9), Some("hot")), Some("warm"));
        assert_eq!(heat_step(Some(30.0), Some("hot")), Some("cool"));
    }

    #[test]
    fn status_words() {
        assert_eq!(temperature_status(59.9).0, "calm");
        assert_eq!(temperature_status(60.0).0, "warm");
        assert_eq!(temperature_status(80.0).0, "hot");
    }

    #[test]
    fn nearest_limit_is_the_one_with_least_room() {
        assert_eq!(nearest_limit(&[(Some(60.0), 95.0), (Some(70.0), 90.0)]), Some((90.0, 20.0)));
        assert_eq!(nearest_limit(&[(Some(92.0), 90.0)]), Some((90.0, 0.0)));
        assert_eq!(nearest_limit(&[(None, 90.0)]), None);
    }
}
