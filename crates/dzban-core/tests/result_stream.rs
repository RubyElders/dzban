use dzban_core::error::ExecutionError;
use dzban_core::result_store::{ResultCutoff, ResultLimits};
use dzban_core::result_stream::{Delivery, ResultPhase, ResultTermination, start_results};
use dzban_core::{ResultCell, ResultColumn};

fn columns() -> Vec<ResultColumn> {
    vec![ResultColumn {
        name: "value".into(),
        type_name: "text".into(),
    }]
}

struct LegacySession;

#[async_trait::async_trait]
impl dzban_core::adapter::DatabaseSession for LegacySession {
    async fn connect(&self) -> Result<(), String> {
        Ok(())
    }
    async fn ping(&self) -> Result<u64, String> {
        Ok(0)
    }
    async fn execute(&self, _: &str) -> Result<dzban_core::ResultSet, String> {
        Ok(dzban_core::ResultSet {
            columns: columns(),
            rows: vec![vec![ResultCell::Null]],
        })
    }
    async fn cancel(&self) -> Result<(), String> {
        Ok(())
    }
    async fn version(&self) -> Result<String, String> {
        Ok("fixture".into())
    }
    async fn explain(&self, _: &str) -> Result<dzban_core::QueryPlan, String> {
        Err("unavailable".into())
    }
}

#[tokio::test]
async fn a_legacy_adapter_does_not_disguise_collected_results_as_streaming() {
    use dzban_core::adapter::DatabaseSession;
    let (mut producer, reader) = start_results(ResultLimits::default());
    let outcome = LegacySession
        .execute_streamed("SELECT 1", &mut producer)
        .await;
    assert_eq!(
        outcome,
        Err(ExecutionError::Unsupported(
            "Streaming query execution".into()
        ))
    );
    assert_eq!(reader.borrow().phase, ResultPhase::WaitingForSchema);
    assert!(reader.borrow().result.is_none());
    producer.finish(outcome.map(|_| ())).await.unwrap();
    assert!(matches!(
        reader.borrow().phase,
        ResultPhase::Finished(ResultTermination::Failed(ExecutionError::Unsupported(_)))
    ));
}

#[tokio::test]
async fn pages_are_visible_before_completion_and_limits_wait_for_cleanup() {
    let (mut producer, reader) = start_results(ResultLimits {
        rows: 2,
        memory_rows: 1,
        ..Default::default()
    });
    assert_eq!(reader.borrow().phase, ResultPhase::WaitingForSchema);
    assert_eq!(
        producer.schema(columns()).await.unwrap(),
        Delivery::Continue
    );
    assert_eq!(reader.borrow().phase, ResultPhase::Receiving);
    producer
        .row(vec![ResultCell::Text("žluť\nline".into())])
        .await
        .unwrap();
    let first = reader.borrow().clone();
    assert_eq!(first.rows, 1);
    assert_eq!(first.phase, ResultPhase::Receiving);
    let result = first.result.unwrap();
    assert_eq!(
        result.read_page(0, 1).await.unwrap().rows,
        vec![vec![ResultCell::Text("žluť\nline".into())]]
    );
    assert_eq!(
        producer.row(vec![ResultCell::Null]).await.unwrap(),
        Delivery::Stop(ResultCutoff::Rows)
    );
    assert_eq!(
        reader.borrow().phase,
        ResultPhase::Draining(ResultCutoff::Rows)
    );
    assert_eq!(
        producer
            .row(vec![ResultCell::Text("not captured".into())])
            .await
            .unwrap(),
        Delivery::Stop(ResultCutoff::Rows)
    );
    assert_eq!(reader.borrow().rows, 2);
    assert_eq!(reader.borrow().received_rows, 3);
    producer.finish(Ok(())).await.unwrap();
    assert_eq!(
        reader.borrow().phase,
        ResultPhase::Finished(ResultTermination::Limited(ResultCutoff::Rows))
    );
    assert_eq!(
        result.read_page(1, 1).await.unwrap().rows,
        vec![vec![ResultCell::Null]]
    );
    assert!(producer.row(vec![ResultCell::Null]).await.is_err());
}

