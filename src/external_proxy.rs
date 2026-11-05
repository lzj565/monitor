//! Bounded YAML input and protocol-independent Mihomo proxy validation.
use anyhow::{bail, Context, Result};
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::{cell::Cell, fmt, rc::Rc};

pub const MAX_YAML_BYTES: usize = 1024 * 1024;
struct BoundedValue {
    depth: usize,
    remaining: Rc<Cell<usize>>,
    bytes: Rc<Cell<usize>>,
}
impl<'de> DeserializeSeed<'de> for BoundedValue {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> std::result::Result<Value, D::Error> {
        if self.depth > 32 || self.remaining.get() == 0 {
            return Err(serde::de::Error::custom("配置嵌套或展开规模过大"));
        }
        self.remaining.set(self.remaining.get() - 1);
        d.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for BoundedValue {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("不含自定义标签的静态代理配置")
    }
    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Value, E> {
        serde_json::Number::from_f64(v).map(Value::Number).ok_or_else(|| E::custom("不支持非有限数字"))
    }
    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Value, E> {
        if v.len() > self.bytes.get() {
            return Err(E::custom("展开后的字符串过大"));
        }
        self.bytes.set(self.bytes.get() - v.len());
        Ok(Value::String(v.into()))
    }
    fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Value, E> {
        self.visit_str(&v)
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(v) = seq.next_element_seed(BoundedValue {
            depth: self.depth + 1,
            remaining: self.remaining.clone(),
            bytes: self.bytes.clone(),
        })? {
            values.push(v);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        // A seed enforces string keys: numeric keys are never silently coerced.
        struct Key;
        impl<'de> DeserializeSeed<'de> for Key {
            type Value = String;
            fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<String, D::Error> {
                struct StringKey;
                impl Visitor<'_> for StringKey {
                    type Value = String;
                    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                        f.write_str("字符串键")
                    }
                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<String, E> {
                        Ok(v.into())
                    }
                }
                d.deserialize_any(StringKey)
            }
        }
        while let Some(key) = map.next_key_seed(Key)? {
            if key.len() > self.bytes.get() {
                return Err(serde::de::Error::custom("展开后的键过大"));
            }
            self.bytes.set(self.bytes.get() - key.len());
            if values.contains_key(&key) {
                return Err(serde::de::Error::custom("配置含重复键"));
            }
            let value = map.next_value_seed(BoundedValue {
                depth: self.depth + 1,
                remaining: self.remaining.clone(),
                bytes: self.bytes.clone(),
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

pub fn parse_yaml(input: &str) -> Result<(Vec<Value>, bool)> {
    if input.len() > MAX_YAML_BYTES {
        bail!("YAML最多1 MiB");
    }
    let mut documents = serde_yaml_ng::Deserializer::from_str(input);
    let document = documents.next().context("请粘贴YAML配置")?;
    let value = BoundedValue {
        depth: 0,
        remaining: Rc::new(Cell::new(100_000)),
        bytes: Rc::new(Cell::new(2 * MAX_YAML_BYTES)),
    }
    .deserialize(document)
    .map_err(|e| {
        // The parser's original message may contain credentials. Only expose location.
        let location =
            e.location().map(|l| format!("（第{}行，第{}列）", l.line(), l.column())).unwrap_or_default();
        anyhow::anyhow!("YAML格式不合法、含重复键、自定义标签或超过解析限制{location}")
    })?;
    if documents.next().is_some() {
        bail!("只支持单份YAML文档");
    }
    let (items, ignored) = match value {
        Value::Array(items) => (items, false),
        Value::Object(mut map) => {
            let items = map
                .remove("proxies")
                .and_then(|v| v.as_array().cloned())
                .context("需要静态proxies列表，不支持仅有proxy-providers的配置")?;
            (items, !map.is_empty())
        }
        _ => bail!("请提供proxies配置或代理列表"),
    };
    if items.is_empty() || items.len() > 500 {
        bail!("一次须导入1至500条代理");
    }
    Ok((items, ignored))
}

pub fn validate(entry: &Value) -> Result<Value> {
    if !entry.is_object() {
        bail!("条目必须为代理对象");
    }
    let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let name = text("name").context("缺少代理名称name")?;
    if name.chars().count() > 100 || name.chars().any(char::is_control) {
        bail!("名称最多100字且不能包含控制字符");
    }
    text("type").context("缺少代理类型type")?;
    text("server").context("缺少服务器地址server")?;
    entry
        .get("port")
        .and_then(Value::as_i64)
        .filter(|p| (1..=65535).contains(p))
        .context("端口须为1至65535的整数")?;
    if entry.get("dialer-proxy").is_some() {
        bail!("不支持依赖其他代理的dialer-proxy");
    }
    let mut value = entry.clone();
    value["name"] = Value::String(name.into());
    if serde_json::to_vec(&value)?.len() > MAX_YAML_BYTES {
        bail!("代理配置过大");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn yaml_preserves_protocol_parameters_and_ignores_top_level_sections() {
        let text="proxies:\n  - name: 香港 # comment\n    type: trojan\n    server: example.com\n    port: 443\n    password: 'false:#secret'\n    ws-opts: {path: /ws, headers: {Host: edge.example.com}}\nrules: [MATCH,DIRECT]\n";
        let (items, ignored) = parse_yaml(text).unwrap();
        assert!(ignored);
        let entry = validate(&items[0]).unwrap();
        assert_eq!(entry["password"], "false:#secret");
        assert_eq!(entry["ws-opts"]["headers"]["Host"], "edge.example.com");
        let yaml = serde_yaml_ng::to_string(&vec![entry.clone()]).unwrap();
        assert_eq!(parse_yaml(&yaml).unwrap().0[0], entry);
        assert!(parse_yaml("proxy-providers: {remote: {url: https://example.com}}").is_err());
    }
    #[test]
    fn duplicate_keys_tags_non_string_keys_and_multiple_documents_are_rejected_without_secrets() {
        for yaml in [
            "- {name: first, name: second}",
            "- !secret {password: do-not-echo}",
            "- {123: value}",
            "- a\n---\n- b",
            "- {password: do-not-echo, invalid",
        ] {
            let error = parse_yaml(yaml).unwrap_err().to_string();
            assert!(!error.contains("do-not-echo"));
        }
    }
    #[test]
    fn parse_and_expand_limits_bound_untrusted_yaml() {
        assert!(parse_yaml(&"a".repeat(MAX_YAML_BYTES + 1)).is_err());
        assert!(parse_yaml(&format!("- {}0{}", "[".repeat(40), "]".repeat(40))).is_err());
        assert!(parse_yaml(&serde_json::to_string(&vec![serde_json::json!({}); 501]).unwrap()).is_err());
        let mut alias = "seed: &seed '".to_owned();
        alias.push_str(&"x".repeat(1024));
        alias.push_str("'\nproxies: [");
        alias.push_str(&vec!["*seed"; 3000].join(","));
        alias.push(']');
        assert!(parse_yaml(&alias).is_err());
    }
    #[test]
    fn structural_validation_rejects_invalid_entries_and_dependencies() {
        let valid = serde_json::json!({"name":" n ","type":"ss","server":"::1","port":443,"password":"original","cipher":"aes-128-gcm"});
        assert_eq!(validate(&valid).unwrap()["name"], "n");
        for (field, value) in [
            ("name", Value::String("bad\nname".into())),
            ("port", 0.into()),
            ("port", 65536.into()),
            ("port", "443".into()),
            ("server", Value::Null),
            ("dialer-proxy", "parent".into()),
        ] {
            let mut bad = valid.clone();
            bad[field] = value;
            assert!(validate(&bad).is_err());
        }
    }
}
