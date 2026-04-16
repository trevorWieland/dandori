use async_trait::async_trait;
use dandori_app_services::{OutboxPublisher, PublishError, PublishErrorKind};
use dandori_domain::IssueCreatedEventV1;

#[derive(Debug)]
pub(super) struct AlwaysOkPublisher;

#[async_trait]
impl OutboxPublisher for AlwaysOkPublisher {
    async fn publish_issue_created(
        &self,
        _message: &dandori_store::OutboxMessage,
        _event: &IssueCreatedEventV1,
    ) -> Result<(), PublishError> {
        Ok(())
    }
}

#[derive(Debug)]
pub(super) struct TransientFailurePublisher;

#[async_trait]
impl OutboxPublisher for TransientFailurePublisher {
    async fn publish_issue_created(
        &self,
        _message: &dandori_store::OutboxMessage,
        _event: &IssueCreatedEventV1,
    ) -> Result<(), PublishError> {
        Err(PublishError {
            kind: PublishErrorKind::Transient,
            message: "temporary downstream outage".to_owned(),
        })
    }
}

#[derive(Debug)]
pub(super) struct PermanentFailurePublisher;

#[async_trait]
impl OutboxPublisher for PermanentFailurePublisher {
    async fn publish_issue_created(
        &self,
        _message: &dandori_store::OutboxMessage,
        _event: &IssueCreatedEventV1,
    ) -> Result<(), PublishError> {
        Err(PublishError {
            kind: PublishErrorKind::Permanent,
            message: "permanent downstream rejection".to_owned(),
        })
    }
}
