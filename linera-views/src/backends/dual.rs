// Copyright (c) Zefchain Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Implements [`crate::store::KeyValueStore`] by combining two existing stores.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(with_testing)]
use crate::store::TestKeyValueStore;
use crate::{
    batch::Batch,
    store::{
        AdminKeyValueStore, KeyValueStoreError, ReadableKeyValueStore, WithError,
        WritableKeyValueStore,
    },
};

/// The initial configuration of the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DualStoreConfig<C1, C2> {
    /// The first config.
    pub first_config: C1,
    /// The second config.
    pub second_config: C2,
}

/// The store in use.
#[derive(Clone, Copy, Debug)]
pub enum StoreInUse {
    /// The first store.
    First,
    /// The second store.
    Second,
}

/// The trait for a (static) root key assignment.
#[cfg_attr(not(web), trait_variant::make(Send + Sync))]
pub trait DualStoreRootKeyAssignment {
    /// Obtains the store assigned to this root key.
    fn assigned_store(root_key: &[u8]) -> Result<StoreInUse, bcs::Error>;
}

/// A store made of two existing stores.
#[derive(Clone)]
pub struct DualStore<S1, S2, A> {
    /// The first underlying store.
    first_store: S1,
    /// The second underlying store.
    second_store: S2,
    /// Which store is currently in use given the root key. (The root key in the other store will be set arbitrarily.)
    store_in_use: StoreInUse,
    /// Marker for the static root key assignment.
    _marker: std::marker::PhantomData<A>,
}

impl<S1, S2, A> WithError for DualStore<S1, S2, A>
where
    S1: WithError,
    S2: WithError,
{
    type Error = DualStoreError<S1::Error, S2::Error>;
}

