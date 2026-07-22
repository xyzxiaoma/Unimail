//! Runtime-neutral received-attachment retrieval orchestration.

use std::sync::Arc;

use unimail_core::{
    AttachmentDownload, AttachmentDownloadSource, AttachmentId, AttachmentRequest, AttachmentSink,
    AttachmentVerificationInput, Cancellation, MailProvider, Provider, ProviderError,
    ProviderFuture, RepositoryError,
};

use crate::StoreFuture;

/// Asynchronous storage boundary for attachment source and verification metadata.
pub trait AttachmentStore: Send + Sync {
    fn get_attachment_download_source(
        &self,
        attachment_id: AttachmentId,
    ) -> StoreFuture<'_, Option<AttachmentDownloadSource>>;

    fn record_attachment_verification(
        &self,
        input: AttachmentVerificationInput,
    ) -> StoreFuture<'_, ()>;
}

/// Narrow provider boundary exposing only received-attachment retrieval.
pub trait AttachmentProvider: Send + Sync {
    fn provider(&self) -> Provider;

    fn fetch_attachment<'a>(
        &'a self,
        request: AttachmentRequest,
        sink: &'a mut dyn AttachmentSink,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AttachmentDownload>;
}

impl<T> AttachmentProvider for T
where
    T: MailProvider + ?Sized,
{
    fn provider(&self) -> Provider {
        MailProvider::provider(self)
    }

    fn fetch_attachment<'a>(
        &'a self,
        request: AttachmentRequest,
        sink: &'a mut dyn AttachmentSink,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AttachmentDownload> {
        MailProvider::fetch_attachment(self, request, sink, cancellation)
    }
}

/// Safe application failure for one attachment operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentServiceError {
    NotFound,
    AccountUnavailable,
    TooLarge,
    VerificationFailed,
    Storage(RepositoryError),
    Provider(ProviderError),
}

impl From<RepositoryError> for AttachmentServiceError {
    fn from(value: RepositoryError) -> Self {
        Self::Storage(value)
    }
}

/// Provider-neutral attachment service with one explicit decoded-byte ceiling.
pub struct AttachmentDownloadService {
    store: Arc<dyn AttachmentStore>,
    provider: Arc<dyn AttachmentProvider>,
    maximum_bytes: u64,
}

impl AttachmentDownloadService {
    /// Creates one service for an already selected provider runtime.
    #[must_use]
    pub fn new(
        store: Arc<dyn AttachmentStore>,
        provider: Arc<dyn AttachmentProvider>,
        maximum_bytes: u64,
    ) -> Self {
        Self {
            store,
            provider,
            maximum_bytes,
        }
    }

    /// Loads and provider-validates one backend-only attachment source.
    ///
    /// # Errors
    ///
    /// Returns a safe typed category for missing, mismatched, or oversized sources.
    pub async fn source(
        &self,
        attachment_id: AttachmentId,
    ) -> Result<AttachmentDownloadSource, AttachmentServiceError> {
        let source = self
            .store
            .get_attachment_download_source(attachment_id)
            .await?
            .ok_or(AttachmentServiceError::NotFound)?;
        if source.provider != self.provider.provider() {
            return Err(AttachmentServiceError::AccountUnavailable);
        }
        if source
            .size_bytes
            .is_some_and(|size| size > self.maximum_bytes)
        {
            return Err(AttachmentServiceError::TooLarge);
        }
        Ok(source)
    }

    /// Streams one resolved source and validates provider-reported transfer metadata.
    ///
    /// # Errors
    ///
    /// Returns a safe typed category for provider, limit, or metadata failures.
    pub async fn download(
        &self,
        source: &AttachmentDownloadSource,
        sink: &mut dyn AttachmentSink,
        cancellation: &dyn Cancellation,
    ) -> Result<AttachmentDownload, AttachmentServiceError> {
        if source.provider != self.provider.provider() {
            return Err(AttachmentServiceError::AccountUnavailable);
        }
        let download = self
            .provider
            .fetch_attachment(
                AttachmentRequest {
                    key: source.key.clone(),
                    provider_part_id: source.provider_part_id.clone(),
                },
                sink,
                cancellation,
            )
            .await
            .map_err(AttachmentServiceError::Provider)?;
        if download.bytes_written > self.maximum_bytes
            || source
                .size_bytes
                .is_some_and(|expected| expected != download.bytes_written)
            || source.checksum_sha256.as_deref().is_some_and(|expected| {
                download
                    .checksum_sha256
                    .as_deref()
                    .is_some_and(|actual| !expected.eq_ignore_ascii_case(actual))
            })
        {
            return Err(AttachmentServiceError::VerificationFailed);
        }
        Ok(download)
    }

