//! Reusable JSONL stdio server loop for sandbox provider binaries.

use crate::{JsonlRequestEnvelope, JsonlResponseEnvelope, ProviderError};
use serde::Serialize;
use serde_json::Value;
use tokio::io::{self, AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

#[async_trait::async_trait]
pub trait JsonlProvider {
    async fn handle<W>(
        &mut self,
        request_id: &str,
        method: &str,
        params: Value,
        events: &mut JsonlEventWriter<'_, W>,
    ) -> Result<JsonlProviderAction, ProviderError>
    where
        W: AsyncWrite + Unpin + Send;
}

#[derive(Debug, Clone, PartialEq)]
pub enum JsonlProviderAction {
    Respond(Value),
    Shutdown(Value),
}

pub struct JsonlEventWriter<'a, W> {
    request_id: &'a str,
    writer: &'a mut W,
}

impl<'a, W> JsonlEventWriter<'a, W>
where
    W: AsyncWrite + Unpin,
{
    pub fn new(request_id: &'a str, writer: &'a mut W) -> Self {
        Self { request_id, writer }
    }

    pub async fn emit<T>(&mut self, event: T) -> Result<(), ProviderError>
    where
        T: Serialize,
    {
        let event = serde_json::to_value(event).map_err(|source| {
            provider_failure(format!("failed to encode JSONL event: {source}"))
        })?;
        let response = JsonlResponseEnvelope {
            id: self.request_id.to_string(),
            result: None,
            error: None,
            event: Some(event),
        };
        write_response(self.writer, &response)
            .await
            .map_err(|source| {
                provider_failure(format!("failed to write JSONL event response: {source}"))
            })
    }
}

pub async fn serve_stdio<P>(provider: P) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    P: JsonlProvider + Send,
{
    let stdin = io::stdin();
    let stdout = io::stdout();
    serve(BufReader::new(stdin), stdout, provider).await
}

pub async fn serve<R, W, P>(
    reader: R,
    mut writer: W,
    mut provider: P,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send,
    P: JsonlProvider + Send,
{
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let request: Result<JsonlRequestEnvelope<Value>, ProviderError> =
            serde_json::from_str(&line).map_err(|source| {
                invalid_request(format!("failed to parse JSONL request: {source}"))
            });

        let request = match request {
            Ok(request) => request,
            Err(error) => {
                write_response(
                    &mut writer,
                    &JsonlResponseEnvelope {
                        id: "unknown".to_string(),
                        result: None,
                        error: Some(error),
                        event: None,
                    },
                )
                .await?;
                continue;
            }
        };

        let result = {
            let mut events = JsonlEventWriter::new(&request.id, &mut writer);
            provider
                .handle(&request.id, &request.method, request.params, &mut events)
                .await
        };

        match result {
            Ok(JsonlProviderAction::Respond(result)) => {
                write_response(&mut writer, &success_response(request.id, result)).await?;
            }
            Ok(JsonlProviderAction::Shutdown(result)) => {
                write_response(&mut writer, &success_response(request.id, result)).await?;
                break;
            }
            Err(error) => {
                write_response(
                    &mut writer,
                    &JsonlResponseEnvelope {
                        id: request.id,
                        result: None,
                        error: Some(error),
                        event: None,
                    },
                )
                .await?;
            }
        }
    }
    Ok(())
}

fn success_response(id: String, result: Value) -> JsonlResponseEnvelope {
    JsonlResponseEnvelope {
        id,
        result: Some(result),
        error: None,
        event: None,
    }
}

async fn write_response<W>(writer: &mut W, response: &JsonlResponseEnvelope) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut line = serde_json::to_vec(response)
        .map_err(|source| io::Error::new(io::ErrorKind::InvalidData, source))?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await
}

fn provider_error(
    code: impl Into<String>,
    message: impl Into<String>,
    retryable: bool,
) -> ProviderError {
    ProviderError {
        code: code.into(),
        message: message.into(),
        retryable,
    }
}

fn invalid_request(message: impl Into<String>) -> ProviderError {
    provider_error("invalid_request", message, false)
}

fn provider_failure(message: impl Into<String>) -> ProviderError {
    provider_error("provider_failure", message, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::BufReader;

    #[derive(Default)]
    struct TestProvider;

    #[async_trait::async_trait]
    impl JsonlProvider for TestProvider {
        async fn handle<W>(
            &mut self,
            _request_id: &str,
            method: &str,
            _params: Value,
            events: &mut JsonlEventWriter<'_, W>,
        ) -> Result<JsonlProviderAction, ProviderError>
        where
            W: AsyncWrite + Unpin + Send,
        {
            match method {
                "capabilities" => Ok(JsonlProviderAction::Respond(json!({ "exec": true }))),
                "exec" => {
                    events
                        .emit(json!({ "type": "started", "process_id": "p1" }))
                        .await?;
                    Ok(JsonlProviderAction::Respond(json!({ "exit_code": 0 })))
                }
                "shutdown" => Ok(JsonlProviderAction::Shutdown(json!({}))),
                other => Err(provider_error("unsupported_method", other, false)),
            }
        }
    }

    #[tokio::test]
    async fn serve_loop_handles_responses_events_and_shutdown() {
        let input = br#"{"id":"req_1","method":"capabilities","params":{}}
{"id":"req_2","method":"exec","params":{}}
{"id":"req_3","method":"shutdown","params":{}}
"#;
        let mut output = Vec::new();
        serve(BufReader::new(&input[..]), &mut output, TestProvider)
            .await
            .unwrap();
        let lines = String::from_utf8(output).unwrap();
        assert!(lines.contains(r#"{"id":"req_1","result":{"exec":true}}"#));
        assert!(lines.contains(r#"{"id":"req_2","event":{"process_id":"p1","type":"started"}}"#));
        assert!(lines.contains(r#"{"id":"req_2","result":{"exit_code":0}}"#));
        assert!(lines.contains(r#"{"id":"req_3","result":{}}"#));
    }
}
