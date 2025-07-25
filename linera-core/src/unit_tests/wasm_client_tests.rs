// Copyright (c) Zefchain Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Wasm specific client tests.
//!
//! These tests only run if a Wasm runtime has been configured by enabling either the `wasmer` or
//! the `wasmtime` feature flags.

// Tests for `RocksDb`, `DynamoDb`, `ScyllaDb` and `Service` are currently disabled
// because they are slow and their behavior appears to be correctly check by the
// test with memory.

#![allow(clippy::large_futures)]
#![cfg(any(feature = "wasmer", feature = "wasmtime"))]

use std::collections::BTreeMap;

use assert_matches::assert_matches;
use async_graphql::Request;
use counter::CounterAbi;
use fungible::{FungibleOperation, InitialState, Parameters};
use linera_base::{
    crypto::InMemorySigner,
    data_types::{Amount, Bytecode, Event, OracleResponse},
    identifiers::{ApplicationId, BlobId, BlobType, StreamId, StreamName},
    ownership::{ChainOwnership, TimeoutConfig},
    vm::VmRuntime,
};
use linera_chain::{data_types::MessageAction, ChainError, ChainExecutionContext};
use linera_execution::{
    ExecutionError, Message, MessageKind, Operation, QueryOutcome, ResourceControlPolicy,
    SystemMessage, SystemOperation, WasmRuntime,
};
use linera_storage::Storage as _;
use serde_json::json;
use test_case::test_case;

#[cfg(feature = "dynamodb")]
use crate::client::client_tests::DynamoDbStorageBuilder;
#[cfg(feature = "rocksdb")]
use crate::client::client_tests::RocksDbStorageBuilder;
#[cfg(feature = "scylladb")]
use crate::client::client_tests::ScyllaDbStorageBuilder;
#[cfg(feature = "storage-service")]
use crate::client::client_tests::ServiceStorageBuilder;
use crate::{
    client::{
        client_tests::{MemoryStorageBuilder, StorageBuilder, TestBuilder},
        ChainClientError,
    },
    local_node::LocalNodeError,
    test_utils::FaultType,
    worker::WorkerError,
};

#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer ; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime ; "wasmtime"))]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn test_memory_create_application(wasm_runtime: WasmRuntime) -> anyhow::Result<()> {
    run_test_create_application(MemoryStorageBuilder::with_wasm_runtime(wasm_runtime)).await
}

#[ignore]
#[cfg(feature = "storage-service")]
#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer ; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime ; "wasmtime"))]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn test_service_create_application(wasm_runtime: WasmRuntime) -> anyhow::Result<()> {
    run_test_create_application(ServiceStorageBuilder::with_wasm_runtime(wasm_runtime).await).await
}

#[ignore]
#[cfg(feature = "rocksdb")]
#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer ; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime ; "wasmtime"))]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn test_rocks_db_create_application(wasm_runtime: WasmRuntime) -> anyhow::Result<()> {
    run_test_create_application(RocksDbStorageBuilder::with_wasm_runtime(wasm_runtime).await).await
}

#[ignore]
#[cfg(feature = "dynamodb")]
#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer ; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime ; "wasmtime"))]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn test_dynamo_db_create_application(wasm_runtime: WasmRuntime) -> anyhow::Result<()> {
    run_test_create_application(DynamoDbStorageBuilder::with_wasm_runtime(wasm_runtime)).await
}

#[ignore]
#[cfg(feature = "scylladb")]
#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer ; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime ; "wasmtime"))]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn test_scylla_db_create_application(wasm_runtime: WasmRuntime) -> anyhow::Result<()> {
    run_test_create_application(ScyllaDbStorageBuilder::with_wasm_runtime(wasm_runtime)).await
}

