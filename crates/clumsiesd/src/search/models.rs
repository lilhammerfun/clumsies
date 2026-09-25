use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fastembed::{
    InitOptionsUserDefined, Pooling, RerankInitOptionsUserDefined, TextEmbedding, TextRerank,
    TokenizerFiles, UserDefinedEmbeddingModel, UserDefinedRerankingModel,
};
use hf_hub::{
    Cache, Repo, RepoType,
    api::{Progress, sync::ApiBuilder},
};
use sha2::{Digest, Sha256};

use super::SearchFailure;

const EMBEDDING_MODEL_ID: &str = "intfloat/multilingual-e5-small";
const EMBEDDING_MODEL_REVISION: &str = "614241f622f53c4eeff9890bdc4f31cfecc418b3";
const RERANKER_MODEL_ID: &str = "Xenova/bge-reranker-base";
const RERANKER_MODEL_REVISION: &str = "280bcc27a84e0b898c251e06fddb25171bd9b101";
const EMBEDDING_DIMENSIONS: usize = 384;
const EMBEDDING_MODEL_FILE: &str = "onnx/model_qint8_avx512_vnni.onnx";
const RERANKER_MODEL_FILE: &str = "onnx/model_quantized.onnx";
const MAX_MODEL_INTRA_THREADS: usize = 4;
const PASSAGE_PREFIX: &str = "passage: ";

#[derive(Clone, Copy)]
struct ModelArtifact {
    path: &'static str,
    size: u64,
    sha256: &'static str,
}

#[derive(Clone, Copy)]
struct ModelManifest {
    repository: &'static str,
    revision: &'static str,
    artifacts: &'static [ModelArtifact],
}

const EMBEDDING_ARTIFACTS: &[ModelArtifact] = &[
    ModelArtifact {
        path: "config.json",
        size: 655,
        sha256: "69137736cab8b8903a07fe8afaafdda25aac55415a12a55d1bffa9f581abf959",
    },
    ModelArtifact {
        path: EMBEDDING_MODEL_FILE,
        size: 118_346_824,
        sha256: "dd476dd0c2514e9b9be83aeb3853fac0763e0bdf4a71645407587d77c48a2d88",
    },
    ModelArtifact {
        path: "special_tokens_map.json",
        size: 167,
        sha256: "d05497f1da52c5e09554c0cd874037a083e1dc1b9cfd48034d1c717f1afc07a7",
    },
    ModelArtifact {
        path: "tokenizer.json",
        size: 17_082_730,
        sha256: "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
    },
    ModelArtifact {
        path: "tokenizer_config.json",
        size: 443,
        sha256: "a1d6bc8734a6f635dc158508bef000f8e2e5a759c7d92f984b2c86e5ff53425b",
    },
];

const RERANKER_ARTIFACTS: &[ModelArtifact] = &[
    ModelArtifact {
        path: "config.json",
        size: 782,
        sha256: "b6575b9d5be20d6747417c8e20c5a0db1636356e0b6d422d7244c628423c4d4c",
    },
    ModelArtifact {
        path: RERANKER_MODEL_FILE,
        size: 279_301_077,
        sha256: "dd98f3e67837d23210a6b7550c08cced4f61845b940ac45be3565840a10f3244",
    },
    ModelArtifact {
        path: "special_tokens_map.json",
        size: 279,
        sha256: "d5469a60db23249c7f8945013d78df30b44b6bf686c6bb4740f4223f77b1b535",
    },
    ModelArtifact {
        path: "tokenizer.json",
        size: 17_098_079,
        sha256: "48564c5c7d3fa64d85d95e65414a542385f88b0f128fd8d4163fd7a57f2be05c",
    },
    ModelArtifact {
        path: "tokenizer_config.json",
        size: 443,
        sha256: "a1d6bc8734a6f635dc158508bef000f8e2e5a759c7d92f984b2c86e5ff53425b",
    },
];

const MODEL_MANIFESTS: &[ModelManifest] = &[
    ModelManifest {
        repository: EMBEDDING_MODEL_ID,
        revision: EMBEDDING_MODEL_REVISION,
        artifacts: EMBEDDING_ARTIFACTS,
    },
    ModelManifest {
        repository: RERANKER_MODEL_ID,
        revision: RERANKER_MODEL_REVISION,
        artifacts: RERANKER_ARTIFACTS,
    },
];