    /// Persists a sink-verified size/checksum without a destination or cache key.
    ///
    /// # Errors
    ///
    /// Returns the underlying safe storage category.
    pub async fn record_verification(
        &self,
        input: AttachmentVerificationInput,
    ) -> Result<(), AttachmentServiceError> {
        self.store
            .record_attachment_verification(input)
            .await
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use unimail_core::{
        AccountId, AttachmentDownload, AttachmentDownloadSource, AttachmentId, AttachmentRequest,
        AttachmentSink, AttachmentSinkError, AttachmentSinkFuture, AttachmentVerificationInput,
        Cancellation, CancellationFuture, MessageId, Provider, ProviderFuture, RemoteMessageKey,
        RepositoryError,
    };

    use super::{
        AttachmentDownloadService, AttachmentProvider, AttachmentServiceError, AttachmentStore,
    };
    use crate::StoreFuture;

    struct FakeStore {
        source: AttachmentDownloadSource,
        verification: Mutex<Option<AttachmentVerificationInput>>,
    }

    impl AttachmentStore for FakeStore {
        fn get_attachment_download_source(
            &self,
            attachment_id: AttachmentId,
        ) -> StoreFuture<'_, Option<AttachmentDownloadSource>> {
            let source = (attachment_id == self.source.attachment_id).then(|| self.source.clone());
            Box::pin(async move { Ok(source) })
        }

        fn record_attachment_verification(
            &self,
            input: AttachmentVerificationInput,
        ) -> StoreFuture<'_, ()> {
            let result = self
                .verification
                .lock()
                .map(|mut slot| *slot = Some(input))
                .map_err(|_| RepositoryError::Internal);
            Box::pin(async move { result })
        }
    }

    struct FakeProvider;

    impl AttachmentProvider for FakeProvider {
        fn provider(&self) -> Provider {
            Provider::Gmail
        }

        fn fetch_attachment<'a>(
            &'a self,
            _request: AttachmentRequest,
            sink: &'a mut dyn AttachmentSink,
            _cancellation: &'a dyn Cancellation,
        ) -> ProviderFuture<'a, AttachmentDownload> {
            Box::pin(async move {
                sink.write_chunk(b"hello").await.map_err(|_| {
                    unimail_core::ProviderError::new(
                        unimail_core::ProviderErrorKind::Permanent,
                        "attachment_sink_rejected",
                    )
                })?;
                Ok(AttachmentDownload {
                    bytes_written: 5,
                    checksum_sha256: None,
                })
            })
        }
    }

    #[derive(Default)]
    struct BufferSink(Vec<u8>);

    impl AttachmentSink for BufferSink {
        fn write_chunk<'a>(&'a mut self, chunk: &'a [u8]) -> AttachmentSinkFuture<'a> {
            Box::pin(async move {
                self.0.extend_from_slice(chunk);
                Ok::<(), AttachmentSinkError>(())
            })
        }
    }

    struct NeverCancelled;

    impl Cancellation for NeverCancelled {
        fn is_cancelled(&self) -> bool {
            false
        }

        fn cancelled(&self) -> CancellationFuture<'_> {
            Box::pin(std::future::pending())
        }
    }

    fn source(size_bytes: u64) -> AttachmentDownloadSource {
        let account_id = AccountId::new();
        AttachmentDownloadSource {
            attachment_id: AttachmentId::new(),
            message_id: MessageId::new(),
            account_id,
            provider: Provider::Gmail,
            key: RemoteMessageKey {
                account_id,
                provider_mailbox_id: "inbox".to_owned(),
                provider_message_id: "message".to_owned(),
            },
            provider_part_id: "part".to_owned(),
            file_name: Some("report.txt".to_owned()),
            media_type: "text/plain".to_owned(),
            size_bytes: Some(size_bytes),
            checksum_sha256: None,
        }
    }

    #[test]
    fn source_and_streaming_enforce_provider_and_size_contracts() {
        let runtime = tokio_test::block_on(async {
            let source = source(5);
            let store = Arc::new(FakeStore {
                source: source.clone(),
                verification: Mutex::new(None),
            });
            let service = AttachmentDownloadService::new(store.clone(), Arc::new(FakeProvider), 8);
            let loaded = service.source(source.attachment_id).await?;
            let mut sink = BufferSink::default();
            let download = service
                .download(&loaded, &mut sink, &NeverCancelled)
                .await?;
            service
                .record_verification(AttachmentVerificationInput {
                    attachment_id: source.attachment_id,
                    size_bytes: download.bytes_written,
                    checksum_sha256:
                        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
                            .to_owned(),
                })
                .await?;
            Ok::<_, AttachmentServiceError>((sink.0, store))
        })
        .expect("attachment service");
        assert_eq!(runtime.0, b"hello");
        assert!(
            runtime
                .1
                .verification
                .lock()
                .expect("verification")
                .is_some()
        );

        let oversized = Arc::new(FakeStore {
            source: source(9),
            verification: Mutex::new(None),
        });
        let service = AttachmentDownloadService::new(oversized.clone(), Arc::new(FakeProvider), 8);
        assert_eq!(
            tokio_test::block_on(service.source(oversized.source.attachment_id)),
            Err(AttachmentServiceError::TooLarge)
        );
    }
}
