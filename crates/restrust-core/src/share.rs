//! Sharing grants: who a collection is shared with and at what access level
//! (PRD §6). Full Access manages content but never ownership — deleting the
//! collection or changing its sharing stays owner-only.

use serde::{Deserialize, Serialize};

/// What a share subject may do with a collection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccessLevel {
    ReadOnly,
    FullAccess,
}

/// Who a share is granted to: a user, an organization, or everyone (public).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ShareSubject {
    User { id: String, username: String },
    Organization { id: String, name: String },
    Public,
}

/// A single sharing grant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareGrant {
    pub subject: ShareSubject,
    pub level: AccessLevel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_types_json_roundtrip() {
        let grants = vec![
            ShareGrant {
                subject: ShareSubject::User {
                    id: "u1".to_string(),
                    username: "alice".to_string(),
                },
                level: AccessLevel::ReadOnly,
            },
            ShareGrant {
                subject: ShareSubject::Organization {
                    id: "o1".to_string(),
                    name: "Acme".to_string(),
                },
                level: AccessLevel::FullAccess,
            },
            ShareGrant {
                subject: ShareSubject::Public,
                level: AccessLevel::ReadOnly,
            },
        ];
        let json = serde_json::to_string(&grants).unwrap();
        assert!(json.contains("\"fullAccess\""));
        assert!(json.contains("\"readOnly\""));
        let back: Vec<ShareGrant> = serde_json::from_str(&json).unwrap();
        assert_eq!(grants, back);
    }
}