pub(crate) trait SearchModels: Send + Sync {
    fn begin_preparation(&self) {}
    fn prepare(&self) -> Result<(), SearchFailure> {
        self.revision().map(|_| ())
    }
    fn revision(&self) -> Result<String, SearchFailure>;
    fn embedding_revision(&self) -> Result<String, SearchFailure> {
        self.revision()
    }
    fn token_offsets(&self, text: &str) -> Result<Vec<(usize, usize)>, SearchFailure>;
    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SearchFailure>;
    fn embed_query(&self, query: &str) -> Result<Vec<f32>, SearchFailure>;
    fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<f32>, SearchFailure>;
    fn dimensions(&self) -> usize;
    fn status(&self) -> SearchModelRuntimeStatus;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SearchModelRuntimeStatus {
    Missing,
    Preparing {
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    Ready,
    Failed,
}

pub(crate) struct FastEmbedSearchModels {
    cache_dir: PathBuf,
    embedding: Mutex<Option<TextEmbedding>>,
    reranker: Mutex<Option<TextRerank>>,
    last_error: Mutex<Option<String>>,
    revision: Mutex<Option<String>>,
    preparation_lock: Mutex<()>,
    preparation_status: Arc<Mutex<SearchModelRuntimeStatus>>,
}

impl FastEmbedSearchModels {
    pub(crate) fn new(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            embedding: Mutex::new(None),
            reranker: Mutex::new(None),
            last_error: Mutex::new(None),
            revision: Mutex::new(None),
            preparation_lock: Mutex::new(()),
            preparation_status: Arc::new(Mutex::new(SearchModelRuntimeStatus::Missing)),
        }
    }

    fn with_embedding<T>(
        &self,
        operation: impl FnOnce(&mut TextEmbedding) -> Result<T, SearchFailure>,
    ) -> Result<T, SearchFailure> {
        let mut guard = self
            .embedding
            .lock()
            .map_err(|_| SearchFailure::model("embedding model lock is poisoned"))?;
        let model = guard
            .as_mut()
            .ok_or_else(|| SearchFailure::model("embedding model is not prepared"))?;
        operation(model)
    }

    fn with_reranker<T>(
        &self,
        operation: impl FnOnce(&mut TextRerank) -> Result<T, SearchFailure>,
    ) -> Result<T, SearchFailure> {
        let mut guard = self
            .reranker
            .lock()
            .map_err(|_| SearchFailure::model("reranker model lock is poisoned"))?;
        let model = guard
            .as_mut()
            .ok_or_else(|| SearchFailure::model("reranker model is not prepared"))?;
        operation(model)
    }

    fn record_error(&self, error: &SearchFailure) {
        if let Ok(mut guard) = self.last_error.lock() {
            *guard = Some(error.message.clone());
        }
        if let Ok(mut status) = self.preparation_status.lock() {
            *status = SearchModelRuntimeStatus::Failed;
        }
    }

    fn artifact_root(&self) -> PathBuf {
        std::env::var_os("HF_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.cache_dir.clone())
    }

    fn artifact_revision(manifests: &[ModelManifest]) -> String {
        let mut hasher = Sha256::new();
        for manifest in manifests {
            hasher.update(manifest.repository.as_bytes());
            hasher.update([0]);
            hasher.update(manifest.revision.as_bytes());
            hasher.update([0]);
            for artifact in manifest.artifacts {
                hasher.update(artifact.path.as_bytes());
                hasher.update([0]);
                hasher.update(artifact.size.to_le_bytes());
                hasher.update(artifact.sha256.as_bytes());
                hasher.update([0]);
            }
        }
        hex::encode(hasher.finalize())
    }

    fn embedding_revision_value() -> String {
        format!(
            "fastembed-5.17.3:{EMBEDDING_MODEL_ID}@{EMBEDDING_MODEL_REVISION}:int8:mean:l2:dim-{EMBEDDING_DIMENSIONS}:passage-prefix-{}:{}",
            hex::encode(PASSAGE_PREFIX),
            Self::artifact_revision(&MODEL_MANIFESTS[..1])
        )
    }

    fn prepare_inner(&self) -> Result<(), SearchFailure> {
        let root = self.artifact_root();
        let paths = prepare_artifacts(&root, self.preparation_status.clone())?;
        set_preparation_progress(
            &self.preparation_status,
            model_download_size(),
            model_download_size(),
        );

        let embedding_model = UserDefinedEmbeddingModel::new(
            read_artifact(&paths, EMBEDDING_MODEL_ID, EMBEDDING_MODEL_FILE)?,
            tokenizer_files(&paths, EMBEDDING_MODEL_ID)?,
        )
        .with_pooling(Pooling::Mean);
        let embedding = TextEmbedding::try_new_from_user_defined(
            embedding_model,
            InitOptionsUserDefined::default().with_intra_threads(model_intra_threads()),
        )
        .map_err(|error| {
            SearchFailure::model(format!(
                "failed to initialize {EMBEDDING_MODEL_ID}: {error}"
            ))
        })?;

        let reranker_model = UserDefinedRerankingModel::new(
            artifact_path(&paths, RERANKER_MODEL_ID, RERANKER_MODEL_FILE)?.to_path_buf(),
            tokenizer_files(&paths, RERANKER_MODEL_ID)?,
        );
        let reranker = TextRerank::try_new_from_user_defined(
            reranker_model,
            RerankInitOptionsUserDefined::default().with_intra_threads(model_intra_threads()),
        )
        .map_err(|error| {
            SearchFailure::model(format!("failed to initialize {RERANKER_MODEL_ID}: {error}"))
        })?;

        *self
            .embedding
            .lock()
            .map_err(|_| SearchFailure::model("embedding model lock is poisoned"))? =
            Some(embedding);
        *self
            .reranker
            .lock()
            .map_err(|_| SearchFailure::model("reranker model lock is poisoned"))? = Some(reranker);
        let revision = format!(
            "fastembed-5.17.3:{EMBEDDING_MODEL_ID}@{EMBEDDING_MODEL_REVISION}:{RERANKER_MODEL_ID}@{RERANKER_MODEL_REVISION}:int8:dim-{EMBEDDING_DIMENSIONS}:l2:{}",
            Self::artifact_revision(MODEL_MANIFESTS)
        );
        *self
            .revision
            .lock()
            .map_err(|_| SearchFailure::model("model revision lock is poisoned"))? = Some(revision);
        if let Ok(mut guard) = self.last_error.lock() {
            *guard = None;
        }
        if let Ok(mut status) = self.preparation_status.lock() {
            *status = SearchModelRuntimeStatus::Ready;
        }
        Ok(())
    }
}

impl SearchModels for FastEmbedSearchModels {
    fn begin_preparation(&self) {
        if let Ok(mut status) = self.preparation_status.lock()
            && !matches!(
                *status,
                SearchModelRuntimeStatus::Preparing { .. } | SearchModelRuntimeStatus::Ready
            )
        {
            *status = SearchModelRuntimeStatus::Preparing {
                downloaded_bytes: 0,
                total_bytes: model_download_size(),
            };
        }
    }

    fn prepare(&self) -> Result<(), SearchFailure> {
        let _guard = self
            .preparation_lock
            .lock()
            .map_err(|_| SearchFailure::model("model preparation lock is poisoned"))?;
        if matches!(self.status(), SearchModelRuntimeStatus::Ready) {
            return Ok(());
        }
        self.begin_preparation();
        match self.prepare_inner() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.record_error(&error);
                Err(error)
            }
        }
    }

