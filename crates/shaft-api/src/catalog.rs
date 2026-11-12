//! Catalog of the ngrok API resources Shaft models in its UI. Anything not
//! listed here is still reachable through the generic API explorer.

/// A column shown in resource tables: title and JSON pointer into the item.
#[derive(Debug, Clone, Copy)]
pub struct Column {
    pub title: &'static str,
    pub pointer: &'static str,
}

/// An extra POST action on a single item (e.g. restart an agent).
#[derive(Debug, Clone, Copy)]
pub struct ItemAction {
    pub label: &'static str,
    /// Appended to `/{path}/{id}`.
    pub suffix: &'static str,
    pub destructive: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ResourceKind {
    pub id: &'static str,
    pub title: &'static str,
    /// Collection path, e.g. `/reserved_domains`.
    pub path: &'static str,
    /// Key of the array in list responses.
    pub list_key: &'static str,
    pub columns: &'static [Column],
    /// Pointer to a human name used in delete confirmations.
    pub name_pointer: &'static str,
    /// JSON body pre-filled when creating. None = cannot be created.
    pub create_template: Option<&'static str>,
    pub updatable: bool,
    pub deletable: bool,
    pub actions: &'static [ItemAction],
    /// Field of the create response that is shown only once (e.g. a token).
    pub secret_on_create: Option<&'static str>,
}

const fn col(title: &'static str, pointer: &'static str) -> Column {
    Column { title, pointer }
}

pub const RESOURCES: &[ResourceKind] = &[
    ResourceKind {
        id: "reserved_domains",
        title: "Domains",
        path: "/reserved_domains",
        list_key: "reserved_domains",
        columns: &[
            col("Domain", "/domain"),
            col("CNAME target", "/cname_target"),
            col("Description", "/description"),
            col("Created", "/created_at"),
        ],
        name_pointer: "/domain",
        create_template: Some(
            r#"{
  "domain": "my-app.ngrok.app",
  "description": "",
  "metadata": ""
}"#,
        ),
        updatable: true,
        deletable: true,
        actions: &[],
        secret_on_create: None,
    },
    ResourceKind {
        id: "reserved_addrs",
        title: "TCP Addresses",
        path: "/reserved_addrs",
        list_key: "reserved_addrs",
        columns: &[col("Address", "/addr"), col("Description", "/description"), col("Created", "/created_at")],
        name_pointer: "/addr",
        create_template: Some(
            r#"{
  "description": "",
  "metadata": ""
}"#,
        ),
        updatable: true,
        deletable: true,
        actions: &[],
        secret_on_create: None,
    },
    ResourceKind {
        id: "endpoints",
        title: "Endpoints",
        path: "/endpoints",
        list_key: "endpoints",
        columns: &[
            col("URL", "/url"),
            col("Type", "/type"),
            col("Bindings", "/bindings"),
            col("Description", "/description"),
            col("Created", "/created_at"),
        ],
        name_pointer: "/url",
        create_template: Some(
            r#"{
  "type": "cloud",
  "url": "https://my-app.ngrok.app",
  "bindings": ["public"],
  "description": "",
  "traffic_policy": "on_http_request:\n  - actions:\n      - type: custom-response\n        config:\n          status_code: 200\n          body: Hello from a cloud endpoint\n"
}"#,
        ),
        updatable: true,
        deletable: true,
        actions: &[],
        secret_on_create: None,
    },
    ResourceKind {
        id: "tunnel_sessions",
        title: "Agents",
        path: "/tunnel_sessions",
        list_key: "tunnel_sessions",
        columns: &[
            col("ID", "/id"),
            col("Agent version", "/agent_version"),
            col("IP", "/ip"),
            col("OS", "/os"),
            col("Region", "/region"),
            col("Started", "/started_at"),
        ],
        name_pointer: "/id",
        create_template: None,
        updatable: false,
        deletable: false,
        actions: &[
            ItemAction { label: "Restart", suffix: "/restart", destructive: false },
            ItemAction { label: "Update", suffix: "/update", destructive: false },
            ItemAction { label: "Stop", suffix: "/stop", destructive: true },
        ],
        secret_on_create: None,
    },
    ResourceKind {
        id: "tunnels",
        title: "Tunnels",
        path: "/tunnels",
        list_key: "tunnels",
        columns: &[
            col("Public URL", "/public_url"),
            col("Proto", "/proto"),
            col("Forwards to", "/forwards_to"),
            col("Region", "/region"),
            col("Started", "/started_at"),
        ],
        name_pointer: "/public_url",
        create_template: None,
        updatable: false,
        deletable: false,
        actions: &[],
        secret_on_create: None,
    },
    ResourceKind {
        id: "credentials",
        title: "Authtokens",
        path: "/credentials",
        list_key: "credentials",
        columns: &[col("ID", "/id"), col("Description", "/description"), col("ACL", "/acl"), col("Created", "/created_at")],
        name_pointer: "/description",
        create_template: Some(
            r#"{
  "description": "created by Shaft",
  "metadata": "",
  "acl": []
}"#,
        ),
        updatable: true,
        deletable: true,
        actions: &[],
        secret_on_create: Some("token"),
    },
    ResourceKind {
        id: "api_keys",
        title: "API Keys",
        path: "/api_keys",
        list_key: "keys",
        columns: &[col("ID", "/id"), col("Description", "/description"), col("Owner", "/owner_id"), col("Created", "/created_at")],
        name_pointer: "/description",
        create_template: Some(
            r#"{
  "description": "created by Shaft",
  "metadata": ""
}"#,
        ),
        updatable: true,
        deletable: true,
        actions: &[],
        secret_on_create: Some("token"),
    },
    ResourceKind {
        id: "ssh_credentials",
        title: "SSH Keys",
        path: "/ssh_credentials",
        list_key: "ssh_credentials",
        columns: &[col("ID", "/id"), col("Description", "/description"), col("ACL", "/acl"), col("Created", "/created_at")],
        name_pointer: "/description",
        create_template: Some(
            r#"{
  "public_key": "ssh-ed25519 AAAA…",
  "description": "",
  "acl": []
}"#,
        ),
        updatable: true,
        deletable: true,
        actions: &[],
        secret_on_create: None,
    },
    ResourceKind {
        id: "ip_policies",
        title: "IP Policies",
        path: "/ip_policies",
        list_key: "ip_policies",
        columns: &[col("ID", "/id"), col("Description", "/description"), col("Created", "/created_at")],
        name_pointer: "/description",
        create_template: Some(
            r#"{
  "description": "office",
  "metadata": ""
}"#,
        ),
        updatable: true,
        deletable: true,
        actions: &[],
        secret_on_create: None,
    },
    ResourceKind {
        id: "ip_policy_rules",
        title: "IP Policy Rules",
        path: "/ip_policy_rules",
        list_key: "ip_policy_rules",
        columns: &[col("CIDR", "/cidr"), col("Action", "/action"), col("Policy", "/ip_policy/id"), col("Description", "/description")],
        name_pointer: "/cidr",
        create_template: Some(
            r#"{
  "ip_policy_id": "ipp_…",
  "cidr": "203.0.113.0/24",
  "action": "allow",
  "description": ""
}"#,
        ),
        updatable: true,
        deletable: true,
        actions: &[],
        secret_on_create: None,
    },
    ResourceKind {
        id: "ip_restrictions",
        title: "IP Restrictions",
        path: "/ip_restrictions",
        list_key: "ip_restrictions",
        columns: &[col("ID", "/id"), col("Type", "/type"), col("Enforced", "/enforced"), col("Description", "/description")],
        name_pointer: "/id",
        create_template: Some(
            r#"{
  "type": "dashboard",
  "enforced": false,
  "ip_policy_ids": ["ipp_…"],
  "description": ""
}"#,
        ),
        updatable: true,
        deletable: true,
        actions: &[],
        secret_on_create: None,
    },
    ResourceKind {
        id: "tls_certificates",
        title: "TLS Certificates",
        path: "/tls_certificates",
        list_key: "tls_certificates",
        columns: &[col("Subject CN", "/subject_common_name"), col("Not after", "/not_after"), col("Description", "/description")],
        name_pointer: "/subject_common_name",
        create_template: Some(
            r#"{
  "certificate_pem": "-----BEGIN CERTIFICATE-----\n…",
  "private_key_pem": "-----BEGIN PRIVATE KEY-----\n…",
  "description": ""
}"#,
        ),
        updatable: true,
        deletable: true,
        actions: &[],
        secret_on_create: None,
    },
    ResourceKind {
        id: "event_subscriptions",
        title: "Event Subscriptions",
        path: "/event_subscriptions",
        list_key: "event_subscriptions",
        columns: &[col("ID", "/id"), col("Description", "/description"), col("Created", "/created_at")],
        name_pointer: "/description",
        create_template: Some(
            r#"{
  "description": "",
  "sources": [{ "type": "ip_policy_created.v0" }],
  "destination_ids": ["ed_…"]
}"#,
        ),
        updatable: true,
        deletable: true,
        actions: &[],
        secret_on_create: None,
    },
    ResourceKind {
        id: "event_destinations",
        title: "Event Destinations",
        path: "/event_destinations",
        list_key: "event_destinations",
        columns: &[col("ID", "/id"), col("Format", "/format"), col("Description", "/description")],
        name_pointer: "/description",
        create_template: Some(
            r#"{
  "description": "",
  "format": "json",
  "target": { "debug": { "log": true } }
}"#,
        ),
        updatable: true,
        deletable: true,
        actions: &[],
        secret_on_create: None,
    },
];

