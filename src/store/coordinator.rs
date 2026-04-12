//! Write-coordinator task for serialized redb writes.
//!
//! Only one `WriteTransaction` can be active at a time in redb.
//! The coordinator receives write requests via an mpsc channel
//! and executes them sequentially. Other components send requests
//! rather than acquiring write locks directly.

use std::sync::Arc;

use redb::Database;
use tokio::sync::{mpsc, oneshot};

use crate::store::{
    crud,
    schema::{
        Decision,
        EmbeddingArtifact,
        Entity,
        Episode,
        ExtractionArtifact,
        RawEvent,
        SummaryArtifact,
        Task,
    },
};

/// A write request sent to the coordinator.
pub enum WriteRequest {
    AppendRawEvent {
        event: RawEvent,
        reply: oneshot::Sender<Result<(), crud::StoreError>>,
    },
    PutEpisode {
        episode: Episode,
        reply: oneshot::Sender<Result<(), crud::StoreError>>,
    },
    PutDecision {
        decision: Decision,
        reply: oneshot::Sender<Result<(), crud::StoreError>>,
    },
    PutTask {
        task: Task,
        reply: oneshot::Sender<Result<(), crud::StoreError>>,
    },
    PutEntity {
        entity: Entity,
        reply: oneshot::Sender<Result<(), crud::StoreError>>,
    },
    PutSummaryArtifact {
        artifact: SummaryArtifact,
        reply: oneshot::Sender<Result<(), crud::StoreError>>,
    },
    PutExtractionArtifact {
        artifact: ExtractionArtifact,
        reply: oneshot::Sender<Result<(), crud::StoreError>>,
    },
    PutEmbeddingArtifact {
        artifact: EmbeddingArtifact,
        reply: oneshot::Sender<Result<(), crud::StoreError>>,
    },
}

/// Handle for sending write requests to the coordinator.
#[derive(Clone)]
pub struct WriteHandle {
    tx: mpsc::Sender<WriteRequest>,
}

impl WriteHandle {
    /// Append a raw event. Returns when the write is committed.
    ///
    /// # Errors
    ///
    /// Returns `StoreError` if serialization or the redb write fails.
    pub async fn append_raw_event(
        &self,
        event: RawEvent,
    ) -> Result<(), crud::StoreError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(WriteRequest::AppendRawEvent {
                event,
                reply: reply_tx,
            })
            .await
            .map_err(|_| {
                crud::StoreError::Redb("coordinator channel closed".into())
            })?;
        reply_rx.await.map_err(|_| {
            crud::StoreError::Redb("coordinator reply dropped".into())
        })?
    }

    /// Write an episode record.
    ///
    /// # Errors
    ///
    /// Returns `StoreError` if the write fails.
    pub async fn put_episode(
        &self,
        episode: Episode,
    ) -> Result<(), crud::StoreError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(WriteRequest::PutEpisode {
                episode,
                reply: reply_tx,
            })
            .await
            .map_err(|_| {
                crud::StoreError::Redb("coordinator channel closed".into())
            })?;
        reply_rx.await.map_err(|_| {
            crud::StoreError::Redb("coordinator reply dropped".into())
        })?
    }
}

/// Spawn the write coordinator task.
///
/// Returns a `WriteHandle` for sending requests and a
/// `JoinHandle` for the background task.
#[must_use]
pub fn spawn(
    db: Arc<Database>,
    buffer_size: usize,
) -> (WriteHandle, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<WriteRequest>(buffer_size);

    let handle = tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            match req {
                WriteRequest::AppendRawEvent { event, reply } => {
                    let result = crud::append_raw_event(&db, &event);
                    let _ = reply.send(result);
                }
                WriteRequest::PutEpisode { episode, reply } => {
                    let result = crud::put_episode(&db, &episode);
                    let _ = reply.send(result);
                }
                WriteRequest::PutDecision { decision, reply } => {
                    let result = crud::put_decision(&db, &decision);
                    let _ = reply.send(result);
                }
                WriteRequest::PutTask { task, reply } => {
                    let result = crud::put_task(&db, &task);
                    let _ = reply.send(result);
                }
                WriteRequest::PutEntity { entity, reply } => {
                    let result = crud::put_entity(&db, &entity);
                    let _ = reply.send(result);
                }
                WriteRequest::PutSummaryArtifact { artifact, reply } => {
                    let result = crud::put_summary_artifact(&db, &artifact);
                    let _ = reply.send(result);
                }
                WriteRequest::PutExtractionArtifact { artifact, reply } => {
                    let result = crud::put_extraction_artifact(&db, &artifact);
                    let _ = reply.send(result);
                }
                WriteRequest::PutEmbeddingArtifact { artifact, reply } => {
                    let result = crud::put_embedding_artifact(&db, &artifact);
                    let _ = reply.send(result);
                }
            }
        }
    });

    (WriteHandle { tx }, handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{
        db,
        ids::{EpisodeId, RepoId},
        schema::{EventKind, ProcessingState},
    };

    #[tokio::test]
    async fn test_coordinator_append_raw_event() {
        let database = Arc::new(db::open_in_memory().expect("db"));
        let (write_handle, _join) = spawn(database.clone(), 16);

        let event = RawEvent {
            seq: 1,
            repo_id: RepoId::derive(b"test"),
            ts_utc_ms: 1_700_000_000_000,
            event_kind: EventKind::UserPromptSubmit,
            payload_hash: [0; 32],
            payload_bytes: b"hello".to_vec(),
        };

        write_handle
            .append_raw_event(event.clone())
            .await
            .expect("append");

        // Read back directly
        let loaded = crud::get_raw_event(&database, 1).expect("get");
        assert_eq!(event, loaded);
    }

    #[tokio::test]
    async fn test_coordinator_put_episode() {
        let database = Arc::new(db::open_in_memory().expect("db"));
        let (write_handle, _join) = spawn(database.clone(), 16);

        let ep = Episode {
            episode_id: EpisodeId::derive(b"ep1"),
            repo_id: RepoId::derive(b"repo"),
            start_seq: 0,
            end_seq: 5,
            task_id: None,
            processing_state: ProcessingState::Pending,
            finalized_ts_utc_ms: 1_700_000_000_000,
            retry_count: 0,
            is_noisy: false,
        };

        write_handle.put_episode(ep.clone()).await.expect("put");

        let loaded =
            crud::get_episode(&database, &ep.episode_id.raw()).expect("get");
        assert_eq!(ep, loaded);
    }

    #[tokio::test]
    async fn test_coordinator_serializes_writes() {
        let database = Arc::new(db::open_in_memory().expect("db"));
        let (write_handle, _join) = spawn(database.clone(), 64);

        // Send multiple writes concurrently
        let mut handles = vec![];
        for i in 0..10 {
            let wh = write_handle.clone();
            handles.push(tokio::spawn(async move {
                let event = RawEvent {
                    seq: i,
                    repo_id: RepoId::derive(b"test"),
                    #[allow(clippy::cast_possible_wrap)]
                    ts_utc_ms: 1_700_000_000_000 + i as i64,
                    event_kind: EventKind::ToolUse,
                    payload_hash: [0; 32],
                    payload_bytes: vec![],
                };
                wh.append_raw_event(event).await.expect("append");
            }));
        }
        for h in handles {
            h.await.expect("join");
        }

        // All 10 should be readable
        for i in 0..10 {
            let loaded = crud::get_raw_event(&database, i).expect("get");
            assert_eq!(loaded.seq, i);
        }
    }
}
