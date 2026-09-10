//! Configurable, display-only branch formatting using native Rust regex syntax.
use fancy_regex::Regex;
use serde_json::Value;

#[derive(Debug)]
pub struct Formatter {
    regex: Option<Regex>,
    replacement: String,
}

impl Formatter {
    pub fn new(config: &Value) -> Result<Self, String> {
        let object = config
            .as_object()
            .ok_or("config.json accepts only pattern and replacement")?;
        if object
            .keys()
            .any(|key| key != "pattern" && key != "replacement")
        {
            return Err("config.json accepts only pattern and replacement".into());
        }
        let replacement = match object.get("replacement") {
            None => "",
            Some(value) => value.as_str().ok_or("replacement must be a string")?,
        };
        let regex = match object.get("pattern") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                Regex::new(value.as_str().ok_or("pattern must be a string or null")?)
                    .map_err(|error| error.to_string())?,
            ),
        };
        Ok(Self {
            regex,
            replacement: replacement.to_owned(),
        })
    }

    pub fn format(&self, branch: &str) -> Result<String, String> {
        let Some(regex) = &self.regex else {
            return Ok(branch.to_owned());
        };
        let result = regex
            .try_replacen(branch, 1, self.replacement.as_str())
            .map_err(|error| error.to_string())?;
        let stripped = result.trim();
        Ok(if stripped.is_empty() {
            branch
        } else {
            stripped
        }
        .to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn format(config: Value, branch: &str) -> String {
        Formatter::new(&config).unwrap().format(branch).unwrap()
    }

    #[test]
    fn missing_or_null_pattern_is_exact_identity() {
        for config in [json!({}), json!({"pattern": null, "replacement": "$1"})] {
            assert_eq!(format(config, " main \n"), " main \n");
        }
    }

    #[test]
    fn consumer_pattern_keeps_lookahead_and_defaults_to_empty_replacement() {
        let config = json!({"pattern": r"^[^/]+/[0-9]{4}-[0-9]{2}-[0-9]{2}-(?=.)"});
        assert_eq!(
            format(config.clone(), "feat/2026-09-10-invoice-validation"),
            "invoice-validation"
        );
        assert_eq!(format(config, "feat/2026-09-10-"), "feat/2026-09-10-");
    }

    #[test]
    fn only_first_match_changes_and_empty_result_preserves_original() {
        assert_eq!(format(json!({"pattern": "foo"}), "foo-foo"), "-foo");
        assert_eq!(format(json!({"pattern": ".*"}), " original "), " original ");
        assert_eq!(format(json!({"pattern": "missing"}), " main \n"), "main");
    }

    #[test]
    fn native_numbered_named_and_literal_dollar_replacements() {
        let config = json!({"pattern": r"^(?<type>[^/]+)/(.+)$", "replacement": "${2} ($type) $$"});
        assert_eq!(format(config, "feature/invoices"), "invoices (feature) $");
        assert_eq!(
            format(json!({"pattern": "(foo)", "replacement": r"\1"}), "foo"),
            r"\1"
        );
    }

    #[test]
    fn invalid_configuration_fails_before_formatting() {
        for config in [
            json!(null),
            json!([]),
            json!({"unknown": true}),
            json!({"replacement": null}),
            json!({"pattern": 1}),
            json!({"pattern": "("}),
        ] {
            assert!(Formatter::new(&config).is_err(), "{config}");
        }
    }

    #[test]
    fn backtracking_failure_is_returned_as_an_error() {
        let formatter = Formatter {
            regex: Some(
                fancy_regex::RegexBuilder::new(r"(?i)(a|b|ab)*(?>c)")
                    .backtrack_limit(10)
                    .seek(false)
                    .build()
                    .unwrap(),
            ),
            replacement: String::new(),
        };
        assert!(formatter
            .format("abababababababababababababababababababababababababababab")
            .is_err());
    }
}
