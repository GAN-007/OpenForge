use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! typed_id {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            pub fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            pub fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self)
            }
        }
    };
}

typed_id!(RunId);
typed_id!(ProjectId);
typed_id!(TaskId);
typed_id!(EventId);
typed_id!(SessionId);
typed_id!(ApprovalId);
typed_id!(AgentInstanceId);
typed_id!(ToolInvocationId);
typed_id!(ModelInvocationId);
typed_id!(SecretLeaseId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_time_ordered_uuid_v7_values() {
        let first = RunId::new();
        let second = RunId::new();
        assert_eq!(first.0.get_version_num(), 7);
        assert_eq!(second.0.get_version_num(), 7);
        assert!(first <= second);
    }
}