async fn run_test_create_application<B>(storage_builder: B) -> anyhow::Result<()>
where
    B: StorageBuilder,
{
    let keys = InMemorySigner::new(None);
    let vm_runtime = VmRuntime::Wasm;
    let (contract_path, service_path) =
        linera_execution::wasm_test::get_example_bytecode_paths("counter")?;
    let contract_bytecode = Bytecode::load_from_file(contract_path).await?;
    let service_bytecode = Bytecode::load_from_file(service_path).await?;
    let contract_compressed_len = contract_bytecode.compress().compressed_bytes.len();
    let service_compressed_len = service_bytecode.compress().compressed_bytes.len();

    let mut policy = ResourceControlPolicy::all_categories();
    policy.maximum_bytecode_size = contract_bytecode
        .bytes
        .len()
        .max(service_bytecode.bytes.len()) as u64;
    policy.maximum_blob_size = contract_compressed_len.max(service_compressed_len) as u64;
    let mut builder = TestBuilder::new(storage_builder, 4, 1, keys)
        .await?
        .with_policy(policy.clone());
    let publisher = builder.add_root_chain(0, Amount::from_tokens(3)).await?;
    let creator = builder.add_root_chain(1, Amount::ONE).await?;

    let (module_id, _cert) = publisher
        .publish_module(contract_bytecode, service_bytecode, vm_runtime)
        .await
        .unwrap()
        .unwrap();
    let module_id = module_id.with_abi::<counter::CounterAbi, (), u64>();

    creator.synchronize_from_validators().await.unwrap();
    creator.process_inbox().await.unwrap();

    // No fuel was used so far.
    let balance_after_messaging = creator.local_balance().await?;
    assert_eq!(balance_after_messaging, Amount::ONE);

    let initial_value = 10_u64;
    let (application_id, _) = creator
        .create_application(module_id, &(), &initial_value, vec![])
        .await
        .unwrap()
        .unwrap();

    let increment = 5_u64;
    creator
        .execute_operation(Operation::user(application_id, &increment)?)
        .await
        .unwrap();

    let query = Request::new("{ value }");
    let outcome = creator
        .query_user_application(application_id, &query)
        .await
        .unwrap();

    let expected = QueryOutcome {
        response: async_graphql::Response::new(
            async_graphql::Value::from_json(json!({"value": 15})).unwrap(),
        ),
        operations: vec![],
    };

    assert_eq!(outcome, expected);
    // Creating the application used fuel because of the `instantiate` call.
    let balance_after_init = creator.local_balance().await?;
    assert!(balance_after_init < balance_after_messaging);

    let large_bytecode = Bytecode::new(vec![0; policy.maximum_bytecode_size as usize + 1]);
    let small_bytecode = Bytecode::new(vec![]);
    // Publishing bytecode that exceeds the limit fails.
    let result = publisher
        .publish_module(large_bytecode.clone(), small_bytecode.clone(), vm_runtime)
        .await;
    assert_matches!(
        result,
        Err(ChainClientError::LocalNodeError(
            LocalNodeError::WorkerError(WorkerError::ChainError(chain_error))
        )) if matches!(&*chain_error, ChainError::ExecutionError(
            error, ChainExecutionContext::Block
        ) if matches!(**error, ExecutionError::BytecodeTooLarge))
    );
    let result = publisher
        .publish_module(small_bytecode, large_bytecode, vm_runtime)
        .await;
    assert_matches!(
        result,
        Err(ChainClientError::LocalNodeError(
            LocalNodeError::WorkerError(WorkerError::ChainError(chain_error))
        )) if matches!(&*chain_error, ChainError::ExecutionError(
            error, ChainExecutionContext::Block
        ) if matches!(**error, ExecutionError::BytecodeTooLarge))
    );

    Ok(())
}

#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer ; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime ; "wasmtime"))]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn test_memory_run_application_with_dependency(
    wasm_runtime: WasmRuntime,
) -> anyhow::Result<()> {
    run_test_run_application_with_dependency(MemoryStorageBuilder::with_wasm_runtime(wasm_runtime))
        .await
}

#[ignore]
#[cfg(feature = "storage-service")]
#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer ; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime ; "wasmtime"))]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn test_service_run_application_with_dependency(
    wasm_runtime: WasmRuntime,
) -> anyhow::Result<()> {
    run_test_run_application_with_dependency(
        ServiceStorageBuilder::with_wasm_runtime(wasm_runtime).await,
    )
    .await
}