    fn revision(&self) -> Result<String, SearchFailure> {
        self.prepare()?;
        self.revision
            .lock()
            .map_err(|_| SearchFailure::model("model revision lock is poisoned"))?
            .clone()
            .ok_or_else(|| SearchFailure::model("prepared model revision is missing"))
    }

    fn embedding_revision(&self) -> Result<String, SearchFailure> {
        self.prepare()?;
        Ok(Self::embedding_revision_value())
    }

    fn token_offsets(&self, text: &str) -> Result<Vec<(usize, usize)>, SearchFailure> {
        self.with_embedding(|model| {
            let previous_truncation = model.tokenizer.get_truncation().cloned();
            model.tokenizer.with_truncation(None).map_err(|error| {
                SearchFailure::model(format!("failed to disable tokenizer truncation: {error}"))
            })?;

            let encoding = model.tokenizer.encode(text, true);
            model
                .tokenizer
                .with_truncation(previous_truncation)
                .map_err(|error| {
                    SearchFailure::model(format!("failed to restore tokenizer truncation: {error}"))
                })?;

            let encoding = encoding.map_err(|error| {
                SearchFailure::model(format!("failed to tokenize memory content: {error}"))
            })?;
            Ok(encoding
                .get_offsets()
                .iter()
                .copied()
                .filter(|(start, end)| end > start)
                .collect())
        })
    }

    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SearchFailure> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let prefixed = texts
            .iter()
            .map(|text| format!("{PASSAGE_PREFIX}{text}"))
            .collect::<Vec<_>>();
        self.with_embedding(|model| {
            let embeddings = model.embed(&prefixed, Some(32)).map_err(|error| {
                SearchFailure::model(format!("passage embedding failed: {error}"))
            })?;
            validate_and_normalize_embeddings(embeddings, EMBEDDING_DIMENSIONS)
        })
    }

    fn embed_query(&self, query: &str) -> Result<Vec<f32>, SearchFailure> {
        let prefixed = [format!("query: {query}")];
        self.with_embedding(|model| {
            let embeddings = model.embed(&prefixed, Some(1)).map_err(|error| {
                SearchFailure::model(format!("query embedding failed: {error}"))
            })?;
            validate_and_normalize_embeddings(embeddings, EMBEDDING_DIMENSIONS)?
                .into_iter()
                .next()
                .ok_or_else(|| SearchFailure::model("query embedding returned no vector"))
        })
    }

    fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<f32>, SearchFailure> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        self.with_reranker(|model| {
            let documents = documents.iter().map(String::as_str).collect::<Vec<_>>();
            let results = model
                .rerank(query, &documents, false, Some(16))
                .map_err(|error| SearchFailure::model(format!("reranking failed: {error}")))?;
            let mut scores = vec![f32::NEG_INFINITY; documents.len()];
            for result in results {
                let Some(score) = scores.get_mut(result.index) else {
                    return Err(SearchFailure::model(
                        "reranker returned an out-of-range document index",
                    ));
                };
                if !result.score.is_finite() {
                    return Err(SearchFailure::model("reranker returned a non-finite score"));
                }
                *score = result.score;
            }
            if scores.iter().any(|score| !score.is_finite()) {
                return Err(SearchFailure::model(
                    "reranker did not return a score for every document",
                ));
            }
            Ok(scores)
        })
    }

    fn dimensions(&self) -> usize {
        EMBEDDING_DIMENSIONS
    }

    fn status(&self) -> SearchModelRuntimeStatus {
        self.preparation_status
            .lock()
            .map(|status| status.clone())
            .unwrap_or(SearchModelRuntimeStatus::Failed)
    }
}

