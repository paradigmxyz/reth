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
    io::{BufReader, Read, Seek, SeekFrom, Write},
    sync::Mutex,
};
use url::Url;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

#[derive(Debug)]
pub(crate) struct FileSpanExporter {
    file: Mutex<Option<File>>,
    resource: ResourceAttributesWithSchema,
}

impl FileSpanExporter {
    pub(crate) fn new(endpoint: &Url) -> eyre::Result<Self> {
        let path = trace_file_path(endpoint)?;
        let mut options = OpenOptions::new();
        options.create(true).read(true).append(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options.open(&path).map_err(|err| FsPathError::open(err, &path))?;
        // Readers can acquire advisory locks too, so reject shared existing files.
        #[cfg(unix)]
        eyre::ensure!(
            file.metadata()?.permissions().mode() & 0o077 == 0,
            "Trace file {path:?} must be owner-only; restrict its permissions to 0600"
        );
        // Keep the lock for the exporter's lifetime so partial writes cannot interleave.
        file.try_lock()
            .wrap_err_with(|| format!("Could not exclusively lock trace file {path:?}"))?;
        // Never append after an incomplete record left by an interrupted writer.
        // Inspect framing without allocating or decoding entire trace batches.
        let length = file.metadata()?.len();
        let mut reader = BufReader::new(&file);
        while reader.stream_position()? < length {
            let mut delimiter = [0u8; 10];
            let mut used = 0;
            for byte in &mut delimiter {
                reader
                    .read_exact(std::slice::from_mut(byte))
                    .wrap_err("Incomplete OTLP file length delimiter")?;
                used += 1;
                if *byte & 0x80 == 0 {
                    break;
                }
            }
            let payload = prost::decode_length_delimiter(&delimiter[..used])?;
            let end = reader
                .stream_position()?
                .checked_add(u64::try_from(payload)?)
                .filter(|end| *end <= length)
                .ok_or_else(|| eyre::eyre!("Incomplete OTLP file batch"))?;
            reader.seek(SeekFrom::Start(end))?;
        }
        Ok(Self {
            file: Mutex::new(Some(file)),
            resource: (&Resource::builder_empty().build()).into(),
        })
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
        let writer = file.as_mut().ok_or_else(|| {
            OTelSdkError::InternalFailure("Trace file disabled after a write failure".into())
        })?;
        let result = writer.write_all(&bytes).and_then(|()| writer.flush());
        if result.is_err() {
            // A failed write may have committed a partial record. Do not append
            // another batch after it, including on a later SDK retry.
            file.take();
        }
        result.map_err(|err| OTelSdkError::InternalFailure(err.to_string()))
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.resource = resource.into();
    }
}