#[ignore]
#[cfg(feature = "rocksdb")]
#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer ; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime ; "wasmtime"))]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn test_rocks_db_run_application_with_dependency(
    wasm_runtime: WasmRuntime,
) -> anyhow::Result<()> {
    run_test_run_application_with_dependency(
        RocksDbStorageBuilder::with_wasm_runtime(wasm_runtime).await,
    )
    .await
}

#[ignore]
#[cfg(feature = "dynamodb")]
#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer ; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime ; "wasmtime"))]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn test_dynamo_db_run_application_with_dependency(
    wasm_runtime: WasmRuntime,
) -> anyhow::Result<()> {
    run_test_run_application_with_dependency(DynamoDbStorageBuilder::with_wasm_runtime(
        wasm_runtime,
    ))
    .await
}

#[ignore]
#[cfg(feature = "scylladb")]
#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer ; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime ; "wasmtime"))]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn test_scylla_db_run_application_with_dependency(
    wasm_runtime: WasmRuntime,
) -> anyhow::Result<()> {
    run_test_run_application_with_dependency(ScyllaDbStorageBuilder::with_wasm_runtime(
        wasm_runtime,
    ))
    .await
}

async fn run_test_run_application_with_dependency<B>(storage_builder: B) -> anyhow::Result<()>
where
    B: StorageBuilder,
{
    let keys = InMemorySigner::new(None);
    let vm_runtime = VmRuntime::Wasm;
    let mut builder = TestBuilder::new(storage_builder, 4, 1, keys)
        .await?
        .with_policy(ResourceControlPolicy::all_categories());
    // Will publish the module.
    let publisher = builder.add_root_chain(0, Amount::from_tokens(3)).await?;
    // Will create the apps and use them to send a message.
    let creator = builder.add_root_chain(1, Amount::ONE).await?;
    // Will receive the message.
    let receiver = builder.add_root_chain(2, Amount::ONE).await?;
    let receiver_id = receiver.chain_id();

    // Handling the message causes an oracle request to the counter service, so no fast blocks
    // are allowed.
    let receiver_key = receiver.identity().await.unwrap();

    receiver
        .change_ownership(ChainOwnership::multiple(
            [(receiver_key, 100)],
            100,
            TimeoutConfig::default(),
        ))
        .await
        .unwrap();

    let creator_key = creator.identity().await.unwrap();
    creator
        .change_ownership(ChainOwnership::multiple(
            [(creator_key, 100)],
            100,
            TimeoutConfig::default(),
        ))
        .await
        .unwrap();

    let (module_id1, _cert1) = {
        let (contract_path, service_path) =
            linera_execution::wasm_test::get_example_bytecode_paths("counter")?;
        publisher
            .publish_module(
                Bytecode::load_from_file(contract_path).await?,
                Bytecode::load_from_file(service_path).await?,
                vm_runtime,
            )
            .await
            .unwrap()
            .unwrap()
    };
    let module_id1 = module_id1.with_abi::<counter::CounterAbi, (), u64>();
    let (module_id2, _cert2) = {
        let (contract_path, service_path) =
            linera_execution::wasm_test::get_example_bytecode_paths("meta_counter")?;
        publisher
            .publish_module(
                Bytecode::load_from_file(contract_path).await?,
                Bytecode::load_from_file(service_path).await?,
                vm_runtime,
            )
            .await
            .unwrap()
            .unwrap()
    };
    let module_id2 =
        module_id2.with_abi::<meta_counter::MetaCounterAbi, ApplicationId<CounterAbi>, ()>();

    // Creator receives the bytecode files then creates the app.
    creator.synchronize_from_validators().await.unwrap();
    let initial_value = 10_u64;
    let (application_id1, _) = creator
        .create_application(module_id1, &(), &initial_value, vec![])
        .await
        .unwrap()
        .unwrap();
    let (application_id2, certificate) = creator
        .create_application(
            module_id2,
            &application_id1,
            &(),
            vec![application_id1.forget_abi()],
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        certificate.block().body.events,
        vec![vec![Event {
            stream_id: StreamId {
                application_id: application_id2.forget_abi().into(),
                stream_name: StreamName(b"announcements".to_vec()),
            },
            index: 0,
            value: bcs::to_bytes(&"instantiated".to_string()).unwrap(),
        }]]
    );

    let mut operation = meta_counter::Operation::increment(receiver_id, 5, true);
    operation.fuel_grant = 1000000;
    let cert = creator
        .execute_operation(Operation::user(application_id2, &operation)?)
        .await
        .unwrap()
        .unwrap();
    let block = cert.block();
    let responses = &block.body.oracle_responses;
    let [_, responses] = &responses[..] else {
        panic!("Unexpected oracle responses: {:?}", responses);
    };
    let [OracleResponse::Service(json)] = &responses[..] else {
        assert_eq!(&responses[..], &[]);
        panic!("Unexpected oracle responses: {:?}", responses);
    };
    let response_json = serde_json::from_slice::<serde_json::Value>(json).unwrap();
    assert_eq!(response_json["data"], json!({"value": 10}));

    receiver.synchronize_from_validators().await.unwrap();
    receiver
        .receive_certificate_and_update_validators(cert)
        .await
        .unwrap();
    receiver.process_inbox().await.unwrap();

    let query = Request::new("{ value }");
    let outcome = receiver
        .query_user_application(application_id2, &query)
        .await
        .unwrap();

    let expected = QueryOutcome {
        response: async_graphql::Response::new(
            async_graphql::Value::from_json(json!({"value": 5})).unwrap(),
        ),
        operations: vec![],
    };

    assert_eq!(outcome, expected);

    // Try again with a value that will make the (untracked) message fail.
    let operation = meta_counter::Operation::fail(receiver_id);
    let cert = creator
        .execute_operation(Operation::user(application_id2, &operation)?)
        .await
        .unwrap()
        .unwrap();

    receiver
        .receive_certificate_and_update_validators(cert)
        .await
        .unwrap();
    let mut certs = receiver.process_inbox().await.unwrap().0;
    assert_eq!(certs.len(), 1);
    let cert = certs.pop().unwrap();
    let incoming_bundles = &cert.block().body.incoming_bundles;
    assert_eq!(incoming_bundles.len(), 1);
    assert_eq!(incoming_bundles[0].action, MessageAction::Reject);
    assert_eq!(
        incoming_bundles[0].bundle.messages[0].kind,
        MessageKind::Simple
    );
    let messages = cert.block().messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].len(), 0);

    // Try again with a value that will make the (tracked) message fail.
    let mut operation = meta_counter::Operation::fail(receiver_id);
    operation.is_tracked = true;
    let cert = creator
        .execute_operation(Operation::user(application_id2, &operation)?)
        .await
        .unwrap()
        .unwrap();

    receiver
        .receive_certificate_and_update_validators(cert)
        .await
        .unwrap();
    let mut certs = receiver.process_inbox().await.unwrap().0;
    assert_eq!(certs.len(), 1);
    let cert = certs.pop().unwrap();
    let incoming_bundles = &cert.block().body.incoming_bundles;
    assert_eq!(incoming_bundles.len(), 1);
    assert_eq!(incoming_bundles[0].action, MessageAction::Reject);
    assert_eq!(
        incoming_bundles[0].bundle.messages[0].kind,
        MessageKind::Tracked
    );
    let messages = cert.block().messages();
    assert_eq!(messages.len(), 1);

    // The bounced message is marked as "bouncing" in the Wasm context and succeeds.
    creator
        .receive_certificate_and_update_validators(cert)
        .await
        .unwrap();
    let mut certs = creator.process_inbox().await.unwrap().0;
    assert_eq!(certs.len(), 1);
    let cert = certs.pop().unwrap();
    let incoming_bundles = &cert.block().body.incoming_bundles;
    assert_eq!(incoming_bundles.len(), 2);
    // First message is the grant refund for the successful message sent before.
    assert_eq!(incoming_bundles[0].action, MessageAction::Accept);
    assert_eq!(
        incoming_bundles[0].bundle.messages[0].kind,
        MessageKind::Tracked
    );
    assert_matches!(
        incoming_bundles[0].bundle.messages[0].message,
        Message::System(SystemMessage::Credit { .. })
    );
    // Second message is the bounced message.
    assert_eq!(incoming_bundles[1].action, MessageAction::Accept);
    assert_eq!(
        incoming_bundles[1].bundle.messages[0].kind,
        MessageKind::Bouncing
    );
    assert_matches!(
        incoming_bundles[1].bundle.messages[0].message,
        Message::User { .. }
    );

    Ok(())
}

