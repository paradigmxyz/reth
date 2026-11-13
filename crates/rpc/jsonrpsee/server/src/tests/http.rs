// Copyright 2019-2021 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use crate::{
    middleware::rpc::RpcServiceBuilder, BatchRequestConfig, RegisterMethodError, RpcModule,
    ServerBuilder, ServerConfig, ServerHandle,
};
use jsonrpsee_core::{server::ResponseStream, RpcResult};
use jsonrpsee_test_utils::{
    helpers::*,
    mocks::{Id, StatusCode},
    TimeoutFutureExt,
};
use jsonrpsee_types::{ErrorCode, ErrorObjectOwned};
use serde_json::Value as JsonValue;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::Notify,
};

use super::helpers::{MyAppError, TestContext};

fn init_logger() {
    let _ = tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
}

async fn server() -> (SocketAddr, ServerHandle) {
    let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
    let ctx = TestContext;
    let mut module = RpcModule::new(ctx);
    let addr = server.local_addr().unwrap();
    module.register_method("say_hello", |_, _| "lo").unwrap();
    module
        .register_async_method("say_hello_async", |_, _| async move { RpcResult::Ok("lo") })
        .unwrap();
    module
        .register_method("add", |params, _| {
            let params: Vec<u64> = params.parse()?;
            let sum: u64 = params.into_iter().sum();
            RpcResult::Ok(sum)
        })
        .unwrap();
    module
        .register_method::<Result<String, ErrorObjectOwned>, _>("multiparam", |params, _| {
            let params: (String, String, Vec<u8>) = params.parse()?;
            let r = format!(
                "string1={}, string2={}, vec={}",
                params.0.len(),
                params.1.len(),
                params.2.len()
            );
            Ok(r)
        })
        .unwrap();
    module.register_method("notif", |_, _| "").unwrap();
    module
        .register_method("should_err", |_, ctx| {
            ctx.err()?;
            Ok::<_, MyAppError>("err")
        })
        .unwrap();

    module
        .register_method("should_ok", |_, ctx| {
            ctx.ok()?;
            Ok::<_, MyAppError>("ok")
        })
        .unwrap();
    module
        .register_async_method("should_ok_async", |_p, ctx| async move {
            ctx.ok()?;
            Ok::<_, MyAppError>("ok")
        })
        .unwrap();

    let server_handle = server.start(module);
    (addr, server_handle)
}

#[tokio::test]
async fn single_method_call_works() {
    init_logger();
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    for i in 0..10 {
        let req = format!(r#"{{"jsonrpc":"2.0","method":"say_hello","id":{i}}}"#);
        let response =
            http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, ok_response(JsonValue::String("lo".to_owned()), Id::Num(i)));
    }
}

#[tokio::test]
async fn async_method_call_works() {
    init_logger();
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    for i in 0..10 {
        let req = format!(r#"{{"jsonrpc":"2.0","method":"say_hello_async","id":{i}}}"#);
        let response = http_request(req.into(), uri.clone()).await.unwrap();
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, ok_response(JsonValue::String("lo".to_owned()), Id::Num(i)));
    }
}

#[tokio::test]
async fn invalid_single_method_call() {
    init_logger();
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":1, "params": "bar"}"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, invalid_request(Id::Null));
}

#[tokio::test]
async fn single_method_call_with_params() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"add", "params":[1, 2],"id":1}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, ok_response(JsonValue::Number(3.into()), Id::Num(1)));
}

#[tokio::test]
async fn single_method_call_with_multiple_params_of_different_types() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req =
        r#"{"jsonrpc":"2.0","method":"multiparam", "params":["Hello", "World", [0,1,2,3]],"id":1}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.body,
        ok_response(JsonValue::String("string1=5, string2=5, vec=4".into()), Id::Num(1))
    );
}

#[tokio::test]
async fn single_method_call_with_faulty_params_returns_err() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);
    let expected = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"Invalid params","data":"invalid type: string \"this should be a number\", expected u64 at line 1 column 26"}}"#;

    let req = r#"{"jsonrpc":"2.0","method":"add", "params":["this should be a number"],"id":1}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, expected);
}

#[tokio::test]
async fn single_method_call_with_faulty_context() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"should_err","params":[],"id":1}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, call_execution_failed("MyAppError", Id::Num(1)));
}

