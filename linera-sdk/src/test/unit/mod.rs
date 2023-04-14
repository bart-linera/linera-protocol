// Copyright (c) Zefchain Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Helper types for writing unit tests for WebAssembly applications.
//!
//! Unit tests are usually written with the application's source code, and are placed inside the
//! `src` directory together with the main code. The tests are executed by a custom test runner
//! inside an isolated WebAssembly runtime.
//!
//! The system API isn't available to the tests by default. However, calls to them are intercepted
//! and can be controlled by the test to return mock values using the functions in this module.

// Import the contract system interface.
wit_bindgen_guest_rust::export!("mock_system_api.wit");

use self::mock_system_api as wit;

/// Implementation of type that exports an interface for using the mock system API.
pub struct MockSystemApi;

impl wit::MockSystemApi for MockSystemApi {
    fn mocked_chain_id() -> wit::CryptoHash {
        todo!();
    }

    fn mocked_application_id() -> wit::ApplicationId {
        todo!();
    }

    fn mocked_application_parameters() -> Vec<u8> {
        todo!();
    }

    fn mocked_read_system_balance() -> wit::Balance {
        todo!();
    }

    fn mocked_read_system_timestamp() -> u64 {
        todo!();
    }

    fn mocked_log(message: String, level: wit::LogLevel) {
        todo!();
    }

    fn mocked_load() -> Vec<u8> {
        todo!();
    }

    fn mocked_load_and_lock() -> Option<Vec<u8>> {
        todo!();
    }

    fn mocked_store_and_unlock(state: Vec<u8>) -> bool {
        todo!();
    }

    fn mocked_lock() -> bool {
        todo!();
    }

    fn mocked_unlock() -> bool {
        todo!();
    }

    fn mocked_read_key_bytes(key: Vec<u8>) -> Option<Vec<u8>> {
        todo!();
    }

    fn mocked_find_keys(prefix: Vec<u8>) -> Vec<Vec<u8>> {
        todo!();
    }

    fn mocked_find_key_values(prefix: Vec<u8>) -> Vec<(Vec<u8>, Vec<u8>)> {
        todo!();
    }

    fn mocked_write_batch(operations: Vec<wit::WriteOperation>) {
        todo!();
    }

    fn mocked_try_query_application(
        application: wit::ApplicationId,
        query: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        todo!();
    }
}