#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer ; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime ; "wasmtime"))]
#[test_log::test(tokio::test)]
async fn test_memory_cross_chain_message(wasm_runtime: WasmRuntime) -> anyhow::Result<()> {
    run_test_cross_chain_message(MemoryStorageBuilder::with_wasm_runtime(wasm_runtime)).await
}

#[ignore]
#[cfg(feature = "storage-service")]
#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer ; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime ; "wasmtime"))]
#[test_log::test(tokio::test)]
async fn test_service_cross_chain_message(wasm_runtime: WasmRuntime) -> anyhow::Result<()> {
    run_test_cross_chain_message(ServiceStorageBuilder::with_wasm_runtime(wasm_runtime).await).await
}

#[ignore]
#[cfg(feature = "rocksdb")]
#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer ; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime ; "wasmtime"))]
#[test_log::test(tokio::test)]
async fn test_rocks_db_cross_chain_message(wasm_runtime: WasmRuntime) -> anyhow::Result<()> {
    run_test_cross_chain_message(RocksDbStorageBuilder::with_wasm_runtime(wasm_runtime).await).await
}

#[ignore]
#[cfg(feature = "dynamodb")]
#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer ; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime ; "wasmtime"))]
#[test_log::test(tokio::test)]
async fn test_dynamo_db_cross_chain_message(wasm_runtime: WasmRuntime) -> anyhow::Result<()> {
    run_test_cross_chain_message(DynamoDbStorageBuilder::with_wasm_runtime(wasm_runtime)).await
}

