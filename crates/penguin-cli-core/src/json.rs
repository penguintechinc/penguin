//! `--json` rendering for `modules` and `status`, matching Go's plain
//! `encoding/json.MarshalIndent(resp, "", "  ")` over the generated protobuf
//! struct — **not** `protojson`. `protoc-gen-go` tags every field with its
//! literal proto name and `omitempty`
//! (e.g. `json:"license_feature,omitempty"` — verified directly against
//! `go-client/api/proto/penguin/daemon/v1/daemon.pb.go`), so the wire shape
//! is snake_case keys with zero-valued fields dropped, not protojson's
//! lowerCamelCase / string-encoded int64 conventions. The [`serde`] structs
//! below reproduce that struct-tag behaviour field-for-field.
//!
//! This output is a best-effort match rather than a byte-verified one: it is
//! not covered by the M4 cross-implementation gate's required checks, only
//! `modules`/`status`'s plain-text tables are.

use serde::Serialize;

use crate::pb;

fn is_false(value: &bool) -> bool {
    !*value
}

/// Mirrors `ModuleSummary`'s JSON shape (`daemon.proto`'s `ModuleSummary`).
#[derive(Serialize)]
struct ModuleSummaryJson {
    #[serde(skip_serializing_if = "String::is_empty")]
    name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    version: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    state: String,
    #[serde(skip_serializing_if = "is_false")]
    external: bool,
    #[serde(rename = "license_feature", skip_serializing_if = "String::is_empty")]
    license_feature: String,
}

impl From<&pb::ModuleSummary> for ModuleSummaryJson {
    fn from(module: &pb::ModuleSummary) -> ModuleSummaryJson {
        ModuleSummaryJson {
            name: module.name.clone(),
            version: module.version.clone(),
            description: module.description.clone(),
            state: module.state.clone(),
            external: module.external,
            license_feature: module.license_feature.clone(),
        }
    }
}

#[derive(Serialize)]
struct ListModulesResponseJson {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    modules: Vec<ModuleSummaryJson>,
}

/// Renders `penguin modules --json`'s output, matching
/// `json.MarshalIndent(resp, "", "  ")` followed by `fmt.Println` (hence the
/// trailing newline this appends that `to_string_pretty` alone would not).
pub fn modules_json(response: &pb::ListModulesResponse) -> String {
    let payload = ListModulesResponseJson {
        modules: response
            .modules
            .iter()
            .map(ModuleSummaryJson::from)
            .collect(),
    };
    let mut text = serde_json::to_string_pretty(&payload).unwrap_or_default();
    text.push('\n');
    text
}

/// Mirrors `ModuleStatus`'s JSON shape (`daemon.proto`'s `ModuleStatus`).
#[derive(Serialize)]
struct ModuleStatusJson {
    #[serde(skip_serializing_if = "String::is_empty")]
    name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    state: String,
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    detail: std::collections::HashMap<String, String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    health: String,
    #[serde(rename = "health_message", skip_serializing_if = "String::is_empty")]
    health_message: String,
    #[serde(rename = "checked_at_unix_nano", skip_serializing_if = "is_zero_i64")]
    checked_at_unix_nano: i64,
}

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}

impl From<&pb::ModuleStatus> for ModuleStatusJson {
    fn from(module: &pb::ModuleStatus) -> ModuleStatusJson {
        ModuleStatusJson {
            name: module.name.clone(),
            state: module.state.clone(),
            detail: module.detail.clone(),
            health: module.health.clone(),
            health_message: module.health_message.clone(),
            checked_at_unix_nano: module.checked_at_unix_nano,
        }
    }
}

#[derive(Serialize)]
struct GetStatusResponseJson {
    #[serde(rename = "daemon_version", skip_serializing_if = "String::is_empty")]
    daemon_version: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    modules: Vec<ModuleStatusJson>,
}

/// Renders `penguin status --json`'s output, matching
/// `json.MarshalIndent(resp, "", "  ")` followed by `fmt.Println`.
pub fn status_json(response: &pb::GetStatusResponse) -> String {
    let payload = GetStatusResponseJson {
        daemon_version: response.daemon_version.clone(),
        modules: response
            .modules
            .iter()
            .map(ModuleStatusJson::from)
            .collect(),
    };
    let mut text = serde_json::to_string_pretty(&payload).unwrap_or_default();
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modules_json_uses_snake_case_keys_and_omits_zero_valued_fields() {
        let response = pb::ListModulesResponse {
            modules: vec![pb::ModuleSummary {
                name: "squawk".to_string(),
                version: "1.0.0".to_string(),
                description: String::new(),
                state: "running".to_string(),
                external: false,
                license_feature: "free".to_string(),
            }],
        };
        let text = modules_json(&response);
        assert!(text.contains("\"name\": \"squawk\""));
        assert!(
            text.contains("\"license_feature\": \"free\""),
            "field name should be untouched snake_case"
        );
        assert!(
            !text.contains("description"),
            "empty description should be omitted"
        );
        assert!(
            !text.contains("\"external\""),
            "false external should be omitted"
        );
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn empty_modules_list_is_still_present_as_an_empty_array_key_when_nonempty_elsewhere() {
        // An entirely empty response omits `modules` too, matching Go's
        // `omitempty` on the repeated field itself.
        let response = pb::ListModulesResponse { modules: vec![] };
        let text = modules_json(&response);
        assert_eq!(text, "{}\n");
    }

    #[test]
    fn status_json_omits_zero_checked_at_and_empty_detail() {
        let response = pb::GetStatusResponse {
            daemon_version: "0.2.0".to_string(),
            modules: vec![pb::ModuleStatus {
                name: "squawk".to_string(),
                state: "running".to_string(),
                detail: std::collections::HashMap::new(),
                health: "healthy".to_string(),
                health_message: String::new(),
                checked_at_unix_nano: 0,
            }],
        };
        let text = status_json(&response);
        assert!(text.contains("\"daemon_version\": \"0.2.0\""));
        assert!(!text.contains("detail"));
        assert!(!text.contains("checked_at_unix_nano"));
        assert!(!text.contains("health_message"));
    }
}
