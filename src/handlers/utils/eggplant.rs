use crate::config::PeezyForkSettings;
use crate::handlers::utils::SignedIncrement;

const EGGPLANT: &str = "🍆";

pub fn calculate_eggplants_string(growth: &SignedIncrement, peezy_settings: PeezyForkSettings) -> String {
    let PeezyForkSettings { max_eggplants, centimeters_per_eggplant, .. } = peezy_settings;
    (max_eggplants > 0 && centimeters_per_eggplant.is_positive())
        .then(|| (growth.total.max(0) / centimeters_per_eggplant) as usize)
        .filter(|eggplants_count| eggplants_count > &0)
        .map(|eggplants_count| eggplants_count.min(max_eggplants))
        .map(|limited_count| format!("\n\n{}", EGGPLANT.repeat(limited_count)))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use crate::config::PeezyForkSettings;
    use crate::handlers::utils::SignedIncrement;
    use super::{calculate_eggplants_string, EGGPLANT};

    #[test]
    fn success() {
        let cases = [
            (20, "\n\n🍆"),
            (21, "\n\n🍆"),
            (30, "\n\n🍆"),
            (40, "\n\n🍆🍆"),
            (100, "\n\n🍆🍆🍆🍆🍆"),
        ];

        for (case, expected) in cases {
            let result = execute_test(case);
            assert_eq!(result, expected, "Case: {case}, result: {result}")
        }
    }

    #[test]
    fn empty() {
        let cases = [
            -20, -1, 0, 10, 19
        ];

        for case in cases {
            let result = execute_test(case);
            assert!(result.is_empty(), "Case: {case}, result: {result}")
        }
    }

    #[test]
    fn max_value() {
        let result = execute_test(i32::MAX);
        assert_eq!(4, EGGPLANT.len());
        assert_eq!(402, result.len())
    }

    #[test]
    fn disabled() {
        let result = calculate_eggplants_string(&create_increment(10), PeezyForkSettings::default());
        assert!(result.is_empty(), "The string must be empty when zero values in the configuration")
    }

    fn create_increment(total: i32) -> SignedIncrement {
        total.into()
    }

    fn execute_test(increment: i32) -> String {
        calculate_eggplants_string(&create_increment(increment), PeezyForkSettings {
            centimeters_per_eggplant: 20,
            max_eggplants: 100,
            ..PeezyForkSettings::default()
        })
    }
}