#[ignore]
#[cfg(feature = "scylladb")]
#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer ; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime ; "wasmtime"))]
#[test_log::test(tokio::test)]
async fn test_scylla_db_cross_chain_message(wasm_runtime: WasmRuntime) -> anyhow::Result<()> {
    run_test_cross_chain_message(ScyllaDbStorageBuilder::with_wasm_runtime(wasm_runtime)).await
}

async fn run_test_cross_chain_message<B>(storage_builder: B) -> anyhow::Result<()>
where
    B: StorageBuilder,
{
    let keys = InMemorySigner::new(None);
    let vm_runtime = VmRuntime::Wasm;
    let mut builder = TestBuilder::new(storage_builder, 4, 1, keys)
        .await?
        .with_policy(ResourceControlPolicy::all_categories());
    let _admin = builder.add_root_chain(0, Amount::ONE).await?;
    let sender = builder.add_root_chain(1, Amount::from_tokens(3)).await?;
    let receiver = builder.add_root_chain(2, Amount::ONE).await?;
    let receiver2 = builder.add_root_chain(3, Amount::ONE).await?;

    let (module_id, _pub_cert) = {
        let bytecode_name = "fungible";
        let (contract_path, service_path) =
            linera_execution::wasm_test::get_example_bytecode_paths(bytecode_name)?;
        sender
            .publish_module(
                Bytecode::load_from_file(contract_path).await?,
                Bytecode::load_from_file(service_path).await?,
                vm_runtime,
            )
            .await
            .unwrap()
            .unwrap()
    };
    let module_id = module_id.with_abi::<fungible::FungibleTokenAbi, Parameters, InitialState>();

    let sender_owner = sender.preferred_owner.unwrap();
    let receiver_owner = receiver.preferred_owner.unwrap();
    let receiver2_owner = receiver2.preferred_owner.unwrap();

    let accounts = BTreeMap::from_iter([(sender_owner, Amount::from_tokens(1_000_000))]);
    let state = InitialState { accounts };
    let params = Parameters::new("FUN");
    let (application_id, _cert) = sender
        .create_application(module_id, &params, &state, vec![])
        .await
        .unwrap()
        .unwrap();

    // Make a transfer using the fungible app.
    let transfer = FungibleOperation::Transfer {
        owner: sender_owner,
        amount: 100.into(),
        target_account: fungible::Account {
            chain_id: receiver.chain_id(),
            owner: receiver_owner,
        },
    };
    let cert = sender
        .execute_operation(Operation::user(application_id, &transfer)?)
        .await
        .unwrap()
        .unwrap();

    receiver.synchronize_from_validators().await.unwrap();
    {
        // The receiver did not execute the sender chain.
        let chain = receiver
            .storage_client()
            .load_chain(sender.chain_id())
            .await?;
        assert_eq!(chain.tip_state.get().next_block_height.0, 0);
        assert_eq!(
            chain
                .preprocessed_blocks
                .get(&cert.inner().height())
                .await?,
            Some(cert.hash())
        );
    }
    let certs = receiver.process_inbox().await.unwrap().0;
    assert_eq!(certs.len(), 1);
    let messages = &certs[0].block().body.incoming_bundles;
    assert!(messages
        .iter()
        .flat_map(|msg| &msg.bundle.messages)
        .any(|msg| matches!(msg.message, Message::User { .. })));

    // Make another transfer.
    let transfer = FungibleOperation::Transfer {
        owner: sender_owner,
        amount: 200.into(),
        target_account: fungible::Account {
            chain_id: receiver.chain_id(),
            owner: receiver_owner,
        },
    };
    let cert = sender
        .execute_operation(Operation::user(application_id, &transfer)?)
        .await
        .unwrap()
        .unwrap();

    receiver
        .receive_certificate_and_update_validators(cert)
        .await
        .unwrap();
    let certs = receiver.process_inbox().await.unwrap().0;
    assert_eq!(certs.len(), 1);
    let messages = &certs[0].block().body.incoming_bundles;
    assert!(messages
        .iter()
        .flat_map(|msg| &msg.bundle.messages)
        .any(|msg| matches!(msg.message, Message::User { .. })));

    // Try another transfer except that the amount is too large.
    let transfer = FungibleOperation::Transfer {
        owner: receiver_owner,
        amount: 301.into(),
        target_account: fungible::Account {
            chain_id: receiver2.chain_id(),
            owner: receiver2_owner,
        },
    };
    assert!(receiver
        .execute_operation(Operation::user(application_id, &transfer)?)
        .await
        .is_err());
    receiver.clear_pending_proposal();

    // Try another transfer with the correct amount.
    let transfer = FungibleOperation::Transfer {
        owner: receiver_owner,
        amount: 300.into(),
        target_account: fungible::Account {
            chain_id: receiver2.chain_id(),
            owner: receiver2_owner,
        },
    };
    let certificate = receiver
        .execute_operation(Operation::user(application_id, &transfer)?)
        .await
        .unwrap()
        .unwrap();

    receiver2
        .receive_certificate_and_update_validators(certificate)
        .await
        .unwrap();

    Ok(())
}

