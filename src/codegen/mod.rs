pub mod api;
pub mod models;

/// feignhttp features required by the generated code.
#[derive(Default)]
pub struct Emission {
    pub json: bool,
    pub multipart: bool,
    pub serde_json_value: bool,
}

impl Emission {
    /// feignhttp Cargo features for the default (reqwest) client stack.
    pub fn cargo_features(&self) -> Vec<String> {
        let mut v = vec!["\"reqwest-client\"".to_string()];
        if self.json {
            v.push("\"reqwest-json\"".to_string());
        }
        if self.multipart {
            v.push("\"reqwest-multipart\"".to_string());
        }
        v
    }

    /// Short feature tags used in diagnostics.
    pub fn summary(&self) -> String {
        let mut parts = vec!["reqwest-client"];
        if self.json {
            parts.push("reqwest-json");
        }
        if self.multipart {
            parts.push("reqwest-multipart");
        }
        parts.join(", ")
    }
}
