use dzban_core::error::ExecutionError;
use dzban_core::result_stream::{Delivery, ResultSink};
use dzban_core::{CommandAcknowledgment, ResultColumn};

use super::{clickhouse_cell, clickhouse_error, clickhouse_transport_error};

const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 4096;

#[derive(Default)]
struct Decoder {
    names: Option<Vec<String>>,
    columns: Option<usize>,
}

impl Decoder {
    async fn line(
        &mut self,
        line: &[u8],
        results: &mut ResultSink,
        delivery: Delivery,
    ) -> Result<Delivery, ExecutionError> {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            return Ok(delivery);
        }
        if line.starts_with(b"Code: ") {
            let body = String::from_utf8_lossy(&line[..line.len().min(MAX_ERROR_BYTES)]);
            let code = body
                .strip_prefix("Code: ")
                .and_then(|rest| rest.split_once('.'))
                .map(|(code, _)| code);
            return Err(ExecutionError::Database(clickhouse_error(
                reqwest::StatusCode::OK,
                code,
                &body,
            )));
        }
        if self.columns.is_some() && matches!(delivery, Delivery::Stop(_)) {
            return Ok(delivery);
        }
        match (&self.names, self.columns) {
            (None, _) => {
                self.names = Some(serde_json::from_slice(line).map_err(|error| {
                    ExecutionError::Message(format!("Invalid ClickHouse column names: {error}"))
                })?);
            }
            (Some(names), None) => {
                let types: Vec<String> = serde_json::from_slice(line).map_err(|error| {
                    ExecutionError::Message(format!("Invalid ClickHouse column types: {error}"))
                })?;
                if names.len() != types.len() {
                    return Err(ExecutionError::Message(
                        "ClickHouse column names and types differ in length".into(),
                    ));
                }
                self.columns = Some(names.len());
                return results
                    .schema(
                        names
                            .iter()
                            .cloned()
                            .zip(types)
                            .map(|(name, type_name)| ResultColumn { name, type_name })
                            .collect(),
                    )
                    .await;
            }
            (_, Some(count)) => {
                let row: Vec<serde_json::Value> =
                    serde_json::from_slice(line).map_err(|error| {
                        ExecutionError::Message(format!("Invalid ClickHouse result row: {error}"))
                    })?;
                if row.len() != count {
                    return Err(ExecutionError::Message(
                        "ClickHouse result row does not match its columns".into(),
                    ));
                }
                return results
                    .row(row.into_iter().map(clickhouse_cell).collect())
                    .await;
            }
        }
        Ok(delivery)
    }
}

pub(super) async fn execute(
    request: reqwest::RequestBuilder,
    results: &mut ResultSink,
) -> Result<Option<CommandAcknowledgment>, ExecutionError> {
    execute_limited(request, results, MAX_FRAME_BYTES).await
}