#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime; "wasmtime"))]
#[test_log::test(tokio::test)]
async fn test_memory_event_streams(wasm_runtime: WasmRuntime) -> anyhow::Result<()> {
    run_test_event_streams(MemoryStorageBuilder::with_wasm_runtime(wasm_runtime)).await
}

#[ignore]
#[cfg(feature = "storage-service")]
#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime; "wasmtime"))]
#[test_log::test(tokio::test)]
async fn test_service_event_streams(wasm_runtime: WasmRuntime) -> anyhow::Result<()> {
    run_test_event_streams(ServiceStorageBuilder::with_wasm_runtime(wasm_runtime).await).await
}

#[ignore]
#[cfg(feature = "rocksdb")]
#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime; "wasmtime"))]
#[test_log::test(tokio::test)]
async fn test_rocks_db_event_streams(wasm_runtime: WasmRuntime) -> anyhow::Result<()> {
    run_test_event_streams(RocksDbStorageBuilder::with_wasm_runtime(wasm_runtime).await).await
}

#[ignore]
#[cfg(feature = "dynamodb")]
#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime; "wasmtime"))]
#[test_log::test(tokio::test)]
async fn test_dynamo_db_event_streams(wasm_runtime: WasmRuntime) -> anyhow::Result<()> {
    run_test_event_streams(DynamoDbStorageBuilder::with_wasm_runtime(wasm_runtime)).await
}

