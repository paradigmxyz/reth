//! Local length-delimited OTLP trace batch exporter.

use crate::trace_file_path;
use eyre::WrapErr;
use opentelemetry_proto::{
    tonic::collector::trace::v1::ExportTraceServiceRequest,
    transform::{
        common::tonic::ResourceAttributesWithSchema,
        trace::tonic::group_spans_by_resource_and_scope,
    },
};
use opentelemetry_sdk::{
    error::{OTelSdkError, OTelSdkResult},
    trace::{SpanData, SpanExporter},
    Resource,
};
use prost::Message;
use reth_fs_util::FsPathError;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    sync::Mutex,
};
use url::Url;

#[derive(Debug)]
pub(crate) struct FileSpanExporter {
    file: Mutex<File>,
    resource: ResourceAttributesWithSchema,
}

impl FileSpanExporter {
    pub(crate) fn new(endpoint: &Url) -> eyre::Result<Self> {
        let path = trace_file_path(endpoint)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|err| FsPathError::open(err, &path))?;
        // Keep the lock for the exporter's lifetime so partial writes cannot interleave.
        file.try_lock()
            .wrap_err_with(|| format!("Could not exclusively lock trace file {path:?}"))?;
        Ok(Self { file: Mutex::new(file), resource: (&Resource::builder_empty().build()).into() })
    }
}

impl SpanExporter for FileSpanExporter {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        let request = ExportTraceServiceRequest {
            resource_spans: group_spans_by_resource_and_scope(batch, &self.resource),
        };
        let bytes = request.encode_length_delimited_to_vec();
        let mut file =
            self.file.lock().map_err(|err| OTelSdkError::InternalFailure(err.to_string()))?;
        file.write_all(&bytes)
            .and_then(|()| file.flush())
            .map_err(|err| OTelSdkError::InternalFailure(err.to_string()))
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.resource = resource.into();
    }
}