#[tokio::test]
async fn single_method_call_with_ok_context() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"should_ok", "params":[],"id":1}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, ok_response("ok".into(), Id::Num(1)));
}

#[tokio::test]
async fn async_method_call_with_ok_context() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"should_ok_async", "params":[],"id":1}"#;
    let response = http_request(req.into(), uri).await.unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, ok_response("ok".into(), Id::Num(1)));
}

#[tokio::test]
async fn valid_batched_method_calls() {
    init_logger();

    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"[
        {"jsonrpc":"2.0","method":"add", "params":[1, 2],"id":1},
        {"jsonrpc":"2.0","method":"add", "params":[3, 4],"id":2},
        {"jsonrpc":"2.0","method":"say_hello","id":3},
        {"jsonrpc":"2.0","method":"add", "params":[5, 6],"id":4}
    ]"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.body,
        r#"[{"jsonrpc":"2.0","id":1,"result":3},{"jsonrpc":"2.0","id":2,"result":7},{"jsonrpc":"2.0","id":3,"result":"lo"},{"jsonrpc":"2.0","id":4,"result":11}]"#
    );
}

#[tokio::test]
async fn batched_notifications() {
    init_logger();

    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"[{"jsonrpc": "2.0", "method": "notif", "params": [1,2,4]},{"jsonrpc": "2.0", "method": "notif", "params": [7]}]"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    // Note: on HTTP acknowledge the notification with an empty response.
    assert_eq!(response.body, "null");
}

#[tokio::test]
async fn invalid_batch_calls() {
    init_logger();

    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    // batch with no requests
    let req = r#"[]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, invalid_request(Id::Null));

    // batch with invalid request
    let req = r#"[123]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, invalid_batch(vec![Id::Null]));

    // batch with invalid request
    let req = r#"[1, 2, 3]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, invalid_batch(vec![Id::Null, Id::Null, Id::Null]));

    // invalid JSON in batch
    let req = r#"[
        {"jsonrpc": "2.0", "method": "sum", "params": [1,2,4], "id": "1"},
        {"jsonrpc": "2.0", "method"
      ]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, parse_error(Id::Null));
}

#[tokio::test]
async fn batch_with_mixed_calls() {
    init_logger();

    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);
    // mixed notifications, method calls and valid json should be valid.
    let req = r#"[
            {"jsonrpc": "2.0", "method": "add", "params": [1,2,4], "id": "1"},
            {"jsonrpc": "2.0", "method": "add", "params": [7]},
            {"foo": "boo"},
            {"jsonrpc": "2.0", "method": "foo.get", "params": {"name": "myself"}, "id": "5"}
        ]"#;
    let res = r#"[{"jsonrpc":"2.0","id":"1","result":7},{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Invalid request"}},{"jsonrpc":"2.0","id":"5","error":{"code":-32601,"message":"Method not found"}}]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, res);
}

#[tokio::test]
async fn batch_notif_without_params_works() {
    init_logger();

    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);
    // mixed notifications, method calls and valid json should be valid.
    let req = r#"[
            {"jsonrpc": "2.0", "method": "add", "params": [1,2,4], "id": "1"},
            {"jsonrpc": "2.0", "method": "add"}
        ]"#;
    let res = r#"[{"jsonrpc":"2.0","id":"1","result":7}]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, res);
}

#[tokio::test]
async fn garbage_request_fails() {
    let (addr, _handle) = server().await;
    let uri = to_http_uri(addr);

    let req = r#"dsdfs fsdsfds"#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.body, parse_error(Id::Null));

    let req = r#"{ "#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.body, parse_error(Id::Null));

    let req = r#"{}"#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.body, invalid_request(Id::Null));

    let req = r#"{sds}"#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.body, parse_error(Id::Null));

    let req = r#"["#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.body, parse_error(Id::Null));

    let req = r#"[dsds]"#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.body, parse_error(Id::Null));

    let req = r#"[]"#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.body, invalid_request(Id::Null));

    let req = r#"[{"jsonrpc":"2.0","method":"add", "params":[1, 2],"id":1}"#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.body, parse_error(Id::Null));
}