#[ignore]
#[cfg(feature = "scylladb")]
#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime; "wasmtime"))]
#[test_log::test(tokio::test)]
async fn test_scylla_db_event_streams(wasm_runtime: WasmRuntime) -> anyhow::Result<()> {
    run_test_event_streams(ScyllaDbStorageBuilder::with_wasm_runtime(wasm_runtime)).await
}

async fn run_test_event_streams<B>(storage_builder: B) -> anyhow::Result<()>
where
    B: StorageBuilder,
{
    let keys = InMemorySigner::new(None);
    let vm_runtime = VmRuntime::Wasm;
    let mut builder = TestBuilder::new(storage_builder, 4, 0, keys)
        .await?
        .with_policy(ResourceControlPolicy::all_categories());
    builder.set_fault_type([3], FaultType::Offline).await;

    let sender = builder.add_root_chain(0, Amount::ONE).await?;
    let receiver = builder.add_root_chain(1, Amount::ONE).await?;

    let (module_id, _pub_cert) = {
        let (contract_path, service_path) =
            linera_execution::wasm_test::get_example_bytecode_paths("social")?;
        receiver
            .publish_module(
                Bytecode::load_from_file(contract_path).await?,
                Bytecode::load_from_file(service_path).await?,
                vm_runtime,
            )
            .await
            .unwrap()
            .unwrap()
    };
    let module_id = module_id.with_abi::<social::SocialAbi, (), ()>();

    let (application_id, _cert) = receiver
        .create_application(module_id, &(), &(), vec![])
        .await
        .unwrap()
        .unwrap();

    // Request to subscribe to the sender.
    let request_subscribe = social::Operation::Subscribe {
        chain_id: sender.chain_id(),
    };
    receiver
        .execute_operation(Operation::user(application_id, &request_subscribe)?)
        .await
        .unwrap()
        .unwrap();

    // Make a post.
    let text = "Please like and comment!".to_string();
    let post = social::Operation::Post {
        text: text.clone(),
        image_url: None,
    };
    let cert = sender
        .execute_operation(Operation::user(application_id, &post)?)
        .await
        .unwrap()
        .unwrap();

    receiver
        .receive_certificate_and_update_validators(cert.clone())
        .await
        .unwrap();

    builder.set_fault_type([3], FaultType::Honest).await;
    builder.set_fault_type([2], FaultType::Offline).await;

    let certs = receiver.process_inbox().await.unwrap().0;
    assert_eq!(certs.len(), 1);

    // There should be an UpdateStreams operation due to the new post.
    let [Operation::System(operation)] = &*certs[0].block().body.operations else {
        panic!(
            "Expected one operation, got {:?}",
            certs[0].block().body.operations
        );
    };
    let stream_id = StreamId {
        application_id: application_id.forget_abi().into(),
        stream_name: b"posts".into(),
    };
    assert_eq!(
        **operation,
        SystemOperation::UpdateStreams(vec![(sender.chain_id(), stream_id, 1)])
    );

    let query = async_graphql::Request::new("{ receivedPosts { keys { author, index } } }");
    let outcome = receiver
        .query_user_application(application_id, &query)
        .await?;
    let expected = QueryOutcome {
        response: async_graphql::Response::new(
            async_graphql::Value::from_json(json!({
                "receivedPosts": {
                    "keys": [
                        { "author": sender.chain_id, "index": 0 }
                    ]
                }
            }))
            .unwrap(),
        ),
        operations: vec![],
    };
    assert_eq!(outcome, expected);

    // Request to unsubscribe from the sender.
    let request_unsubscribe = social::Operation::Unsubscribe {
        chain_id: sender.chain_id(),
    };
    let cert = receiver
        .execute_operation(Operation::user(application_id, &request_unsubscribe)?)
        .await
        .unwrap()
        .unwrap();

    // Unsubscribe the receiver.
    sender.synchronize_from_validators().await.unwrap();
    sender
        .receive_certificate_and_update_validators(cert)
        .await
        .unwrap();
    let _certs = sender.process_inbox().await.unwrap();

    // Make a post.
    let post = social::Operation::Post {
        text: "Nobody will read this!".to_string(),
        image_url: None,
    };
    let cert = sender
        .execute_operation(Operation::user(application_id, &post)?)
        .await
        .unwrap()
        .unwrap();

    // The post will not be received by the unsubscribed chain.
    receiver
        .receive_certificate_and_update_validators(cert)
        .await
        .unwrap();
    let certs = receiver.process_inbox().await.unwrap().0;
    assert!(certs.is_empty());

    // There is still only one post it can see.
    let query = async_graphql::Request::new("{ receivedPosts { keys { author, index } } }");
    let outcome = receiver
        .query_user_application(application_id, &query)
        .await
        .unwrap();
    let expected = QueryOutcome {
        response: async_graphql::Response::new(
            async_graphql::Value::from_json(json!({
                "receivedPosts": {
                    "keys": [ { "author": sender.chain_id, "index": 0 } ]
                }
            }))
            .unwrap(),
        ),
        operations: vec![],
    };
    assert_eq!(outcome, expected);

    Ok(())
}

