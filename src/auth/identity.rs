use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A unique user identity within the tutti system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
    pub email: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// An organisation that owns one or more workspaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Org {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

/// A workspace scoped to an org, containing agents and configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub org_id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn user_serialization_roundtrip() {
        let user = User {
            id: "u-001".into(),
            name: "Alice".into(),
            email: Some("alice@example.com".into()),
            created_at: Utc::now(),
        };
        let toml_str = toml::to_string(&user).expect("serialize");
        let parsed: User = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(user, parsed);
    }

    #[test]
    fn org_serialization_roundtrip() {
        let org = Org {
            id: "org-001".into(),
            name: "NuttHouse".into(),
            created_at: Utc::now(),
        };
        let toml_str = toml::to_string(&org).expect("serialize");
        let parsed: Org = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(org, parsed);
    }

    #[test]
    fn workspace_serialization_roundtrip() {
        let ws = Workspace {
            id: "ws-001".into(),
            org_id: "org-001".into(),
            name: "default".into(),
            created_at: Utc::now(),
        };
        let toml_str = toml::to_string(&ws).expect("serialize");
        let parsed: Workspace = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(ws, parsed);
    }

    #[test]
    fn user_without_email() {
        let user = User {
            id: "u-002".into(),
            name: "Bob".into(),
            email: None,
            created_at: Utc::now(),
        };
        let toml_str = toml::to_string(&user).expect("serialize");
        let parsed: User = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(parsed.email, None);
    }
}