#[tokio::test]
async fn dropped_observers_do_not_stop_owned_storage_or_page_readers() {
    let (mut producer, reader) = start_results(ResultLimits {
        memory_rows: 0,
        ..Default::default()
    });
    producer.schema(columns()).await.unwrap();
    let result = reader.borrow().result.clone().unwrap();
    drop(reader);
    producer
        .row(vec![ResultCell::Text("disk value".into())])
        .await
        .unwrap();
    producer.finish(Ok(())).await.unwrap();
    drop(producer);
    let page = result.read_page(0, 1).await.unwrap();
    assert_eq!(page.columns, columns());
    assert_eq!(page.rows, vec![vec![ResultCell::Text("disk value".into())]]);
}

#[tokio::test]
async fn zero_rows_cancellation_and_failure_have_distinct_terminal_states() {
    for outcome in [
        Ok(()),
        Err(ExecutionError::Canceled),
        Err(ExecutionError::Message("fixture error".into())),
    ] {
        let expected = match &outcome {
            Ok(()) => ResultTermination::Complete,
            Err(ExecutionError::Canceled) => ResultTermination::Canceled,
            Err(error) => ResultTermination::Failed(error.clone()),
        };
        let (mut producer, reader) = start_results(ResultLimits::default());
        producer.schema(columns()).await.unwrap();
        producer.finish(outcome).await.unwrap();
        assert_eq!(reader.borrow().phase, ResultPhase::Finished(expected));
        let result = reader.borrow().result.clone().unwrap();
        let page = result.read_page(0, 200).await.unwrap();
        assert_eq!(page.columns, columns());
        assert!(page.rows.is_empty());
    }
}

#[tokio::test]
async fn invalid_delivery_keeps_its_error_until_backend_cleanup_finishes() {
    let (mut producer, reader) = start_results(ResultLimits::default());
    producer.schema(columns()).await.unwrap();
    let error = producer.row(Vec::new()).await.unwrap_err();
    assert_eq!(
        reader.borrow().phase,
        ResultPhase::FailedAwaitingCleanup(error.clone())
    );
    assert_eq!(producer.schema(columns()).await.unwrap_err(), error);
    producer.finish(Ok(())).await.unwrap();
    assert_eq!(
        reader.borrow().phase,
        ResultPhase::Finished(ResultTermination::Failed(error))
    );
}

#[tokio::test]
async fn abandoned_producers_are_not_reported_as_successful_results() {
    let (mut producer, mut reader) = start_results(ResultLimits::default());
    producer.schema(columns()).await.unwrap();
    reader.borrow_and_update();
    drop(producer);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if matches!(
                reader.borrow_and_update().phase,
                ResultPhase::Finished(ResultTermination::Failed(_))
            ) {
                break;
            }
            reader.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn cancellation_during_cutoff_cleanup_retains_the_capture_limit() {
    let (mut producer, reader) = start_results(ResultLimits {
        rows: 1,
        ..Default::default()
    });
    producer.schema(columns()).await.unwrap();
    assert_eq!(
        producer.row(vec![ResultCell::Null]).await.unwrap(),
        Delivery::Stop(ResultCutoff::Rows)
    );
    producer
        .finish(Err(ExecutionError::Canceled))
        .await
        .unwrap();
    assert_eq!(
        reader.borrow().phase,
        ResultPhase::Finished(ResultTermination::Canceled)
    );
    assert_eq!(reader.borrow().cutoff, Some(ResultCutoff::Rows));
    assert_eq!(reader.borrow().rows, 1);
}
#[tokio::test]
async fn oversized_wire_row_is_limited_without_allocating_or_storing_it() {
    let (mut producer, reader) = dzban_core::result_stream::start_results(Default::default());
    producer
        .schema(vec![dzban_core::ResultColumn {
            name: "value".into(),
            type_name: "String".into(),
        }])
        .await
        .unwrap();
    assert_eq!(
        producer.oversized_row().await.unwrap(),
        dzban_core::result_stream::Delivery::Stop(dzban_core::result_store::ResultCutoff::Bytes)
    );
    assert_eq!(reader.borrow().rows, 0);
    assert_eq!(reader.borrow().received_rows, 1);
    producer.finish(Ok(())).await.unwrap();
    assert_eq!(
        reader.borrow().phase,
        dzban_core::result_stream::ResultPhase::Finished(
            dzban_core::result_stream::ResultTermination::Limited(
                dzban_core::result_store::ResultCutoff::Bytes
            )
        )
    );
}