#[tokio::test]
async fn whitespace_is_not_significant() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"         {"jsonrpc":"2.0","method":"add", "params":[1, 2],"id":1}"#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    let expected = r#"{"jsonrpc":"2.0","id":1,"result":3}"#;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, expected);

    let req = r#" [{"jsonrpc":"2.0","method":"add", "params":[1, 2],"id":1}]"#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    let expected = r#"[{"jsonrpc":"2.0","id":1,"result":3}]"#;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, expected);

    // Up to 127 whitespace chars are accepted.
    let req = format!(
        "{}{}",
        " ".repeat(127),
        r#"{"jsonrpc":"2.0","method":"add", "params":[1, 2],"id":1}"#
    );
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    let expected = r#"{"jsonrpc":"2.0","id":1,"result":3}"#;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, expected);

    // More than 127 whitespace chars are not accepted.
    let req = format!(
        "{}{}",
        " ".repeat(128),
        r#"{"jsonrpc":"2.0","method":"add", "params":[1, 2],"id":1}"#
    );
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.body, parse_error(Id::Null));
}

#[tokio::test]
async fn should_return_method_not_found() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"bar","id":"foo"}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, method_not_found(Id::Str("foo".into())));
}

#[tokio::test]
async fn invalid_json_id_missing_value() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"say_hello","id"}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    // If there was an error in detecting the id in the Request object (e.g. Parse error/Invalid
    // Request), it MUST be Null.
    assert_eq!(response.body, parse_error(Id::Null));
}

#[tokio::test]
async fn shared_server_behavior_http_and_websocket_invalid_envelopes() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);
    for req in [r#"{"jsonrpc":"1.0","method":"say_hello"}"#, r#"{"jsonrpc":"2.0"}"#, r#"{}"#] {
        let response =
            http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, invalid_request(Id::Null));
        let response =
            http_request(r#"{"jsonrpc":"2.0","method":"say_hello","id":1}"#.into(), uri.clone())
                .with_default_timeout()
                .await
                .unwrap()
                .unwrap();
        assert_eq!(response.body, ok_response(JsonValue::String("lo".to_owned()), Id::Num(1)));
    }
}

#[tokio::test]
async fn invalid_request_object() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"method":"bar","id":1}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, invalid_request(Id::Num(1)));
}

#[tokio::test]
async fn unknown_field_is_ok() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"say_hello","id":1,"is_not_request_object":1}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, ok_response(JsonValue::String("lo".to_owned()), Id::Num(1)));
}

#[tokio::test]
async fn notif_works() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"bar"}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, "null");
}

#[tokio::test]
async fn can_register_modules() {
    let cx = String::new();
    let mut mod1 = RpcModule::new(cx);

    let cx2 = Vec::<u8>::new();
    let mut mod2 = RpcModule::new(cx2);

    assert_eq!(mod1.method_names().count(), 0);
    mod1.register_method("bla", |_, cx| format!("Gave me {cx}")).unwrap();
    mod1.register_method("bla2", |_, cx| format!("Gave me {cx}")).unwrap();
    mod2.register_method("yada", |_, cx| format!("Gave me {cx:?}")).unwrap();

    // Won't register, name clashes
    mod2.register_method("bla", |_, cx| format!("Gave me {cx:?}")).unwrap();

    assert_eq!(mod1.method_names().count(), 2);

    let err = mod1.merge(mod2).unwrap_err();

    let expected_err = RegisterMethodError::AlreadyRegistered(String::from("bla"));
    assert_eq!(err.to_string(), expected_err.to_string());
    assert_eq!(mod1.method_names().count(), 2);
}

#[tokio::test]
async fn can_set_the_max_request_body_size() {
    let addr = "127.0.0.1:0";
    // Rejects all requests larger than 100 bytes
    let config = ServerConfig::builder().max_request_body_size(100).build();
    let server = ServerBuilder::with_config(config).build(addr).await.unwrap();
    let mut module = RpcModule::new(());
    module.register_method("anything", |_p, _cx| "a".repeat(100)).unwrap();
    let addr = server.local_addr().unwrap();
    let uri = to_http_uri(addr);
    let handle = server.start(module);

    // Invalid: too long
    let req = format!(r#"{{"jsonrpc":"2.0", "method":{}, "id":1}}"#, "a".repeat(100));
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.body, oversized_request(100));

    // Max request body size should not override the max response size
    let req = r#"{"jsonrpc":"2.0", "method":"anything", "id":1}"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.body, ok_response(JsonValue::String("a".repeat(100)), Id::Num(1)));

    handle.stop().unwrap();
    handle.stopped().await;
}

