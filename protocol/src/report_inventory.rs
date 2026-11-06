//! Cold inventory is content-addressed; live readings and privileges always travel in full.
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

pub const FIELDS: &[&str] = &[
    "block_devices", "hardware_details", "users", "interfaces", "overlay_networks",
    "routes", "proxy_services", "inventory_collected_at", "running_services",
    "host_security", "permission_audit", "ai_tools", "programming_tools",
];

pub fn take(metrics: &mut Value) -> Value {
    let mut inventory = Map::new();
    if let Some(metrics) = metrics.as_object_mut() {
        for &field in FIELDS {
            if let Some(value) = metrics.remove(field) {
                inventory.insert(field.to_owned(), value);
            }
        }
    }
    Value::Object(inventory)
}

pub fn restore(metrics: &mut Value, inventory: Value) {
    if let (Some(metrics), Value::Object(inventory)) = (metrics.as_object_mut(), inventory) {
        metrics.extend(inventory);
    }
}

pub fn digest(inventory: &Value) -> Result<String, serde_json::Error> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(inventory)?)))
}

pub fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn empty(inventory: &Value) -> bool {
    inventory.as_object().is_some_and(|fields| fields.values().all(|v| {
        v.is_null() || v.as_array().is_some_and(Vec::is_empty)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inventory_round_trip_preserves_changes_and_keeps_live_privileges_separate() {
        let original = json!({"users":[{"name":"fixture"}],"permission_audit":null,
            "inventory_collected_at":"2026-11-05T00:00:00Z", "privilege":{"uid":0},
            "disks":[{"available_bytes":100}],"cpu_usage_percent":15,"collection_policy":{}});
        let mut dynamic = original.clone();
        let inventory = take(&mut dynamic);
        assert_eq!(dynamic["privilege"]["uid"], 0);
        assert_eq!(dynamic["disks"][0]["available_bytes"], 100);
        assert!(dynamic.get("users").is_none());
        let before = digest(&inventory).unwrap();
        assert!(valid_digest(&before));
        let mut changed = inventory.clone();
        changed["users"] = json!([]);
        assert_ne!(digest(&changed).unwrap(), before);
        restore(&mut dynamic, inventory);
        assert_eq!(dynamic, original);
        assert!(!valid_digest(&"F".repeat(64)));
        assert!(!valid_digest("short"));
        assert!(empty(&json!({"users":[],"hardware_details":null})));
        assert!(!empty(&json!({"users":[{"name":"fixture"}]})));
    }
}