async fn execute_limited(
    request: reqwest::RequestBuilder,
    results: &mut ResultSink,
    frame_limit: usize,
) -> Result<Option<CommandAcknowledgment>, ExecutionError> {
    let mut response = request
        .send()
        .await
        .map_err(|error| ExecutionError::Database(clickhouse_transport_error(error)))?;
    let status = response.status();
    let exception_code = response
        .headers()
        .get("X-ClickHouse-Exception-Code")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let mut decoder = Decoder::default();
    let mut frame = Vec::new();
    let mut failure = None;
    let mut delivery = Delivery::Continue;
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => {
                if frame.starts_with(b"Code: ") {
                    decoder.line(&frame, results, delivery).await?;
                }
                if let Some(ExecutionError::Database(native)) = &failure
                    && matches!(
                        native.code,
                        Some(dzban_core::error::NativeErrorCode::ClickHouse(_))
                    )
                {
                    return Err(failure.unwrap());
                }
                return Err(ExecutionError::Database(clickhouse_transport_error(error)));
            }
        };
        for part in chunk.split_inclusive(|byte| *byte == b'\n') {
            let newline = part.last() == Some(&b'\n');
            let part = if newline {
                &part[..part.len() - 1]
            } else {
                part
            };
            let limit = if status.is_success()
                && failure.is_none()
                && matches!(delivery, Delivery::Continue)
            {
                frame_limit
            } else {
                MAX_ERROR_BYTES
            };
            if frame.len().saturating_add(part.len()) <= limit {
                frame.extend_from_slice(part);
            } else if status.is_success()
                && failure.is_none()
                && matches!(delivery, Delivery::Continue)
            {
                if decoder.columns.is_some() {
                    match results.oversized_row().await {
                        Ok(next) => delivery = next,
                        Err(error) => failure = Some(error),
                    }
                } else {
                    failure = Some(ExecutionError::Message(
                        "ClickHouse schema frame exceeds 32 MiB".into(),
                    ));
                }
                frame.truncate(MAX_ERROR_BYTES);
            } else {
                frame.extend_from_slice(&part[..part.len().min(limit.saturating_sub(frame.len()))]);
            }
            if newline && status.is_success() {
                if failure.is_none() || frame.starts_with(b"Code: ") {
                    match decoder.line(&frame, results, delivery).await {
                        Ok(next) => delivery = next,
                        Err(error) => failure = Some(error),
                    }
                }
                frame.clear();
            } else if newline && frame.len() < MAX_ERROR_BYTES {
                frame.push(b'\n');
            }
        }
    }
    if !status.is_success() {
        return Err(ExecutionError::Database(clickhouse_error(
            status,
            exception_code.as_deref(),
            &String::from_utf8_lossy(&frame),
        )));
    }
    if !frame.is_empty() && (failure.is_none() || frame.starts_with(b"Code: ")) {
        decoder.line(&frame, results, delivery).await?;
    }
    if let Some(error) = failure {
        return Err(error);
    }
    if decoder.names.is_some() && decoder.columns.is_none() {
        return Err(ExecutionError::Message(
            "ClickHouse response ended before column types".into(),
        ));
    }
    Ok(decoder.columns.is_none().then_some(CommandAcknowledgment {
        affected_rows: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dzban_core::ResultCell;
    use dzban_core::error::{ErrorKind, NativeErrorCode};
    use dzban_core::result_store::{ResultCutoff, ResultLimits};
    use dzban_core::result_stream::{ResultPhase, ResultTermination, start_results};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn response(body: &[u8], status: &str, length: usize) -> reqwest::RequestBuilder {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let body = body.to_vec();
        let headers =
            format!("HTTP/1.1 {status}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            socket.write_all(headers.as_bytes()).await.unwrap();
            for byte in body {
                socket.write_all(&[byte]).await.unwrap();
                tokio::task::yield_now().await;
            }
        });
        reqwest::Client::new().get(format!("http://{address}"))
    }

    #[tokio::test]
    async fn fragmented_rows_preserve_utf8_null_schema_and_order() {
        let body = "[\"id\",\"value\"]\r\n[\"UInt64\",\"Nullable(String)\"]\r\n[\"1\",\"žluť\\n\"]\n[\"2\",null]";
        let (mut sink, reader) = start_results(ResultLimits {
            memory_rows: 1,
            ..Default::default()
        });
        let outcome = execute(
            response(body.as_bytes(), "200 OK", body.len()).await,
            &mut sink,
        )
        .await
        .unwrap();
        assert_eq!(outcome, None);
        sink.finish(Ok(())).await.unwrap();
        let snapshot = reader.borrow().clone();
        assert_eq!(
            snapshot.phase,
            ResultPhase::Finished(ResultTermination::Complete)
        );
        let page = snapshot.result.unwrap().read_page(0, 2).await.unwrap();
        assert_eq!(page.columns[1].type_name, "Nullable(String)");
        assert_eq!(page.rows[0][1], ResultCell::Text("žluť\n".into()));
        assert_eq!(page.rows[1][1], ResultCell::Null);
    }

    #[tokio::test]
    async fn late_native_error_wins_over_cutoff_after_draining() {
        let body = "[\"id\"]\n[\"UInt64\"]\n[\"1\"]\n[\"2\"]\nCode: 159. DB::Exception: Timeout exceeded\n";
        let (mut sink, reader) = start_results(ResultLimits {
            rows: 1,
            ..Default::default()
        });
        let error = execute(
            response(body.as_bytes(), "200 OK", body.len()).await,
            &mut sink,
        )
        .await
        .unwrap_err();
        let ExecutionError::Database(native) = &error else {
            panic!("expected native error");
        };
        assert_eq!(native.code, Some(NativeErrorCode::ClickHouse(159)));
        sink.finish(Err(error.clone())).await.unwrap();
        let snapshot = reader.borrow().clone();
        assert_eq!(snapshot.rows, 1);
        assert_eq!(snapshot.cutoff, Some(ResultCutoff::Rows));
        assert_eq!(
            snapshot.phase,
            ResultPhase::Finished(ResultTermination::Failed(error))
        );
    }

    #[tokio::test]
    async fn native_cancel_error_survives_an_incomplete_http_trailer() {
        for newline in ["", "\n"] {
            let body = format!(
                "[\"id\"]\n[\"UInt64\"]\n[\"1\"]\nCode: 394. DB::Exception: Query was cancelled{newline}"
            );
            let (mut sink, _) = start_results(ResultLimits::default());
            let error = execute(
                response(body.as_bytes(), "200 OK", body.len() + 1).await,
                &mut sink,
            )
            .await
            .unwrap_err();
            assert!(
                matches!(error, ExecutionError::Database(ref native) if native.code == Some(NativeErrorCode::ClickHouse(394)))
            );
        }
    }

    #[tokio::test]
    async fn incomplete_transport_is_not_success() {
        let body = b"[\"id\"]\n[\"UInt64\"]\n[\"1\"]\n";
        let (mut sink, _) = start_results(ResultLimits::default());
        let error = execute(response(body, "200 OK", body.len() + 1).await, &mut sink)
            .await
            .unwrap_err();
        assert!(
            matches!(error, ExecutionError::Database(ref native) if native.kind == ErrorKind::Transport)
        );
        sink.finish(Err(error)).await.unwrap();
    }

    #[tokio::test]
    async fn incomplete_schema_and_mismatched_rows_are_not_success() {
        for body in [
            "[\"id\"]\n",
            "[\"id\"]\n[]\n",
            "[\"id\"]\n[\"UInt64\"]\n[]\n",
        ] {
            let (mut sink, _) = start_results(ResultLimits::default());
            let error = execute(
                response(body.as_bytes(), "200 OK", body.len()).await,
                &mut sink,
            )
            .await
            .unwrap_err();
            sink.finish(Err(error)).await.unwrap();
        }
    }

    #[tokio::test]
    async fn error_responses_preserve_format_boundaries_for_native_diagnostics() {
        let body = b"[]\n[]\n[\"Code: 131. DB::Exception: Too large\"]\n";
        let (mut sink, _) = start_results(ResultLimits::default());
        let error = execute(
            response(body, "500 Internal Server Error", body.len()).await,
            &mut sink,
        )
        .await
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Code: 131. DB::Exception: Too large")
        );
        assert!(!error.to_string().contains("[][]"));
        sink.finish(Err(error)).await.unwrap();
    }

    #[tokio::test]
    async fn oversized_frame_is_a_byte_cutoff_and_keeps_previous_rows() {
        let body = "[\"x\"]\n[\"String\"]\n[\"first\"]\n[\"a value larger than the frame limit\"]\n[\"last\"]\n";
        let (mut sink, reader) = start_results(ResultLimits::default());
        execute_limited(
            response(body.as_bytes(), "200 OK", body.len()).await,
            &mut sink,
            16,
        )
        .await
        .unwrap();
        sink.finish(Ok(())).await.unwrap();
        let snapshot = reader.borrow().clone();
        assert_eq!(
            snapshot.phase,
            ResultPhase::Finished(ResultTermination::Limited(ResultCutoff::Bytes))
        );
        assert_eq!(snapshot.rows, 1);
        assert_eq!(
            snapshot.result.unwrap().read_page(0, 1).await.unwrap().rows[0][0].display(),
            "first"
        );
    }
}
