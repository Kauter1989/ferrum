//! Reading command parameters from a JSON object, with errors an agent
//! can act on.

use serde_json::{Map, Value};

use crate::envelope::AgentError;

/// The parameters of one command call.
#[derive(Debug, Clone, Copy)]
pub struct Params<'a>(pub &'a Map<String, Value>);

impl<'a> Params<'a> {
    /// Wraps `v`, which must be a JSON object.
    pub fn new(v: &'a Value) -> Result<Self, AgentError> {
        v.as_object().map(Params).ok_or_else(|| AgentError::bad_request("parameters must be a JSON object"))
    }

    /// Raw value of `key`, `None` if missing or `null`.
    pub fn get(&self, key: &str) -> Option<&'a Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }

    /// Required value.
    pub fn req(&self, key: &str) -> Result<&'a Value, AgentError> {
        self.get(key).ok_or_else(|| AgentError::bad_request(format!("{key} is required")))
    }

    /// Optional text.
    pub fn str(&self, key: &str) -> Result<Option<&'a str>, AgentError> {
        self.get(key)
            .map(|v| v.as_str().ok_or_else(|| AgentError::bad_request(format!("{key} must be text"))))
            .transpose()
    }

    /// Required text.
    pub fn req_str(&self, key: &str) -> Result<&'a str, AgentError> {
        self.str(key)?.ok_or_else(|| AgentError::bad_request(format!("{key} is required")))
    }

    /// Optional number.
    pub fn f64(&self, key: &str) -> Result<Option<f64>, AgentError> {
        self.get(key)
            .map(|v| {
                v.as_f64()
                    .filter(|x| x.is_finite())
                    .ok_or_else(|| AgentError::bad_request(format!("{key} must be a number")))
            })
            .transpose()
    }

    /// Required number.
    pub fn req_f64(&self, key: &str) -> Result<f64, AgentError> {
        self.f64(key)?.ok_or_else(|| AgentError::bad_request(format!("{key} is required")))
    }

    /// Optional non-negative integer.
    pub fn u64(&self, key: &str) -> Result<Option<u64>, AgentError> {
        self.get(key)
            .map(|v| v.as_u64().ok_or_else(|| AgentError::bad_request(format!("{key} must be a non-negative integer"))))
            .transpose()
    }

    /// Optional boolean.
    pub fn bool(&self, key: &str) -> Result<Option<bool>, AgentError> {
        self.get(key)
            .map(|v| v.as_bool().ok_or_else(|| AgentError::bad_request(format!("{key} must be true or false"))))
            .transpose()
    }

    /// Optional list.
    pub fn list(&self, key: &str) -> Result<Option<&'a Vec<Value>>, AgentError> {
        self.get(key)
            .map(|v| v.as_array().ok_or_else(|| AgentError::bad_request(format!("{key} must be a list"))))
            .transpose()
    }
}

/// `[x, y, z]` of finite numbers.
pub fn vec3(v: &Value, what: &str) -> Result<[f64; 3], AgentError> {
    let bad = || AgentError::bad_request(format!("{what} must be a list of 3 numbers"));
    let a = v.as_array().filter(|a| a.len() == 3).ok_or_else(bad)?;
    let mut out = [0.0; 3];
    for (o, x) in out.iter_mut().zip(a) {
        *o = x.as_f64().filter(|x| x.is_finite()).ok_or_else(bad)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn typed_access() {
        let v = json!({ "s": "x", "n": 1.5, "u": 3, "b": true, "l": [1], "z": null });
        let p = Params::new(&v).unwrap();
        assert_eq!((p.req_str("s").unwrap(), p.req_f64("n").unwrap(), p.u64("u").unwrap()), ("x", 1.5, Some(3)));
        assert_eq!((p.bool("b").unwrap(), p.list("l").unwrap().map(Vec::len)), (Some(true), Some(1)));
        assert_eq!((p.str("z").unwrap(), p.get("missing")), (None, None));
        assert!(p.req("z").is_err() && p.req_str("missing").is_err() && p.req_f64("missing").is_err());
        assert!(p.str("n").is_err() && p.f64("s").is_err() && p.u64("n").is_err());
        assert!(p.bool("s").is_err() && p.list("s").is_err());
        assert!(Params::new(&json!([1])).is_err());
        assert_eq!(vec3(&json!([1, 2, 3.5]), "p").unwrap(), [1.0, 2.0, 3.5]);
        assert!(vec3(&json!([1, 2]), "p").is_err() && vec3(&json!([1, "a", 2]), "p").is_err());
    }
}