#[tokio::test]
async fn can_set_the_max_response_size() {
    let addr = "127.0.0.1:0";
    // Set the max response size to 100 bytes
    let config = ServerConfig::builder().max_response_body_size(100).build();
    let server = ServerBuilder::with_config(config).build(addr).await.unwrap();
    let mut module = RpcModule::new(());
    module.register_method("anything", |_p, _cx| "a".repeat(101)).unwrap();
    let addr = server.local_addr().unwrap();
    let uri = to_http_uri(addr);
    let handle = server.start(module);

    // Oversized response.
    let req = r#"{"jsonrpc":"2.0", "method":"anything", "id":1}"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.body, oversized_response(Id::Num(1), 100));

    handle.stop().unwrap();
    handle.stopped().await;
}

#[tokio::test]
async fn can_set_the_max_response_size_to_batch() {
    let addr = "127.0.0.1:0";
    // Set the max response size to 100 bytes
    let config = ServerConfig::builder().max_response_body_size(100).build();
    let server = ServerBuilder::with_config(config).build(addr).await.unwrap();
    let mut module = RpcModule::new(());
    module.register_method("anything", |_p, _cx| "a".repeat(51)).unwrap();
    let addr = server.local_addr().unwrap();
    let uri = to_http_uri(addr);
    let handle = server.start(module);

    // Two response will end up in a response of 102 bytes which is too big.
    let req = r#"[{"jsonrpc":"2.0", "method":"anything", "id":1},{"jsonrpc":"2.0", "method":"anything", "id":2}]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.body, batch_response_too_large(100));

    handle.stop().unwrap();
    handle.stopped().await;
}

#[tokio::test]
async fn disabled_batches() {
    let addr = "127.0.0.1:0";
    // Disable batches support.
    let config =
        ServerConfig::builder().set_batch_request_config(BatchRequestConfig::Disabled).build();
    let server = ServerBuilder::with_config(config).build(addr).await.unwrap();
    let mut module = RpcModule::new(());
    module.register_method("should_ok", |_, _ctx| "ok").unwrap();
    let addr = server.local_addr().unwrap();
    let uri = to_http_uri(addr);
    let handle = server.start(module);

    // Send a valid batch.
    let req = r#"[
        {"jsonrpc":"2.0","method":"should_ok", "params":[],"id":1},
        {"jsonrpc":"2.0","method":"should_ok", "params":[],"id":2}
    ]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.body, batches_not_supported());

    handle.stop().unwrap();
    handle.stopped().await;
}

#[tokio::test]
async fn batch_limit_works() {
    let addr = "127.0.0.1:0";
    // Disable batches support.
    let config =
        ServerConfig::builder().set_batch_request_config(BatchRequestConfig::Limit(1)).build();
    let server = ServerBuilder::with_config(config).build(addr).await.unwrap();
    let mut module = RpcModule::new(());
    module.register_method("should_ok", |_, _ctx| "ok").unwrap();
    let addr = server.local_addr().unwrap();
    let uri = to_http_uri(addr);
    let handle = server.start(module);

    // Send a valid batch.
    let req = r#"[
        {"jsonrpc":"2.0","method":"should_ok", "params":[],"id":1},
        {"jsonrpc":"2.0","method":"should_ok", "params":[],"id":2}
    ]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.body, batches_too_large(1));

    handle.stop().unwrap();
    handle.stopped().await;
}

#[tokio::test]
async fn http2_method_call_works() {
    init_logger();

    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"add", "params":[1, 2],"id":1}"#;
    let response = http2_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, ok_response(JsonValue::Number(3.into()), Id::Num(1)));
}

