use std::time::{Duration, SystemTime};

use opentelemetry::trace::{Span, Status, Tracer, TracerProvider as _};
use opentelemetry::{KeyValue, Value};
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{BatchConfigBuilder, BatchSpanProcessor, SdkTracerProvider};
use sha2::{Digest, Sha256};

use crate::config::RetrievalTelemetryConfig;
use crate::retrieval_history::{RetrievalCandidateInput, RetrievalRunCompletion};

const EXPORT_TIMEOUT: Duration = Duration::from_secs(3);
const EXPORT_QUEUE_SIZE: usize = 256;
const EXPORT_BATCH_SIZE: usize = 64;

pub(crate) struct RetrievalTelemetry {
    provider: SdkTracerProvider,
    capture_content: bool,
}

impl RetrievalTelemetry {
    pub(crate) fn from_config(config: &RetrievalTelemetryConfig) -> Option<Self> {
        let endpoint = config.endpoint.as_deref()?;
        match Self::build(endpoint, config.capture_content) {
            Ok(telemetry) => Some(telemetry),
            Err(error) => {
                tracing::warn!("retrieval telemetry is disabled: {error}");
                None
            }
        }
    }

    fn build(endpoint: &str, capture_content: bool) -> Result<Self, String> {
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(endpoint)
            .with_timeout(EXPORT_TIMEOUT)
            .build()
            .map_err(|error| error.to_string())?;
        let processor = BatchSpanProcessor::builder(exporter)
            .with_batch_config(
                BatchConfigBuilder::default()
                    .with_max_queue_size(EXPORT_QUEUE_SIZE)
                    .with_max_export_batch_size(EXPORT_BATCH_SIZE)
                    .build(),
            )
            .build();
        let provider = SdkTracerProvider::builder()
            .with_span_processor(processor)
            .with_resource(Resource::builder().with_service_name("clumsiesd").build())
            .build();
        Ok(Self {
            provider,
            capture_content,
        })
    }

    pub(crate) fn emit(
        &self,
        run_id: Option<&str>,
        project_id: &str,
        query: &str,
        completion: &RetrievalRunCompletion,
    ) {
        let ended_at = SystemTime::now();
        let started_at = ended_at
            .checked_sub(Duration::from_micros(completion.latencies.total_us))
            .unwrap_or(ended_at);
        let mut attributes =
            telemetry_attributes(run_id, project_id, query, completion, self.capture_content);
        let tracer = self.provider.tracer("clumsies.retrieval");
        let mut span = tracer
            .span_builder("clumsies.memory.retrieve")
            .with_start_time(started_at)
            .with_attributes(attributes.drain(..))
            .start(&tracer);
        if let Some(code) = completion.error_code.as_deref() {
            span.set_status(Status::error(code.to_owned()));
        } else {
            span.set_status(Status::Ok);
        }
        span.end_with_timestamp(ended_at);
    }

    #[cfg(test)]
    fn force_flush(&self) -> opentelemetry_sdk::error::OTelSdkResult {
        self.provider.force_flush()
    }
}

fn telemetry_attributes(
    run_id: Option<&str>,
    project_id: &str,
    query: &str,
    completion: &RetrievalRunCompletion,
    capture_content: bool,
) -> Vec<KeyValue> {
    let status = if completion.error_code.is_some() {
        "failed"
    } else {
        "succeeded"
    };
    let mut attributes = vec![
        KeyValue::new("openinference.span.kind", "RETRIEVER"),
        KeyValue::new("clumsies.project.id", project_id.to_owned()),
        KeyValue::new("clumsies.retrieval.status", status),
        KeyValue::new(
            "clumsies.retrieval.resource_count",
            integer(completion.resources.len()),
        ),
        KeyValue::new(
            "clumsies.retrieval.unit_count",
            integer(completion.unit_count),
        ),
        KeyValue::new(
            "clumsies.retrieval.returned_fragment_count",
            integer(completion.returned_fragment_count),
        ),
        KeyValue::new(
            "clumsies.retrieval.returned_token_count",
            integer(completion.returned_token_count),
        ),
    ];
    optional_string(&mut attributes, "clumsies.retrieval.run_id", run_id);
    optional_string(
        &mut attributes,
        "clumsies.retrieval.effective_hash",
        completion.effective_hash.as_deref(),
    );
    optional_string(
        &mut attributes,
        "clumsies.retrieval.index_revision",
        completion.index_revision.as_deref(),
    );
    optional_string(
        &mut attributes,
        "clumsies.retrieval.parser_version",
        completion.parser_version.as_deref(),
    );
    optional_string(
        &mut attributes,
        "clumsies.retrieval.chunker_version",
        completion.chunker_version.as_deref(),
    );
    optional_string(
        &mut attributes,
        "clumsies.retrieval.model_revision",
        completion.model_revision.as_deref(),
    );
    optional_string(
        &mut attributes,
        "clumsies.retrieval.ranking_profile",
        completion.ranking_profile.as_deref(),
    );
    optional_string(
        &mut attributes,
        "clumsies.retrieval.error_stage",
        completion.error_stage.as_deref(),
    );
    optional_string(
        &mut attributes,
        "clumsies.retrieval.error_code",
        completion.error_code.as_deref(),
    );
    add_latencies(&mut attributes, completion);
    add_documents(&mut attributes, &completion.candidates, capture_content);
    if capture_content {
        attributes.push(KeyValue::new("input.value", query.to_owned()));
        attributes.push(KeyValue::new("input.mime_type", "text/plain"));
    }
    attributes
}

