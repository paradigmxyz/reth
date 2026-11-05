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

use std::net::SocketAddr;

use crate::mocks::{HttpResponse, Id, Uri};
use http_body_util::BodyExt;
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use serde::Serialize;
use serde_json::Value;

pub type Body = http_body_util::Full<hyper::body::Bytes>;

pub const PARSE_ERROR: &str = "Parse error";
pub const INTERNAL_ERROR: &str = "Internal error";
pub const INVALID_PARAMS: &str = "Invalid params";
pub const INVALID_REQUEST: &str = "Invalid request";
pub const METHOD_NOT_FOUND: &str = "Method not found";

/// Converts a sockaddress to a HTTP URI.
pub fn to_http_uri(sockaddr: SocketAddr) -> Uri {
    let s = sockaddr.to_string();
    Uri::builder().scheme("http").authority(s.as_str()).path_and_query("/").build().unwrap()
}

pub fn ok_response(result: Value, id: Id) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"result":{}}}"#,
        serde_json::to_string(&id).unwrap(),
        result
    )
}

pub fn method_not_found(id: Id) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":-32601,"message":"Method not found"}}}}"#,
        serde_json::to_string(&id).unwrap()
    )
}

pub fn parse_error(id: Id) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":-32700,"message":"Parse error"}}}}"#,
        serde_json::to_string(&id).unwrap()
    )
}

pub fn oversized_request(max_limit: u32) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32007,"message":"Request is too big","data":"Exceeded max limit of {max_limit}"}}}}"#
    )
}

pub fn batches_not_supported() -> String {
    r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32005,"message":"Batched requests are not supported by this server"}}"#.into()
}

pub fn batches_too_large(max_limit: usize) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32010,"message":"The batch request was too large","data":"Exceeded max limit of {max_limit}"}}}}"#
    )
}

pub fn batch_response_too_large(max_limit: usize) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32011,"message":"The batch response was too large","data":"Exceeded max limit of {max_limit}"}}}}"#
    )
}

pub fn oversized_response(id: Id, max_limit: u32) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":-32008,"message":"Response is too big","data":"Exceeded max limit of {}"}}}}"#,
        serde_json::to_string(&id).unwrap(),
        max_limit,
    )
}

pub fn invalid_request(id: Id) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":-32600,"message":"Invalid request"}}}}"#,
        serde_json::to_string(&id).unwrap()
    )
}

pub fn invalid_batch(ids: Vec<Id>) -> String {
    use std::fmt::Write;
    let mut result = String::new();
    result.push('[');
    for (i, id) in ids.iter().enumerate() {
        write!(
            result,
            r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":-32600,"message":"Invalid request"}}}}{}"#,
            serde_json::to_string(&id).unwrap(),
            if i + 1 == ids.len() { "" } else { "," }
        )
        .unwrap();
    }
    result.push(']');
    result
}

pub fn invalid_params(id: Id) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":-32602,"message":"Invalid params"}}}}"#,
        serde_json::to_string(&id).unwrap()
    )
}

pub fn call<T: Serialize>(method: &str, params: Vec<T>, id: Id) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","method":{},"params":{},"id":{}}}"#,
        serde_json::to_string(method).unwrap(),
        serde_json::to_string(&params).unwrap(),
        serde_json::to_string(&id).unwrap()
    )
}

pub fn call_execution_failed(msg: &str, id: Id) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":-32000,"message":"{}"}}}}"#,
        serde_json::to_string(&id).unwrap(),
        msg,
    )
}

pub fn internal_error(id: Id) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":-32603,"message":"Internal error"}}}}"#,
        serde_json::to_string(&id).unwrap()
    )
}

pub async fn http_request(body: Body, uri: Uri) -> Result<HttpResponse, String> {
    let client = hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build_http();
    http_post(client, body, uri).await
}

pub async fn http2_request(body: Body, uri: Uri) -> Result<HttpResponse, String> {
    let client = hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build_http();
    http_post(client, body, uri).await
}

async fn http_post<C>(client: Client<C, Body>, body: Body, uri: Uri) -> Result<HttpResponse, String>
where
    C: hyper_util::client::legacy::connect::Connect + Clone + Send + Sync + 'static,
{
    let r = hyper::Request::post(uri)
        .header(
            hyper::header::CONTENT_TYPE,
            hyper::header::HeaderValue::from_static("application/json"),
        )
        .body(body)
        .expect("uri and request headers are valid; qed");
    let res = client.request(r).await.map_err(|e| format!("{e:?}"))?;

    let (parts, mut body) = res.into_parts();
    let mut bytes = Vec::new();

    while let Some(frame) = body.frame().await {
        let data = frame.unwrap().into_data().unwrap();
        bytes.extend(data);
    }

    Ok(HttpResponse {
        status: parts.status,
        header: parts.headers,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
    })
}
