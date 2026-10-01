//! Sharding interface — one function, one rule.

use uuid::Uuid;

use crate::types::{OrganizationId, ProjectId, UserId};

/// The ids a row may be sharded by — the two tenancy levels and the person,
/// the three keys row security binds, and nothing else.
pub trait ShardSubject: sealed::Sealed {
    /// The bytes hashed into the shard key.
    fn shard_input_bytes(&self) -> Vec<u8>;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::ProjectId {}
    impl Sealed for super::OrganizationId {}
    impl Sealed for super::UserId {}
}

impl ShardSubject for ProjectId {
    fn shard_input_bytes(&self) -> Vec<u8> {
        self.to_string().into_bytes()
    }
}

impl ShardSubject for OrganizationId {
    fn shard_input_bytes(&self) -> Vec<u8> {
        self.to_string().into_bytes()
    }
}

impl ShardSubject for UserId {
    fn shard_input_bytes(&self) -> Vec<u8> {
        self.to_string().into_bytes()
    }
}

/// Stable namespace UUID for shard-key derivation. ⚠ Must never move: changing
/// it would re-route every tenant to a different shard on the day we split.
const SHARD_KEY_NAMESPACE: Uuid = Uuid::from_u128(0x6f0a_4d8a_7c52_4f6e_9f8a_b7c1_d2e3_f4a5);

/// Derive the shard key for a tenant id — a deterministic `UUIDv5`, so one id
/// always gives one key and a tenant's rows co-locate. The sealed trait makes a
/// swapped argument a compile error; the key itself is a function of the
/// characters alone.
#[must_use]
pub fn derive_shard_key<T: ShardSubject + ?Sized>(id: &T) -> Uuid {
    Uuid::new_v5(&SHARD_KEY_NAMESPACE, &id.shard_input_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(s: &str) -> ProjectId {
        ProjectId::try_new(s).expect("valid test project id")
    }

    fn person(s: &str) -> crate::types::UserId {
        crate::types::UserId::try_new(s).expect("valid test user id")
    }

    #[test]
    fn deterministic_per_subject() {
        assert_eq!(
            derive_shard_key(&project("project_acme")),
            derive_shard_key(&project("project_acme")),
            "same project id must produce the same shard_key"
        );
        assert_eq!(
            derive_shard_key(&person("user_ada")),
            derive_shard_key(&person("user_ada")),
            "same user id must produce the same shard_key"
        );
    }

    #[test]
    fn different_subjects_yield_different_keys() {
        assert_ne!(
            derive_shard_key(&project("project_acme")),
            derive_shard_key(&project("project_evil"))
        );
        assert_ne!(
            derive_shard_key(&person("user_ada")),
            derive_shard_key(&person("user_bob"))
        );
    }
}