struct ModelDownloadProgress {
    status: Arc<Mutex<SearchModelRuntimeStatus>>,
    completed_before: u64,
    current_file_bytes: u64,
    total_bytes: u64,
}

impl ModelDownloadProgress {
    fn new(
        status: Arc<Mutex<SearchModelRuntimeStatus>>,
        completed_before: u64,
        total_bytes: u64,
    ) -> Self {
        Self {
            status,
            completed_before,
            current_file_bytes: 0,
            total_bytes,
        }
    }
}

impl Progress for ModelDownloadProgress {
    fn init(&mut self, _size: usize, _filename: &str) {
        self.current_file_bytes = 0;
        set_preparation_progress(&self.status, self.completed_before, self.total_bytes);
    }

    fn update(&mut self, size: usize) {
        self.current_file_bytes = self.current_file_bytes.saturating_add(size as u64);
        set_preparation_progress(
            &self.status,
            self.completed_before
                .saturating_add(self.current_file_bytes)
                .min(self.total_bytes),
            self.total_bytes,
        );
    }

    fn finish(&mut self) {}
}

fn prepare_artifacts(
    cache_dir: &Path,
    status: Arc<Mutex<SearchModelRuntimeStatus>>,
) -> Result<HashMap<String, PathBuf>, SearchFailure> {
    let endpoint =
        std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_owned());
    let api = ApiBuilder::new()
        .with_cache_dir(cache_dir.to_path_buf())
        .with_endpoint(endpoint)
        .with_progress(false)
        .build()
        .map_err(|error| SearchFailure::model(format!("failed to create model client: {error}")))?;
    let cache = Cache::new(cache_dir.to_path_buf());
    let total_bytes = model_download_size();
    let mut completed_bytes = 0;
    let mut paths = HashMap::new();
    set_preparation_progress(&status, 0, total_bytes);

    for manifest in MODEL_MANIFESTS {
        let repo = Repo::with_revision(
            manifest.repository.to_owned(),
            RepoType::Model,
            manifest.revision.to_owned(),
        );
        let cache_repo = cache.repo(repo.clone());
        let api_repo = api.repo(repo);
        for artifact in manifest.artifacts {
            let cached = cache_repo
                .get(artifact.path)
                .filter(|path| verify_artifact(path, artifact).is_ok());
            let path = match cached {
                Some(path) => path,
                None => api_repo
                    .download_with_progress(
                        artifact.path,
                        ModelDownloadProgress::new(status.clone(), completed_bytes, total_bytes),
                    )
                    .map_err(|error| {
                        SearchFailure::model(format!(
                            "failed to download {}@{} {}: {error}",
                            manifest.repository, manifest.revision, artifact.path
                        ))
                    })?,
            };
            verify_artifact(&path, artifact)?;
            completed_bytes = completed_bytes.saturating_add(artifact.size);
            set_preparation_progress(&status, completed_bytes, total_bytes);
            paths.insert(artifact_key(manifest.repository, artifact.path), path);
        }
    }
    Ok(paths)
}