#[tokio::test]
async fn notification_id_classification_http_singles_and_mixed_batches() {
    use serde_json::json;
    let (addr, handle, calls) = super::helpers::server_with_call_counter().await;
    let uri = to_http_uri(addr);
    let error: JsonValue = serde_json::from_str(&invalid_request(Id::Null)).unwrap();
    for id in [json!(true), json!(false), json!({}), json!([])] {
        let invalid = json!({"jsonrpc":"2.0","method":"tick","id":id});
        for batch in [false, true] {
            let before = calls.load(std::sync::atomic::Ordering::SeqCst);
            let request = if batch {
                json!([invalid, {"jsonrpc":"2.0","method":"tick","id":"okay"}, {"jsonrpc":"2.0","method":"tick","id":null}])
            } else {
                invalid.clone()
            };
            let response = http_request(request.to_string().into(), uri.clone())
                .with_default_timeout()
                .await
                .unwrap()
                .unwrap();
            assert_eq!(response.status, StatusCode::OK);
            let reply: JsonValue = serde_json::from_str(&response.body).unwrap();
            let expected = if batch {
                json!([error, {"jsonrpc":"2.0","id":"okay","result":before}, {"jsonrpc":"2.0","id":null,"result":before + 1}])
            } else {
                error.clone()
            };
            assert_eq!(reply, expected, "invalid_id_error_missing");
            let after = before + if batch { 2 } else { 0 };
            assert_eq!(
                calls.load(std::sync::atomic::Ordering::SeqCst),
                after,
                "invalid ID dispatched"
            );
            let request = json!({"jsonrpc":"2.0","method":"tick","id":"recovery"});
            let response = http_request(request.to_string().into(), uri.clone())
                .with_default_timeout()
                .await
                .unwrap()
                .unwrap();
            assert_eq!(response.status, StatusCode::OK);
            let reply: JsonValue = serde_json::from_str(&response.body).unwrap();
            assert_eq!(reply, json!({"jsonrpc":"2.0","id":"recovery","result":after}));
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), after + 1);
        }
    }
    for id in [json!(0), json!("valid"), JsonValue::Null] {
        let before = calls.load(std::sync::atomic::Ordering::SeqCst);
        let request = json!({"jsonrpc":"2.0","method":"tick","id":id});
        let response = http_request(request.to_string().into(), uri.clone())
            .with_default_timeout()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status, StatusCode::OK);
        let reply: JsonValue = serde_json::from_str(&response.body).unwrap();
        assert_eq!(reply, json!({"jsonrpc":"2.0","id":id,"result":before}));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), before + 1);
    }
    let request = json!({"jsonrpc":"2.0","method":"tick","extra":{"id":true}});
    let response = http_request(request.to_string().into(), uri.clone())
        .with_default_timeout()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, "null");
    let request = json!({"jsonrpc":"2.0","method":"tick","id":"after-notification"});
    let response = http_request(request.to_string().into(), uri)
        .with_default_timeout()
        .await
        .unwrap()
        .unwrap();
    let reply: JsonValue = serde_json::from_str(&response.body).unwrap();
    assert_eq!(reply["id"], "after-notification");
    assert!(reply.get("result").is_some());
    assert!(reply.get("error").is_none());
    handle.stop().unwrap();
    handle.stopped().with_default_timeout().await.unwrap();
}

#[tokio::test]
async fn panicking_call_returns_internal_error() {
    init_logger();

    let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
    let mut module = RpcModule::new(());
    module
        .register_async_method("maybe_panic", |params, _| async move {
            assert!(!params.one::<bool>().unwrap(), "requested panic");
            "ok"
        })
        .unwrap();
    module
        .register_method("maybe_panic_sync", |params, _| {
            assert!(!params.one::<bool>().unwrap(), "requested panic");
            "ok"
        })
        .unwrap();
    let uri = to_http_uri(server.local_addr().unwrap());
    let handle = server.start(module);

    for method in ["maybe_panic", "maybe_panic_sync"] {
        let req = format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":[true],"id":1}}"#);
        let response =
            http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, internal_error(Id::Num(1)));

        let req = format!(
            r#"[
                {{"jsonrpc":"2.0","method":"{method}","params":[true],"id":1}},
                {{"jsonrpc":"2.0","method":"{method}","params":[false],"id":2}}
            ]"#
        );
        let response =
            http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(
            response.body,
            format!("[{},{}]", internal_error(Id::Num(1)), ok_response("ok".into(), Id::Num(2)))
        );
    }

    handle.stop().unwrap();
    handle.stopped().await;
}

