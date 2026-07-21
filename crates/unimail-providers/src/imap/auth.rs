use std::sync::Arc;

use unimail_core::{
    AccountAuthenticator, AuthenticatedAccount, AuthorizationCodeLoginRequest, Cancellation,
    CompleteLoginRequest, CredentialRef, CredentialStore, LoginStart, ProviderError,
    ProviderErrorKind, ProviderFuture, ProviderResult, StartLoginRequest,
};

use super::{
    credential::{ImapCredentialEnvelopeV1, ImapCredentialManager},
    preset::ImapSmtpPreset,
    session::connect,
};

pub struct ImapAuthenticator {
    preset: &'static ImapSmtpPreset,
    credentials: ImapCredentialManager,
}

impl ImapAuthenticator {
    #[must_use]
    pub fn new(
        preset: &'static ImapSmtpPreset,
        credential_store: Arc<dyn CredentialStore>,
    ) -> Self {
        Self {
            preset,
            credentials: ImapCredentialManager::new(credential_store),
        }
    }

    async fn connect_authorization_code(
        &self,
        request: AuthorizationCodeLoginRequest,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<AuthenticatedAccount> {
        if request.provider != self.preset.provider {
            return Err(ProviderError::new(
                ProviderErrorKind::Permanent,
                "imap_auth_provider_mismatch",
            ));
        }
        let account_address = self
            .preset
            .normalize_account_address(&request.account_address)?;
        let mut session = connect(
            self.preset,
            &account_address,
            &request.authorization_code,
            cancellation,
        )
        .await?;
        let mailboxes = session
            .discover_mailboxes(self.preset, cancellation)
            .await?;
        let reference = ImapCredentialManager::create_reference(self.preset.provider)?;
        let envelope = ImapCredentialEnvelopeV1::new(
            self.preset.provider,
            account_address.clone(),
            &request.authorization_code,
        )?;
        self.credentials.persist(&reference, &envelope)?;
        let mut capabilities = vec!["imap_sync".to_owned(), "smtp_send".to_owned()];
        if mailboxes.sent.is_some() {
            capabilities.push("sent_reconciliation".to_owned());
        }
        Ok(AuthenticatedAccount {
            provider: self.preset.provider,
            account_address,
            display_name: None,
            credential_ref: reference,
            capabilities,
        })
    }
}

impl AccountAuthenticator for ImapAuthenticator {
    fn start_login<'a>(
        &'a self,
        _request: StartLoginRequest,
        _cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, LoginStart> {
        Box::pin(async {
            Err(ProviderError::new(
                ProviderErrorKind::Permanent,
                "imap_oauth_unsupported",
            ))
        })
    }

    fn complete_login<'a>(
        &'a self,
        _request: CompleteLoginRequest,
        _cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AuthenticatedAccount> {
        Box::pin(async {
            Err(ProviderError::new(
                ProviderErrorKind::Permanent,
                "imap_oauth_unsupported",
            ))
        })
    }

    fn connect_with_authorization_code<'a>(
        &'a self,
        request: AuthorizationCodeLoginRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AuthenticatedAccount> {
        Box::pin(async move { self.connect_authorization_code(request, cancellation).await })
    }

    fn refresh<'a>(
        &'a self,
        credential_ref: &'a CredentialRef,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AuthenticatedAccount> {
        Box::pin(async move {
            let envelope = self
                .credentials
                .load(credential_ref, self.preset.provider)?;
            let authorization_code = envelope.authorization_code();
            let mut session = connect(
                self.preset,
                envelope.account_address(),
                &authorization_code,
                cancellation,
            )
            .await?;
            let mailboxes = session
                .discover_mailboxes(self.preset, cancellation)
                .await?;
            let mut capabilities = vec!["imap_sync".to_owned(), "smtp_send".to_owned()];
            if mailboxes.sent.is_some() {
                capabilities.push("sent_reconciliation".to_owned());
            }
            Ok(AuthenticatedAccount {
                provider: self.preset.provider,
                account_address: envelope.account_address().to_owned(),
                display_name: None,
                credential_ref: credential_ref.clone(),
                capabilities,
            })
        })
    }

    fn revoke<'a>(
        &'a self,
        credential_ref: &'a CredentialRef,
        _cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, ()> {
        Box::pin(async move { self.credentials.delete(credential_ref) })
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use secrecy::{ExposeSecret, SecretBox};
    use unimail_core::{
        CancellationFuture, CredentialStoreError, CredentialStoreKind, Provider, SecretBytes,
        SensitiveString,
    };

    use super::*;
    use crate::imap::QQ_PRESET;

    #[derive(Default)]
    struct TestCredentialStore {
        values: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl CredentialStore for TestCredentialStore {
        fn kind(&self) -> CredentialStoreKind {
            CredentialStoreKind::Unsupported
        }

        fn get(
            &self,
            reference: &CredentialRef,
        ) -> Result<Option<SecretBytes>, CredentialStoreError> {
            Ok(self
                .values
                .lock()
                .unwrap()
                .get(reference.as_str())
                .cloned()
                .map(|value| SecretBox::new(value.into_boxed_slice())))
        }

        fn put(
            &self,
            reference: &CredentialRef,
            value: SecretBytes,
        ) -> Result<(), CredentialStoreError> {
            self.values.lock().unwrap().insert(
                reference.as_str().to_owned(),
                value.expose_secret().to_vec(),
            );
            Ok(())
        }

        fn delete(&self, reference: &CredentialRef) -> Result<(), CredentialStoreError> {
            self.values.lock().unwrap().remove(reference.as_str());
            Ok(())
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

    #[tokio::test]
    async fn oauth_methods_are_rejected_without_exposing_authorization_codes() {
        let authenticator =
            ImapAuthenticator::new(&QQ_PRESET, Arc::new(TestCredentialStore::default()));
        let error = authenticator
            .start_login(
                StartLoginRequest {
                    provider: Provider::Qq,
                    redirect_uri: SensitiveString::new("private-code"),
                },
                &NeverCancelled,
            )
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, "imap_oauth_unsupported");
        assert!(!format!("{error:?}").contains("private-code"));
    }
}