fn artifact_key(repository: &str, path: &str) -> String {
    format!("{repository}\0{path}")
}

fn artifact_path<'a>(
    paths: &'a HashMap<String, PathBuf>,
    repository: &str,
    path: &str,
) -> Result<&'a Path, SearchFailure> {
    paths
        .get(&artifact_key(repository, path))
        .map(PathBuf::as_path)
        .ok_or_else(|| SearchFailure::model(format!("prepared model artifact is missing: {path}")))
}

fn read_artifact(
    paths: &HashMap<String, PathBuf>,
    repository: &str,
    path: &str,
) -> Result<Vec<u8>, SearchFailure> {
    let path = artifact_path(paths, repository, path)?;
    fs::read(path).map_err(|error| {
        SearchFailure::model(format!(
            "failed to read model artifact {}: {error}",
            path.display()
        ))
    })
}

fn tokenizer_files(
    paths: &HashMap<String, PathBuf>,
    repository: &str,
) -> Result<TokenizerFiles, SearchFailure> {
    Ok(TokenizerFiles {
        tokenizer_file: read_artifact(paths, repository, "tokenizer.json")?,
        config_file: read_artifact(paths, repository, "config.json")?,
        special_tokens_map_file: read_artifact(paths, repository, "special_tokens_map.json")?,
        tokenizer_config_file: read_artifact(paths, repository, "tokenizer_config.json")?,
    })
}