#[tokio::test]
async fn batch_array_entries_are_invalid_requests() {
    init_logger();

    let (addr, handle, calls) = super::helpers::server_with_call_counter().await;
    let uri = to_http_uri(addr);
    let mixed_response = r#"[{"jsonrpc":"2.0","id":1,"result":0},{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Invalid request"}}]"#;

    for (request, expected) in [
        (r#"[["2.0",1,"tick",[]]]"#, invalid_batch(vec![Id::Null])),
        (r#"[[7]]"#, invalid_batch(vec![Id::Null])),
        (
            r#"[{"jsonrpc":"2.0","method":"tick","id":1},["2.0",2,"tick",[]]]"#,
            mixed_response.to_string(),
        ),
    ] {
        let response = http_request(request.into(), uri.clone())
            .with_default_timeout()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, expected, "{request}");
    }

    // Only the object entry of the mixed batch ran.
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

    handle.stop().unwrap();
    handle.stopped().with_default_timeout().await.unwrap();
}

#[tokio::test]
async fn batch_limit_applies_while_reading() {
    init_logger();

    let config =
        ServerConfig::builder().set_batch_request_config(BatchRequestConfig::Limit(2)).build();
    let server = ServerBuilder::with_config(config).build("127.0.0.1:0").await.unwrap();
    let mut module = RpcModule::new(());
    module.register_method("should_ok", |_, _ctx| "ok").unwrap();
    let uri = to_http_uri(server.local_addr().unwrap());
    let handle = server.start(module);

    let call = r#"{"jsonrpc":"2.0","method":"should_ok","params":[],"id":1}"#;
    let ok = r#"{"jsonrpc":"2.0","id":1,"result":"ok"}"#;
    for (request, expected) in [
        (format!("[{call},{call}]"), format!("[{ok},{ok}]")),
        (format!("[{call},{call},{call}]"), batches_too_large(2)),
        // The entries after the limit are never parsed.
        (format!(r#"[{call},{call},{call},{{"jsonrpc""#), batches_too_large(2)),
        // Within the limit, invalid JSON is still a parse error.
        (format!(r#"[{call},{{"jsonrpc""#), parse_error(Id::Null)),
    ] {
        let response = http_request(request.clone().into(), uri.clone())
            .with_default_timeout()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, expected, "{request}");
    }

    handle.stop().unwrap();
    handle.stopped().await;
}

const BIG_RESPONSE_LEN: usize = 32 * 1024 * 1024;

const WRITE_TIMEOUT: Duration = Duration::from_secs(1);

const BIG_REQUEST: &str = r#"{"jsonrpc":"2.0","method":"big","id":1}"#;

/// Starts a server that allows a single connection. Its `big` method returns a response too large
/// for the socket buffers and notifies the returned `Notify` when it is called.
async fn server_with_big_response() -> (SocketAddr, ServerHandle, Arc<Notify>) {
    let config = ServerConfig::builder()
        .max_connections(1)
        .max_response_body_size(BIG_RESPONSE_LEN as u32 + 1024)
        .set_write_timeout(WRITE_TIMEOUT)
        .build();
    let server = ServerBuilder::with_config(config).build("127.0.0.1:0").await.unwrap();
    let called = Arc::new(Notify::new());
    let mut module = RpcModule::new(called.clone());
    module
        .register_method("big", |_, called| {
            called.notify_one();
            "x".repeat(BIG_RESPONSE_LEN)
        })
        .unwrap();
    module.register_method("say_hello", |_, _| "lo").unwrap();
    let addr = server.local_addr().unwrap();
    (addr, server.start(module), called)
}

/// Sends the `big` request on a new connection, without reading the response.
async fn send_big_request(addr: SocketAddr) -> TcpStream {
    let mut client = TcpStream::connect(addr).with_default_timeout().await.unwrap().unwrap();
    let request = format!(
        "POST / HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\n\
         connection: close\r\ncontent-length: {}\r\n\r\n{BIG_REQUEST}",
        BIG_REQUEST.len()
    );
    client.write_all(request.as_bytes()).with_default_timeout().await.unwrap().unwrap();
    client
}

#[tokio::test]
async fn http_permit_is_held_until_response_is_written() {
    init_logger();

    let (addr, handle, called) = server_with_big_response().await;
    let uri = to_http_uri(addr);
    let hello = r#"{"jsonrpc":"2.0","method":"say_hello","id":2}"#;

    // The client never reads, so the response stays buffered on the server.
    let _stalled = send_big_request(addr).await;
    called.notified().with_default_timeout().await.unwrap();

    let response =
        http_request(hello.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);

    // The write timeout closes the stalled connection, which releases its permit.
    let response = async {
        loop {
            let response = http_request(hello.into(), uri.clone()).await.unwrap();
            if response.status == StatusCode::OK {
                break response;
            }
            assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    .with_timeout(WRITE_TIMEOUT * 10)
    .await
    .unwrap();
    assert_eq!(response.body, ok_response("lo".to_string().into(), Id::Num(2)));

    handle.stop().unwrap();
    handle.stopped().with_default_timeout().await.unwrap();
}

#[tokio::test]
async fn http_response_is_delivered_to_slow_reader() {
    init_logger();

    let (addr, handle, _called) = server_with_big_response().await;
    let mut client = send_big_request(addr).await;

    // Reading pauses for much less than the write timeout, but reading the whole response takes
    // longer than it.
    let mut response = Vec::new();
    let mut chunk = vec![0; 256 * 1024];
    loop {
        let read = client.read(&mut chunk).with_default_timeout().await.unwrap().unwrap();
        if read == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..read]);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let body_start = response.windows(4).position(|window| window == b"\r\n\r\n").unwrap() + 4;
    assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
    let expected =
        format!(r#"{{"jsonrpc":"2.0","id":1,"result":"{}"}}"#, "x".repeat(BIG_RESPONSE_LEN));
    assert!(response[body_start..] == *expected.as_bytes(), "response body differs");

    handle.stop().unwrap();
    handle.stopped().with_default_timeout().await.unwrap();
}

#[tokio::test]
async fn only_json_whitespace_is_skipped_before_request() {
    init_logger();

    let (addr, handle, calls) = super::helpers::server_with_call_counter().await;
    let uri = to_http_uri(addr);
    let request = r#"{"jsonrpc":"2.0","method":"tick","id":1}"#;

    // Form feed is not JSON whitespace.
    let response = http_request(format!("\x0c{request}").into(), uri.clone())
        .with_default_timeout()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.body, parse_error(Id::Null));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);

    let response = http_request(format!(" \t\r\n{request}").into(), uri)
        .with_default_timeout()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, r#"{"jsonrpc":"2.0","id":1,"result":0}"#);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

    handle.stop().unwrap();
    handle.stopped().with_default_timeout().await.unwrap();
}

#[tokio::test]
async fn rpc_middleware_is_built_once_per_connection() {
    init_logger();

    let built = Arc::new(AtomicUsize::new(0));
    let rpc_middleware = RpcServiceBuilder::new().layer_fn({
        let built = built.clone();
        move |service| {
            built.fetch_add(1, Ordering::SeqCst);
            service
        }
    });
    let server = ServerBuilder::default()
        .set_rpc_middleware(rpc_middleware)
        .build("127.0.0.1:0")
        .await
        .unwrap();
    let mut module = RpcModule::new(());
    module.register_method("say_hello", |_, _| "lo").unwrap();
    let addr = server.local_addr().unwrap();
    let handle = server.start(module);

    let request = r#"{"jsonrpc":"2.0","method":"say_hello","id":1}"#;
    let post = |connection: &str| {
        format!(
            "POST / HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\n\
             connection: {connection}\r\ncontent-length: {}\r\n\r\n{request}",
            request.len()
        )
    };
    for connections in 1..=2 {
        let mut client = TcpStream::connect(addr).with_default_timeout().await.unwrap().unwrap();
        let requests = format!("{}{}", post("keep-alive"), post("close"));
        client.write_all(requests.as_bytes()).with_default_timeout().await.unwrap().unwrap();
        let mut responses = String::new();
        client.read_to_string(&mut responses).with_default_timeout().await.unwrap().unwrap();
        assert_eq!(responses.matches("HTTP/1.1 200 OK").count(), 2);
        assert_eq!(built.load(Ordering::SeqCst), connections);
    }

    handle.stop().unwrap();
    handle.stopped().with_default_timeout().await.unwrap();
}

/// Starts a server with `stream_parts`, which sends `[1`, waits for `release`, and then sends
/// `,2]`, and `stream_fails`, which sends `[1` and then fails.
async fn streaming_server(release: Arc<Notify>) -> (SocketAddr, ServerHandle) {
    let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
    let addr = server.local_addr().unwrap();
    let mut module = RpcModule::new(release);
    module
        .register_streaming_method("stream_parts", |_, release| async move {
            RpcResult::Ok(ResponseStream::new(move |sink| async move {
                sink.send("[1".into()).map_err(|_| ErrorCode::InternalError)?;
                release.notified().await;
                sink.send(",2]".into()).map_err(|_| ErrorCode::InternalError)
            }))
        })
        .unwrap();
    module
        .register_streaming_method("stream_fails", |_, _| async move {
            RpcResult::Ok(ResponseStream::new(move |sink| async move {
                sink.send("[1".into()).map_err(|_| ErrorCode::InternalError)?;
                Err(ErrorCode::InternalError)
            }))
        })
        .unwrap();
    (addr, server.start(module))
}

/// Sends a call to `method` on a raw connection, so the test sees the chunked framing.
async fn raw_call(addr: SocketAddr, method: &str) -> TcpStream {
    let mut client = TcpStream::connect(addr).with_default_timeout().await.unwrap().unwrap();
    let body = format!(r#"{{"jsonrpc":"2.0","method":"{method}","id":1}}"#);
    let request = format!(
        "POST / HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\n\
         connection: close\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    client.write_all(request.as_bytes()).with_default_timeout().await.unwrap().unwrap();
    client
}

/// Reads from `client` until the bytes read so far contain `needle`.
async fn read_until(client: &mut TcpStream, response: &mut Vec<u8>, needle: &[u8]) {
    while !response.windows(needle.len()).any(|window| window == needle) {
        let mut buf = [0; 1024];
        let n = client.read(&mut buf).with_default_timeout().await.unwrap().unwrap();
        assert_ne!(n, 0, "connection closed before {:?}", String::from_utf8_lossy(needle));
        response.extend_from_slice(&buf[..n]);
    }
}

#[tokio::test]
async fn http_streaming_method_sends_parts_as_produced() {
    init_logger();
    let release = Arc::new(Notify::new());
    let (addr, handle) = streaming_server(release.clone()).await;

    let mut client = raw_call(addr, "stream_parts").await;
    let mut response = Vec::new();
    // The first part arrives while the producer still waits.
    read_until(&mut client, &mut response, br#"{"jsonrpc":"2.0","id":1,"result":[1"#).await;
    let head = String::from_utf8(response.clone()).unwrap();
    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(head.contains("transfer-encoding: chunked\r\n"), "{head}");

    release.notify_one();
    client.read_to_end(&mut response).with_default_timeout().await.unwrap().unwrap();
    let body_start = response.windows(4).position(|window| window == b"\r\n\r\n").unwrap() + 4;
    let body = String::from_utf8(response[body_start..].to_vec()).unwrap();
    let chunks: Vec<_> = body.split("\r\n").collect();
    // Chunks alternate between their size and their data, and the body ends with an empty chunk.
    let data: String = chunks.iter().skip(1).step_by(2).copied().collect();
    assert_eq!(data, r#"{"jsonrpc":"2.0","id":1,"result":[1,2]}"#);
    assert!(body.ends_with("\r\n0\r\n\r\n"), "{body}");

    handle.stop().unwrap();
    handle.stopped().with_default_timeout().await.unwrap();
}

#[tokio::test]
async fn http_streaming_method_aborts_after_failure() {
    init_logger();
    let (addr, handle) = streaming_server(Arc::new(Notify::new())).await;

    let mut client = raw_call(addr, "stream_fails").await;
    let mut response = Vec::new();
    // hyper closes the connection without the empty chunk that ends a complete body. It may not
    // flush the parts before that either, and a reset is a valid way to close it too.
    let _ = client.read_to_end(&mut response).with_default_timeout().await.unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(!response.ends_with("0\r\n\r\n"), "{response}");

    handle.stop().unwrap();
    handle.stopped().with_default_timeout().await.unwrap();
}
