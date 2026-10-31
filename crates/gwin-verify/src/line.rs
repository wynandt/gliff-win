//! The output contract: one JSON object per line on stdout,
//! `{"check": .., "ok": bool, ..metrics}`.

use serde_json::Map;
use serde_json::Value;

/// One result line. `check` and `ok` always come first.
#[derive(Debug, Clone)]
pub struct Line {
    pub check: &'static str,
    pub ok: bool,
    pub metrics: Map<String, Value>,
}

impl Line {
    pub fn new(check: &'static str, ok: bool) -> Self {
        Self {
            check,
            ok,
            metrics: Map::new(),
        }
    }

    /// Adds one metric; later keys keep their insertion order.
    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.metrics.insert(key.to_owned(), value.into());
        self
    }

    pub fn to_json(&self) -> String {
        let mut object = Map::new();
        object.insert("check".into(), Value::from(self.check));
        object.insert("ok".into(), Value::from(self.ok));
        for (key, value) in &self.metrics {
            object.insert(key.clone(), value.clone());
        }
        Value::Object(object).to_string()
    }
}

/// A float rounded to 0.01 for stable, readable output.
pub fn round2(x: f64) -> Value {
    Value::from((x * 100.0).round() / 100.0)
}

#[cfg(test)]
mod tests {
    use super::Line;

    #[test]
    fn check_and_ok_come_first_then_insertion_order() {
        let line = Line::new("decode", false)
            .with("size", "1920x1080")
            .with("a", 1);
        assert_eq!(
            line.to_json(),
            r#"{"check":"decode","ok":false,"size":"1920x1080","a":1}"#
        );
    }
}