pub fn resource(id: &str) -> Option<&'static ResourceKind> {
    RESOURCES.iter().find(|r| r.id == id)
}

/// Render a JSON value as a short table cell.
pub fn cell(item: &serde_json::Value, pointer: &str) -> String {
    match item.pointer(pointer) {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(a)) => {
            a.iter().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).collect::<Vec<_>>().join(", ")
        }
        Some(other) => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_are_valid_json_and_ids_unique() {
        let mut ids = std::collections::HashSet::new();
        for r in RESOURCES {
            assert!(ids.insert(r.id), "duplicate {}", r.id);
            assert!(r.path.starts_with('/'));
            if let Some(t) = r.create_template {
                serde_json::from_str::<serde_json::Value>(t).unwrap_or_else(|e| panic!("{}: {e}", r.id));
            }
        }
        assert!(resource("endpoints").is_some());
    }

    #[test]
    fn cells() {
        let v = serde_json::json!({"a": "x", "b": ["public", "internal"], "c": {"d": 3}, "n": null});
        assert_eq!(cell(&v, "/a"), "x");
        assert_eq!(cell(&v, "/b"), "public, internal");
        assert_eq!(cell(&v, "/c/d"), "3");
        assert_eq!(cell(&v, "/n"), "");
        assert_eq!(cell(&v, "/zz"), "");
    }
}