fn add_latencies(attributes: &mut Vec<KeyValue>, completion: &RetrievalRunCompletion) {
    let latencies = &completion.latencies;
    for (name, value) in [
        ("effective_memory", latencies.effective_memory_us),
        ("index_ensure", latencies.index_ensure_us),
        ("bm25", latencies.bm25_us),
        ("embedding", latencies.embedding_us),
        ("vector", latencies.vector_us),
        ("rrf", latencies.rrf_us),
        ("rerank", latencies.rerank_us),
        ("assembly", latencies.assembly_us),
        ("total", latencies.total_us),
    ] {
        attributes.push(KeyValue::new(
            format!("clumsies.retrieval.latency.{name}_us"),
            integer(value),
        ));
    }
}

fn add_documents(
    attributes: &mut Vec<KeyValue>,
    candidates: &[RetrievalCandidateInput],
    capture_content: bool,
) {
    for (index, candidate) in candidates
        .iter()
        .filter(|candidate| candidate.final_rank.is_some())
        .enumerate()
    {
        let prefix = format!("retrieval.documents.{index}.document");
        attributes.push(KeyValue::new(
            format!("{prefix}.id"),
            opaque_document_id(candidate),
        ));
        if let Some(score) = document_score(candidate).filter(|score| score.is_finite()) {
            attributes.push(KeyValue::new(format!("{prefix}.score"), score));
        }
        let mut metadata = serde_json::json!({
            "final_rank": candidate.final_rank,
            "memory_kind": candidate.kind.as_str(),
            "scope": candidate.scope.as_str(),
            "content_hash": candidate.content_hash,
            "resource_content_hash": candidate.resource_content_hash,
        });
        if capture_content {
            metadata["unit_key"] = serde_json::json!(candidate.unit_key);
            metadata["resource_id"] = serde_json::json!(candidate.resource_id);
            metadata["path"] = serde_json::json!(candidate.path);
            metadata["heading_path"] = serde_json::json!(candidate.heading_path);
            attributes.push(KeyValue::new(
                format!("{prefix}.content"),
                candidate.evidence_excerpt.clone(),
            ));
        }
        attributes.push(KeyValue::new(
            format!("{prefix}.metadata"),
            metadata.to_string(),
        ));
    }
}

fn opaque_document_id(candidate: &RetrievalCandidateInput) -> String {
    format!(
        "sha256:{}",
        hex::encode(Sha256::digest(candidate.unit_key.as_bytes()))
    )
}

fn document_score(candidate: &RetrievalCandidateInput) -> Option<f64> {
    candidate
        .reranker_relevance
        .or(candidate.rrf_score)
        .or(candidate.vector_score)
        .or(candidate.bm25_score)
        .map(f64::from)
}

fn optional_string(attributes: &mut Vec<KeyValue>, name: &'static str, value: Option<&str>) {
    if let Some(value) = value {
        attributes.push(KeyValue::new(name, value.to_owned()));
    }
}