fn verify_artifact(path: &Path, artifact: &ModelArtifact) -> Result<(), SearchFailure> {
    let mut file = File::open(path).map_err(|error| {
        SearchFailure::model(format!(
            "failed to open model artifact {}: {error}",
            path.display()
        ))
    })?;
    let size = file
        .metadata()
        .map_err(|error| {
            SearchFailure::model(format!(
                "failed to inspect model artifact {}: {error}",
                path.display()
            ))
        })?
        .len();
    if size != artifact.size {
        return Err(SearchFailure::model(format!(
            "model artifact {} has size {size}, expected {}",
            path.display(),
            artifact.size
        )));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            SearchFailure::model(format!(
                "failed to verify model artifact {}: {error}",
                path.display()
            ))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if hex::encode(hasher.finalize()) != artifact.sha256 {
        return Err(SearchFailure::model(format!(
            "model artifact {} failed SHA-256 verification",
            path.display()
        )));
    }
    Ok(())
}

fn model_download_size() -> u64 {
    MODEL_MANIFESTS
        .iter()
        .flat_map(|manifest| manifest.artifacts)
        .map(|artifact| artifact.size)
        .sum()
}

fn model_intra_threads() -> usize {
    std::thread::available_parallelism()
        .map(|parallelism| parallelism.get().div_ceil(2).min(MAX_MODEL_INTRA_THREADS))
        .unwrap_or(1)
}

fn set_preparation_progress(
    status: &Arc<Mutex<SearchModelRuntimeStatus>>,
    downloaded_bytes: u64,
    total_bytes: u64,
) {
    if let Ok(mut status) = status.lock() {
        *status = SearchModelRuntimeStatus::Preparing {
            downloaded_bytes: downloaded_bytes.min(total_bytes),
            total_bytes,
        };
    }
}

fn validate_and_normalize_embeddings(
    embeddings: Vec<Vec<f32>>,
    dimensions: usize,
) -> Result<Vec<Vec<f32>>, SearchFailure> {
    embeddings
        .into_iter()
        .map(|embedding| {
            if embedding.len() != dimensions || embedding.iter().any(|value| !value.is_finite()) {
                return Err(SearchFailure::vector(
                    "embedding has an invalid dimension or non-finite value",
                ));
            }
            let norm = embedding
                .iter()
                .map(|value| value * value)
                .sum::<f32>()
                .sqrt();
            if !norm.is_finite() || norm <= f32::EPSILON {
                return Err(SearchFailure::vector("embedding has an invalid norm"));
            }
            Ok(embedding.into_iter().map(|value| value / norm).collect())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_model_payload_is_bounded() {
        assert_eq!(model_download_size(), 431_831_479);
    }

    #[test]
    fn model_threads_leave_capacity_for_foreground_work() {
        assert!((1..=MAX_MODEL_INTRA_THREADS).contains(&model_intra_threads()));
    }

    #[test]
    fn embedding_revision_excludes_reranker_artifacts() {
        let revision = FastEmbedSearchModels::embedding_revision_value();
        assert!(revision.contains(EMBEDDING_MODEL_ID));
        assert!(revision.contains(&format!("passage-prefix-{}", hex::encode(PASSAGE_PREFIX))));
        assert!(!revision.contains(RERANKER_MODEL_ID));
        assert_ne!(
            FastEmbedSearchModels::artifact_revision(&MODEL_MANIFESTS[..1]),
            FastEmbedSearchModels::artifact_revision(MODEL_MANIFESTS)
        );
    }

    #[test]
    fn artifact_verification_rejects_corruption() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artifact.bin");
        fs::write(&path, b"abc").unwrap();
        let artifact = ModelArtifact {
            path: "artifact.bin",
            size: 3,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        };
        verify_artifact(&path, &artifact).unwrap();

        fs::write(&path, b"abd").unwrap();
        let error = verify_artifact(&path, &artifact).unwrap_err();
        assert!(error.message.contains("SHA-256"));
    }

    #[test]
    fn download_progress_is_monotonic_and_bounded() {
        let status = Arc::new(Mutex::new(SearchModelRuntimeStatus::Missing));
        let mut progress = ModelDownloadProgress::new(status.clone(), 100, 200);
        progress.init(80, "model.onnx");
        progress.update(30);
        progress.update(100);
        assert_eq!(
            *status.lock().unwrap(),
            SearchModelRuntimeStatus::Preparing {
                downloaded_bytes: 200,
                total_bytes: 200,
            }
        );
    }

    #[test]
    #[ignore = "downloads the pinned 412 MiB model payload"]
    fn quantized_models_run_multilingual_retrieval() {
        let cache_dir = std::env::var_os("CLUMSIES_MODEL_TEST_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|| tempfile::tempdir().unwrap().keep());
        let models = FastEmbedSearchModels::new(cache_dir);
        models.prepare().unwrap();

        let query = models.embed_query("如何避免重复注入相同记忆？").unwrap();
        let passages = models
            .embed_passages(&[
                "memory delta 使用内容哈希避免重复注入。".to_owned(),
                "macOS 菜单栏使用原生状态项。".to_owned(),
            ])
            .unwrap();
        assert_eq!(query.len(), EMBEDDING_DIMENSIONS);
        assert_eq!(passages.len(), 2);

        let scores = models
            .rerank(
                "如何避免重复注入相同记忆？",
                &[
                    "memory delta 使用内容哈希避免重复注入。".to_owned(),
                    "macOS 菜单栏使用原生状态项。".to_owned(),
                ],
            )
            .unwrap();
        assert!(scores[0] > scores[1]);
        assert_eq!(models.status(), SearchModelRuntimeStatus::Ready);
    }
}