impl<S1, S2, A> ReadableKeyValueStore for DualStore<S1, S2, A>
where
    S1: ReadableKeyValueStore,
    S2: ReadableKeyValueStore,
    A: DualStoreRootKeyAssignment,
{
    // TODO(#2524): consider changing MAX_KEY_SIZE into a function.
    const MAX_KEY_SIZE: usize = if S1::MAX_KEY_SIZE < S2::MAX_KEY_SIZE {
        S1::MAX_KEY_SIZE
    } else {
        S2::MAX_KEY_SIZE
    };

    fn max_stream_queries(&self) -> usize {
        match self.store_in_use {
            StoreInUse::First => self.first_store.max_stream_queries(),
            StoreInUse::Second => self.second_store.max_stream_queries(),
        }
    }

    async fn read_value_bytes(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
        let result = match self.store_in_use {
            StoreInUse::First => self
                .first_store
                .read_value_bytes(key)
                .await
                .map_err(DualStoreError::First)?,
            StoreInUse::Second => self
                .second_store
                .read_value_bytes(key)
                .await
                .map_err(DualStoreError::Second)?,
        };
        Ok(result)
    }

    async fn contains_key(&self, key: &[u8]) -> Result<bool, Self::Error> {
        let result = match self.store_in_use {
            StoreInUse::First => self
                .first_store
                .contains_key(key)
                .await
                .map_err(DualStoreError::First)?,
            StoreInUse::Second => self
                .second_store
                .contains_key(key)
                .await
                .map_err(DualStoreError::Second)?,
        };
        Ok(result)
    }

    async fn contains_keys(&self, keys: Vec<Vec<u8>>) -> Result<Vec<bool>, Self::Error> {
        let result = match self.store_in_use {
            StoreInUse::First => self
                .first_store
                .contains_keys(keys)
                .await
                .map_err(DualStoreError::First)?,
            StoreInUse::Second => self
                .second_store
                .contains_keys(keys)
                .await
                .map_err(DualStoreError::Second)?,
        };
        Ok(result)
    }

    async fn read_multi_values_bytes(
        &self,
        keys: Vec<Vec<u8>>,
    ) -> Result<Vec<Option<Vec<u8>>>, Self::Error> {
        let result = match self.store_in_use {
            StoreInUse::First => self
                .first_store
                .read_multi_values_bytes(keys)
                .await
                .map_err(DualStoreError::First)?,
            StoreInUse::Second => self
                .second_store
                .read_multi_values_bytes(keys)
                .await
                .map_err(DualStoreError::Second)?,
        };
        Ok(result)
    }

    async fn find_keys_by_prefix(&self, key_prefix: &[u8]) -> Result<Vec<Vec<u8>>, Self::Error> {
        let result = match self.store_in_use {
            StoreInUse::First => self
                .first_store
                .find_keys_by_prefix(key_prefix)
                .await
                .map_err(DualStoreError::First)?,
            StoreInUse::Second => self
                .second_store
                .find_keys_by_prefix(key_prefix)
                .await
                .map_err(DualStoreError::Second)?,
        };
        Ok(result)
    }

    async fn find_key_values_by_prefix(
        &self,
        key_prefix: &[u8],
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, Self::Error> {
        let result = match self.store_in_use {
            StoreInUse::First => self
                .first_store
                .find_key_values_by_prefix(key_prefix)
                .await
                .map_err(DualStoreError::First)?,
            StoreInUse::Second => self
                .second_store
                .find_key_values_by_prefix(key_prefix)
                .await
                .map_err(DualStoreError::Second)?,
        };
        Ok(result)
    }
}

impl<S1, S2, A> WritableKeyValueStore for DualStore<S1, S2, A>
where
    S1: WritableKeyValueStore,
    S2: WritableKeyValueStore,
    A: DualStoreRootKeyAssignment,
{
    const MAX_VALUE_SIZE: usize = usize::MAX;

    async fn write_batch(&self, batch: Batch) -> Result<(), Self::Error> {
        match self.store_in_use {
            StoreInUse::First => self
                .first_store
                .write_batch(batch)
                .await
                .map_err(DualStoreError::First)?,
            StoreInUse::Second => self
                .second_store
                .write_batch(batch)
                .await
                .map_err(DualStoreError::Second)?,
        }
        Ok(())
    }

    async fn clear_journal(&self) -> Result<(), Self::Error> {
        match self.store_in_use {
            StoreInUse::First => self
                .first_store
                .clear_journal()
                .await
                .map_err(DualStoreError::First)?,
            StoreInUse::Second => self
                .second_store
                .clear_journal()
                .await
                .map_err(DualStoreError::Second)?,
        }
        Ok(())
    }
}

impl<S1, S2, A> AdminKeyValueStore for DualStore<S1, S2, A>
where
    S1: AdminKeyValueStore,
    S2: AdminKeyValueStore,
    A: DualStoreRootKeyAssignment,
{
    type Config = DualStoreConfig<S1::Config, S2::Config>;

    fn get_name() -> String {
        format!("dual {} and {}", S1::get_name(), S2::get_name())
    }

    async fn connect(config: &Self::Config, namespace: &str) -> Result<Self, Self::Error> {
        let first_store = S1::connect(&config.first_config, namespace)
            .await
            .map_err(DualStoreError::First)?;
        let second_store = S2::connect(&config.second_config, namespace)
            .await
            .map_err(DualStoreError::Second)?;
        let store_in_use = A::assigned_store(&[])?;
        Ok(Self {
            first_store,
            second_store,
            store_in_use,
            _marker: std::marker::PhantomData,
        })
    }

    fn open_exclusive(&self, root_key: &[u8]) -> Result<Self, Self::Error> {
        let first_store = self
            .first_store
            .open_exclusive(root_key)
            .map_err(DualStoreError::First)?;
        let second_store = self
            .second_store
            .open_exclusive(root_key)
            .map_err(DualStoreError::Second)?;
        let store_in_use = A::assigned_store(root_key)?;
        Ok(Self {
            first_store,
            second_store,
            store_in_use,
            _marker: std::marker::PhantomData,
        })
    }

    async fn list_all(config: &Self::Config) -> Result<Vec<String>, Self::Error> {
        let namespaces1 = S1::list_all(&config.first_config)
            .await
            .map_err(DualStoreError::First)?;
        let mut namespaces = Vec::new();
        for namespace in namespaces1 {
            if S2::exists(&config.second_config, &namespace)
                .await
                .map_err(DualStoreError::Second)?
            {
                namespaces.push(namespace);
            } else {
                tracing::warn!("Namespace {} only exists in the first store", namespace);
            }
        }
        Ok(namespaces)
    }

    async fn list_root_keys(
        config: &Self::Config,
        namespace: &str,
    ) -> Result<Vec<Vec<u8>>, Self::Error> {
        let mut root_keys = S1::list_root_keys(&config.first_config, namespace)
            .await
            .map_err(DualStoreError::First)?;
        root_keys.extend(
            S2::list_root_keys(&config.second_config, namespace)
                .await
                .map_err(DualStoreError::Second)?,
        );
        Ok(root_keys)
    }

    async fn exists(config: &Self::Config, namespace: &str) -> Result<bool, Self::Error> {
        Ok(S1::exists(&config.first_config, namespace)
            .await
            .map_err(DualStoreError::First)?
            && S2::exists(&config.second_config, namespace)
                .await
                .map_err(DualStoreError::Second)?)
    }

    async fn create(config: &Self::Config, namespace: &str) -> Result<(), Self::Error> {
        let exists1 = S1::exists(&config.first_config, namespace)
            .await
            .map_err(DualStoreError::First)?;
        let exists2 = S2::exists(&config.second_config, namespace)
            .await
            .map_err(DualStoreError::Second)?;
        if exists1 && exists2 {
            return Err(DualStoreError::StoreAlreadyExists);
        }
        if !exists1 {
            S1::create(&config.first_config, namespace)
                .await
                .map_err(DualStoreError::First)?;
        }
        if !exists2 {
            S2::create(&config.second_config, namespace)
                .await
                .map_err(DualStoreError::Second)?;
        }
        Ok(())
    }

    async fn delete(config: &Self::Config, namespace: &str) -> Result<(), Self::Error> {
        S1::delete(&config.first_config, namespace)
            .await
            .map_err(DualStoreError::First)?;
        S2::delete(&config.second_config, namespace)
            .await
            .map_err(DualStoreError::Second)?;
        Ok(())
    }
}

#[cfg(with_testing)]
impl<S1, S2, A> TestKeyValueStore for DualStore<S1, S2, A>
where
    S1: TestKeyValueStore,
    S2: TestKeyValueStore,
    A: DualStoreRootKeyAssignment,
{
    async fn new_test_config() -> Result<Self::Config, Self::Error> {
        let first_config = S1::new_test_config().await.map_err(DualStoreError::First)?;
        let second_config = S2::new_test_config()
            .await
            .map_err(DualStoreError::Second)?;
        Ok(DualStoreConfig {
            first_config,
            second_config,
        })
    }
}

/// The error type for [`DualStore`].
#[derive(Error, Debug)]
pub enum DualStoreError<E1, E2> {
    /// Store already exists during a create operation
    #[error("Store already exists during a create operation")]
    StoreAlreadyExists,

    /// Serialization error with BCS.
    #[error(transparent)]
    BcsError(#[from] bcs::Error),

    /// First store.
    #[error("Error in first store: {0}")]
    First(E1),

    /// Second store.
    #[error("Error in second store: {0}")]
    Second(E2),
}

impl<E1, E2> KeyValueStoreError for DualStoreError<E1, E2>
where
    E1: KeyValueStoreError,
    E2: KeyValueStoreError,
{
    const BACKEND: &'static str = "dual_store";
}
