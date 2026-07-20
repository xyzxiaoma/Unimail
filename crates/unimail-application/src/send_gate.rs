//! Offline draft retention and explicit-send confirmation boundary.

use unimail_core::{
    AccountId, DraftSendReviewKey, OfflineDraftReviewInput, OfflineDraftReviewResult,
    RepositoryError, SendConfirmationRequired,
};

use crate::SyncStore;

/// Gate that can retain, query, and consume offline review markers but cannot submit mail.
pub struct ExplicitSendGate<S> {
    store: S,
}

impl<S> ExplicitSendGate<S>
where
    S: SyncStore,
{
    #[must_use]
    pub const fn new(store: S) -> Self {
        Self { store }
    }

    /// Saves the latest draft and its offline review marker atomically.
    ///
    /// # Errors
    ///
    /// Returns a safe repository category when the durable operation fails.
    pub async fn retain_offline(
        &self,
        input: OfflineDraftReviewInput,
    ) -> Result<OfflineDraftReviewResult, RepositoryError> {
        self.store.retain_offline_draft(input).await
    }

    /// Lists revision-matched confirmations. Reconnect and restart call only this query.
    ///
    /// # Errors
    ///
    /// Returns a safe repository category when storage cannot be queried.
    pub async fn confirmations(
        &self,
        account_id: Option<AccountId>,
    ) -> Result<Vec<SendConfirmationRequired>, RepositoryError> {
        self.store.list_send_confirmation_required(account_id).await
    }

    /// Consumes the exact reviewed revision before handing control to a future explicit-send use case.
    ///
    /// # Errors
    ///
    /// Returns a safe repository category when the guarded marker cannot be updated.
    pub async fn consume_confirmation(
        &self,
        key: DraftSendReviewKey,
    ) -> Result<bool, RepositoryError> {
        self.store.consume_draft_send_review(key).await
    }
}

#[cfg(test)]
mod tests {
    use unimail_core::{
        AccountId, DraftId, DraftSendReviewKey, DraftSendReviewReason, SendConfirmationRequired,
    };

    use crate::test_support::{FakeStore, block_on};

    use super::ExplicitSendGate;

    #[test]
    fn reconnect_only_queries_and_explicit_confirmation_consumes_exact_revision() {
        let store = FakeStore::default();
        let draft_id = DraftId::new();
        let account_id = AccountId::new();
        let confirmation = SendConfirmationRequired {
            draft_id,
            account_id,
            draft_revision: 7,
            reason: DraftSendReviewReason::Offline,
        };
        store
            .state
            .lock()
            .expect("fake store lock")
            .confirmations
            .push(confirmation);
        let gate = ExplicitSendGate::new(store.clone());

        assert_eq!(
            block_on(gate.confirmations(Some(account_id))).expect("confirmation query"),
            vec![confirmation]
        );
        let key = DraftSendReviewKey {
            draft_id,
            draft_revision: 7,
        };
        assert!(block_on(gate.consume_confirmation(key)).expect("consume marker"));
        assert_eq!(
            store
                .state
                .lock()
                .expect("fake store lock")
                .consumed_reviews,
            vec![key]
        );
    }
}