#[cfg_attr(feature = "wasmer", test_case(WasmRuntime::Wasmer ; "wasmer"))]
#[cfg_attr(feature = "wasmtime", test_case(WasmRuntime::Wasmtime ; "wasmtime"))]
#[test_log::test(tokio::test(flavor = "multi_thread"))]
async fn test_memory_fuel_limit(wasm_runtime: WasmRuntime) -> anyhow::Result<()> {
    let vm_runtime = VmRuntime::Wasm;
    let storage_builder = MemoryStorageBuilder::with_wasm_runtime(wasm_runtime);
    // Set a fuel limit that is enough to instantiate the application and do one increment
    // operation, but not ten. We also verify blob fees for the bytecode.
    let policy = ResourceControlPolicy {
        maximum_wasm_fuel_per_block: 30_000,
        blob_read: Amount::from_tokens(10), // Should not be charged.
        blob_published: Amount::from_attos(100),
        blob_byte_read: Amount::from_tokens(10), // Should not be charged.
        blob_byte_published: Amount::from_attos(1),
        ..ResourceControlPolicy::default()
    };
    let keys = InMemorySigner::new(None);
    let mut builder = TestBuilder::new(storage_builder, 4, 1, keys)
        .await?
        .with_policy(policy.clone());
    let publisher = builder.add_root_chain(0, Amount::from_tokens(3)).await?;

    let (contract_path, service_path) =
        linera_execution::wasm_test::get_example_bytecode_paths("counter")?;

    let mut expected_balance = publisher.local_balance().await?;
    let (module_id, _cert) = publisher
        .publish_module(
            Bytecode::load_from_file(contract_path).await?,
            Bytecode::load_from_file(service_path).await?,
            vm_runtime,
        )
        .await
        .unwrap()
        .unwrap();
    let module_id = module_id.with_abi::<counter::CounterAbi, (), u64>();
    let mut blobs = publisher
        .storage_client()
        .read_blobs(&[
            BlobId::new(module_id.contract_blob_hash, BlobType::ContractBytecode),
            BlobId::new(module_id.service_blob_hash, BlobType::ServiceBytecode),
        ])
        .await?
        .into_iter()
        .flatten();
    expected_balance = expected_balance
        - policy.blob_published * 2
        - policy.blob_byte_published
            * (blobs.next().unwrap().bytes().len() as u128
                + blobs.next().unwrap().bytes().len() as u128);
    assert_eq!(publisher.local_balance().await?, expected_balance);

    let initial_value = 10_u64;
    let (application_id, _) = publisher
        .create_application(module_id, &(), &initial_value, vec![])
        .await
        .unwrap()
        .unwrap();

    let increment = 5_u64;
    publisher
        .execute_operation(Operation::user(application_id, &increment)?)
        .await
        .unwrap()
        .unwrap();

    assert!(publisher
        .execute_operations(
            vec![Operation::user(application_id, &increment)?; 10],
            vec![]
        )
        .await
        .is_err());

    Ok(())
}