fn integer(value: impl TryInto<i64>) -> Value {
    Value::I64(value.try_into().unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;
    use crate::{MemoryKind, RetrievalExclusionReason, SourceLocator, SourceScope};

    #[test]
    fn exporter_is_absent_without_an_endpoint() {
        assert!(RetrievalTelemetry::from_config(&RetrievalTelemetryConfig::default()).is_none());
    }

    #[test]
    fn exporter_sends_redacted_openinference_retriever_span() {
        let (endpoint, request) = capture_one_otlp_request();
        let telemetry = RetrievalTelemetry::build(&endpoint, false).unwrap();
        telemetry.emit(
            Some("run_test"),
            "prj_test",
            "secret query",
            &completion("secret memory excerpt"),
        );
        telemetry.force_flush().unwrap();
        let request = request.join().unwrap();

        assert!(request.starts_with(b"POST /v1/traces HTTP/1.1\r\n"));
        assert!(contains(&request, b"application/x-protobuf"));
        assert!(contains(&request, b"openinference.span.kind"));
        assert!(contains(&request, b"RETRIEVER"));
        assert!(contains(&request, b"sha256:selected"));
        assert!(!contains(&request, b"secret query"));
        assert!(!contains(&request, b"secret memory excerpt"));
        assert!(!contains(&request, b"memory-1/history/0/0"));
    }

    #[test]
    fn content_capture_is_explicit() {
        let (endpoint, request) = capture_one_otlp_request();
        let telemetry = RetrievalTelemetry::build(&endpoint, true).unwrap();
        telemetry.emit(
            Some("run_test"),
            "prj_test",
            "visible query",
            &completion("visible memory excerpt"),
        );
        telemetry.force_flush().unwrap();
        let request = request.join().unwrap();

        assert!(contains(&request, b"visible query"));
        assert!(contains(&request, b"visible memory excerpt"));
        assert!(contains(&request, b"memory-1/history/0/0"));
    }

    #[test]
    fn exporter_failure_never_enters_the_retrieval_result_path() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/traces", listener.local_addr().unwrap());
        drop(listener);
        let telemetry = RetrievalTelemetry::build(&endpoint, false).unwrap();

        telemetry.emit(None, "prj_test", "query", &completion("excerpt"));

        assert!(telemetry.force_flush().is_err());
    }

    fn completion(excerpt: &str) -> RetrievalRunCompletion {
        RetrievalRunCompletion {
            effective_hash: Some("sha256:effective".to_owned()),
            index_revision: Some("search_test".to_owned()),
            candidates: vec![RetrievalCandidateInput {
                unit_key: "memory-1/history/0/0".to_owned(),
                resource_id: "memory-1".to_owned(),
                scope: SourceScope::Project,
                kind: MemoryKind::Memory,
                path: "memory/history.md".to_owned(),
                heading_path: vec!["History".to_owned()],
                locator: SourceLocator::MarkdownSpan {
                    start_byte: 0,
                    end_byte: excerpt.len(),
                    heading_path: vec!["History".to_owned()],
                },
                content_hash: "sha256:selected".to_owned(),
                resource_content_hash: "sha256:resource".to_owned(),
                token_count: 4,
                evidence_excerpt: excerpt.to_owned(),
                exact_rank: None,
                bm25_rank: Some(1),
                bm25_score: Some(1.0),
                vector_rank: Some(1),
                vector_score: Some(0.9),
                rrf_rank: Some(1),
                rrf_score: Some(0.8),
                reranker_rank: Some(1),
                reranker_logit: Some(2.0),
                reranker_relevance: Some(0.95),
                final_rank: Some(1),
                exclusion_reason: RetrievalExclusionReason::Selected,
                delta_action: None,
            }],
            unit_count: 1,
            returned_fragment_count: 1,
            returned_token_count: 4,
            latencies: crate::RetrievalStageLatencies {
                total_us: 1_000,
                ..crate::RetrievalStageLatencies::default()
            },
            ..RetrievalRunCompletion::default()
        }
    }

    fn capture_one_otlp_request() -> (String, thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/traces", listener.local_addr().unwrap());
        let request = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 8_192];
            loop {
                let read = stream.read(&mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if complete_http_request(&request) {
                    break;
                }
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/x-protobuf\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .unwrap();
            request
        });
        (endpoint, request)
    }

    fn complete_http_request(request: &[u8]) -> bool {
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            return false;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        request.len() >= header_end + 4 + content_length
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }
}
